//! Bounded SQLite normalization and independent full-file/state verification.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::Instant;

use bitcoincore_rpc::bitcoin::{Block, ScriptBuf, consensus};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use sha2::{Digest, Sha256};
use usdb_util::{ToBtcScriptHash, parse_json_strict};

use super::{
    BASELINE_MANIFEST, BASELINE_SCHEMA, BaselineIdentity, BaselineManifest, BaselineState,
    BaselineTableDigest, Result, SQL,
};
use crate::index::verify_snapshot_artifact_manifest_signature;

/// One unpublished output, with disk-backed sorting and bounded page cache.
pub(crate) struct Writer {
    pub(crate) conn: Connection,
    pub(crate) identity: BaselineIdentity,
    started: Instant,
    heartbeat: Instant,
    rows: u64,
    automatic_commit: bool,
}

impl Writer {
    pub(crate) fn create(path: &Path, identity: BaselineIdentity, block: &Block) -> Result<Self> {
        identity.validate()?;
        validate_block(block, &identity)?;
        File::options().write(true).create_new(true).open(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=FILE; PRAGMA cache_size=-65536; PRAGMA journal_mode=DELETE;")?;
        conn.execute_batch(SQL)?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        conn.execute(
            "INSERT INTO genesis_block VALUES (1, ?1)",
            [consensus::serialize(block)],
        )?;
        Ok(Self {
            conn,
            identity,
            started: Instant::now(),
            heartbeat: Instant::now(),
            rows: 0,
            automatic_commit: true,
        })
    }

    pub(crate) fn progress(&mut self, stage: &str) -> Result<()> {
        self.rows += 1;
        // Bound transaction/WAL work as well as application memory. An unfinished
        // directory never has a completed manifest and cannot be installed.
        if self.automatic_commit && self.rows.is_multiple_of(20_000) {
            self.conn.execute_batch("COMMIT; BEGIN IMMEDIATE")?;
        }
        if self.heartbeat.elapsed().as_secs() >= 10 {
            eprintln!(
                "Baseline export progress: stage={stage}, rows={}, elapsed_seconds={:.1}",
                self.rows,
                self.started.elapsed().as_secs_f64()
            );
            self.heartbeat = Instant::now();
        }
        Ok(())
    }

    pub(crate) fn put_balance(&mut self, hash: &[u8], balance: u64) -> Result<()> {
        if balance > 0 {
            self.conn
                .prepare_cached("INSERT INTO balances VALUES (?1, ?2)")?
                .execute(params![hash, i64::try_from(balance)?])?;
        }
        self.progress("balances")
    }

    /// Resume an unpublished DB; callers checkpoint rows and their cursor in one transaction.
    pub(crate) fn resume(path: &Path, identity: BaselineIdentity) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA temp_store=FILE; PRAGMA cache_size=-65536; BEGIN IMMEDIATE")?;
        Ok(Self {
            conn,
            identity,
            started: Instant::now(),
            heartbeat: Instant::now(),
            rows: 0,
            automatic_commit: false,
        })
    }

    pub(crate) fn use_external_checkpoints(&mut self) {
        self.automatic_commit = false;
    }

    pub(crate) fn put_utxo(&mut self, outpoint: &[u8], hash: &[u8], value: u64) -> Result<()> {
        self.conn
            .prepare_cached("INSERT INTO utxos VALUES (?1, ?2, ?3)")?
            .execute(params![outpoint, hash, i64::try_from(value)?])?;
        self.progress("utxos")
    }

    pub(crate) fn put_script(&mut self, hash: &[u8], script: &[u8]) -> Result<()> {
        let expected = ScriptBuf::from(script.to_vec()).to_btc_script_hash();
        if hash != expected.as_ref() as &[u8] {
            return Err("Baseline script hash does not match scriptPubKey".into());
        }
        self.conn
            .prepare_cached("INSERT INTO script_registry VALUES (?1, ?2)")?
            .execute(params![hash, script])?;
        self.progress("script_registry")
    }

    pub(crate) fn required_scripts(&self, block: &Block) -> Result<()> {
        build_required_scripts(&self.conn, block)
    }

    pub(crate) fn script_page(&self, after: Option<&[u8]>) -> Result<Vec<Vec<u8>>> {
        let mut query = self.conn.prepare("SELECT script_hash FROM required_scripts WHERE script_hash > ?1 ORDER BY script_hash LIMIT 4096")?;
        Ok(query
            .query_map([after.unwrap_or(&[])], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn finish(self) -> Result<BaselineState> {
        self.conn.execute_batch("COMMIT")?;
        let state = scan_state(&self.conn, self.identity)?;
        let existing: Option<String> = self
            .conn
            .query_row("SELECT state_json FROM meta WHERE id=1", [], |r| r.get(0))
            .optional()?;
        let encoded = serde_json::to_string(&state)?;
        match existing {
            Some(value) if value != encoded => {
                return Err("Existing baseline state metadata differs".into());
            }
            Some(_) => (),
            None => {
                self.conn
                    .execute("INSERT INTO meta VALUES (1, ?1)", [encoded])?;
            }
        }
        self.conn.close().map_err(|(_, error)| error)?;
        eprintln!(
            "Baseline export state finished: logical_sha256={}, elapsed_seconds={:.1}",
            state.logical_sha256()?,
            self.started.elapsed().as_secs_f64()
        );
        Ok(state)
    }
}

pub(crate) fn validate_block(block: &Block, identity: &BaselineIdentity) -> Result<()> {
    if block.block_hash() != identity.block_hash
        || !block.check_merkle_root()
        || !block.check_witness_commitment()
    {
        return Err("Genesis block hash, Merkle root or witness commitment mismatch".into());
    }
    Ok(())
}

// Group/sort on disk rather than holding tens of millions of script hashes in RAM.
fn build_required_scripts(conn: &Connection, block: &Block) -> Result<()> {
    sql_stage(conn, "aggregate_utxos", || {
        conn.execute_batch("DROP TABLE IF EXISTS temp.required_scripts;
            CREATE TEMP TABLE required_scripts (script_hash BLOB PRIMARY KEY, balance INTEGER NOT NULL) WITHOUT ROWID;
            INSERT INTO required_scripts SELECT script_hash, sum(value) FROM utxos GROUP BY script_hash;")?;
        Ok(())
    })?;
    let mut insert =
        conn.prepare_cached("INSERT OR IGNORE INTO required_scripts VALUES (?1, 0)")?;
    for tx in &block.txdata {
        for output in &tx.output {
            insert.execute([output.script_pubkey.to_btc_script_hash().as_ref() as &[u8]])?;
        }
    }
    Ok(())
}

// SQLite may sort or check hundreds of millions of rows before returning one.
// Report VM work and elapsed time without presenting it as a completion percentage.
fn sql_stage<T>(
    conn: &Connection,
    stage: &'static str,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let started = Instant::now();
    let mut heartbeat = Instant::now();
    let mut vm_steps = 0u64;
    eprintln!("Baseline verification started: stage={stage}");
    conn.progress_handler(1_000_000, Some(move || {
        vm_steps += 1_000_000;
        if heartbeat.elapsed().as_secs() >= 10 {
            eprintln!("Baseline verification progress: stage={stage}, vm_steps={vm_steps}, elapsed_seconds={:.1}", started.elapsed().as_secs_f64());
            heartbeat = Instant::now();
        }
        false
    }))?;
    let result = work();
    conn.progress_handler(0, None::<fn() -> bool>)?;
    eprintln!(
        "Baseline verification finished: stage={stage}, status={}, elapsed_seconds={:.1}",
        if result.is_ok() { "ok" } else { "failed" },
        started.elapsed().as_secs_f64()
    );
    result
}

fn no_rows(conn: &Connection, sql: &str, error: &str) -> Result<()> {
    let mut query = conn.prepare(sql)?;
    if query.query([])?.next()?.is_some() {
        return Err(error.to_owned().into());
    }
    Ok(())
}

fn schema(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut query = conn.prepare("SELECT type, name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name")?;
    Ok(query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

fn scan_state(conn: &Connection, identity: BaselineIdentity) -> Result<BaselineState> {
    identity.validate()?;
    let template = Connection::open_in_memory()?;
    template.execute_batch(SQL)?;
    if schema(conn)? != schema(&template)? {
        return Err("Baseline SQLite schema mismatch".into());
    }
    let integrity: String = sql_stage(conn, "sqlite_integrity", || {
        Ok(conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?)
    })?;
    if integrity != "ok" {
        return Err(format!("Baseline SQLite integrity check failed: {integrity}").into());
    }
    let raw: Vec<u8> = conn.query_row(
        "SELECT raw_block FROM genesis_block WHERE id=1 AND length(raw_block)<=4000000",
        [],
        |r| r.get(0),
    )?;
    let block: Block = consensus::deserialize(&raw)?;
    validate_block(&block, &identity)?;
    build_required_scripts(conn, &block)?;
    sql_stage(conn, "balance_registry_consistency", || {
        no_rows(conn, "SELECT 1 FROM required_scripts r LEFT JOIN balances b USING(script_hash)
        WHERE (r.balance>0 AND (b.balance IS NULL OR r.balance!=b.balance)) OR (r.balance=0 AND b.balance IS NOT NULL) LIMIT 1",
        "Baseline per-script balance differs from UTXOs")?;
        no_rows(
            conn,
            "SELECT 1 FROM balances b LEFT JOIN required_scripts r USING(script_hash) WHERE r.script_hash IS NULL LIMIT 1",
            "Baseline balance has no corresponding UTXOs",
        )?;
        no_rows(
            conn,
            "SELECT 1 FROM required_scripts r LEFT JOIN script_registry s USING(script_hash) WHERE s.script_hash IS NULL LIMIT 1",
            "Baseline required script mapping is missing",
        )?;
        no_rows(
            conn,
            "SELECT 1 FROM script_registry s LEFT JOIN required_scripts r USING(script_hash) WHERE r.script_hash IS NULL LIMIT 1",
            "Baseline registry contains unrelated historical scripts",
        )?;
        Ok(())
    })?;
    let commit_count: i64 =
        conn.query_row("SELECT count(*) FROM block_commits", [], |r| r.get(0))?;
    let (height, hash): (u32, Vec<u8>) = conn.query_row(
        "SELECT block_height, btc_block_hash FROM block_commits",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if commit_count != 1
        || height != identity.height
        || hash != identity.block_hash.as_ref() as &[u8]
    {
        return Err("Baseline must retain exactly the original genesis commit".into());
    }
    let mut tables = BTreeMap::new();
    // Keys are raw, ascending byte strings. Numeric fields use u32/u64 big-endian;
    // registry scripts and raw block bytes use u64 length prefixes.
    for (name, query) in [
        (
            "balances",
            "SELECT script_hash, balance FROM balances ORDER BY script_hash",
        ),
        (
            "utxos",
            "SELECT outpoint, script_hash, value FROM utxos ORDER BY outpoint",
        ),
        (
            "block_commits",
            "SELECT block_height, btc_block_hash, balance_delta_root, block_commit FROM block_commits ORDER BY block_height",
        ),
        (
            "script_registry",
            "SELECT script_hash, script_pubkey FROM script_registry ORDER BY script_hash",
        ),
        (
            "genesis_block",
            "SELECT raw_block FROM genesis_block ORDER BY id",
        ),
    ] {
        let started = Instant::now();
        let mut heartbeat = Instant::now();
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut statement = conn.prepare(query)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            match name {
                "balances" => {
                    hash.update(row.get::<_, Vec<u8>>(0)?);
                    hash.update(u64::try_from(row.get::<_, i64>(1)?)?.to_be_bytes());
                }
                "utxos" => {
                    hash.update(row.get::<_, Vec<u8>>(0)?);
                    hash.update(row.get::<_, Vec<u8>>(1)?);
                    hash.update(u64::try_from(row.get::<_, i64>(2)?)?.to_be_bytes());
                }
                "block_commits" => {
                    hash.update(row.get::<_, u32>(0)?.to_be_bytes());
                    for i in 1..4 {
                        hash.update(row.get::<_, Vec<u8>>(i)?);
                    }
                }
                "script_registry" => {
                    let key: Vec<u8> = row.get(0)?;
                    let script: Vec<u8> = row.get(1)?;
                    if ScriptBuf::from(script.clone())
                        .to_btc_script_hash()
                        .as_ref() as &[u8]
                        != key
                    {
                        return Err("Baseline registry script/hash mismatch".into());
                    }
                    hash.update(key);
                    hash.update((script.len() as u64).to_be_bytes());
                    hash.update(script);
                }
                _ => {
                    let raw: Vec<u8> = row.get(0)?;
                    hash.update((raw.len() as u64).to_be_bytes());
                    hash.update(raw);
                }
            }
            count += 1;
            if heartbeat.elapsed().as_secs() >= 10 {
                eprintln!(
                    "Baseline verification progress: table={name}, rows={count}, elapsed_seconds={:.1}",
                    started.elapsed().as_secs_f64()
                );
                heartbeat = Instant::now();
            }
        }
        eprintln!(
            "Baseline verification finished: table={name}, rows={count}, elapsed_seconds={:.1}",
            started.elapsed().as_secs_f64()
        );
        tables.insert(
            name.to_owned(),
            BaselineTableDigest {
                rows: count,
                sha256: crate::assumeutxo::format::hex(&hash.finalize()),
            },
        );
    }
    Ok(BaselineState { identity, tables })
}

pub(crate) fn file_hash(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut bytes = 0u64;
    let started = Instant::now();
    let mut heartbeat = Instant::now();
    eprintln!(
        "Baseline file verification started: path={}",
        path.display()
    );
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
        if heartbeat.elapsed().as_secs() >= 10 {
            eprintln!(
                "Baseline file verification progress: bytes={bytes}, elapsed_seconds={:.1}",
                started.elapsed().as_secs_f64()
            );
            heartbeat = Instant::now();
        }
    }
    eprintln!(
        "Baseline file verification finished: bytes={bytes}, elapsed_seconds={:.1}",
        started.elapsed().as_secs_f64()
    );
    Ok(crate::assumeutxo::format::hex(&hash.finalize()))
}

/// Verify a signed baseline artifact without installing it or querying a Bitcoin node.
/// The caller must independently pin the expected network/G/hash before deployment.
pub fn verify_baseline_snapshot(
    manifest_path: &Path,
    trusted_keys: &Path,
) -> std::result::Result<BaselineManifest, String> {
    verify(manifest_path, trusted_keys).map_err(|error| {
        let message = format!(
            "Baseline snapshot verification failed: manifest={}, error={error}",
            manifest_path.display()
        );
        log::error!("{message}");
        message
    })
}

fn verify(manifest_path: &Path, trusted_keys: &Path) -> Result<BaselineManifest> {
    if fs::metadata(manifest_path)?.len() > 1024 * 1024 {
        return Err("Baseline manifest exceeds size limit".into());
    }
    let manifest: BaselineManifest = parse_json_strict(&fs::read_to_string(manifest_path)?)?;
    manifest.state.identity.validate()?;
    if manifest.manifest_version != BASELINE_MANIFEST
        || manifest.snapshot_schema_version != BASELINE_SCHEMA
        || manifest.file_name
            != format!(
                "balance_history_baseline_{}.db",
                manifest.state.identity.height
            )
        || manifest.logical_sha256 != manifest.state.logical_sha256()?
    {
        return Err("Unsupported or inconsistent baseline manifest".into());
    }
    verify_snapshot_artifact_manifest_signature(
        Some(&manifest.signature_scheme),
        Some(&manifest.signing_key_id),
        manifest_path,
        &manifest.signature_payload()?,
        trusted_keys,
    )?;
    let path = manifest_path
        .parent()
        .ok_or("Baseline manifest has no parent directory")?
        .join(&manifest.file_name);
    // A signed single-file DB must not acquire unsigned journal/WAL contents.
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = path.with_file_name(format!("{}{suffix}", manifest.file_name));
        if sidecar.try_exists()? {
            return Err(format!(
                "Baseline SQLite sidecar is not allowed: {}",
                sidecar.display()
            )
            .into());
        }
    }
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || metadata.len() != manifest.file_size
        || file_hash(&path)? != manifest.file_sha256
    {
        return Err("Baseline snapshot file identity mismatch".into());
    }
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch(
        "PRAGMA trusted_schema=OFF; PRAGMA temp_store=FILE; PRAGMA cache_size=-65536; BEGIN",
    )?;
    let stored: String = conn.query_row(
        "SELECT state_json FROM meta WHERE id=1 AND length(state_json)<=1048576",
        [],
        |r| r.get(0),
    )?;
    let stored: BaselineState = parse_json_strict(&stored)?;
    if stored != manifest.state
        || scan_state(&conn, manifest.state.identity.clone())? != manifest.state
    {
        return Err("Baseline logical state differs from signed manifest".into());
    }
    super::export::validate_source(&manifest.source, &conn, &manifest.state.identity)?;
    conn.close().map_err(|(_, error)| error)?;
    let after = fs::symlink_metadata(&path)?;
    if !after.is_file()
        || after.len() != metadata.len()
        || after.modified()? != metadata.modified()?
    {
        return Err("Baseline snapshot changed during verification".into());
    }
    Ok(manifest)
}
