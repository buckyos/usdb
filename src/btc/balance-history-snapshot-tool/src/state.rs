use named_lock::{NamedLock, NamedLockGuard};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const BUILDER_STATE_VERSION: u32 = 1;
pub(crate) const JOB_STATE_VERSION: u32 = 1;
pub(crate) const COMPLETE_MARKER_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub(crate) struct BuilderPaths {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub jobs: PathBuf,
    pub snapshots: PathBuf,
    pub temp: PathBuf,
    pub state_file: PathBuf,
}

impl BuilderPaths {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            workspace: root.join("workspace"),
            jobs: root.join("jobs"),
            snapshots: root.join("snapshots"),
            temp: root.join("tmp"),
            state_file: root.join("builder-state.json"),
            root,
        }
    }

    pub(crate) fn create_dirs(&self) -> Result<(), String> {
        for path in [
            &self.root,
            &self.workspace,
            &self.jobs,
            &self.snapshots,
            &self.temp,
        ] {
            std::fs::create_dir_all(path).map_err(|e| {
                format!(
                    "Failed to create snapshot builder directory {}: {}",
                    path.display(),
                    e
                )
            })?;
        }
        Ok(())
    }

    pub(crate) fn job_dir(&self, height: u32) -> PathBuf {
        self.jobs.join(format!("{:012}", height))
    }

    pub(crate) fn job_file(&self, height: u32) -> PathBuf {
        self.job_dir(height).join("job.json")
    }

    pub(crate) fn snapshot_height_dir(&self, height: u32) -> PathBuf {
        self.snapshots.join(format!("{:012}", height))
    }

    pub(crate) fn snapshot_artifact_dir(&self, height: u32, block_hash: &str) -> PathBuf {
        self.snapshot_height_dir(height).join(block_hash)
    }
}

/// Durable phase of one exact-height snapshot build.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotBuildStage {
    /// The job exists but has not opened the mutable workspace.
    Prepare,
    /// The workspace is synchronizing or reconciling toward the target.
    Syncing,
    /// The target BTC identity and state reference have been sealed.
    Sealed,
    /// Snapshot files are being exported into a temporary directory.
    Building,
    /// The generated files and metadata are being verified.
    Verifying,
    /// The immutable artifact has been published and committed to builder state.
    Complete,
}

/// Identity and managed path of a completed immutable snapshot artifact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletedSnapshotRef {
    /// Exact BTC height represented by the artifact.
    pub height: u32,
    /// Canonical BTC block hash at `height` when the artifact was published.
    pub btc_block_hash: String,
    /// Consensus snapshot identity derived by balance-history.
    pub snapshot_id: String,
    /// Artifact directory relative to the builder root.
    pub artifact_dir: String,
}

/// Root-level state coordinating the single mutable workspace.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotBuilderState {
    /// Persistent builder state format version.
    pub version: u32,
    /// BTC network bound to this builder root.
    pub network: String,
    /// Latest completed checkpoint that may be used as an incremental base.
    pub latest_completed: Option<CompletedSnapshotRef>,
    /// Currently active target height, if a job needs completion or recovery.
    pub active_job_height: Option<u32>,
    /// Last state update as Unix seconds.
    pub updated_at: u64,
}

impl SnapshotBuilderState {
    pub(crate) fn new(network: String) -> Self {
        Self {
            version: BUILDER_STATE_VERSION,
            network,
            latest_completed: None,
            active_job_height: None,
            updated_at: unix_timestamp(),
        }
    }
}

/// Recoverable state for one target height.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotBuildJob {
    /// Persistent job format version.
    pub version: u32,
    /// Requested exact BTC height.
    pub target_height: u32,
    /// Completed checkpoint from which this job incrementally starts.
    pub base_checkpoint: Option<CompletedSnapshotRef>,
    /// Current durable phase of the build.
    pub stage: SnapshotBuildStage,
    /// Optional operator-pinned canonical BTC block hash.
    pub expected_block_hash: Option<String>,
    /// BTC block hash sealed from the synchronized workspace.
    pub btc_block_hash: Option<String>,
    /// Consensus snapshot identity sealed at the target.
    pub snapshot_id: Option<String>,
    /// Interrupted temporary directory relative to the builder root.
    pub temp_dir: Option<String>,
    /// Published artifact directory relative to the builder root.
    pub artifact_dir: Option<String>,
    /// Number of snapshot export attempts for this job.
    pub attempt: u32,
    /// Job creation time as Unix seconds.
    pub started_at: u64,
    /// Last job update as Unix seconds.
    pub updated_at: u64,
    /// Publication time as Unix seconds, once complete.
    pub completed_at: Option<u64>,
}

impl SnapshotBuildJob {
    pub(crate) fn new(
        target_height: u32,
        base_checkpoint: Option<CompletedSnapshotRef>,
        expected_block_hash: Option<String>,
    ) -> Self {
        let now = unix_timestamp();
        Self {
            version: JOB_STATE_VERSION,
            target_height,
            base_checkpoint,
            stage: SnapshotBuildStage::Prepare,
            expected_block_hash,
            btc_block_hash: None,
            snapshot_id: None,
            temp_dir: None,
            artifact_dir: None,
            attempt: 0,
            started_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    pub(crate) fn set_stage(&mut self, stage: SnapshotBuildStage) {
        self.stage = stage;
        self.updated_at = unix_timestamp();
    }
}

/// Final commit marker stored beside one verified immutable artifact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotCompleteMarker {
    /// Completion marker format version.
    pub version: u32,
    /// Exact BTC height represented by the artifact.
    pub height: u32,
    /// BTC network bound to the artifact.
    pub network: String,
    /// Canonical BTC block hash represented by the artifact.
    pub btc_block_hash: String,
    /// Consensus snapshot identity from the manifest.
    pub snapshot_id: String,
    /// Snapshot DB file name relative to the artifact directory.
    pub snapshot_file: String,
    /// Manifest file name relative to the artifact directory.
    pub manifest_file: String,
    /// Optional detached signature file name relative to the artifact directory.
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
    /// Completion time as Unix seconds.
    pub completed_at: u64,
}

pub(crate) struct BuilderLock {
    _lock: NamedLock,
    _guard: NamedLockGuard,
}

impl BuilderLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| {
            format!(
                "Failed to create snapshot builder root {} before locking: {}",
                root.display(),
                e
            )
        })?;
        let canonical_root = root.canonicalize().map_err(|e| {
            format!(
                "Failed to canonicalize snapshot builder root {}: {}",
                root.display(),
                e
            )
        })?;
        let digest = Sha256::digest(canonical_root.to_string_lossy().as_bytes());
        let lock_name = format!("balance_history_snapshot_{:x}", digest);
        let lock = NamedLock::create(&lock_name)
            .map_err(|e| format!("Failed to create snapshot builder lock: {}", e))?;
        let guard = lock.lock().map_err(|e| {
            format!(
                "Another snapshot builder is already using {}: {}",
                canonical_root.display(),
                e
            )
        })?;
        Ok(Self {
            _lock: lock,
            _guard: guard,
        })
    }
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn unique_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{}", std::process::id(), nanos)
}

pub(crate) fn load_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read(path)
        .map_err(|e| format!("Failed to read JSON state {}: {}", path.display(), e))?;
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(|e| format!("Failed to parse JSON state {}: {}", path.display(), e))
}

pub(crate) fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("JSON state path {} has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| {
        format!(
            "Failed to create JSON state directory {}: {}",
            parent.display(),
            e
        )
    })?;

    let temp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("state"),
        unique_run_id()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|e| {
            format!(
                "Failed to create temporary JSON state {}: {}",
                temp_path.display(),
                e
            )
        })?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value).map_err(|e| {
        format!(
            "Failed to serialize JSON state {}: {}",
            temp_path.display(),
            e
        )
    })?;
    writer
        .write_all(b"\n")
        .map_err(|e| format!("Failed to finish JSON state {}: {}", temp_path.display(), e))?;
    writer
        .flush()
        .map_err(|e| format!("Failed to flush JSON state {}: {}", temp_path.display(), e))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("Failed to sync JSON state {}: {}", temp_path.display(), e))?;
    drop(writer);

    std::fs::rename(&temp_path, path).map_err(|e| {
        format!(
            "Failed to atomically publish JSON state {} as {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })?;
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| {
            format!(
                "Failed to sync JSON state directory {}: {}",
                parent.display(),
                e
            )
        })?;
    Ok(())
}
