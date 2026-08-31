use crate::verify::VerifiedSnapshot;
use crate::{
    BuilderLock, BuilderPaths, CompletedSnapshotRef, SnapshotBuildJob, SnapshotBuildStage,
    SnapshotBuilderState, SnapshotCompleteMarker, SnapshotVerificationPhase,
    SnapshotVerificationProgress, build_complete_marker, load_json, save_json_atomic,
    unique_run_id, unix_timestamp, verify_published_artifact, verify_published_artifact_marker,
    verify_snapshot_files_with_progress,
};
use balance_history::{
    BalanceHistoryConfig, BalanceHistoryIndexer, IndexOutput, SnapshotHash, SnapshotIndexer,
    SnapshotManifest, SyncStatusManager, build_historical_state_ref_at_height,
    manifest_path_for_snapshot_file, verify_snapshot_manifest_signature,
};
use chrono::Local;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use log::{error, info};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use usdb_util::{BTCRpcClient, get_memory_usage_snapshot};

const VERIFICATION_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

fn snapshot_hash_progress_bar(path: &Path) -> Result<ProgressBar, String> {
    let total_bytes = std::fs::metadata(path)
        .map_err(|e| {
            format!(
                "Failed to read snapshot file metadata {}: {}",
                path.display(),
                e
            )
        })?
        .len();
    let progress = ProgressBar::new(total_bytes);
    progress.set_draw_target(ProgressDrawTarget::stderr_with_hz(4));
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                "{prefix:.bold} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} {bytes_per_sec} {percent}% ({eta_precise} remaining) {msg}",
            )
            .expect("Invalid snapshot hash progress template")
            .progress_chars("#>-"),
    );
    progress.set_prefix("Finalize hash");
    progress.set_message(
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("snapshot.db")
            .to_string(),
    );
    progress.enable_steady_tick(Duration::from_millis(250));
    Ok(progress)
}

fn verification_phase_name(phase: SnapshotVerificationPhase) -> &'static str {
    match phase {
        SnapshotVerificationPhase::FileHash => "file_hash",
        SnapshotVerificationPhase::IntegrityCheck => "integrity_check",
        SnapshotVerificationPhase::BalanceHistoryCount => "balance_history_count",
        SnapshotVerificationPhase::UtxoCount => "utxo_count",
        SnapshotVerificationPhase::BlockCommitCount => "block_commit_count",
        SnapshotVerificationPhase::ScriptRegistryCount => "script_registry_count",
        SnapshotVerificationPhase::CommitIdentity => "commit_identity",
    }
}

fn format_elapsed(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn format_verification_progress_line(
    timestamp: &str,
    event: &str,
    height: u32,
    phase: SnapshotVerificationPhase,
    phase_elapsed: Duration,
    verification_elapsed: Duration,
) -> String {
    format!(
        "[{timestamp}] Snapshot verification {event}: height={height}, phase={}, phase_elapsed={}, verify_elapsed={}",
        verification_phase_name(phase),
        format_elapsed(phase_elapsed),
        format_elapsed(verification_elapsed)
    )
}

fn print_verification_progress(
    event: &str,
    height: u32,
    phase: SnapshotVerificationPhase,
    phase_elapsed: Duration,
    verification_elapsed: Duration,
) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S %:z");
    eprintln!(
        "{}",
        format_verification_progress_line(
            &timestamp.to_string(),
            event,
            height,
            phase,
            phase_elapsed,
            verification_elapsed,
        )
    );
}

/// Inputs controlling one exact-height create or resume operation.
#[derive(Clone, Debug)]
pub struct SnapshotCreateOptions {
    /// Exact stable BTC height to synchronize and publish.
    pub target_height: u32,
    /// Optional operator-pinned canonical block hash for the target.
    pub expected_block_hash: Option<String>,
    /// Delay between retries while the target waits to enter the stable range.
    pub poll_interval: Duration,
    /// Balance-history config copied into an unused workspace on first use.
    pub config_file: Option<PathBuf>,
}

/// Inputs for resuming verification of an already-generated temporary artifact.
#[derive(Clone, Debug)]
pub struct SnapshotResumeVerifyOptions {
    /// Exact BTC height of the persisted verifying job.
    pub target_height: u32,
    /// Optional operator-pinned canonical block hash for the target.
    pub expected_block_hash: Option<String>,
}

enum VerificationMessage {
    Phase(SnapshotVerificationPhase),
    Finished(Box<Result<VerifiedSnapshot, String>>),
}

/// Machine-readable result of a successful create or idempotent replay.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotCreateReport {
    /// Whether a persisted job already existed when the command started.
    pub resumed: bool,
    /// Whether an already-published artifact satisfied the request.
    pub already_complete: bool,
    /// Exact BTC height represented by the artifact.
    pub height: u32,
    /// BTC network bound to the artifact.
    pub network: String,
    /// Canonical BTC block hash represented by the artifact.
    pub btc_block_hash: String,
    /// Consensus snapshot identity from balance-history.
    pub snapshot_id: String,
    /// Artifact directory relative to the builder root.
    pub artifact_dir: String,
    /// Snapshot DB file name relative to the artifact directory.
    pub snapshot_file: String,
    /// Manifest file name relative to the artifact directory.
    pub manifest_file: String,
    /// Optional detached signature file name.
    pub signature_file: Option<String>,
    /// SHA-256 digest of the finalized snapshot DB.
    pub file_sha256: String,
    /// Verified number of balance-history rows.
    pub balance_history_count: u64,
    /// Verified number of live UTXOs.
    pub utxo_count: u64,
    /// Verified number of block commitment rows.
    pub block_commit_count: u64,
    /// Verified number of script registry rows.
    pub script_registry_count: u64,
}

/// Lightweight release-finalization result for one immutable snapshot artifact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotArtifactFinalizationReport {
    /// Exact BTC height represented by the artifact.
    pub height: u32,
    /// BTC network bound to the artifact.
    pub network: String,
    /// Canonical BTC block hash represented by the artifact.
    pub btc_block_hash: String,
    /// Consensus snapshot identity from the signed manifest.
    pub snapshot_id: String,
    /// Artifact directory relative to the builder root.
    pub artifact_dir: String,
    /// Snapshot DB file name relative to the artifact directory.
    pub snapshot_file: String,
    /// Manifest file name relative to the artifact directory.
    pub manifest_file: String,
    /// Detached signature file name relative to the artifact directory.
    pub signature_file: String,
    /// Recomputed SHA-256 digest of the immutable snapshot DB.
    pub file_sha256: String,
    /// Trusted signer that produced the detached manifest signature.
    pub signing_key_id: String,
    /// SHA-256 digest of the trusted-key catalog used for finalization.
    pub trusted_keys_sha256: String,
}

/// Persisted builder state together with the selected per-height job, if one exists.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotStatusReport {
    /// Root-level workspace state, or `None` before first use.
    pub state: Option<SnapshotBuilderState>,
    /// Requested or currently active job, if present.
    pub job: Option<SnapshotBuildJob>,
}

impl SnapshotCreateReport {
    fn from_marker(
        root: &Path,
        artifact_dir: &Path,
        marker: &SnapshotCompleteMarker,
        resumed: bool,
        already_complete: bool,
    ) -> Self {
        Self {
            resumed,
            already_complete,
            height: marker.height,
            network: marker.network.clone(),
            btc_block_hash: marker.btc_block_hash.clone(),
            snapshot_id: marker.snapshot_id.clone(),
            artifact_dir: display_relative(root, artifact_dir),
            snapshot_file: marker.snapshot_file.clone(),
            manifest_file: marker.manifest_file.clone(),
            signature_file: marker.signature_file.clone(),
            file_sha256: marker.file_sha256.clone(),
            balance_history_count: marker.balance_history_count,
            utxo_count: marker.utxo_count,
            block_commit_count: marker.block_commit_count,
            script_registry_count: marker.script_registry_count,
        }
    }
}

/// Coordinates one restartable mutable workspace and its immutable snapshots.
pub struct ExactHeightSnapshotBuilder {
    paths: BuilderPaths,
}

impl ExactHeightSnapshotBuilder {
    /// Creates a builder rooted at `root` without opening its workspace.
    pub fn new(root: PathBuf) -> Self {
        Self {
            paths: BuilderPaths::new(root),
        }
    }

    /// Creates or resumes one exact-height full-UTXO snapshot build.
    pub fn create(&self, options: SnapshotCreateOptions) -> Result<SnapshotCreateReport, String> {
        if options.target_height == 0 {
            return Err(
                "Snapshot height 0 is unsupported because balance-history block commitments start at height 1"
                    .to_string(),
            );
        }
        self.paths.create_dirs()?;
        let _lock = BuilderLock::acquire(&self.paths.root)?;
        self.prepare_workspace_config(options.config_file.as_deref())?;

        let mut config = BalanceHistoryConfig::load(&self.paths.workspace)?;
        config.root_dir = self.paths.workspace.clone();
        config.sync.max_sync_block_height = options.target_height;
        let memory = get_memory_usage_snapshot();
        config.validate_memory_budget(memory.limit_bytes)?;
        info!(
            "Snapshot builder memory plan: source={}, limit_bytes={}, used_bytes={}, utxo_cache_bytes={}, balance_cache_bytes={}, total_cache_bytes={}, pressure_threshold_percent={}",
            memory.source,
            memory.limit_bytes,
            memory.used_bytes,
            config.sync.utxo_max_cache_bytes,
            config.sync.balance_max_cache_bytes,
            config.sync.cache_budget_bytes()?,
            config.sync.max_memory_percent
        );
        let network = config.btc.network().to_string();
        let expected_block_hash = normalize_optional_hash(options.expected_block_hash)?;

        let mut state = self.load_or_create_state(&network)?;
        self.validate_state(&state, &network)?;
        if let Some(active_height) = state.active_job_height
            && active_height != options.target_height
        {
            return Err(format!(
                "Snapshot job {} is still active; resume or complete it before starting target {}",
                active_height, options.target_height
            ));
        }
        let resuming_active_job = state.active_job_height == Some(options.target_height);
        if let Some(latest) = state.latest_completed.as_ref()
            && options.target_height < latest.height
        {
            return Err(format!(
                "Snapshot target {} is below latest completed height {}",
                options.target_height, latest.height
            ));
        }

        let rpc_client = BTCRpcClient::new(config.btc.rpc_url(), config.btc.auth())?;
        let rebuilding_replaced_latest =
            self.validate_completed_base(&mut state, options.target_height, &network, &rpc_client)?;

        let job_file = self.paths.job_file(options.target_height);
        let existing_job: Option<SnapshotBuildJob> = load_json(&job_file)?;
        let resumed = existing_job.is_some();
        let mut job = match existing_job {
            Some(job) if job.stage == SnapshotBuildStage::Complete => {
                let canonical_hash =
                    format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
                if job
                    .btc_block_hash
                    .as_deref()
                    .map(|hash| hash.eq_ignore_ascii_case(&canonical_hash))
                    .unwrap_or(false)
                {
                    self.validate_job(&job, options.target_height, expected_block_hash.as_deref())?;
                    job
                } else {
                    SnapshotBuildJob::new(
                        options.target_height,
                        job.base_checkpoint,
                        expected_block_hash.clone(),
                    )
                }
            }
            Some(job) => {
                self.validate_job(&job, options.target_height, expected_block_hash.as_deref())?;
                job
            }
            None => SnapshotBuildJob::new(
                options.target_height,
                state.latest_completed.clone(),
                expected_block_hash.clone(),
            ),
        };

        if job.stage == SnapshotBuildStage::Complete
            && let Some(artifact_dir) = job.artifact_dir.as_deref()
        {
            let artifact_dir = self.resolve_managed_path(artifact_dir)?;
            let canonical_hash = format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
            if job
                .btc_block_hash
                .as_deref()
                .map(|hash| hash.eq_ignore_ascii_case(&canonical_hash))
                .unwrap_or(false)
            {
                let marker = verify_published_artifact(
                    &artifact_dir,
                    &network,
                    options.target_height,
                    Some(&canonical_hash),
                )?;
                self.finalize_state_from_marker(
                    &mut state,
                    &mut job,
                    &job_file,
                    &artifact_dir,
                    &marker,
                )?;
                return Ok(SnapshotCreateReport::from_marker(
                    &self.paths.root,
                    &artifact_dir,
                    &marker,
                    resumed,
                    true,
                ));
            }

            return Err(format!(
                "Completed snapshot job {} has inconsistent canonical branch state",
                options.target_height
            ));
        }

        if job.stage == SnapshotBuildStage::Verifying {
            return Err(format!(
                "Snapshot job {} already has a resumable verification artifact; run resume-verify --height {} instead of rebuilding it",
                options.target_height, options.target_height
            ));
        }

        job.set_stage(SnapshotBuildStage::Syncing);
        save_json_atomic(&job_file, &job)?;
        state.active_job_height = Some(options.target_height);
        state.updated_at = unix_timestamp();
        save_json_atomic(&self.paths.state_file, &state)?;
        crate::abort_after_checkpoint("syncing");

        let config = Arc::new(config);
        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let indexer = BalanceHistoryIndexer::new(config.clone(), output.clone())?;
        let workspace_height = indexer.db().get_btc_block_height()?;
        self.validate_workspace_height(
            &job,
            workspace_height,
            resuming_active_job || rebuilding_replaced_latest,
        )?;
        indexer.sync_to_height(options.target_height, options.poll_interval)?;

        let sealed_state_ref = build_historical_state_ref_at_height(
            config.as_ref(),
            indexer.db().as_ref(),
            options.target_height,
        )?
        .ok_or_else(|| {
            format!(
                "Missing block commit after synchronizing snapshot target {}",
                options.target_height
            )
        })?;
        let canonical_hash = format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
        if !sealed_state_ref
            .stable_block_hash
            .eq_ignore_ascii_case(&canonical_hash)
        {
            return Err(format!(
                "Sealed workspace block hash {} does not match canonical BTC block hash {} at height {}",
                sealed_state_ref.stable_block_hash, canonical_hash, options.target_height
            ));
        }
        if let Some(expected) = expected_block_hash.as_deref()
            && !canonical_hash.eq_ignore_ascii_case(expected)
        {
            return Err(format!(
                "Canonical BTC block hash mismatch at height {}: expected {}, got {}",
                options.target_height, expected, canonical_hash
            ));
        }

        job.btc_block_hash = Some(canonical_hash.clone());
        job.snapshot_id = Some(sealed_state_ref.snapshot_id.clone());
        job.set_stage(SnapshotBuildStage::Sealed);
        save_json_atomic(&job_file, &job)?;
        crate::abort_after_checkpoint("sealed");

        let final_dir = self
            .paths
            .snapshot_artifact_dir(options.target_height, &canonical_hash);
        if final_dir.exists() {
            let marker = verify_published_artifact(
                &final_dir,
                &network,
                options.target_height,
                Some(&canonical_hash),
            )?;
            self.finalize_state_from_marker(&mut state, &mut job, &job_file, &final_dir, &marker)?;
            return Ok(SnapshotCreateReport::from_marker(
                &self.paths.root,
                &final_dir,
                &marker,
                true,
                true,
            ));
        }

        self.remove_previous_temp_dir(&job)?;
        let temp_dir =
            self.paths
                .temp
                .join(format!("{:012}-{}", options.target_height, unique_run_id()));
        std::fs::create_dir_all(&temp_dir).map_err(|e| {
            format!(
                "Failed to create snapshot temporary directory {}: {}",
                temp_dir.display(),
                e
            )
        })?;
        job.attempt = job.attempt.saturating_add(1);
        job.temp_dir = Some(display_relative(&self.paths.root, &temp_dir));
        job.set_stage(SnapshotBuildStage::Building);
        save_json_atomic(&job_file, &job)?;
        crate::abort_after_checkpoint("building");

        let snapshot_file_name = format!("snapshot_{}.db", options.target_height);
        let snapshot_path = temp_dir.join(&snapshot_file_name);
        let snapshot_indexer =
            SnapshotIndexer::new(config.clone(), indexer.db().clone(), output.clone());
        let creation = snapshot_indexer.run_to_path(options.target_height, true, &snapshot_path)?;

        info!(
            "Explicitly releasing snapshot export and RocksDB/indexer resources before verification"
        );
        drop(snapshot_indexer);
        drop(indexer);
        drop(output);
        drop(config);
        info!("Snapshot export and RocksDB/indexer resources released");

        job.set_stage(SnapshotBuildStage::Verifying);
        save_json_atomic(&job_file, &job)?;
        crate::abort_after_checkpoint("verifying");
        let verified = self.verify_snapshot_files_with_job_progress(
            &mut job,
            &job_file,
            &creation.db_path,
            &creation.manifest_path,
            &network,
            options.target_height,
            Some(&canonical_hash),
        )?;
        if verified.manifest.state_ref != sealed_state_ref {
            return Err(format!(
                "Generated snapshot state reference changed after sealing target {}",
                options.target_height
            ));
        }
        let marker = build_complete_marker(&verified, &network)?;
        save_json_atomic(&temp_dir.join("complete.json"), &marker)?;

        let canonical_hash_after_build =
            format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
        if !canonical_hash_after_build.eq_ignore_ascii_case(&canonical_hash) {
            return Err(format!(
                "BTC block hash changed while building snapshot at height {}: sealed={}, current={}",
                options.target_height, canonical_hash, canonical_hash_after_build
            ));
        }

        let final_parent = final_dir.parent().ok_or_else(|| {
            format!(
                "Snapshot artifact path {} has no parent",
                final_dir.display()
            )
        })?;
        std::fs::create_dir_all(final_parent).map_err(|e| {
            format!(
                "Failed to create snapshot artifact parent {}: {}",
                final_parent.display(),
                e
            )
        })?;
        crate::fail_at_checkpoint("before_publish")?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| {
            format!(
                "Failed to atomically publish snapshot directory {} as {}: {}",
                temp_dir.display(),
                final_dir.display(),
                e
            )
        })?;
        std::fs::File::open(final_parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                format!(
                    "Failed to sync published snapshot parent {}: {}",
                    final_parent.display(),
                    e
                )
            })?;
        crate::abort_after_checkpoint("published");

        let published_marker = verify_published_artifact_marker(
            &final_dir,
            &network,
            options.target_height,
            Some(&canonical_hash),
        )?;
        if published_marker != marker {
            return Err(format!(
                "Published snapshot marker changed during atomic publication at {}",
                final_dir.display()
            ));
        }
        self.remove_stale_sqlite_sidecars(&final_dir, &marker)?;
        self.finalize_state_from_marker(&mut state, &mut job, &job_file, &final_dir, &marker)?;

        Ok(SnapshotCreateReport::from_marker(
            &self.paths.root,
            &final_dir,
            &marker,
            resumed,
            false,
        ))
    }

    /// Resumes full verification and publication without opening the mutable RocksDB workspace.
    pub fn resume_verify(
        &self,
        options: SnapshotResumeVerifyOptions,
    ) -> Result<SnapshotCreateReport, String> {
        if options.target_height == 0 {
            return Err(
                "Snapshot height 0 is unsupported because balance-history block commitments start at height 1"
                    .to_string(),
            );
        }
        self.paths.create_dirs()?;
        let _lock = BuilderLock::acquire(&self.paths.root)?;

        let config = BalanceHistoryConfig::load(&self.paths.workspace)?;
        let network = config.btc.network().to_string();
        let expected_block_hash = normalize_optional_hash(options.expected_block_hash)?;
        let mut state: SnapshotBuilderState =
            load_json(&self.paths.state_file)?.ok_or_else(|| {
                format!(
                    "Snapshot builder state does not exist at {}",
                    self.paths.state_file.display()
                )
            })?;
        self.validate_state(&state, &network)?;

        let job_file = self.paths.job_file(options.target_height);
        let mut job: SnapshotBuildJob = load_json(&job_file)?.ok_or_else(|| {
            format!(
                "Snapshot job {} does not exist at {}",
                options.target_height,
                job_file.display()
            )
        })?;
        self.validate_job(&job, options.target_height, expected_block_hash.as_deref())?;

        if job.stage == SnapshotBuildStage::Complete {
            let artifact_dir = job.artifact_dir.as_deref().ok_or_else(|| {
                format!(
                    "Completed snapshot job {} is missing artifact_dir",
                    options.target_height
                )
            })?;
            let artifact_dir = self.resolve_managed_path(artifact_dir)?;
            let marker = verify_published_artifact_marker(
                &artifact_dir,
                &network,
                options.target_height,
                expected_block_hash.as_deref(),
            )?;
            self.validate_marker_for_job(&job, &marker)?;
            return Ok(SnapshotCreateReport::from_marker(
                &self.paths.root,
                &artifact_dir,
                &marker,
                true,
                true,
            ));
        }
        if job.stage != SnapshotBuildStage::Verifying {
            return Err(format!(
                "Snapshot job {} is in stage {:?}; resume-verify requires stage Verifying",
                options.target_height, job.stage
            ));
        }
        if state.active_job_height != Some(options.target_height) {
            return Err(format!(
                "Snapshot builder active job mismatch: expected {}, got {:?}",
                options.target_height, state.active_job_height
            ));
        }

        let canonical_hash = job.btc_block_hash.clone().ok_or_else(|| {
            format!(
                "Verifying snapshot job {} is missing its sealed BTC block hash",
                options.target_height
            )
        })?;
        if let Some(expected) = expected_block_hash.as_deref()
            && !canonical_hash.eq_ignore_ascii_case(expected)
        {
            return Err(format!(
                "Snapshot job {} sealed BTC block hash {} does not match requested {}",
                options.target_height, canonical_hash, expected
            ));
        }
        let rpc_client = BTCRpcClient::new(config.btc.rpc_url(), config.btc.auth())?;
        let current_hash = format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
        if !current_hash.eq_ignore_ascii_case(&canonical_hash) {
            return Err(format!(
                "Canonical BTC block hash changed before resumed verification at height {}: sealed={}, current={}",
                options.target_height, canonical_hash, current_hash
            ));
        }

        let final_dir = self
            .paths
            .snapshot_artifact_dir(options.target_height, &canonical_hash);
        if final_dir.exists() {
            let marker = verify_published_artifact_marker(
                &final_dir,
                &network,
                options.target_height,
                Some(&canonical_hash),
            )?;
            self.validate_marker_for_job(&job, &marker)?;
            self.remove_stale_sqlite_sidecars(&final_dir, &marker)?;
            self.finalize_state_from_marker(&mut state, &mut job, &job_file, &final_dir, &marker)?;
            return Ok(SnapshotCreateReport::from_marker(
                &self.paths.root,
                &final_dir,
                &marker,
                true,
                false,
            ));
        }

        let temp_dir = job.temp_dir.as_deref().ok_or_else(|| {
            format!(
                "Verifying snapshot job {} is missing temp_dir",
                options.target_height
            )
        })?;
        let temp_dir = self.resolve_managed_path(temp_dir)?;
        if !temp_dir.starts_with(&self.paths.temp) || !temp_dir.is_dir() {
            return Err(format!(
                "Verifying snapshot job {} references unavailable temporary directory {}",
                options.target_height,
                temp_dir.display()
            ));
        }

        let marker_path = temp_dir.join("complete.json");
        let marker = if marker_path.is_file() {
            let marker = verify_published_artifact_marker(
                &temp_dir,
                &network,
                options.target_height,
                Some(&canonical_hash),
            )?;
            self.validate_marker_for_job(&job, &marker)?;
            self.remove_stale_sqlite_sidecars(&temp_dir, &marker)?;
            marker
        } else {
            let db_path = temp_dir.join(format!("snapshot_{}.db", options.target_height));
            let manifest_path = manifest_path_for_snapshot_file(&db_path);
            let verified = self.verify_snapshot_files_with_job_progress(
                &mut job,
                &job_file,
                &db_path,
                &manifest_path,
                &network,
                options.target_height,
                Some(&canonical_hash),
            )?;
            let expected_snapshot_id = job.snapshot_id.as_deref().ok_or_else(|| {
                format!(
                    "Verifying snapshot job {} is missing its sealed snapshot ID",
                    options.target_height
                )
            })?;
            if verified.manifest.state_ref.snapshot_id != expected_snapshot_id {
                return Err(format!(
                    "Generated snapshot ID {} does not match sealed job snapshot ID {}",
                    verified.manifest.state_ref.snapshot_id, expected_snapshot_id
                ));
            }
            let marker = build_complete_marker(&verified, &network)?;
            save_json_atomic(&marker_path, &marker)?;
            marker
        };

        let canonical_hash_after_verify =
            format!("{:x}", rpc_client.get_block_hash(options.target_height)?);
        if !canonical_hash_after_verify.eq_ignore_ascii_case(&canonical_hash) {
            return Err(format!(
                "BTC block hash changed while resuming snapshot verification at height {}: sealed={}, current={}",
                options.target_height, canonical_hash, canonical_hash_after_verify
            ));
        }

        let final_parent = final_dir.parent().ok_or_else(|| {
            format!(
                "Snapshot artifact path {} has no parent",
                final_dir.display()
            )
        })?;
        std::fs::create_dir_all(final_parent).map_err(|e| {
            format!(
                "Failed to create snapshot artifact parent {}: {}",
                final_parent.display(),
                e
            )
        })?;
        crate::fail_at_checkpoint("before_publish")?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| {
            format!(
                "Failed to atomically publish snapshot directory {} as {}: {}",
                temp_dir.display(),
                final_dir.display(),
                e
            )
        })?;
        File::open(final_parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                format!(
                    "Failed to sync published snapshot parent {}: {}",
                    final_parent.display(),
                    e
                )
            })?;
        crate::abort_after_checkpoint("published");

        let published_marker = verify_published_artifact_marker(
            &final_dir,
            &network,
            options.target_height,
            Some(&canonical_hash),
        )?;
        if published_marker != marker {
            return Err(format!(
                "Published snapshot marker changed during atomic publication at {}",
                final_dir.display()
            ));
        }
        self.remove_stale_sqlite_sidecars(&final_dir, &marker)?;
        self.finalize_state_from_marker(&mut state, &mut job, &job_file, &final_dir, &marker)?;

        Ok(SnapshotCreateReport::from_marker(
            &self.paths.root,
            &final_dir,
            &marker,
            true,
            false,
        ))
    }

    /// Returns root state and an optional requested or active per-height job.
    pub fn status(&self, height: Option<u32>) -> Result<SnapshotStatusReport, String> {
        let state = load_json(&self.paths.state_file)?;
        let job_height = height.or_else(|| {
            state
                .as_ref()
                .and_then(|value: &SnapshotBuilderState| value.active_job_height)
        });
        let job = match job_height {
            Some(height) => load_json(&self.paths.job_file(height))?,
            None => None,
        };
        Ok(SnapshotStatusReport { state, job })
    }

    /// Lists all persisted jobs in ascending target-height order.
    pub fn list_jobs(&self) -> Result<Vec<SnapshotBuildJob>, String> {
        if !self.paths.jobs.exists() {
            return Ok(Vec::new());
        }
        let mut jobs: Vec<SnapshotBuildJob> = Vec::new();
        for entry in std::fs::read_dir(&self.paths.jobs).map_err(|e| {
            format!(
                "Failed to list snapshot jobs in {}: {}",
                self.paths.jobs.display(),
                e
            )
        })? {
            let entry = entry.map_err(|e| format!("Failed to read snapshot job entry: {}", e))?;
            let job_file = entry.path().join("job.json");
            if let Some(job) = load_json(&job_file)? {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|job| job.target_height);
        Ok(jobs)
    }

    /// Reopens and verifies one completed artifact without modifying builder state.
    pub fn verify(
        &self,
        height: u32,
        block_hash: Option<&str>,
    ) -> Result<SnapshotCompleteMarker, String> {
        let state: SnapshotBuilderState = load_json(&self.paths.state_file)?.ok_or_else(|| {
            format!(
                "Snapshot builder state does not exist at {}",
                self.paths.state_file.display()
            )
        })?;
        let artifact_dir = if let Some(block_hash) = block_hash {
            self.paths.snapshot_artifact_dir(height, block_hash)
        } else {
            self.find_single_artifact_dir(height)?
        };
        verify_published_artifact(&artifact_dir, &state.network, height, block_hash)
    }

    /// Rechecks artifact identity, DB hash, and detached signature without opening SQLite.
    pub fn finalize_artifact(
        &self,
        height: u32,
        block_hash: Option<&str>,
        trusted_keys_path: &Path,
    ) -> Result<SnapshotArtifactFinalizationReport, String> {
        let state: SnapshotBuilderState = load_json(&self.paths.state_file)?.ok_or_else(|| {
            format!(
                "Snapshot builder state does not exist at {}",
                self.paths.state_file.display()
            )
        })?;
        let artifact_dir = if let Some(block_hash) = block_hash {
            self.paths.snapshot_artifact_dir(height, block_hash)
        } else {
            self.find_single_artifact_dir(height)?
        };
        let marker =
            verify_published_artifact_marker(&artifact_dir, &state.network, height, block_hash)?;
        let snapshot_path = artifact_dir.join(&marker.snapshot_file);
        let manifest_path = artifact_dir.join(&marker.manifest_file);
        let manifest = SnapshotManifest::load(&manifest_path)?;
        let signing_key_id =
            verify_snapshot_manifest_signature(&manifest, &manifest_path, trusted_keys_path)?;

        let hash_progress = snapshot_hash_progress_bar(&snapshot_path)?;
        let actual_file_sha256 =
            match SnapshotHash::calc_hash_with_progress(&snapshot_path, |processed, _| {
                hash_progress.set_position(processed);
            }) {
                Ok(hash) => {
                    hash_progress.finish_with_message("hash verified");
                    hash
                }
                Err(error) => {
                    hash_progress.abandon_with_message("hash failed");
                    return Err(error);
                }
            };
        if !actual_file_sha256.eq_ignore_ascii_case(&marker.file_sha256) {
            return Err(format!(
                "Snapshot file hash mismatch during release finalization: marker={}, actual={}",
                marker.file_sha256, actual_file_sha256
            ));
        }
        let trusted_keys_sha256 = SnapshotHash::calc_hash(trusted_keys_path)?;
        let signature_file = marker.signature_file.ok_or_else(|| {
            format!(
                "Signed snapshot artifact {} has no detached signature in complete.json",
                artifact_dir.display()
            )
        })?;

        Ok(SnapshotArtifactFinalizationReport {
            height: marker.height,
            network: marker.network,
            btc_block_hash: marker.btc_block_hash,
            snapshot_id: marker.snapshot_id,
            artifact_dir: display_relative(&self.paths.root, &artifact_dir),
            snapshot_file: marker.snapshot_file,
            manifest_file: marker.manifest_file,
            signature_file,
            file_sha256: actual_file_sha256,
            signing_key_id,
            trusted_keys_sha256,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_snapshot_files_with_job_progress(
        &self,
        job: &mut SnapshotBuildJob,
        job_file: &Path,
        db_path: &Path,
        manifest_path: &Path,
        expected_network: &str,
        expected_height: u32,
        expected_block_hash: Option<&str>,
    ) -> Result<VerifiedSnapshot, String> {
        let db_path = db_path.to_path_buf();
        let manifest_path = manifest_path.to_path_buf();
        let expected_network = expected_network.to_string();
        let expected_block_hash = expected_block_hash.map(str::to_string);
        let (sender, receiver) = mpsc::channel();
        let worker_sender = sender.clone();
        let worker = thread::Builder::new()
            .name(format!("snapshot-verify-{}", expected_height))
            .spawn(move || {
                let progress_sender = worker_sender.clone();
                let result = verify_snapshot_files_with_progress(
                    &db_path,
                    &manifest_path,
                    &expected_network,
                    expected_height,
                    expected_block_hash.as_deref(),
                    move |phase| {
                        progress_sender
                            .send(VerificationMessage::Phase(phase))
                            .map_err(|_| {
                                "Snapshot verification progress receiver disconnected".to_string()
                            })
                    },
                );
                let _ = worker_sender.send(VerificationMessage::Finished(Box::new(result)));
            })
            .map_err(|e| format!("Failed to start snapshot verification worker: {}", e))?;
        drop(sender);

        let verification_started = Instant::now();
        let mut current_phase: Option<(SnapshotVerificationPhase, Instant)> = None;
        let mut persistence_error = None;
        loop {
            match receiver.recv_timeout(VERIFICATION_HEARTBEAT_INTERVAL) {
                Ok(VerificationMessage::Phase(phase)) => {
                    if let Some((previous_phase, previous_started)) = current_phase.take() {
                        print_verification_progress(
                            "phase_completed",
                            expected_height,
                            previous_phase,
                            previous_started.elapsed(),
                            verification_started.elapsed(),
                        );
                    }
                    current_phase = Some((phase, Instant::now()));
                    if let Err(e) = self.persist_verification_progress(job, job_file, phase, true) {
                        error!("{}", e);
                        persistence_error.get_or_insert(e);
                    }
                    print_verification_progress(
                        "phase_started",
                        expected_height,
                        phase,
                        Duration::ZERO,
                        verification_started.elapsed(),
                    );
                }
                Ok(VerificationMessage::Finished(result)) => {
                    worker.join().map_err(|_| {
                        "Snapshot verification worker panicked after reporting completion"
                            .to_string()
                    })?;
                    if let Some((phase, phase_started)) = current_phase.take() {
                        let event = if result.is_ok() {
                            "phase_completed"
                        } else {
                            "phase_failed"
                        };
                        print_verification_progress(
                            event,
                            expected_height,
                            phase,
                            phase_started.elapsed(),
                            verification_started.elapsed(),
                        );
                    }
                    if let Some(error) = persistence_error {
                        return Err(error);
                    }
                    return *result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some((phase, phase_started)) = current_phase.as_ref() {
                        if let Err(e) =
                            self.persist_verification_progress(job, job_file, *phase, false)
                        {
                            error!("{}", e);
                            persistence_error.get_or_insert(e);
                        }
                        print_verification_progress(
                            "heartbeat",
                            expected_height,
                            *phase,
                            phase_started.elapsed(),
                            verification_started.elapsed(),
                        );
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    worker.join().map_err(|_| {
                        "Snapshot verification worker panicked before reporting completion"
                            .to_string()
                    })?;
                    return Err(
                        "Snapshot verification worker exited without reporting completion"
                            .to_string(),
                    );
                }
            }
        }
    }

    fn persist_verification_progress(
        &self,
        job: &mut SnapshotBuildJob,
        job_file: &Path,
        phase: SnapshotVerificationPhase,
        phase_changed: bool,
    ) -> Result<(), String> {
        let now = unix_timestamp();
        let phase_started_at = if phase_changed {
            now
        } else {
            job.verification
                .as_ref()
                .filter(|progress| progress.phase == phase)
                .map(|progress| progress.phase_started_at)
                .unwrap_or(now)
        };
        job.verification = Some(SnapshotVerificationProgress {
            phase,
            phase_started_at,
            heartbeat_at: now,
        });
        job.updated_at = now;
        save_json_atomic(job_file, job)?;
        if phase_changed {
            info!(
                "Snapshot verification entered phase {:?} for height {}",
                phase, job.target_height
            );
        } else {
            info!(
                "Snapshot verification heartbeat: height={}, phase={:?}, phase_elapsed_secs={}",
                job.target_height,
                phase,
                now.saturating_sub(phase_started_at)
            );
        }
        Ok(())
    }

    fn validate_marker_for_job(
        &self,
        job: &SnapshotBuildJob,
        marker: &SnapshotCompleteMarker,
    ) -> Result<(), String> {
        let expected_hash = job.btc_block_hash.as_deref().ok_or_else(|| {
            format!(
                "Snapshot job {} is missing its sealed BTC block hash",
                job.target_height
            )
        })?;
        let expected_snapshot_id = job.snapshot_id.as_deref().ok_or_else(|| {
            format!(
                "Snapshot job {} is missing its sealed snapshot ID",
                job.target_height
            )
        })?;
        if marker.height != job.target_height
            || !marker.btc_block_hash.eq_ignore_ascii_case(expected_hash)
            || marker.snapshot_id != expected_snapshot_id
        {
            return Err(format!(
                "Snapshot completion marker does not match persisted job {}",
                job.target_height
            ));
        }
        Ok(())
    }

    fn remove_stale_sqlite_sidecars(
        &self,
        artifact_dir: &Path,
        marker: &SnapshotCompleteMarker,
    ) -> Result<(), String> {
        let db_path = artifact_dir.join(&marker.snapshot_file);
        let wal_path = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
        let shm_path = PathBuf::from(format!("{}-shm", db_path.to_string_lossy()));
        if wal_path.exists() {
            let wal_size = wal_path
                .metadata()
                .map_err(|e| {
                    format!(
                        "Failed to inspect snapshot WAL sidecar {}: {}",
                        wal_path.display(),
                        e
                    )
                })?
                .len();
            if wal_size != 0 {
                return Err(format!(
                    "Refusing to publish snapshot with non-empty WAL sidecar {} ({} bytes)",
                    wal_path.display(),
                    wal_size
                ));
            }
            std::fs::remove_file(&wal_path).map_err(|e| {
                format!(
                    "Failed to remove empty snapshot WAL sidecar {}: {}",
                    wal_path.display(),
                    e
                )
            })?;
        }
        if shm_path.exists() {
            std::fs::remove_file(&shm_path).map_err(|e| {
                format!(
                    "Failed to remove snapshot shared-memory sidecar {}: {}",
                    shm_path.display(),
                    e
                )
            })?;
        }
        if wal_path.exists() || shm_path.exists() {
            return Err(format!(
                "Snapshot SQLite sidecar cleanup did not complete in {}",
                artifact_dir.display()
            ));
        }
        File::open(artifact_dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                format!(
                    "Failed to sync snapshot artifact directory {} after sidecar cleanup: {}",
                    artifact_dir.display(),
                    e
                )
            })?;
        Ok(())
    }

    fn prepare_workspace_config(&self, source: Option<&Path>) -> Result<(), String> {
        let destination = self.paths.workspace.join("config.toml");
        let Some(source) = source else {
            if destination.is_file() {
                return Ok(());
            }
            return Err(format!(
                "A balance-history config is required for a new snapshot builder workspace; pass --config <FILE> (expected destination {})",
                destination.display()
            ));
        };
        let source_data = std::fs::read(source).map_err(|e| {
            format!(
                "Failed to read snapshot builder config source {}: {}",
                source.display(),
                e
            )
        })?;
        if destination.exists() {
            let existing = std::fs::read(&destination).map_err(|e| {
                format!(
                    "Failed to read existing workspace config {}: {}",
                    destination.display(),
                    e
                )
            })?;
            if existing != source_data {
                let existing_config = parse_balance_history_config(&existing, &destination)?;
                let requested_config = parse_balance_history_config(&source_data, source)?;
                if !configs_equal_except_memory(&existing_config, &requested_config) {
                    return Err(format!(
                        "Workspace config {} differs from requested config {}; only cache limits and the memory-pressure threshold may change for an existing builder",
                        destination.display(),
                        source.display()
                    ));
                }
                info!(
                    "Refreshing operational memory settings in snapshot workspace config {}",
                    destination.display()
                );
            } else {
                return Ok(());
            }
        }
        let temp = self
            .paths
            .workspace
            .join(format!(".config.{}.tmp", unique_run_id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| {
                format!(
                    "Failed to create temporary workspace config {}: {}",
                    temp.display(),
                    e
                )
            })?;
        file.write_all(&source_data).map_err(|e| {
            format!(
                "Failed to write temporary workspace config {}: {}",
                temp.display(),
                e
            )
        })?;
        file.sync_all().map_err(|e| {
            format!(
                "Failed to sync temporary workspace config {}: {}",
                temp.display(),
                e
            )
        })?;
        drop(file);
        std::fs::rename(&temp, &destination).map_err(|e| {
            format!(
                "Failed to publish workspace config {}: {}",
                destination.display(),
                e
            )
        })?;
        File::open(&self.paths.workspace)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| {
                format!(
                    "Failed to sync workspace config directory {}: {}",
                    self.paths.workspace.display(),
                    e
                )
            })
    }

    fn load_or_create_state(&self, network: &str) -> Result<SnapshotBuilderState, String> {
        Ok(load_json(&self.paths.state_file)?
            .unwrap_or_else(|| SnapshotBuilderState::new(network.to_string())))
    }

    fn validate_state(&self, state: &SnapshotBuilderState, network: &str) -> Result<(), String> {
        if state.version != crate::BUILDER_STATE_VERSION {
            return Err(format!(
                "Unsupported snapshot builder state version {}",
                state.version
            ));
        }
        if state.network != network {
            return Err(format!(
                "Snapshot builder network mismatch: state={}, config={}",
                state.network, network
            ));
        }
        Ok(())
    }

    fn validate_job(
        &self,
        job: &SnapshotBuildJob,
        target_height: u32,
        expected_block_hash: Option<&str>,
    ) -> Result<(), String> {
        if job.version != crate::JOB_STATE_VERSION || job.target_height != target_height {
            return Err(format!(
                "Snapshot job identity mismatch at target {}",
                target_height
            ));
        }
        if job.expected_block_hash.as_deref() != expected_block_hash {
            return Err(format!(
                "Snapshot job {} was created with expected block hash {:?}, requested {:?}",
                target_height, job.expected_block_hash, expected_block_hash
            ));
        }
        Ok(())
    }

    fn validate_completed_base(
        &self,
        state: &mut SnapshotBuilderState,
        target_height: u32,
        network: &str,
        rpc_client: &BTCRpcClient,
    ) -> Result<bool, String> {
        let Some(latest) = state.latest_completed.clone() else {
            return Ok(false);
        };
        let canonical_hash = format!("{:x}", rpc_client.get_block_hash(latest.height)?);
        if latest.btc_block_hash.eq_ignore_ascii_case(&canonical_hash) {
            let artifact_dir = self.resolve_managed_path(&latest.artifact_dir)?;
            verify_published_artifact(
                &artifact_dir,
                network,
                latest.height,
                Some(&canonical_hash),
            )?;
            return Ok(false);
        }
        if target_height != latest.height {
            return Err(format!(
                "Latest completed snapshot at height {} belongs to replaced BTC block {}; canonical block is {}. Rebuild height {} before advancing to {}",
                latest.height, latest.btc_block_hash, canonical_hash, latest.height, target_height
            ));
        }

        // Rebuilding the replaced latest height restores the previous completed base, if any.
        let previous_job: Option<SnapshotBuildJob> =
            load_json(&self.paths.job_file(latest.height))?;
        state.latest_completed = previous_job.and_then(|job| job.base_checkpoint);
        state.updated_at = unix_timestamp();
        Ok(true)
    }

    fn validate_workspace_height(
        &self,
        job: &SnapshotBuildJob,
        workspace_height: u32,
        may_have_uncommitted_progress: bool,
    ) -> Result<(), String> {
        if workspace_height > job.target_height {
            return Err(format!(
                "Workspace height {} is above active snapshot target {}",
                workspace_height, job.target_height
            ));
        }
        if let Some(base) = job.base_checkpoint.as_ref()
            && workspace_height < base.height
        {
            return Err(format!(
                "Workspace height {} is below completed base height {}",
                workspace_height, base.height
            ));
        }
        if !may_have_uncommitted_progress
            && let Some(base) = job.base_checkpoint.as_ref()
            && workspace_height != base.height
        {
            return Err(format!(
                "Idle workspace height {} does not match latest completed height {}",
                workspace_height, base.height
            ));
        }
        Ok(())
    }

    fn remove_previous_temp_dir(&self, job: &SnapshotBuildJob) -> Result<(), String> {
        let Some(temp_dir) = job.temp_dir.as_deref() else {
            return Ok(());
        };
        let temp_dir = self.resolve_managed_path(temp_dir)?;
        if !temp_dir.starts_with(&self.paths.temp) {
            return Err(format!(
                "Refusing to remove unmanaged snapshot temporary directory {}",
                temp_dir.display()
            ));
        }
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).map_err(|e| {
                format!(
                    "Failed to clean interrupted snapshot directory {}: {}",
                    temp_dir.display(),
                    e
                )
            })?;
        }
        Ok(())
    }

    fn finalize_state_from_marker(
        &self,
        state: &mut SnapshotBuilderState,
        job: &mut SnapshotBuildJob,
        job_file: &Path,
        artifact_dir: &Path,
        marker: &SnapshotCompleteMarker,
    ) -> Result<(), String> {
        let completed = CompletedSnapshotRef {
            height: marker.height,
            btc_block_hash: marker.btc_block_hash.clone(),
            snapshot_id: marker.snapshot_id.clone(),
            artifact_dir: display_relative(&self.paths.root, artifact_dir),
        };
        job.stage = SnapshotBuildStage::Complete;
        job.btc_block_hash = Some(marker.btc_block_hash.clone());
        job.snapshot_id = Some(marker.snapshot_id.clone());
        job.temp_dir = None;
        job.artifact_dir = Some(completed.artifact_dir.clone());
        job.verification = None;
        job.completed_at = Some(marker.completed_at);
        job.updated_at = unix_timestamp();
        save_json_atomic(job_file, job)?;
        crate::abort_after_checkpoint("job_complete");

        state.latest_completed = Some(completed);
        state.active_job_height = None;
        state.updated_at = unix_timestamp();
        save_json_atomic(&self.paths.state_file, state)
    }

    fn resolve_managed_path(&self, path: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(path);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "Snapshot state references unsafe managed path: {}",
                path.display()
            ));
        }
        Ok(self.paths.root.join(path))
    }

    fn find_single_artifact_dir(&self, height: u32) -> Result<PathBuf, String> {
        let height_dir = self.paths.snapshot_height_dir(height);
        let mut artifacts = Vec::new();
        for entry in std::fs::read_dir(&height_dir).map_err(|e| {
            format!(
                "Failed to list snapshot artifacts at height {} in {}: {}",
                height,
                height_dir.display(),
                e
            )
        })? {
            let path = entry
                .map_err(|e| format!("Failed to read snapshot artifact entry: {}", e))?
                .path();
            if path.is_dir() && path.join("complete.json").is_file() {
                artifacts.push(path);
            }
        }
        match artifacts.len() {
            1 => Ok(artifacts.remove(0)),
            0 => Err(format!("No completed snapshot exists at height {}", height)),
            count => Err(format!(
                "Height {} has {} completed branch artifacts; specify --block-hash",
                height, count
            )),
        }
    }
}

fn parse_balance_history_config(data: &[u8], path: &Path) -> Result<BalanceHistoryConfig, String> {
    let text = std::str::from_utf8(data).map_err(|e| {
        format!(
            "Balance-history config {} is not UTF-8: {}",
            path.display(),
            e
        )
    })?;
    toml::from_str(text).map_err(|e| {
        format!(
            "Failed to parse balance-history config {} while comparing workspace settings: {}",
            path.display(),
            e
        )
    })
}

fn configs_equal_except_memory(
    existing: &BalanceHistoryConfig,
    requested: &BalanceHistoryConfig,
) -> bool {
    let mut existing = existing.clone();
    existing.sync.utxo_max_cache_bytes = requested.sync.utxo_max_cache_bytes;
    existing.sync.balance_max_cache_bytes = requested.sync.balance_max_cache_bytes;
    existing.sync.max_memory_percent = requested.sync.max_memory_percent;
    existing == *requested
}

fn normalize_optional_hash(value: Option<String>) -> Result<Option<String>, String> {
    value.map(|value| normalize_hash(&value)).transpose()
}

fn normalize_hash(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid BTC block hash {}; expected 64 hexadecimal characters",
            value
        ));
    }
    Ok(value)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(height: u32) -> CompletedSnapshotRef {
        CompletedSnapshotRef {
            height,
            btc_block_hash: format!("{:064x}", height),
            snapshot_id: format!("snapshot-{height}"),
            artifact_dir: format!("snapshots/{height}"),
        }
    }

    #[test]
    fn verification_progress_line_is_timestamped_and_human_readable() {
        let line = format_verification_progress_line(
            "2026-08-30 20:26:56 -07:00",
            "heartbeat",
            963_800,
            SnapshotVerificationPhase::IntegrityCheck,
            Duration::from_secs(4_441),
            Duration::from_secs(5_732),
        );

        assert_eq!(
            line,
            "[2026-08-30 20:26:56 -07:00] Snapshot verification heartbeat: height=963800, phase=integrity_check, phase_elapsed=01:14:01, verify_elapsed=01:35:32"
        );
    }

    #[test]
    fn new_incremental_job_requires_workspace_at_completed_base() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let job = SnapshotBuildJob::new(11, Some(completed(10)), None);

        builder.validate_workspace_height(&job, 10, false).unwrap();
        let error = builder
            .validate_workspace_height(&job, 11, false)
            .unwrap_err();
        assert!(error.contains("does not match latest completed height 10"));
    }

    #[test]
    fn resumed_job_may_reuse_uncommitted_workspace_progress() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let job = SnapshotBuildJob::new(12, Some(completed(10)), None);

        builder.validate_workspace_height(&job, 11, true).unwrap();
        let error = builder
            .validate_workspace_height(&job, 13, true)
            .unwrap_err();
        assert!(error.contains("above active snapshot target 12"));
    }

    #[test]
    fn height_zero_is_rejected_before_workspace_creation() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let error = builder
            .create(SnapshotCreateOptions {
                target_height: 0,
                expected_block_hash: None,
                poll_interval: Duration::from_secs(1),
                config_file: None,
            })
            .unwrap_err();

        assert!(error.contains("height 0 is unsupported"));
    }

    #[test]
    fn unused_workspace_requires_explicit_config() {
        let root = std::env::temp_dir().join(format!("snapshot-config-{}", unique_run_id()));
        let builder = ExactHeightSnapshotBuilder::new(root);
        builder.paths.create_dirs().unwrap();

        let error = builder.prepare_workspace_config(None).unwrap_err();

        assert!(error.contains("pass --config <FILE>"));
    }

    #[test]
    fn workspace_config_allows_only_operational_memory_refresh() {
        let root = std::env::temp_dir().join(format!("snapshot-config-{}", unique_run_id()));
        let source = root.join("source.toml");
        let builder = ExactHeightSnapshotBuilder::new(root.join("builder"));
        builder.paths.create_dirs().unwrap();

        let mut config = BalanceHistoryConfig {
            root_dir: builder.paths.workspace.clone(),
            ..BalanceHistoryConfig::default()
        };
        config.sync.utxo_max_cache_bytes = 4 * 1024 * 1024;
        config.sync.balance_max_cache_bytes = 12 * 1024 * 1024;
        config.sync.max_memory_percent = 80;
        std::fs::write(&source, toml::to_string_pretty(&config).unwrap()).unwrap();
        builder.prepare_workspace_config(Some(&source)).unwrap();

        config.sync.utxo_max_cache_bytes = 8 * 1024 * 1024;
        config.sync.balance_max_cache_bytes = 24 * 1024 * 1024;
        std::fs::write(&source, toml::to_string_pretty(&config).unwrap()).unwrap();
        builder.prepare_workspace_config(Some(&source)).unwrap();
        let refreshed =
            std::fs::read_to_string(builder.paths.workspace.join("config.toml")).unwrap();
        let refreshed: BalanceHistoryConfig = toml::from_str(&refreshed).unwrap();
        assert_eq!(
            refreshed.sync.utxo_max_cache_bytes,
            config.sync.utxo_max_cache_bytes
        );
        assert_eq!(
            refreshed.sync.balance_max_cache_bytes,
            config.sync.balance_max_cache_bytes
        );

        config.sync.batch_size += 1;
        std::fs::write(&source, toml::to_string_pretty(&config).unwrap()).unwrap();
        let error = builder.prepare_workspace_config(Some(&source)).unwrap_err();
        assert!(error.contains("only cache limits"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_job_rejects_expected_hash_change() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let expected = "11".repeat(32);
        let job = SnapshotBuildJob::new(10, None, Some(expected.clone()));

        builder.validate_job(&job, 10, Some(&expected)).unwrap();
        let error = builder
            .validate_job(&job, 10, Some(&"22".repeat(32)))
            .unwrap_err();

        assert!(error.contains("was created with expected block hash"));
    }

    #[test]
    fn workspace_height_must_remain_between_base_and_target() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let job = SnapshotBuildJob::new(12, Some(completed(10)), None);

        let below = builder
            .validate_workspace_height(&job, 9, true)
            .unwrap_err();
        let above = builder
            .validate_workspace_height(&job, 13, true)
            .unwrap_err();

        assert!(below.contains("below completed base height 10"));
        assert!(above.contains("above active snapshot target 12"));
    }

    #[test]
    fn block_hash_normalization_accepts_uppercase_and_rejects_invalid_input() {
        let uppercase = "AB".repeat(32);
        assert_eq!(normalize_hash(&uppercase).unwrap(), "ab".repeat(32));

        let error = normalize_hash("not-a-block-hash").unwrap_err();
        assert!(error.contains("expected 64 hexadecimal characters"));
    }

    #[test]
    fn builder_state_rejects_version_and_network_mismatch() {
        let builder = ExactHeightSnapshotBuilder::new(PathBuf::from("/unused"));
        let mut state = SnapshotBuilderState::new("regtest".to_string());

        builder.validate_state(&state, "regtest").unwrap();
        state.version += 1;
        assert!(
            builder
                .validate_state(&state, "regtest")
                .unwrap_err()
                .contains("Unsupported snapshot builder state version")
        );

        state.version = crate::BUILDER_STATE_VERSION;
        assert!(
            builder
                .validate_state(&state, "bitcoin")
                .unwrap_err()
                .contains("Snapshot builder network mismatch")
        );
    }

    #[test]
    fn verification_progress_is_persisted_without_rebuilding_job_state() {
        let root = std::env::temp_dir().join(format!("snapshot-progress-{}", unique_run_id()));
        let builder = ExactHeightSnapshotBuilder::new(root.clone());
        builder.paths.create_dirs().unwrap();
        let job_file = builder.paths.job_file(10);
        let mut job = SnapshotBuildJob::new(10, None, None);
        job.set_stage(SnapshotBuildStage::Verifying);

        builder
            .persist_verification_progress(
                &mut job,
                &job_file,
                SnapshotVerificationPhase::IntegrityCheck,
                true,
            )
            .unwrap();
        let first = job.verification.clone().unwrap();
        builder
            .persist_verification_progress(
                &mut job,
                &job_file,
                SnapshotVerificationPhase::IntegrityCheck,
                false,
            )
            .unwrap();

        let persisted: SnapshotBuildJob = load_json(&job_file).unwrap().unwrap();
        let progress = persisted.verification.unwrap();
        assert_eq!(progress.phase, SnapshotVerificationPhase::IntegrityCheck);
        assert_eq!(progress.phase_started_at, first.phase_started_at);
        assert!(progress.heartbeat_at >= first.heartbeat_at);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_sqlite_sidecars_are_removed_but_nonempty_wal_is_rejected() {
        let root = std::env::temp_dir().join(format!("snapshot-sidecars-{}", unique_run_id()));
        std::fs::create_dir_all(&root).unwrap();
        let builder = ExactHeightSnapshotBuilder::new(root.clone());
        let marker = SnapshotCompleteMarker {
            version: crate::COMPLETE_MARKER_VERSION,
            height: 10,
            network: "regtest".to_string(),
            btc_block_hash: "11".repeat(32),
            snapshot_id: "snapshot-10".to_string(),
            snapshot_file: "snapshot_10.db".to_string(),
            manifest_file: "snapshot_10.manifest.json".to_string(),
            signature_file: None,
            file_sha256: "22".repeat(32),
            balance_history_count: 0,
            utxo_count: 0,
            block_commit_count: 1,
            script_registry_count: 0,
            completed_at: 1,
        };
        let wal_path = root.join("snapshot_10.db-wal");
        let shm_path = root.join("snapshot_10.db-shm");
        std::fs::write(&wal_path, []).unwrap();
        std::fs::write(&shm_path, [1u8]).unwrap();

        builder
            .remove_stale_sqlite_sidecars(&root, &marker)
            .unwrap();
        assert!(!wal_path.exists());
        assert!(!shm_path.exists());

        std::fs::write(&wal_path, [1u8]).unwrap();
        let error = builder
            .remove_stale_sqlite_sidecars(&root, &marker)
            .unwrap_err();
        assert!(error.contains("non-empty WAL sidecar"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
