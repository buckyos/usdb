//! Offline, resumable AssumeUTXO bootstrap prototype. Production startup does not invoke this module.

pub(crate) mod format;
pub use format::{SnapshotCoin, SnapshotIdentity, SnapshotScan, scan_snapshot};
mod audit;
pub use audit::audit_snapshot;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use bitcoincore_rpc::bitcoin::{BlockHash, Network};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::btc::BTCClientRef;
use crate::cache::{AddressBalanceCache, CacheStrategy, UTXOCache};
use crate::index::BatchBlockProcessor;
use crate::{
    AssumeUtxoComparison, AssumeUtxoImportState, BalanceHistoryConfig, BalanceHistoryDB,
    BalanceHistoryDBIdentity, BalanceHistoryDBMode, BlockCommitEntry, CoreSnapshotDb,
};

/// Immutable import request persisted independently of the resumable RocksDB checkpoint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssumeUtxoImportOptions {
    /// Downloaded Core v2 snapshot; never modified by this tool.
    pub snapshot: PathBuf,
    /// Expected source network, baseline and both commitments.
    pub identity: SnapshotIdentity,
    /// Existing exact-height core SQLite, used as a commit reference only during import.
    pub reference_core: PathBuf,
    /// Trusted SHA-256 of the reference SQLite, rechecked before use.
    pub reference_sha256: String,
}

/// Exclusive lock for commands operating on one prototype root.
pub struct AssumeUtxoWorkspaceLock(File);

impl AssumeUtxoWorkspaceLock {
    /// Acquire a process-held lock, requiring an absolute, explicitly selected workspace.
    /// Only import callers may create a new root. A crashed process automatically releases the lock.
    pub fn acquire(root: &Path, create: bool) -> Result<Self, String> {
        if !root.is_absolute() || root.parent().is_none() {
            return Err("Use an absolute dedicated AssumeUTXO workspace path".to_string());
        }
        if create {
            fs::create_dir_all(root).map_err(|e| e.to_string())?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(root.join(".assumeutxo.lock"))
            .map_err(|e| format!("Open workspace lock: {e}"))?;
        file.try_lock()
            .map_err(|e| format!("AssumeUTXO workspace is locked: {e}"))?;
        Ok(Self(file))
    }
}

/// Write a complete JSON report by rename after fsync, retaining old reports on failed writes.
pub fn write_report(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let pending = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file =
        File::create(&pending).map_err(|e| format!("Create report {}: {e}", pending.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(&pending, path).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Stream and verify a pinned immutable reference file, with visible long-running progress.
pub fn verify_reference_hash(path: &Path, expected: &str) -> Result<(), String> {
    format::hash_bytes(expected)?;
    let mut file =
        File::open(path).map_err(|e| format!("Open reference {}: {e}", path.display()))?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let start = Instant::now();
    let mut progress = Instant::now();
    let mut hash = Sha256::new();
    let mut bytes = vec![0u8; 4 * 1024 * 1024];
    let mut processed = 0u64;
    eprintln!(
        "Reference verification started: path={}, bytes={total}",
        path.display()
    );
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
        processed += n as u64;
        if progress.elapsed().as_secs() >= 10 {
            eprintln!(
                "Reference verification progress: bytes={processed}, total={total}, elapsed_seconds={:.1}",
                start.elapsed().as_secs_f64()
            );
            progress = Instant::now();
        }
    }
    let actual = format::hex(&hash.finalize());
    if actual != expected.to_ascii_lowercase() {
        return Err(format!("Reference SHA256 mismatch: actual={actual}"));
    }
    eprintln!(
        "Reference verification finished: bytes={processed}, elapsed_seconds={:.1}",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

fn reference(options: &AssumeUtxoImportOptions) -> Result<CoreSnapshotDb, String> {
    verify_reference_hash(&options.reference_core, &options.reference_sha256)?;
    let db = CoreSnapshotDb::open_for_verification(&options.reference_core, 65536)?;
    db.verify_schema()?;
    let meta = db.read_meta()?;
    if meta.db_identity != BalanceHistoryDBIdentity::for_network(options.identity.network)
        || meta.block_height < options.identity.base_height
    {
        return Err("Reference core network, data model or height mismatch".to_string());
    }
    Ok(db)
}

fn commit_at(db: &CoreSnapshotDb, height: u32) -> Result<BlockCommitEntry, String> {
    let commit = db
        .get_block_commit_entries(1, height.checked_sub(1))?
        .into_iter()
        .next()
        .ok_or("Reference commit is missing")?;
    if commit.block_height != height {
        return Err(format!("Reference commit is missing at height {height}"));
    }
    Ok(commit)
}

fn config(root: &Path, network: Network) -> BalanceHistoryConfig {
    let mut config = BalanceHistoryConfig {
        root_dir: root.to_path_buf(),
        ..BalanceHistoryConfig::default()
    };
    config.btc.network = network;
    config.sync.utxo_max_cache_bytes = 256 * 1024 * 1024;
    config.sync.balance_max_cache_bytes = 256 * 1024 * 1024;
    config
}

/// Read the immutable input request without opening a database or starting any service.
pub fn read_import_options(root: &Path) -> Result<AssumeUtxoImportOptions, String> {
    let bytes = fs::read(root.join("input.json"))
        .map_err(|e| format!("Read AssumeUTXO input record: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// Import to a staging directory and publish it only after both source commitments pass.
/// A retry rescans and verifies the input while skipping the atomically checkpointed prefix.
/// The caller must hold [`AssumeUtxoWorkspaceLock`] for the duration of this operation.
pub fn import_snapshot(
    root: &Path,
    options: &AssumeUtxoImportOptions,
    batch_size: usize,
) -> Result<SnapshotScan, String> {
    if options.identity.base_height == 0 {
        return Err("A positive snapshot baseline is required".to_string());
    }
    if options.identity.network == Network::Bitcoin
        && (options.identity.base_height != 935000
            || options.identity.base_hash
                != "0000000000000000000147034958af1652b2b91bba607beacc5e72a56f0fb5ee"
            || options.identity.hash_serialized_3
                != "e4b90ef9eae834f56c4b64d2d50143cee10ad87994c614d7d04125e2a6025050")
    {
        return Err(
            "This mainnet prototype only accepts Bitcoin Core's pinned 935000 snapshot".to_string(),
        );
    }
    let input = root.join("input.json");
    if input.exists() {
        if read_import_options(root)? != *options {
            return Err("Workspace input record differs from this import request".to_string());
        }
    } else {
        if fs::read_dir(root)
            .map_err(|e| e.to_string())?
            .any(|entry| entry.map_or(true, |e| e.file_name() != ".assumeutxo.lock"))
        {
            return Err("Refusing to initialize a nonempty, unrecognized workspace".to_string());
        }
        write_report(&input, options)?;
    }
    if root.join("state").exists() {
        return Err("Import is already published; use status or replay".to_string());
    }
    let reference = reference(options)?;
    let anchor = commit_at(&reference, options.identity.base_height)?;
    if anchor.btc_block_hash.to_string() != options.identity.base_hash {
        return Err("Reference BTC baseline hash mismatch".to_string());
    }
    let staging = root.join("staging");
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let config = config(&staging, options.identity.network);
    let db = BalanceHistoryDB::open(Arc::new(config), BalanceHistoryDBMode::Normal)?;
    let state = AssumeUtxoImportState {
        identity: options.identity.clone(),
        reference_sha256: options.reference_sha256.clone(),
        base_commit: format::hex(&anchor.block_commit),
        base_delta_root: format::hex(&anchor.balance_delta_root),
        imported_coins: 0,
        complete: false,
    };
    let applied = db.begin_assumeutxo_import(&state)?;
    let mut progress = Instant::now();
    let scan = scan_snapshot(
        &options.snapshot,
        &options.identity,
        batch_size,
        |coins, processed| {
            if processed <= applied {
                return Ok(());
            }
            let start = processed - coins.len() as u64;
            let skip = applied.saturating_sub(start) as usize;
            db.import_assumeutxo_coins(&coins[skip..], processed)?;
            if progress.elapsed().as_secs() >= 10 {
                write_report(
                    &root.join("import-progress.json"),
                    &serde_json::json!({"imported_coins":processed,"status":"staging"}),
                )?;
                progress = Instant::now();
            }
            Ok(())
        },
    )?;
    if applied > scan.coins {
        return Err("Import checkpoint exceeds verified snapshot length".to_string());
    }
    db.finish_assumeutxo_import(scan.coins, &anchor)?;
    drop(db);
    write_report(&staging.join("assumeutxo-source.json"), &scan)?;
    fs::rename(&staging, root.join("state"))
        .map_err(|e| format!("Publish AssumeUTXO state: {e}"))?;
    File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    write_report(&root.join("import-result.json"), &scan)?;
    Ok(scan)
}

/// Resume a published prototype to a fixed canonical height using the existing block processor.
/// All committed block hashes, delta roots and rolling commits are checked against the reference.
/// A reorg at the saved tip is rejected explicitly; this prototype never silently resets a workspace.
pub fn replay_snapshot(
    root: &Path,
    client: BTCClientRef,
    target: u32,
    batch_size: u32,
) -> Result<(), String> {
    if !(1..=100).contains(&batch_size) || target == u32::MAX {
        return Err("Invalid replay height or batch size".to_string());
    }
    let options = read_import_options(root)?;
    // Check authentication, connectivity and network before scanning a multi-gigabyte reference.
    let genesis = client.get_block_hash(0).map_err(|error| {
        format!("Replay RPC preflight failed before reference verification: {error}; check that the Bitcoin node is running and RPC authentication is valid")
    })?;
    if genesis
        != bitcoincore_rpc::bitcoin::constants::genesis_block(options.identity.network).block_hash()
    {
        return Err("Replay RPC network mismatch".to_string());
    }
    let reference = reference(&options)?;
    let reference_meta = reference.read_meta()?;
    if target < options.identity.base_height || target > reference_meta.block_height {
        return Err("Replay target outside reference range".to_string());
    }
    let target_commit = commit_at(&reference, target)?;
    if client.get_block_hash(target)? != target_commit.btc_block_hash {
        return Err("Replay target BTC hash differs from reference".to_string());
    }
    let config = Arc::new(config(&root.join("state"), options.identity.network));
    let db = Arc::new(BalanceHistoryDB::open(
        config.clone(),
        BalanceHistoryDBMode::Normal,
    )?);
    let imported = db
        .get_assumeutxo_import_state()?
        .ok_or("Missing complete import")?;
    if !imported.complete
        || imported.identity != options.identity
        || imported.reference_sha256 != options.reference_sha256
    {
        return Err("Published import identity mismatch".to_string());
    }
    let mut current = db.get_btc_block_height()?;
    if current > target {
        return Err("Workspace is already beyond the requested target".to_string());
    }
    let local = db
        .get_block_commit(current)?
        .ok_or("Missing saved tip commit")?;
    if local.btc_block_hash != client.get_block_hash(current)? {
        return Err(
            "Saved tip is no longer canonical; explicit reorg recovery is required".to_string(),
        );
    }
    // Verify every previously applied commit on resume, including a batch persisted before a crash.
    check_commits(&db, &reference, options.identity.base_height, current)?;
    let processor = BatchBlockProcessor::new(
        client.clone(),
        db.clone(),
        Arc::new(UTXOCache::new(config.clone(), CacheStrategy::Normal)),
        Arc::new(AddressBalanceCache::new(config, CacheStrategy::Normal)),
    );
    let started = Instant::now();
    eprintln!(
        "AssumeUTXO replay started: from_height={current}, target_height={target}, batch_size={batch_size}"
    );
    while current < target {
        let end = current.saturating_add(batch_size).min(target);
        processor.process_blocks(current + 1..end + 1, target, 288)?;
        check_commits(&db, &reference, current + 1, end)?;
        db.sync_assumeutxo_checkpoint()?;
        current = end;
        write_report(
            &root.join("replay-progress.json"),
            &serde_json::json!({"height":current,"target":target,"elapsed_seconds":started.elapsed().as_secs_f64(),"status":"running"}),
        )?;
        eprintln!(
            "AssumeUTXO replay progress: height={current}, target={target}, elapsed_seconds={:.1}",
            started.elapsed().as_secs_f64()
        );
    }
    if client.get_block_hash(target)? != target_commit.btc_block_hash {
        return Err("Replay target changed during processing".to_string());
    }
    db.flush_all()?;
    write_report(
        &root.join("replay-result.json"),
        &serde_json::json!({"status":"pass","height":target,"btc_block_hash":target_commit.btc_block_hash.to_string(),"block_commit":format::hex(&target_commit.block_commit),"elapsed_seconds":started.elapsed().as_secs_f64(),"requires_completed_bitcoin_background_validation":false}),
    )?;
    eprintln!(
        "AssumeUTXO replay finished: height={target}, elapsed_seconds={:.1}",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn check_commits(
    db: &BalanceHistoryDB,
    reference: &CoreSnapshotDb,
    start: u32,
    end: u32,
) -> Result<(), String> {
    let mut height = start;
    while height <= end {
        let entries = reference
            .get_block_commit_entries((end - height + 1).min(1000), height.checked_sub(1))?;
        if entries.is_empty() {
            return Err(format!("Missing reference commit at {height}"));
        }
        for expected in entries {
            if expected.block_height != height
                || db.get_block_commit(height)?.as_ref() != Some(&expected)
            {
                return Err(format!("AssumeUTXO commit mismatch: height={height}"));
            }
            height += 1;
        }
    }
    Ok(())
}

/// Compare full logical state after replay, with a fresh source-file integrity check.
pub fn compare_snapshot(root: &Path) -> Result<AssumeUtxoComparison, String> {
    let options = read_import_options(root)?;
    let reference = reference(&options)?;
    let target = reference.read_meta()?.block_height;
    let db = BalanceHistoryDB::open_read_only(Arc::new(config(
        &root.join("state"),
        options.identity.network,
    )))?;
    let result = db.compare_assumeutxo_core(&options.reference_core, target)?;
    write_report(&root.join("comparison-result.json"), &result)?;
    Ok(result)
}

/// Inspect persisted progress without contacting Bitcoin or initiating a scan.
pub fn snapshot_status(root: &Path) -> Result<serde_json::Value, String> {
    let options = read_import_options(root)?;
    // Reports are atomically replaced, so status is safe while the writer owns RocksDB.
    let mut reports = serde_json::Map::new();
    for name in [
        "import-progress",
        "import-result",
        "replay-progress",
        "replay-result",
        "comparison-result",
        "p5-semantics-result",
    ] {
        let path = root.join(format!("{name}.json"));
        if path.exists() {
            let value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            reports.insert(name.to_string(), value);
        }
    }
    Ok(
        serde_json::json!({"published":root.join("state").exists(),"input":options,"reports":reports,"note":"Reports show last checkpoints, not process liveness"}),
    )
}

#[cfg(test)]
#[path = "../../../../../tests/common/assumeutxo.rs"]
mod test_common;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_bootstrap.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_semantics.rs"]
mod semantics_tests;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_origin.rs"]
mod origin_tests;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_native.rs"]
mod native_tests;
