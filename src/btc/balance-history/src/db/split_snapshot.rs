use super::{BalanceHistoryDBIdentity, BalanceHistoryEntry, BlockCommitEntry, ScriptRegistryEntry};
use crate::snapshot_contract::{
    CORE_SNAPSHOT_SCHEMA_VERSION, CORE_SNAPSHOT_SQL_SCHEMA_V1, SCRIPT_REGISTRY_POLICY,
    SCRIPT_REGISTRY_SCHEMA_VERSION, SCRIPT_REGISTRY_SQL_SCHEMA_V1, ScriptRegistryBaseIdentity,
};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, OutPoint};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::time::Instant;
use usdb_util::{BtcScriptHash, OutPointCodec, ToBtcScriptHash, UTXOEntry, parse_json_strict};

const SNAPSHOT_WRITE_CACHE_KIB: i64 = 40_000;

/// Metadata stored inside a registry-free core snapshot SQLite artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreSnapshotMeta {
    /// Exact BTC height represented by the artifact.
    pub block_height: u32,
    /// Number of exported current balance rows.
    pub balance_history_count: u64,
    /// Number of exported live UTXOs.
    pub utxo_count: u64,
    /// Number of exported block commitments.
    pub block_commit_count: u64,
    /// Manifest-compatible generation timestamp in Unix seconds.
    pub generated_at: u64,
    /// Identity of the RocksDB state domain that produced this artifact.
    pub db_identity: BalanceHistoryDBIdentity,
    /// Consensus snapshot identity represented by this artifact.
    pub core_snapshot_id: String,
}

/// Metadata stored inside a standalone script-registry SQLite artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptRegistrySnapshotMeta {
    /// BTC and core checkpoint identity represented by the registry.
    pub base: ScriptRegistryBaseIdentity,
    /// Exact number of exported registry mappings.
    pub entry_count: u64,
    /// Manifest-compatible generation timestamp in Unix seconds.
    pub generated_at: u64,
}

/// Basic SQLite storage metrics used by snapshot capacity reports and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotSqliteStorageMetrics {
    /// SQLite page size in bytes.
    pub page_size: u64,
    /// Number of pages currently allocated to the database.
    pub page_count: u64,
    /// Number of unused pages currently on the freelist.
    pub freelist_count: u64,
}

impl SnapshotSqliteStorageMetrics {
    /// Returns the allocated main database size represented by SQLite pages.
    pub fn allocated_bytes(self) -> u64 {
        self.page_size.saturating_mul(self.page_count)
    }
}

/// Writer and verifier for the registry-free core snapshot schema.
pub struct CoreSnapshotDb {
    path: PathBuf,
    conn: Connection,
}

/// Writer and verifier for the standalone script-registry schema.
pub struct ScriptRegistrySnapshotDb {
    path: PathBuf,
    conn: Connection,
}

impl CoreSnapshotDb {
    /// Creates a new core snapshot database and rejects an existing output path.
    pub fn create(path: &Path) -> Result<Self, String> {
        create_database(path, CORE_SNAPSHOT_SQL_SCHEMA_V1, "core snapshot").map(|conn| Self {
            path: path.to_path_buf(),
            conn,
        })
    }

    /// Opens one finalized core snapshot for an offline verification pass.
    pub fn open_for_verification(path: &Path, cache_size_kib: u32) -> Result<Self, String> {
        open_database_for_verification(path, cache_size_kib, "core snapshot")
            .map(|(path, conn)| Self { path, conn })
    }

    /// Inserts one batch of current balance rows.
    pub fn put_balance_history_entries(
        &mut self,
        entries: &[BalanceHistoryEntry],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("Failed to start core balance transaction: {error}"))?;
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO balance_history (script_hash, height, balance, delta) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|error| format!("Failed to prepare core balance insert: {error}"))?;
            for entry in entries {
                let balance = to_sqlite_i64("core balance", entry.balance)?;
                statement
                    .execute((
                        entry.script_hash.as_ref() as &[u8],
                        i64::from(entry.block_height),
                        balance,
                        entry.delta,
                    ))
                    .map_err(|error| format!("Failed to insert core balance row: {error}"))?;
            }
        }
        tx.commit()
            .map_err(|error| format!("Failed to commit core balance transaction: {error}"))
    }

    /// Inserts one batch of live UTXOs.
    pub fn put_utxo_entries(&mut self, entries: &[UTXOEntry]) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("Failed to start core UTXO transaction: {error}"))?;
        {
            let mut statement = tx
                .prepare("INSERT INTO utxos (outpoint, script_hash, value) VALUES (?1, ?2, ?3)")
                .map_err(|error| format!("Failed to prepare core UTXO insert: {error}"))?;
            for entry in entries {
                let value = to_sqlite_i64("core UTXO value", entry.value)?;
                statement
                    .execute((
                        OutPointCodec::encode(&entry.outpoint),
                        entry.script_hash.as_ref() as &[u8],
                        value,
                    ))
                    .map_err(|error| format!("Failed to insert core UTXO row: {error}"))?;
            }
        }
        tx.commit()
            .map_err(|error| format!("Failed to commit core UTXO transaction: {error}"))
    }

    /// Inserts one batch of block commitments.
    pub fn put_block_commit_entries(&mut self, entries: &[BlockCommitEntry]) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("Failed to start core block-commit transaction: {error}"))?;
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO block_commits (block_height, btc_block_hash, balance_delta_root, block_commit) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|error| format!("Failed to prepare core block-commit insert: {error}"))?;
            for entry in entries {
                statement
                    .execute((
                        i64::from(entry.block_height),
                        entry.btc_block_hash.as_ref() as &[u8],
                        &entry.balance_delta_root[..],
                        &entry.block_commit[..],
                    ))
                    .map_err(|error| format!("Failed to insert core block-commit row: {error}"))?;
            }
        }
        tx.commit()
            .map_err(|error| format!("Failed to commit core block-commit transaction: {error}"))
    }

    /// Writes the single authoritative metadata row after all data batches are complete.
    pub fn write_meta(&self, meta: &CoreSnapshotMeta) -> Result<(), String> {
        let db_identity_json = serde_json::to_string(&meta.db_identity)
            .map_err(|error| format!("Failed to serialize core DB identity: {error}"))?;
        let balance_history_count =
            to_sqlite_i64("core balance_history_count", meta.balance_history_count)?;
        let utxo_count = to_sqlite_i64("core utxo_count", meta.utxo_count)?;
        let block_commit_count = to_sqlite_i64("core block_commit_count", meta.block_commit_count)?;
        let generated_at = to_sqlite_i64("core generated_at", meta.generated_at)?;
        self.conn
            .execute(
                "INSERT INTO meta (id, block_height, balance_history_count, utxo_count, block_commit_count, generated_at, schema_version, db_identity_json, core_snapshot_id) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    i64::from(meta.block_height),
                    balance_history_count,
                    utxo_count,
                    block_commit_count,
                    generated_at,
                    CORE_SNAPSHOT_SCHEMA_VERSION,
                    db_identity_json,
                    &meta.core_snapshot_id,
                ),
            )
            .map_err(|error| format!("Failed to write core snapshot metadata: {error}"))?;
        Ok(())
    }

    /// Loads and validates the single core metadata row.
    pub fn read_meta(&self) -> Result<CoreSnapshotMeta, String> {
        let row = self
            .conn
            .query_row(
                "SELECT block_height, balance_history_count, utxo_count, block_commit_count, generated_at, schema_version, db_identity_json, core_snapshot_id FROM meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .map_err(|error| format!("Failed to read core snapshot metadata: {error}"))?;
        if row.5 != CORE_SNAPSHOT_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported core snapshot schema {} (expected {})",
                row.5, CORE_SNAPSHOT_SCHEMA_VERSION
            ));
        }
        let db_identity = parse_json_strict(&row.6)
            .map_err(|error| format!("Failed to decode core DB identity: {error}"))?;
        Ok(CoreSnapshotMeta {
            block_height: checked_u32("core block_height", row.0)?,
            balance_history_count: checked_u64("core balance_history_count", row.1)?,
            utxo_count: checked_u64("core utxo_count", row.2)?,
            block_commit_count: checked_u64("core block_commit_count", row.3)?,
            generated_at: checked_u64("core generated_at", row.4)?,
            db_identity,
            core_snapshot_id: row.7,
        })
    }

    /// Returns the exact number of current balance rows.
    pub fn balance_history_count(&self) -> Result<u64, String> {
        count_rows(&self.conn, "balance_history")
    }

    /// Returns the exact number of live UTXO rows.
    pub fn utxo_count(&self) -> Result<u64, String> {
        count_rows(&self.conn, "utxos")
    }

    /// Returns the exact number of block commitment rows.
    pub fn block_commit_count(&self) -> Result<u64, String> {
        count_rows(&self.conn, "block_commits")
    }

    /// Returns the last block commitment in height order.
    pub fn latest_block_commit(&self) -> Result<Option<BlockCommitEntry>, String> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT block_height, btc_block_hash, balance_delta_root, block_commit FROM block_commits ORDER BY block_height DESC LIMIT 1",
            )
            .map_err(|error| format!("Failed to prepare latest core block-commit query: {error}"))?;
        let mut rows = statement
            .query([])
            .map_err(|error| format!("Failed to query latest core block commit: {error}"))?;
        let Some(row) = rows
            .next()
            .map_err(|error| format!("Failed to read latest core block commit: {error}"))?
        else {
            return Ok(None);
        };
        Ok(Some(decode_block_commit_row(row)?))
    }

    /// Verifies the frozen table set and confirms the registry table is absent.
    pub fn verify_schema(&self) -> Result<(), String> {
        verify_table_set(
            &self.conn,
            &["balance_history", "block_commits", "meta", "utxos"],
            "core snapshot",
        )
    }

    /// Runs SQLite's full integrity check.
    pub fn verify_integrity(&self) -> Result<(), String> {
        verify_integrity(&self.conn, &self.path, "core snapshot")
    }

    /// Returns page-level storage metrics for capacity reporting.
    pub fn storage_metrics(&self) -> Result<SnapshotSqliteStorageMetrics, String> {
        storage_metrics(&self.conn)
    }

    /// Checkpoints WAL content and closes the writer before hashing.
    pub fn finalize_for_distribution(self) -> Result<PathBuf, String> {
        finalize_database(self.conn, self.path, "core snapshot")
    }
}

impl ScriptRegistrySnapshotDb {
    /// Creates a new standalone registry database and rejects an existing output path.
    pub fn create(path: &Path) -> Result<Self, String> {
        create_database(path, SCRIPT_REGISTRY_SQL_SCHEMA_V1, "script registry").map(|conn| Self {
            path: path.to_path_buf(),
            conn,
        })
    }

    /// Opens one finalized registry sidecar for an offline verification pass.
    pub fn open_for_verification(path: &Path, cache_size_kib: u32) -> Result<Self, String> {
        open_database_for_verification(path, cache_size_kib, "script registry")
            .map(|(path, conn)| Self { path, conn })
    }

    /// Inserts or confirms one batch of canonical registry mappings.
    pub fn put_entries(&mut self, entries: &[ScriptRegistryEntry]) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|error| format!("Failed to start script-registry transaction: {error}"))?;
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO script_registry (script_hash, script_pubkey) VALUES (?1, ?2) ON CONFLICT(script_hash) DO UPDATE SET script_pubkey=excluded.script_pubkey WHERE script_pubkey=excluded.script_pubkey",
                )
                .map_err(|error| format!("Failed to prepare script-registry insert: {error}"))?;
            for entry in entries {
                let calculated = entry.script_pubkey.to_btc_script_hash();
                if calculated != entry.script_hash {
                    return Err(format!(
                        "Script registry hash mismatch: recorded={}, calculated={}",
                        entry.script_hash, calculated
                    ));
                }
                let changed = statement
                    .execute((
                        entry.script_hash.as_ref() as &[u8],
                        entry.script_pubkey.as_bytes(),
                    ))
                    .map_err(|error| format!("Failed to insert script-registry row: {error}"))?;
                if changed == 0 {
                    return Err(format!(
                        "Conflicting script registry mapping for {}",
                        entry.script_hash
                    ));
                }
            }
        }
        tx.commit()
            .map_err(|error| format!("Failed to commit script-registry transaction: {error}"))
    }

    /// Writes the single authoritative registry metadata row.
    pub fn write_meta(&self, meta: &ScriptRegistrySnapshotMeta) -> Result<(), String> {
        let entry_count = to_sqlite_i64("registry entry_count", meta.entry_count)?;
        let generated_at = to_sqlite_i64("registry generated_at", meta.generated_at)?;
        self.conn
            .execute(
                "INSERT INTO meta (id, schema_version, policy, btc_network, btc_genesis_hash, base_height, base_block_hash, core_snapshot_id, entry_count, generated_at) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                (
                    SCRIPT_REGISTRY_SCHEMA_VERSION,
                    SCRIPT_REGISTRY_POLICY,
                    &meta.base.btc_network,
                    &meta.base.btc_genesis_hash,
                    i64::from(meta.base.base_height),
                    &meta.base.base_block_hash,
                    &meta.base.core_snapshot_id,
                    entry_count,
                    generated_at,
                ),
            )
            .map_err(|error| format!("Failed to write script-registry metadata: {error}"))?;
        Ok(())
    }

    /// Loads and validates the single registry metadata row.
    pub fn read_meta(&self) -> Result<ScriptRegistrySnapshotMeta, String> {
        let row = self
            .conn
            .query_row(
                "SELECT schema_version, policy, btc_network, btc_genesis_hash, base_height, base_block_hash, core_snapshot_id, entry_count, generated_at FROM meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                },
            )
            .map_err(|error| format!("Failed to read script-registry metadata: {error}"))?;
        if row.0 != SCRIPT_REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported script-registry schema {} (expected {})",
                row.0, SCRIPT_REGISTRY_SCHEMA_VERSION
            ));
        }
        if row.1 != SCRIPT_REGISTRY_POLICY {
            return Err(format!(
                "Unsupported script-registry policy {} (expected {})",
                row.1, SCRIPT_REGISTRY_POLICY
            ));
        }
        Ok(ScriptRegistrySnapshotMeta {
            base: ScriptRegistryBaseIdentity {
                btc_network: row.2,
                btc_genesis_hash: row.3,
                base_height: checked_u32("registry base_height", row.4)?,
                base_block_hash: row.5,
                core_snapshot_id: row.6,
            },
            entry_count: checked_u64("registry entry_count", row.7)?,
            generated_at: checked_u64("registry generated_at", row.8)?,
        })
    }

    /// Returns the exact number of registry mappings.
    pub fn entry_count(&self) -> Result<u64, String> {
        count_rows(&self.conn, "script_registry")
    }

    /// Verifies the frozen table set and `WITHOUT ROWID` storage contract.
    pub fn verify_schema(&self) -> Result<(), String> {
        verify_table_set(&self.conn, &["meta", "script_registry"], "script registry")?;
        let sql: String = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='script_registry'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("Failed to inspect script-registry schema: {error}"))?;
        if !sql.to_ascii_uppercase().contains("WITHOUT ROWID") {
            return Err("Script-registry table must use WITHOUT ROWID".to_string());
        }
        Ok(())
    }

    /// Runs SQLite's full integrity check.
    pub fn verify_integrity(&self) -> Result<(), String> {
        verify_integrity(&self.conn, &self.path, "script registry")
    }

    /// Returns page-level storage metrics for capacity reporting.
    pub fn storage_metrics(&self) -> Result<SnapshotSqliteStorageMetrics, String> {
        storage_metrics(&self.conn)
    }

    /// Checkpoints WAL content and closes the writer before hashing.
    pub fn finalize_for_distribution(self) -> Result<PathBuf, String> {
        finalize_database(self.conn, self.path, "script registry")
    }
}

fn create_database(path: &Path, schema: &str, label: &str) -> Result<Connection, String> {
    if path.exists() {
        return Err(format!("{label} output already exists: {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create {label} directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let started = Instant::now();
    let conn = Connection::open(path)
        .map_err(|error| format!("Failed to create {label} {}: {error}", path.display()))?;
    conn.execute_batch(schema)
        .and_then(|_| conn.pragma_update(None, "journal_mode", "WAL"))
        .and_then(|_| conn.pragma_update(None, "synchronous", "NORMAL"))
        .and_then(|_| conn.pragma_update(None, "cache_size", -SNAPSHOT_WRITE_CACHE_KIB))
        .map_err(|error| format!("Failed to initialize {label} {}: {error}", path.display()))?;
    info!(
        "Created split snapshot SQLite: component={}, path={}, elapsed_ms={}",
        label,
        path.display(),
        started.elapsed().as_millis()
    );
    Ok(conn)
}

fn open_database_for_verification(
    path: &Path,
    cache_size_kib: u32,
    label: &str,
) -> Result<(PathBuf, Connection), String> {
    if cache_size_kib == 0 {
        return Err(format!(
            "{label} verification cache must be greater than zero"
        ));
    }
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("Failed to canonicalize {label} {}: {error}", path.display()))?;
    let mut uri = url::Url::from_file_path(&canonical_path).map_err(|_| {
        format!(
            "Failed to encode {label} path as file URI: {}",
            canonical_path.display()
        )
    })?;
    uri.query_pairs_mut().append_pair("immutable", "1");
    let conn = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| {
        format!(
            "Failed to open {label} {}: {error}",
            canonical_path.display()
        )
    })?;
    conn.pragma_update(None, "query_only", true)
        .and_then(|_| conn.pragma_update(None, "cache_size", -i64::from(cache_size_kib)))
        .and_then(|_| conn.pragma_update(None, "temp_store", "MEMORY"))
        .map_err(|error| format!("Failed to configure {label} verification: {error}"))?;
    Ok((canonical_path, conn))
}

fn finalize_database(conn: Connection, path: PathBuf, label: &str) -> Result<PathBuf, String> {
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| format!("Failed to checkpoint {label} {}: {error}", path.display()))?;
    drop(conn);
    Ok(path)
}

fn verify_integrity(conn: &Connection, path: &Path, label: &str) -> Result<(), String> {
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| format!("Failed to run {label} integrity check: {error}"))?;
    if result != "ok" {
        return Err(format!(
            "{label} integrity check failed for {}: {result}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_table_set(conn: &Connection, expected: &[&str], label: &str) -> Result<(), String> {
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .map_err(|error| format!("Failed to inspect {label} table set: {error}"))?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("Failed to query {label} table set: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to decode {label} table set: {error}"))?;
    if actual != expected {
        return Err(format!(
            "Unexpected {label} table set: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn count_rows(conn: &Connection, table: &str) -> Result<u64, String> {
    let count: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .map_err(|error| format!("Failed to count {table} rows: {error}"))?;
    checked_u64(&format!("{table} row count"), count)
}

fn storage_metrics(conn: &Connection) -> Result<SnapshotSqliteStorageMetrics, String> {
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| format!("Failed to query SQLite page_size: {error}"))?;
    let page_count: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|error| format!("Failed to query SQLite page_count: {error}"))?;
    let freelist_count: i64 = conn
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .map_err(|error| format!("Failed to query SQLite freelist_count: {error}"))?;
    Ok(SnapshotSqliteStorageMetrics {
        page_size: checked_u64("SQLite page_size", page_size)?,
        page_count: checked_u64("SQLite page_count", page_count)?,
        freelist_count: checked_u64("SQLite freelist_count", freelist_count)?,
    })
}

fn decode_block_commit_row(row: &rusqlite::Row<'_>) -> Result<BlockCommitEntry, String> {
    let block_height = checked_u32(
        "block commit height",
        row.get(0)
            .map_err(|error| format!("Failed to decode block commit height: {error}"))?,
    )?;
    let hash_bytes: Vec<u8> = row
        .get(1)
        .map_err(|error| format!("Failed to decode block hash: {error}"))?;
    let btc_block_hash = BlockHash::from_slice(&hash_bytes)
        .map_err(|error| format!("Invalid block hash in core snapshot: {error}"))?;
    let balance_delta_root = decode_array_32(row, 2, "balance delta root")?;
    let block_commit = decode_array_32(row, 3, "block commit")?;
    Ok(BlockCommitEntry {
        block_height,
        btc_block_hash,
        balance_delta_root,
        block_commit,
    })
}

fn decode_array_32(row: &rusqlite::Row<'_>, index: usize, field: &str) -> Result<[u8; 32], String> {
    let bytes: Vec<u8> = row
        .get(index)
        .map_err(|error| format!("Failed to decode {field}: {error}"))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("Invalid {field} length {}; expected 32", bytes.len()))
}

fn checked_u64(field: &str, value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} must be non-negative, got {value}"))
}

fn checked_u32(field: &str, value: i64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field} is outside u32 range: {value}"))
}

fn to_sqlite_i64(field: &str, value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{field} exceeds SQLite INTEGER range: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::{ScriptBuf, Txid};
    use std::str::FromStr;
    use usdb_util::ToBtcScriptHash;

    fn temp_file(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "balance-history-split-snapshot-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.join(format!("{name}.db"))
    }

    fn db_identity() -> BalanceHistoryDBIdentity {
        BalanceHistoryDBIdentity {
            identity_version: "balance-history-db-identity:v1".to_string(),
            service: "balance-history".to_string(),
            schema_version: "balance-history-rocksdb-schema:v1".to_string(),
            data_model_version: "test-model".to_string(),
            btc_network: "regtest".to_string(),
            btc_genesis_hash: "11".repeat(32),
        }
    }

    #[test]
    fn core_snapshot_round_trip_has_no_registry_table() {
        let path = temp_file("core");
        let script_hash = ScriptBuf::from_bytes(vec![0x51]).to_btc_script_hash();
        let outpoint = OutPoint {
            txid: Txid::from_str(&"22".repeat(32)).unwrap(),
            vout: 1,
        };
        let mut db = CoreSnapshotDb::create(&path).unwrap();
        db.put_balance_history_entries(&[BalanceHistoryEntry {
            script_hash,
            block_height: 7,
            balance: 50,
            delta: 50,
        }])
        .unwrap();
        db.put_utxo_entries(&[UTXOEntry {
            outpoint,
            script_hash,
            value: 50,
        }])
        .unwrap();
        db.put_block_commit_entries(&[BlockCommitEntry {
            block_height: 7,
            btc_block_hash: BlockHash::from_slice(&[3; 32]).unwrap(),
            balance_delta_root: [4; 32],
            block_commit: [5; 32],
        }])
        .unwrap();
        let meta = CoreSnapshotMeta {
            block_height: 7,
            balance_history_count: 1,
            utxo_count: 1,
            block_commit_count: 1,
            generated_at: 9,
            db_identity: db_identity(),
            core_snapshot_id: "66".repeat(32),
        };
        db.write_meta(&meta).unwrap();
        db.verify_schema().unwrap();
        db.verify_integrity().unwrap();
        assert_eq!(db.read_meta().unwrap(), meta);
        assert_eq!(db.balance_history_count().unwrap(), 1);
        assert_eq!(db.utxo_count().unwrap(), 1);
        assert_eq!(db.block_commit_count().unwrap(), 1);
        assert_eq!(db.latest_block_commit().unwrap().unwrap().block_height, 7);
        db.finalize_for_distribution().unwrap();

        let db = CoreSnapshotDb::open_for_verification(&path, 4096).unwrap();
        db.verify_schema().unwrap();
        assert_eq!(db.read_meta().unwrap(), meta);
    }

    #[test]
    fn registry_snapshot_round_trip_uses_without_rowid() {
        let path = temp_file("registry");
        let script = ScriptBuf::from_bytes(vec![0x51]);
        let script_hash = script.to_btc_script_hash();
        let mut db = ScriptRegistrySnapshotDb::create(&path).unwrap();
        db.put_entries(&[ScriptRegistryEntry {
            script_hash,
            script_pubkey: script,
        }])
        .unwrap();
        let meta = ScriptRegistrySnapshotMeta {
            base: ScriptRegistryBaseIdentity {
                btc_network: "regtest".to_string(),
                btc_genesis_hash: "11".repeat(32),
                base_height: 7,
                base_block_hash: "22".repeat(32),
                core_snapshot_id: "33".repeat(32),
            },
            entry_count: 1,
            generated_at: 9,
        };
        db.write_meta(&meta).unwrap();
        db.verify_schema().unwrap();
        db.verify_integrity().unwrap();
        assert_eq!(db.read_meta().unwrap(), meta);
        assert_eq!(db.entry_count().unwrap(), 1);
        assert!(db.storage_metrics().unwrap().allocated_bytes() > 0);
        db.finalize_for_distribution().unwrap();

        let db = ScriptRegistrySnapshotDb::open_for_verification(&path, 4096).unwrap();
        db.verify_schema().unwrap();
        assert_eq!(db.read_meta().unwrap(), meta);
    }

    #[test]
    fn registry_rejects_script_hash_mismatch() {
        let path = temp_file("registry-conflict");
        let first = ScriptBuf::from_bytes(vec![0x51]);
        let script_hash = first.to_btc_script_hash();
        let mut db = ScriptRegistrySnapshotDb::create(&path).unwrap();
        db.put_entries(&[ScriptRegistryEntry {
            script_hash,
            script_pubkey: first,
        }])
        .unwrap();
        let error = db
            .put_entries(&[ScriptRegistryEntry {
                script_hash,
                script_pubkey: ScriptBuf::from_bytes(vec![0x52]),
            }])
            .unwrap_err();
        assert!(error.contains("Script registry hash mismatch"));
    }

    #[test]
    fn split_snapshot_rejects_values_outside_sqlite_integer_range() {
        let path = temp_file("core-integer-overflow");
        let db = CoreSnapshotDb::create(&path).unwrap();
        let error = db
            .write_meta(&CoreSnapshotMeta {
                block_height: 7,
                balance_history_count: u64::MAX,
                utxo_count: 0,
                block_commit_count: 0,
                generated_at: 9,
                db_identity: db_identity(),
                core_snapshot_id: "66".repeat(32),
            })
            .unwrap_err();

        assert!(error.contains("exceeds SQLite INTEGER range"));
    }

    #[test]
    fn without_rowid_registry_is_smaller_than_rowid_baseline() {
        let path = temp_file("registry-capacity");
        let rowid_path = path.with_file_name("registry-rowid.db");
        let mut entries = Vec::with_capacity(20_000);
        for value in 0u32..20_000 {
            let script = ScriptBuf::from_bytes(value.to_be_bytes().to_vec());
            entries.push(ScriptRegistryEntry {
                script_hash: script.to_btc_script_hash(),
                script_pubkey: script,
            });
        }

        let mut registry = ScriptRegistrySnapshotDb::create(&path).unwrap();
        for batch in entries.chunks(1_000) {
            registry.put_entries(batch).unwrap();
        }
        registry.finalize_for_distribution().unwrap();

        let mut rowid = Connection::open(&rowid_path).unwrap();
        rowid
            .execute_batch(
                "CREATE TABLE script_registry (script_hash BLOB NOT NULL PRIMARY KEY, script_pubkey BLOB NOT NULL);",
            )
            .unwrap();
        for batch in entries.chunks(1_000) {
            let tx = rowid.transaction().unwrap();
            {
                let mut statement = tx
                    .prepare(
                        "INSERT INTO script_registry (script_hash, script_pubkey) VALUES (?1, ?2)",
                    )
                    .unwrap();
                for entry in batch {
                    statement
                        .execute((
                            entry.script_hash.as_ref() as &[u8],
                            entry.script_pubkey.as_bytes(),
                        ))
                        .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        drop(rowid);

        let without_rowid_bytes = std::fs::metadata(&path).unwrap().len();
        let rowid_bytes = std::fs::metadata(&rowid_path).unwrap().len();
        assert!(
            without_rowid_bytes < rowid_bytes,
            "WITHOUT ROWID registry should use less storage: without_rowid={without_rowid_bytes}, rowid={rowid_bytes}"
        );
    }
}
