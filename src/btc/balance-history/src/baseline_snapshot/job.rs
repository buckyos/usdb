//! Durable baseline export jobs. Batch data and the resume cursor commit together.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoincore_rpc::bitcoin::{Block, consensus};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use usdb_util::{ToBtcScriptHash, parse_json_strict};

use super::source::Stamp;
use super::{
    BASELINE_MANIFEST, BASELINE_SCHEMA, BaselineIdentity, BaselineManifest, BaselineSource, Result,
    storage,
};
use crate::index::sign_snapshot_artifact_manifest;
use crate::{SnapshotSigningKeyFile, SnapshotTrustedKeySet};

const JOB_SCHEMA: &str = "balance-history-baseline-job:v1";

/// Explicit stationary source for a resumable producer job.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaselineJobSource {
    /// A stopped full-replay or sealed native service database.
    Rocksdb {
        /// Service root containing db/balance_history.
        root: PathBuf,
    },
    /// Existing signed and paired split artifacts, converted without restoring RocksDB.
    LegacySplit {
        /// Original signed core manifest; DB/signature must be adjacent.
        core_manifest: PathBuf,
        /// Original signed registry manifest; DB/signature must be adjacent.
        registry_manifest: PathBuf,
    },
}

/// Immutable operator inputs frozen when the job is first created.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineJobInput {
    /// Required network, exact business genesis and query boundaries.
    pub identity: BaselineIdentity,
    /// Source whose bytes must stay unchanged throughout the data stage.
    pub source: BaselineJobSource,
    /// Raw G block; copied into the job so later verification is independent of the source.
    pub genesis_block_file: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schema_version: String,
    input: BaselineJobInput,
    signing_key_id: String,
    trusted_keys_sha256: String,
    created_at: u64,
    stage: String,
    source: Option<BaselineSource>,
    source_files: BTreeMap<PathBuf, Stamp>,
    manifest: Option<BaselineManifest>,
}

/// Compact status for operator polling and release orchestration.
#[derive(Clone, Debug, Serialize)]
pub struct BaselineJobReport {
    /// Durable stage; status alone does not verify an artifact.
    pub stage: String,
    /// Bound business identity.
    pub identity: BaselineIdentity,
    /// Export phase plus the number of input rows durably consumed in that phase.
    pub checkpoint: Option<(String, i64)>,
    /// Final directory, present only after publication.
    pub artifact_dir: Option<PathBuf>,
    /// Signed manifest file, present only after publication.
    pub manifest_file: Option<PathBuf>,
    /// Independent state identity from the completed artifact.
    pub logical_sha256: Option<String>,
}

pub(super) fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    // Only the lock holder writes this small, replaceable job file.
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(path.parent().ok_or("Job file has no parent")?)?.sync_all()?;
    Ok(())
}

fn load_job(root: &Path) -> Result<Job> {
    let path = root.join("job.json");
    if fs::metadata(&path)?.len() > 1024 * 1024 {
        return Err("Baseline job exceeds size limit".into());
    }
    let job: Job = parse_json_strict(&fs::read_to_string(path)?)?;
    job.input.identity.validate()?;
    if job.schema_version != JOB_SCHEMA
        || !["prepared", "exporting", "verifying", "complete"].contains(&job.stage.as_str())
    {
        return Err("Unsupported baseline job state".into());
    }
    Ok(job)
}

fn report(root: &Path, job: &Job) -> BaselineJobReport {
    let complete = job.stage == "complete";
    let file = format!(
        "balance_history_baseline_{}.manifest.json",
        job.input.identity.height
    );
    let checkpoint = if job.stage == "exporting" {
        rusqlite::Connection::open_with_flags(
            root.join("staging").join(format!(
                "balance_history_baseline_{}.db",
                job.input.identity.height
            )),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()
        .and_then(|db| {
            db.query_row(
                "SELECT stage, processed FROM export_checkpoint WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
        })
    } else {
        None
    };
    BaselineJobReport {
        stage: job.stage.clone(),
        identity: job.input.identity.clone(),
        checkpoint,
        artifact_dir: complete.then(|| root.join("artifact")),
        manifest_file: complete.then(|| root.join("artifact").join(file)),
        logical_sha256: job
            .manifest
            .as_ref()
            .map(|manifest| manifest.logical_sha256.clone()),
    }
}

/// Read persisted status without running verification, opening sources or starting services.
pub fn baseline_job_status(root: &Path) -> std::result::Result<BaselineJobReport, String> {
    load_job(root)
        .map(|job| report(root, &job))
        .map_err(|e| format!("Baseline status failed: root={}, error={e}", root.display()))
}

/// Create/resume a locked job, returning only after the signed single-file output verifies.
///
/// Input is required for a new job; a retry may omit it and use the frozen inputs.
/// The observer runs after durable checkpoints, permitting cooperative cancellation.
/// Verification restarts its scans, while completed export batches are reused.
pub fn create_or_resume_baseline(
    root: &Path,
    input: Option<BaselineJobInput>,
    signing_key: &Path,
    trusted_keys: &Path,
    batch_size: usize,
    observer: &dyn Fn(&str) -> std::result::Result<(), String>,
) -> std::result::Result<BaselineJobReport, String> {
    run(root, input, signing_key, trusted_keys, batch_size, observer).map_err(|error| {
        let message = format!(
            "Baseline job failed: root={}, error={error}",
            root.display()
        );
        log::error!("{message}");
        message
    })
}

fn run(
    root: &Path,
    input: Option<BaselineJobInput>,
    signing_key: &Path,
    trusted_keys: &Path,
    batch_size: usize,
    observer: &dyn Fn(&str) -> std::result::Result<(), String>,
) -> Result<BaselineJobReport> {
    if !(1..=20_000).contains(&batch_size) {
        return Err("Baseline batch size must be in 1..=20000".into());
    }
    if root.is_symlink() {
        return Err("Baseline job root must not be a symlink".into());
    }
    if !root.exists() && input.is_none() {
        return Err("New baseline job requires source inputs".into());
    }
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    lock.try_lock()
        .map_err(|e| format!("Another baseline job holds this workspace: {e}"))?;
    let key = SnapshotSigningKeyFile::load(signing_key)?;
    if signing_key.canonicalize()?.starts_with(&root) {
        return Err("Signing key must be outside the job directory".into());
    }
    let trust_hash = storage::file_hash(trusted_keys)?;
    if SnapshotTrustedKeySet::load(trusted_keys)?.find_verifying_key(&key.key_id)?
        != Some(key.to_signing_key()?.verifying_key())
    {
        return Err("Snapshot signer differs from trusted-key catalog".into());
    }
    let input = input
        .map(|mut value| -> Result<_> {
            value.identity.validate()?;
            value.source = value.source.normalize()?;
            value.genesis_block_file = value.genesis_block_file.canonicalize()?;
            Ok(value)
        })
        .transpose()?;
    let mut job = if root.join("job.json").exists() {
        let job = load_job(&root)?;
        if input.as_ref().is_some_and(|value| value != &job.input)
            || key.key_id != job.signing_key_id
            || trust_hash != job.trusted_keys_sha256
        {
            return Err("Baseline retry inputs, signer or trust differ from the frozen job".into());
        }
        job
    } else {
        let input = input.ok_or("New baseline job requires source inputs")?;
        if fs::metadata(&input.genesis_block_file)?.len() > 4_000_000 {
            return Err("Genesis block exceeds size limit".into());
        }
        let bytes = fs::read(&input.genesis_block_file)?;
        storage::validate_block(&consensus::deserialize(&bytes)?, &input.identity)?;
        // Root overlap would allow checkpoint/log writes to mutate the producer input.
        if let BaselineJobSource::Rocksdb { root: source } = &input.source
            && (root.starts_with(source) || source.starts_with(&root))
        {
            return Err("Source and baseline job directories must not contain each other".into());
        }
        for path in input.source.stamps()?.keys() {
            if path.starts_with(&root) {
                return Err("Source files must be outside the baseline job".into());
            }
        }
        let block_path = root.join("genesis.block");
        if block_path.exists() {
            if fs::read(&block_path)? != bytes {
                return Err("Prepared genesis block differs from retry".into());
            }
        } else {
            let temporary = root.join(".genesis.block.tmp");
            fs::write(&temporary, &bytes)?;
            File::open(&temporary)?.sync_all()?;
            fs::rename(temporary, block_path)?;
            File::open(&root)?.sync_all()?;
        }
        let job = Job {
            schema_version: JOB_SCHEMA.into(),
            input,
            signing_key_id: key.key_id.clone(),
            trusted_keys_sha256: trust_hash,
            created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            stage: "prepared".into(),
            source: None,
            source_files: BTreeMap::new(),
            manifest: None,
        };
        save_json(&root.join("job.json"), &job)?;
        job
    };
    let block: Block = consensus::deserialize(&fs::read(root.join("genesis.block"))?)?;
    storage::validate_block(&block, &job.input.identity)?;
    let basename = format!("balance_history_baseline_{}", job.input.identity.height);
    let final_dir = root.join("artifact");
    let staging = root.join("staging");
    if final_dir.exists() {
        verify_completed(&final_dir, &job, trusted_keys)?;
        job.stage = "complete".into();
        save_json(&root.join("job.json"), &job)?;
        return Ok(report(&root, &job));
    }
    if job.stage == "complete" {
        return Err("Completed baseline artifact is missing".into());
    }
    fs::create_dir_all(&staging)?;
    let database = staging.join(format!("{basename}.db"));
    if job.stage != "verifying" {
        let stamps = job.input.source.stamps()?;
        if !job.source_files.is_empty() && job.source_files != stamps {
            return Err("Baseline source changed since the previous checkpoint".into());
        }
        let (reader, provenance, commit) =
            job.input.source.open(&job.input.identity, trusted_keys)?;
        if job.input.source.stamps()? != stamps {
            return Err("Baseline source changed while opening".into());
        }
        if let Some(previous) = &job.source
            && serde_json::to_value(previous)? != serde_json::to_value(&provenance)?
        {
            return Err("Baseline source provenance changed".into());
        }
        job.source = Some(provenance);
        job.source_files = stamps;
        save_json(&root.join("job.json"), &job)?;
        let mut writer = if database.exists() {
            storage::Writer::resume(&database, job.input.identity.clone())?
        } else {
            // Initialize under a scratch name: a crash between schema creation and
            // the first cursor commit must never expose a resumable partial schema.
            let temporary = staging.join(".initializing.db");
            for suffix in ["", "-journal", "-wal", "-shm"] {
                let path = staging.join(format!(".initializing.db{suffix}"));
                if path.is_symlink() {
                    return Err("Baseline initialization path must not be a symlink".into());
                }
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
            let mut writer =
                storage::Writer::create(&temporary, job.input.identity.clone(), &block)?;
            writer.use_external_checkpoints();
            writer.conn.execute_batch("CREATE TABLE export_checkpoint (id INTEGER PRIMARY KEY CHECK(id=1), stage TEXT NOT NULL, cursor BLOB, processed INTEGER NOT NULL)")?;
            writer.conn.execute(
                "INSERT INTO block_commits VALUES (?1,?2,?3,?4)",
                params![
                    commit.block_height,
                    commit.btc_block_hash.as_ref() as &[u8],
                    &commit.balance_delta_root[..],
                    &commit.block_commit[..]
                ],
            )?;
            checkpoint(&writer, "utxos", None, 0)?;
            drop(writer);
            fs::rename(temporary, &database)?;
            File::open(&staging)?.sync_all()?;
            storage::Writer::resume(&database, job.input.identity.clone())?
        };
        job.stage = "exporting".into();
        save_json(&root.join("job.json"), &job)?;
        let (mut stage, mut cursor, mut processed): (String, Option<Vec<u8>>, i64) =
            writer.conn.query_row(
                "SELECT stage,cursor,processed FROM export_checkpoint WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        if processed < 0
            || !["utxos", "balances", "script_registry", "done"].contains(&stage.as_str())
            || cursor
                .as_ref()
                .is_some_and(|key| key.len() != if stage == "script_registry" { 32 } else { 36 })
        {
            return Err("Invalid baseline export cursor".into());
        }
        while stage == "utxos" || stage == "balances" {
            let balances = stage == "balances";
            let rows = reader.rows(balances, cursor.as_deref(), batch_size)?;
            if rows.is_empty() {
                stage = if balances {
                    "script_registry"
                } else {
                    "balances"
                }
                .into();
                cursor = None;
                processed = 0;
                checkpoint(&writer, &stage, None, processed)?;
                continue;
            }
            let mut previous = cursor.as_ref().map(|key| key[..32].to_vec());
            for (raw_key, value) in &rows {
                if raw_key.len() != 36 || value.len() != if balances { 16 } else { 40 } {
                    return Err("Invalid baseline source row encoding".into());
                }
                if balances {
                    if u32::from_be_bytes(raw_key[32..].try_into()?) > job.input.identity.height {
                        return Err("Source balance is above genesis".into());
                    }
                    if previous.as_deref() != Some(&raw_key[..32]) {
                        writer.put_balance(
                            &raw_key[..32],
                            u64::from_be_bytes(value[8..].try_into()?),
                        )?;
                        previous = Some(raw_key[..32].to_vec());
                    } else {
                        writer.progress("balance_history_scan")?;
                    }
                } else {
                    writer.put_utxo(
                        raw_key,
                        &value[..32],
                        u64::from_be_bytes(value[32..].try_into()?),
                    )?;
                }
            }
            cursor = rows.last().map(|(key, _)| key.clone());
            processed += rows.len() as i64;
            checkpoint(&writer, &stage, cursor.as_deref(), processed)?;
            observer(&stage)?;
        }
        if stage == "script_registry" {
            writer.required_scripts(&block)?;
            let genesis: BTreeMap<Vec<u8>, Vec<u8>> = block
                .txdata
                .iter()
                .flat_map(|tx| &tx.output)
                .map(|out| {
                    (
                        (out.script_pubkey.to_btc_script_hash().as_ref() as &[u8]).to_vec(),
                        out.script_pubkey.as_bytes().to_vec(),
                    )
                })
                .collect();
            loop {
                let mut keys = writer.script_page(cursor.as_deref())?;
                keys.truncate(batch_size);
                if keys.is_empty() {
                    break;
                }
                for hash in &keys {
                    let script = reader
                        .script(hash)?
                        .or_else(|| genesis.get(hash).cloned())
                        .ok_or("Missing required source script mapping")?;
                    writer.put_script(hash, &script)?;
                }
                cursor = keys.last().cloned();
                processed += keys.len() as i64;
                checkpoint(&writer, "script_registry", cursor.as_deref(), processed)?;
                observer("script_registry")?;
            }
            checkpoint(&writer, "done", None, 0)?;
        }
        drop(reader);
        if job.input.source.stamps()? != job.source_files {
            return Err("Baseline source changed during export".into());
        }
        job.stage = "verifying".into();
        save_json(&root.join("job.json"), &job)?;
        drop(writer);
        observer("data_finished")?;
    }
    // From here on the source may be archived: all semantic checks use the single DB.
    let writer = storage::Writer::resume(&database, job.input.identity.clone())?;
    writer
        .conn
        .execute_batch("DROP TABLE IF EXISTS export_checkpoint")?;
    let state = writer.finish()?;
    let source = job
        .source
        .clone()
        .ok_or("Missing sealed source provenance")?;
    let manifest = BaselineManifest {
        manifest_version: BASELINE_MANIFEST.into(),
        snapshot_schema_version: BASELINE_SCHEMA.into(),
        file_name: format!("{basename}.db"),
        file_size: fs::metadata(&database)?.len(),
        file_sha256: storage::file_hash(&database)?,
        logical_sha256: state.logical_sha256()?,
        state,
        source,
        signature_scheme: "ed25519".into(),
        signing_key_id: key.key_id.clone(),
        generated_at: job.created_at,
    };
    let manifest_file = staging.join(format!("{basename}.manifest.json"));
    immutable_json(&manifest_file, &manifest)?;
    let signature =
        sign_snapshot_artifact_manifest(&key, &manifest_file, &manifest.signature_payload()?)?;
    File::open(signature)?.sync_all()?;
    storage::verify_baseline_snapshot(&manifest_file, trusted_keys)?;
    job.manifest = Some(manifest);
    save_json(&root.join("job.json"), &job)?;
    observer("verified")?;
    File::open(&staging)?.sync_all()?;
    fs::rename(&staging, &final_dir)?;
    File::open(&root)?.sync_all()?;
    observer("published")?;
    job.stage = "complete".into();
    save_json(&root.join("job.json"), &job)?;
    Ok(report(&root, &job))
}

fn checkpoint(
    writer: &storage::Writer,
    stage: &str,
    cursor: Option<&[u8]>,
    processed: i64,
) -> Result<()> {
    writer.conn.execute(
        "INSERT OR REPLACE INTO export_checkpoint VALUES (1,?1,?2,?3)",
        params![stage, cursor, processed],
    )?;
    writer.conn.execute_batch("COMMIT; BEGIN IMMEDIATE")?;
    Ok(())
}

fn immutable_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if path.exists() {
        if fs::read(path)? != bytes {
            return Err("Existing baseline manifest differs from resumed state".into());
        }
    } else {
        save_json(path, value)?;
    }
    Ok(())
}

/// Reverify a completed job's exact frozen manifest, including its physical file and provenance.
pub fn verify_baseline_job(
    root: &Path,
    trust: &Path,
) -> std::result::Result<BaselineManifest, String> {
    let result = (|| -> Result<BaselineManifest> {
        let job = load_job(root)?;
        if job.stage != "complete" {
            return Err("Baseline job is not complete".into());
        }
        if storage::file_hash(trust)? != job.trusted_keys_sha256 {
            return Err("Baseline job trusted catalog changed".into());
        }
        verify_completed(&root.join("artifact"), &job, trust)
    })();
    result.map_err(|e| {
        let message = format!(
            "Baseline job verification failed: root={}, error={e}",
            root.display()
        );
        log::error!("{message}");
        message
    })
}

fn verify_completed(directory: &Path, job: &Job, trust: &Path) -> Result<BaselineManifest> {
    let actual = storage::verify_baseline_snapshot(
        &directory.join(format!(
            "balance_history_baseline_{}.manifest.json",
            job.input.identity.height
        )),
        trust,
    )?;
    let expected = job
        .manifest
        .as_ref()
        .ok_or("Published baseline has no frozen manifest")?;
    if serde_json::to_value(&actual)? != serde_json::to_value(expected)? {
        return Err("Published baseline differs from the frozen job".into());
    }
    Ok(actual)
}
