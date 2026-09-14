//! Offline export entrypoint; writes only a new staging/output directory.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bitcoincore_rpc::bitcoin::{Block, Network, consensus};
use rusqlite::Connection;

use super::{
    BASELINE_MANIFEST, BASELINE_SCHEMA, BaselineIdentity, BaselineManifest, BaselineSource, Result,
    storage,
};
use crate::bootstrap::NativeBootstrapPhase;
use crate::index::sign_snapshot_artifact_manifest;
use crate::{
    BalanceHistoryConfig, BalanceHistoryDB, BalanceHistoryDBIdentity, SnapshotSigningKeyFile,
    SnapshotTrustedKeySet,
};

/// Explicit paths for an offline, exact-height export. No service is stopped or mutated.
pub struct BaselineExportOptions {
    /// Root of a stopped source service or an offline database checkpoint.
    pub source_root: PathBuf,
    /// Expected Bitcoin network; never inferred from an untrusted source.
    pub network: Network,
    /// Exact business-genesis height.
    pub height: u32,
    /// Expected canonical block hash at the target height.
    pub block_hash: bitcoincore_rpc::bitcoin::BlockHash,
    /// Raw serialized genesis block, including witnesses when present.
    pub genesis_block_file: PathBuf,
    /// New output directory, outside the source; existing paths are rejected.
    pub output_dir: PathBuf,
    /// Existing snapshot signer; private material is never copied to output.
    pub signing_key_file: PathBuf,
    /// Existing release-owned trusted-key catalog used for output self-verification.
    pub trusted_keys_file: PathBuf,
}

/// Export a signed normalized baseline from full replay or a sealed native source.
/// This entrypoint does not install a DB or activate/publish a release.
pub fn export_baseline_snapshot(
    options: &BaselineExportOptions,
) -> std::result::Result<BaselineManifest, String> {
    export(options).map_err(|error| {
        let message = format!(
            "Baseline snapshot export failed: source={}, height={}, output={}, error={error}",
            options.source_root.display(),
            options.height,
            options.output_dir.display()
        );
        log::error!("{message}");
        message
    })
}

// Require a stationary file set in addition to one RocksDB read view. A live
// source that compacts or advances is rejected; no partial artifact is promoted.
fn source_files(path: &Path) -> Result<BTreeMap<PathBuf, (u64, SystemTime)>> {
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let meta = entry.metadata()?;
            if !meta.is_file() || entry.file_type()?.is_symlink() {
                return Err("Source DB must contain regular files only".into());
            }
            Ok((entry.file_name().into(), (meta.len(), meta.modified()?)))
        })
        .collect()
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = File::options().create_new(true).write(true).open(path)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    Ok(())
}

fn export(options: &BaselineExportOptions) -> Result<BaselineManifest> {
    let started = Instant::now();
    let identity = BaselineIdentity::new(options.network, options.height, options.block_hash)?;
    let source = options.source_root.canonicalize()?;
    let parent = options
        .output_dir
        .parent()
        .ok_or("Output directory has no parent")?
        .canonicalize()?;
    let output = parent.join(
        options
            .output_dir
            .file_name()
            .ok_or("Output directory has no name")?,
    );
    if output.starts_with(&source) || source.starts_with(&output) || output.exists() {
        return Err("Output must be a new directory outside the source".into());
    }
    let raw_path = &options.genesis_block_file;
    if fs::metadata(raw_path)?.len() > 4_000_000 {
        return Err("Genesis block exceeds size limit".into());
    }
    let block: Block = consensus::deserialize(&fs::read(raw_path)?)?;
    storage::validate_block(&block, &identity)?;
    let key = SnapshotSigningKeyFile::load(&options.signing_key_file)?;
    // Validate key material before doing any large scans.
    let signing = key.to_signing_key()?;
    if SnapshotTrustedKeySet::load(&options.trusted_keys_file)?.find_verifying_key(&key.key_id)?
        != Some(signing.verifying_key())
    {
        return Err("Snapshot signer does not match the selected trusted-key catalog".into());
    }
    let db_path = source.join("db/balance_history");
    let before = source_files(&db_path)?;
    let mut config = BalanceHistoryConfig {
        root_dir: source,
        ..Default::default()
    };
    config.btc.network = options.network;
    let db = BalanceHistoryDB::open_read_only(Arc::new(config))?;
    let staging = parent.join(format!(
        ".baseline-{}-{}-{}",
        options.height,
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir(&staging)?;
    eprintln!(
        "Baseline export started: height={}, staging={}",
        options.height,
        staging.display()
    );
    let file_name = format!("balance_history_baseline_{}.db", options.height);
    let file = staging.join(&file_name);
    let mut writer = storage::Writer::create(&file, identity, &block)?;
    let source_info = db.export_baseline_view(&mut writer, &block)?;
    validate_source(&source_info, &writer.conn, &writer.identity)?;
    let state = writer.finish()?;
    drop(db);
    if source_files(&db_path)? != before {
        return Err(
            "Source database changed during export; staging retained, nothing published".into(),
        );
    }
    File::open(&file)?.sync_all()?;
    let manifest = BaselineManifest {
        manifest_version: BASELINE_MANIFEST.to_owned(),
        snapshot_schema_version: BASELINE_SCHEMA.to_owned(),
        file_name,
        file_size: fs::metadata(&file)?.len(),
        file_sha256: storage::file_hash(&file)?,
        logical_sha256: state.logical_sha256()?,
        state,
        source: source_info,
        signature_scheme: "ed25519".to_owned(),
        signing_key_id: key.key_id.clone(),
        generated_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    };
    let manifest_path = file.with_extension("manifest.json");
    write_new(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    let signature =
        sign_snapshot_artifact_manifest(&key, &manifest_path, &manifest.signature_payload()?)?;
    File::open(signature)?.sync_all()?;
    // Repeat the reader's full verification before publishing a usable directory.
    storage::verify_baseline_snapshot(&manifest_path, &options.trusted_keys_file)?;
    File::open(&staging)?.sync_all()?;
    // Reserve the final name atomically; never replace another operator's directory.
    fs::create_dir(&output)?;
    fs::rename(&staging, &output)?;
    File::open(&parent)?.sync_all()?;
    eprintln!(
        "Baseline export finished: height={}, output={}, logical_sha256={}, elapsed_seconds={:.1}",
        options.height,
        output.display(),
        manifest.logical_sha256,
        started.elapsed().as_secs_f64()
    );
    Ok(manifest)
}

pub(crate) fn validate_source(
    source: &BaselineSource,
    conn: &Connection,
    identity: &BaselineIdentity,
) -> Result<()> {
    match source {
        BaselineSource::FullReplay { db_identity } => {
            if *db_identity != BalanceHistoryDBIdentity::for_network(identity.network) {
                return Err("Full replay source identity mismatch".into());
            }
        }
        BaselineSource::Assumeutxo { state } => {
            state.validate()?;
            let (commit, delta): (Vec<u8>, Vec<u8>) = conn.query_row(
                "SELECT block_commit, balance_delta_root FROM block_commits WHERE block_height=?1",
                [identity.height],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if state.phase != NativeBootstrapPhase::Sealed
                || state.identity.origin_height != identity.height
                || state.identity.origin_block_hash != identity.block_hash
                || state.identity.snapshot.network != identity.network
                || state.origin_commit.as_deref() != Some(&crate::assumeutxo::format::hex(&commit))
                || state.origin_balance_delta_root.as_deref()
                    != Some(&crate::assumeutxo::format::hex(&delta))
            {
                return Err("Native source provenance differs from the baseline state".into());
            }
        }
        BaselineSource::LegacySplit { core, registry } => {
            core.validate()?;
            registry.validate_against_core(core)?;
            let commit: Vec<u8> = conn.query_row(
                "SELECT block_commit FROM block_commits WHERE block_height=?1",
                [identity.height],
                |r| r.get(0),
            )?;
            if core.db_identity != BalanceHistoryDBIdentity::for_network(identity.network)
                || core.state_ref.block_height != identity.height
                || core.state_ref.stable_block_hash != identity.block_hash.to_string()
                || core.state_ref.latest_block_commit != crate::assumeutxo::format::hex(&commit)
            {
                return Err("Legacy split provenance differs from the baseline state".into());
            }
        }
    }
    Ok(())
}
