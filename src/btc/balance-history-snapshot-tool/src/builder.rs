use crate::{
    BuilderLock, BuilderPaths, CompletedSnapshotRef, SnapshotBuildJob, SnapshotBuildStage,
    SnapshotBuilderState, SnapshotCompleteMarker, build_complete_marker, load_json,
    save_json_atomic, unique_run_id, unix_timestamp, verify_published_artifact,
    verify_snapshot_files,
};
use balance_history::{
    BalanceHistoryConfig, BalanceHistoryIndexer, IndexOutput, SnapshotIndexer, SyncStatusManager,
    build_historical_state_ref_at_height,
};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use usdb_util::BTCRpcClient;

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

        job.set_stage(SnapshotBuildStage::Verifying);
        save_json_atomic(&job_file, &job)?;
        crate::abort_after_checkpoint("verifying");
        let verified = verify_snapshot_files(
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

        let marker = verify_published_artifact(
            &final_dir,
            &network,
            options.target_height,
            Some(&canonical_hash),
        )?;
        self.finalize_state_from_marker(&mut state, &mut job, &job_file, &final_dir, &marker)?;

        Ok(SnapshotCreateReport::from_marker(
            &self.paths.root,
            &final_dir,
            &marker,
            resumed,
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
                return Err(format!(
                    "Workspace config {} differs from requested config {}; use a separate builder root",
                    destination.display(),
                    source.display()
                ));
            }
            return Ok(());
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
}
