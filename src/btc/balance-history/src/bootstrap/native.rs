//! Native bootstrap lifecycle: isolated import, canonical replay, origin sealing and publication.

use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use bitcoincore_rpc::bitcoin::{BlockHash, Network, hashes::Hash};
use serde::{Deserialize, Serialize};

use super::{
    BOOTSTRAP_COMMIT_PROTOCOL_VERSION, BootstrapCommitCheckpoint, BootstrapOriginIdentity,
    derive_origin_state_digest, embedded_bootstrap_checkpoint,
};
use crate::assumeutxo::{
    AssumeUtxoWorkspaceLock, SnapshotIdentity, SnapshotScan, scan_snapshot, write_report,
};
use crate::btc::BTCClientRef;
use crate::cache::{AddressBalanceCache, CacheStrategy, UTXOCache};
use crate::config::BalanceHistoryConfigRef;
use crate::index::BatchBlockProcessor;
use crate::{BalanceHistoryDB, BalanceHistoryDBMode};

/// Metadata schema that keeps native bootstrap provenance separate from old snapshot installation.
pub const NATIVE_BOOTSTRAP_SCHEMA: &str = "balance-history-native-bootstrap:checkpoint-v1";

/// Immutable logical inputs for a native database. File location and batching are operational only.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeBootstrapIdentity {
    /// Exact Core snapshot commitments and baseline.
    pub snapshot: SnapshotIdentity,
    /// Business origin after applying this block.
    pub origin_height: u32,
    /// Canonical BTC hash pinned at the business origin.
    pub origin_block_hash: BlockHash,
    /// Explicit checkpoint for isolated regtest chains; forbidden on public networks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regtest_checkpoint: Option<BootstrapCommitCheckpoint>,
}

impl NativeBootstrapIdentity {
    /// Select an embedded public checkpoint, or an explicit checkpoint for a private regtest chain.
    pub fn checkpoint(&self) -> Result<BootstrapCommitCheckpoint, String> {
        let checkpoint = if self.snapshot.network == Network::Regtest {
            self.regtest_checkpoint
                .clone()
                .ok_or("Regtest native bootstrap requires an explicit commit checkpoint")?
        } else {
            if self.regtest_checkpoint.is_some() {
                return Err(
                    "Public network bootstrap cannot override an embedded checkpoint".to_string(),
                );
            }
            embedded_bootstrap_checkpoint(&self.snapshot)?
        };
        checkpoint.block_entry()?;
        if checkpoint.snapshot != self.snapshot {
            return Err("Bootstrap checkpoint and snapshot identity mismatch".to_string());
        }
        if self.origin_height < self.snapshot.base_height
            || self.origin_height == u32::MAX
            || (self.origin_height == self.snapshot.base_height
                && self.origin_block_hash.to_string() != self.snapshot.base_hash)
        {
            return Err("Business genesis must be at or after the snapshot, with a matching hash when equal".to_string());
        }
        Ok(checkpoint)
    }
}

/// Explicit native startup configuration, reusable after the source file has been archived.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeBootstrapConfig {
    /// Absolute downloaded Core snapshot path, needed only until import validation completes.
    pub snapshot_file: PathBuf,
    /// Immutable source and origin identity.
    pub identity: NativeBootstrapIdentity,
    /// Maximum coins in a durably checkpointed import batch.
    #[serde(default = "default_import_batch")]
    pub import_batch_size: usize,
    /// Maximum contiguous BTC blocks per replay checkpoint.
    #[serde(default = "default_replay_batch")]
    pub replay_batch_size: u32,
}

fn default_import_batch() -> usize {
    20_000
}
fn default_replay_batch() -> u32 {
    20
}

impl NativeBootstrapConfig {
    /// Reject unsupported source identities and ambiguous native origin/network selections.
    pub fn validate(&self, network: Network) -> Result<(), String> {
        let identity = &self.identity;
        let source = &identity.snapshot;
        source
            .base_hash
            .parse::<BlockHash>()
            .map_err(|e| format!("Invalid bootstrap baseline hash: {e}"))?;
        crate::assumeutxo::format::hash_bytes(&source.file_sha256)?;
        crate::assumeutxo::format::hash_bytes(&source.hash_serialized_3)?;
        if !self.snapshot_file.is_absolute()
            || source.network != network
            || source.base_height == 0
            || identity.origin_height < source.base_height
            || identity.origin_height == u32::MAX
            || !(1..=1_000_000).contains(&self.import_batch_size)
            || !(1..=100).contains(&self.replay_batch_size)
        {
            return Err(
                "Invalid native bootstrap path, network, heights or batch sizes".to_string(),
            );
        }
        identity.checkpoint()?;
        Ok(())
    }
}

/// Durable lifecycle phase. Only sealed databases may be published or served.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeBootstrapPhase {
    Importing,
    Replaying,
    Sealed,
}

/// Durable native provenance and progress, written atomically with the corresponding state change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBootstrapState {
    /// Native metadata schema.
    pub schema_version: String,
    /// Source/origin identity permanently bound to this database.
    pub identity: NativeBootstrapIdentity,
    /// Reviewed historical prefix installed after the source UTXO commitment passes verification.
    pub checkpoint: BootstrapCommitCheckpoint,
    /// Import, replay or sealed state.
    pub phase: NativeBootstrapPhase,
    /// Number of source coins in completed durable import batches.
    pub imported_coins: u64,
    /// Full source verification result; present only after strict EOF and both commitments pass.
    pub source_verification: Option<SnapshotScan>,
    /// Logical origin state, present only after independent balance verification.
    pub origin: Option<BootstrapOriginIdentity>,
    /// Original v1 rolling commit retained at business genesis, never replaced by a state digest.
    pub origin_commit: Option<String>,
    /// Original delta root at business genesis, preserving the full block commit RPC contract.
    pub origin_balance_delta_root: Option<String>,
    /// Independently calculated state digest for auditing; not a rolling commit seed.
    pub origin_state_digest: Option<String>,
    /// Block commitment protocol used by this native database.
    pub commit_protocol_version: String,
}

impl NativeBootstrapState {
    /// Validate persisted provenance before resuming or accepting a published native database.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != NATIVE_BOOTSTRAP_SCHEMA
            || self.commit_protocol_version != BOOTSTRAP_COMMIT_PROTOCOL_VERSION
            || self.checkpoint != self.identity.checkpoint()?
        {
            return Err("Unsupported native bootstrap metadata or commit version".to_string());
        }
        match self.phase {
            NativeBootstrapPhase::Importing
                if self.source_verification.is_none()
                    && self.origin.is_none()
                    && self.origin_commit.is_none()
                    && self.origin_balance_delta_root.is_none()
                    && self.origin_state_digest.is_none() =>
            {
                Ok(())
            }
            NativeBootstrapPhase::Replaying | NativeBootstrapPhase::Sealed => {
                let scan = self
                    .source_verification
                    .as_ref()
                    .ok_or("Native bootstrap has no verified source")?;
                if scan.identity != self.identity.snapshot || scan.coins != self.imported_coins {
                    return Err(
                        "Native source verification differs from persisted identity/count"
                            .to_string(),
                    );
                }
                if self.phase == NativeBootstrapPhase::Sealed {
                    let origin = self
                        .origin
                        .as_ref()
                        .ok_or("Native bootstrap has no origin identity")?;
                    if origin.origin_height != self.identity.origin_height
                        || origin.origin_block_hash != self.identity.origin_block_hash
                        || origin.network != self.identity.snapshot.network
                        || self.origin_state_digest.as_ref()
                            != Some(&derive_origin_state_digest(origin)?)
                    {
                        return Err("Native origin state digest identity mismatch".to_string());
                    }
                    for value in [&self.origin_commit, &self.origin_balance_delta_root] {
                        let value = value
                            .as_ref()
                            .ok_or("Missing native origin rolling commit record")?;
                        crate::assumeutxo::format::hash_bytes(value)?;
                    }
                    if self.identity.origin_height == self.identity.snapshot.base_height
                        && (self.origin_commit.as_ref() != Some(&self.checkpoint.block_commit)
                            || self.origin_balance_delta_root.as_ref()
                                != Some(&self.checkpoint.balance_delta_root))
                    {
                        return Err(
                            "Native origin differs from its baseline checkpoint".to_string()
                        );
                    }
                } else if self.origin.is_some()
                    || self.origin_commit.is_some()
                    || self.origin_state_digest.is_some()
                    || self.origin_balance_delta_root.is_some()
                {
                    return Err("Unsealed native bootstrap contains a published origin".to_string());
                }
                Ok(())
            }
            _ => Err("Invalid native bootstrap phase metadata".to_string()),
        }
    }
}

/// Bootstrap from Core UTXOs and a reviewed v1 commit checkpoint, without old USDB snapshot files.
/// Resume uses durable database markers; cancellation leaves staging unpublished and recoverable.
/// Published databases are checked without reopening the source file or replaying old blocks.
pub fn prepare_native_bootstrap(
    config: BalanceHistoryConfigRef,
    client: BTCClientRef,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<NativeBootstrapState>, String> {
    let Some(options) = &config.bootstrap else {
        return Ok(None);
    };
    options.validate(config.btc.network())?;
    if config.sync.max_sync_block_height < options.identity.origin_height {
        return Err("Native bootstrap origin exceeds configured maximum sync height".to_string());
    }
    let result = prepare(&config, options, client, cancelled);
    result.map(Some).map_err(|error| {
        let message = format!(
            "Native bootstrap failed: root_dir={}, origin_height={}, error={error}",
            config.root_dir.display(),
            options.identity.origin_height
        );
        log::error!("{message}");
        eprintln!("{message}");
        message
    })
}

fn prepare(
    config: &BalanceHistoryConfigRef,
    options: &NativeBootstrapConfig,
    client: BTCClientRef,
    cancelled: &dyn Fn() -> bool,
) -> Result<NativeBootstrapState, String> {
    let started = Instant::now();
    let identity = &options.identity;
    let origin = identity.origin_height;
    let base = identity.snapshot.base_height;
    let root = &config.root_dir;
    let _lock = AssumeUtxoWorkspaceLock::acquire(root, true)?;
    let final_db = config.db_dir().join("balance_history");
    if final_db.exists() {
        let db = BalanceHistoryDB::open_read_only(config.clone())?;
        db.validate_native_bootstrap_service()?;
        return db
            .get_native_bootstrap_state()?
            .ok_or("Existing database is not a native bootstrap".to_string());
    }
    let stable_lag = usdb_util::embedded_btc_stable_lag_blocks(identity.snapshot.network)
        .map_err(|e| e.to_string())?;
    // Snapshot import may overlap Core's forward sync; only the baseline must already be active.
    if client.get_block_hash(0)?
        != bitcoincore_rpc::bitcoin::constants::genesis_block(identity.snapshot.network)
            .block_hash()
    {
        return Err("Native bootstrap RPC network mismatch".to_string());
    }
    let mut tip = client.get_latest_block_height()?;
    while tip < base {
        wait_for_blocks(root, base, origin, tip, cancelled)?;
        tip = client.get_latest_block_height()?;
    }
    if client.get_block_hash(base)?.to_string() != identity.snapshot.base_hash
        || (tip >= origin && client.get_block_hash(origin)? != identity.origin_block_hash)
    {
        return Err("Native bootstrap RPC baseline/origin mismatch".to_string());
    }
    if cancelled() {
        return Err("Native bootstrap cancelled before staging".to_string());
    }
    let mut staging_config = (**config).clone();
    staging_config.root_dir = root.join("bootstrap-staging");
    let staging_config = Arc::new(staging_config);
    let db = Arc::new(BalanceHistoryDB::open(
        staging_config.clone(),
        BalanceHistoryDBMode::Normal,
    )?);
    let mut state = db.begin_native_bootstrap(identity)?;
    if state.phase == NativeBootstrapPhase::Importing {
        let applied = state.imported_coins;
        let mut last_report = Instant::now();
        let scan = scan_snapshot(
            &options.snapshot_file,
            &identity.snapshot,
            options.import_batch_size,
            |coins, processed| {
                if cancelled() {
                    return Err("Native bootstrap cancelled during import".to_string());
                }
                let skip = applied
                    .saturating_sub(processed - coins.len() as u64)
                    .min(coins.len() as u64) as usize;
                db.verify_native_import_prefix(&coins[..skip])?;
                if processed > applied {
                    db.import_native_bootstrap_coins(&coins[skip..], processed)?;
                }
                if last_report.elapsed().as_secs() >= 10 {
                    write_report(
                        &root.join("bootstrap-progress.json"),
                        &serde_json::json!({"phase":"importing","imported_coins":processed,"elapsed_seconds":started.elapsed().as_secs_f64()}),
                    )?;
                    last_report = Instant::now();
                }
                Ok(())
            },
        )?;
        if applied > scan.coins {
            return Err("Native import checkpoint exceeds source coin count".to_string());
        }
        db.finish_native_bootstrap_import(&scan)?;
        state = db
            .get_native_bootstrap_state()?
            .ok_or("Missing native bootstrap marker")?;
    }
    if state.phase == NativeBootstrapPhase::Replaying {
        let mut current = db.get_btc_block_height()?;
        if current < base || current > origin {
            return Err("Native replay height is outside the pinned interval".to_string());
        }
        db.resume_rollback_if_needed()?;
        current = db.get_btc_block_height()?;
        let mut replay_config = (**config).clone();
        replay_config.sync.max_sync_block_height = origin;
        let replay_client =
            crate::btc::create_canonical_btc_client(client.clone(), &Arc::new(replay_config))?;
        let utxo_cache = Arc::new(UTXOCache::new(
            staging_config.clone(),
            CacheStrategy::Normal,
        ));
        let balance_cache = Arc::new(AddressBalanceCache::new(
            staging_config,
            CacheStrategy::Normal,
        ));
        let processor = BatchBlockProcessor::new(
            replay_client,
            db.clone(),
            utxo_cache.clone(),
            balance_cache.clone(),
        );
        loop {
            if cancelled() {
                return Err("Native bootstrap cancelled during replay".to_string());
            }
            let tip = client.get_latest_block_height()?;
            if tip < base {
                return Err("Native replay crossed the snapshot baseline".to_string());
            }
            if tip >= origin && client.get_block_hash(origin)? != identity.origin_block_hash {
                return Err("Native bootstrap RPC origin mismatch".to_string());
            }
            // Recheck the saved branch on every iteration, including after waiting for upstream.
            let mut ancestor = current.min(tip);
            while ancestor > base
                && db
                    .get_block_commit(ancestor)?
                    .ok_or("Missing native replay ancestor")?
                    .btc_block_hash
                    != client.get_block_hash(ancestor)?
            {
                ancestor -= 1;
            }
            if db
                .get_block_commit(ancestor)?
                .ok_or("Missing native replay baseline")?
                .btc_block_hash
                != client.get_block_hash(ancestor)?
            {
                return Err(
                    "Native replay crossed the snapshot baseline; a new bootstrap is required"
                        .to_string(),
                );
            }
            if ancestor != current {
                db.rollback_to_block_height(ancestor)?;
                utxo_cache.clear();
                balance_cache.clear();
                current = ancestor;
            }
            let available = tip.saturating_sub(stable_lag).min(origin);
            if current == origin && available >= origin {
                break;
            }
            if available <= current {
                wait_for_blocks(root, current, origin, tip, cancelled)?;
                continue;
            }
            let end = current
                .saturating_add(options.replay_batch_size)
                .min(available);
            processor.process_blocks(
                current + 1..end + 1,
                origin,
                config.sync.undo_retention_blocks,
            )?;
            db.sync_assumeutxo_checkpoint()?;
            current = end;
            eprintln!(
                "Native bootstrap replay progress: height={current}, target={origin}, elapsed_seconds={:.1}",
                started.elapsed().as_secs_f64()
            );
            write_report(
                &root.join("bootstrap-progress.json"),
                &serde_json::json!({"phase":"replaying","height":current,"target":origin,"elapsed_seconds":started.elapsed().as_secs_f64()}),
            )?;
        }
        drop(processor);
        drop(utxo_cache);
        drop(balance_cache);
        // Full origin scans can outlast replay; do not leave a completed replay bar visible.
        write_report(
            &root.join("bootstrap-progress.json"),
            &serde_json::json!({"phase":"verifying","height":origin,"elapsed_seconds":started.elapsed().as_secs_f64()}),
        )?;
        let origin_identity = db.bootstrap_origin_identity_cancellable(
            identity.snapshot.network,
            origin,
            identity.origin_block_hash,
            cancelled,
        )?;
        db.verify_native_bootstrap_balances(&origin_identity, cancelled)?;
        if client.get_block_hash(origin)? != identity.origin_block_hash {
            return Err("Native origin changed during verification".to_string());
        }
        while client.get_latest_block_height()?.saturating_sub(stable_lag) < origin {
            if client.get_block_hash(origin)? != identity.origin_block_hash {
                return Err("Native origin changed during verification".to_string());
            }
            wait_for_blocks(
                root,
                origin,
                origin,
                client.get_latest_block_height()?,
                cancelled,
            )?;
        }
        if cancelled() {
            return Err("Native bootstrap cancelled before sealing".to_string());
        }
        db.seal_native_bootstrap(&origin_identity)?;
    }
    state = db
        .get_native_bootstrap_state()?
        .ok_or("Missing sealed native bootstrap")?;
    db.validate_native_bootstrap_service()?;
    db.flush_all()?;
    drop(db);
    // Rename only the private DB directory, leaving config/logs and existing user files intact.
    fs::create_dir_all(config.db_dir()).map_err(|e| e.to_string())?;
    if final_db.exists() {
        return Err("Native publication destination already exists".to_string());
    }
    fs::rename(root.join("bootstrap-staging/db/balance_history"), &final_db)
        .map_err(|e| format!("Publish native bootstrap database: {e}"))?;
    File::open(config.db_dir())
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    File::open(root.join("bootstrap-staging/db"))
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    write_report(
        &root.join("bootstrap-progress.json"),
        &serde_json::json!({"phase":"sealed","published":true,"height":origin,"origin_commit":state.origin_commit,"elapsed_seconds":started.elapsed().as_secs_f64()}),
    )?;
    eprintln!(
        "Native bootstrap published: height={origin}, elapsed_seconds={:.1}",
        started.elapsed().as_secs_f64()
    );
    Ok(state)
}

// Waiting is an operational phase, not readiness. Source import and replay checkpoints remain durable.
fn wait_for_blocks(
    root: &std::path::Path,
    current: u32,
    target: u32,
    tip: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    eprintln!(
        "Native bootstrap waiting for BTC blocks: height={current}, target={target}, btc_tip={tip}"
    );
    write_report(
        &root.join("bootstrap-progress.json"),
        &serde_json::json!({"phase":"waiting_for_blocks","height":current,"target":target,"btc_tip":tip,"published":false}),
    )?;
    for _ in 0..10 {
        if cancelled() {
            return Err("Native bootstrap cancelled while waiting for BTC blocks".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
