use balance_history::HistoricalSnapshotStateRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Schema version for signed usdb-indexer checkpoint manifests.
pub const INDEXER_CHECKPOINT_MANIFEST_VERSION: &str = "usdb-indexer-checkpoint-manifest:v1";
/// Data-layout version for the files included in one indexer checkpoint.
pub const INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION: &str = "usdb-indexer-data:v1";
/// Signature algorithm required for public paired checkpoints.
pub const CHECKPOINT_SIGNATURE_SCHEME: &str = "ed25519";
/// File name used for the signed manifest inside one checkpoint artifact directory.
pub const CHECKPOINT_MANIFEST_FILE: &str = "usdb-indexer-checkpoint.manifest.json";
/// File name used for the detached manifest signature.
pub const CHECKPOINT_SIGNATURE_FILE: &str = "usdb-indexer-checkpoint.manifest.sig";
/// Version of the durable paired-install journal.
pub const PAIRED_INSTALL_JOURNAL_VERSION: u32 = 1;
/// Version of the post-restart recovery marker.
pub const RECOVERY_MARKER_VERSION: u32 = 1;

/// One file committed by the checkpoint manifest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointFileEntry {
    /// Slash-separated path relative to the checkpoint data directory.
    pub path: String,
    /// Exact file size in bytes.
    pub size: u64,
    /// Lowercase SHA-256 digest of the file contents.
    pub sha256: String,
}

/// Normalized consensus fields extracted from the full indexer state-ref.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexerCheckpointStateIdentity {
    /// Exact BTC height represented by the checkpoint.
    pub block_height: u32,
    /// Stable BTC block hash adopted from balance-history.
    pub stable_block_hash: String,
    /// Balance-history logical block commit at `block_height`.
    pub latest_block_commit: String,
    /// Upstream balance-history snapshot identity.
    pub snapshot_id: String,
    /// BTC activation registry revision used for local derivation.
    pub activation_registry_id: String,
    /// Active version-set identity selected at `block_height`.
    pub active_version_set_id: String,
    /// Recomputed local indexer state commitment.
    pub local_state_commit: String,
    /// Top-level system state identity consumed by USDB-chain.
    pub system_state_id: String,
}

/// Binding to the independently signed balance-history snapshot artifact.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BalanceHistorySnapshotBinding {
    /// Balance-history manifest basename.
    pub manifest_file_name: String,
    /// SHA-256 digest of the exact balance-history manifest bytes.
    pub manifest_sha256: String,
    /// Snapshot database basename declared by the balance-history manifest.
    pub snapshot_file_name: String,
    /// Snapshot database SHA-256 declared by the balance-history manifest.
    pub snapshot_file_sha256: String,
    /// Complete balance-history state-ref at the paired height.
    pub state_ref: HistoricalSnapshotStateRef,
    /// Earliest retained point-balance query height.
    pub balance_query_floor: u32,
    /// Earliest retained exact history/delta query height.
    pub history_query_floor: u32,
}

/// Signed manifest for one immutable usdb-indexer checkpoint directory.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IndexerCheckpointManifest {
    /// External manifest schema version.
    pub manifest_version: String,
    /// Checkpoint producer binary version for release provenance.
    pub tool_version: String,
    /// Indexer data layout understood by the restoring binary.
    pub data_schema_version: String,
    /// Immutable operation identity derived from both artifacts and state.
    pub operation_id: String,
    /// Network bundle to which this checkpoint was exported.
    pub network_bundle_id: String,
    /// USDB chain ID bound by the network bundle.
    pub chain_id: u64,
    /// Bitcoin network name from indexer configuration.
    pub btc_network: String,
    /// Per-network USDB BTC index origin.
    pub index_origin_height: u32,
    /// Exact BTC height represented by both artifacts.
    pub checkpoint_height: u32,
    /// Artifact directory basename containing `data/` and this manifest.
    pub artifact_dir_name: String,
    /// Sorted complete file inventory under `data/`.
    pub files: Vec<CheckpointFileEntry>,
    /// Independently signed upstream snapshot binding.
    pub balance_history: BalanceHistorySnapshotBinding,
    /// Full state-ref returned by usdb-indexer before the clean stop.
    pub indexer_state_ref: Value,
    /// Normalized identity used for offline and post-restart comparisons.
    pub state_identity: IndexerCheckpointStateIdentity,
    /// Required detached signature algorithm.
    pub signature_scheme: String,
    /// Signer selected from the trusted checkpoint key catalog.
    pub signing_key_id: String,
    /// Unix timestamp at which the immutable artifact was created.
    pub generated_at: u64,
}

impl IndexerCheckpointManifest {
    /// Returns the canonical JSON bytes covered by the detached signature.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self)
            .map_err(|error| format!("Failed to serialize checkpoint manifest: {error}"))
    }
}

/// Durable install phase shared by the two independently published data directories.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairedInstallStage {
    /// Inputs and signatures were validated; no live directory was published yet.
    Prepared,
    /// Both target data directories were empty or already matched this exact operation.
    TargetsValidated,
    /// Indexer data was atomically published and can be recognized after a crash.
    IndexerPublished,
    /// Balance-history installation started and any partial staging state is operation-owned.
    BalanceHistoryInstalling,
    /// Balance-history staging install was atomically published.
    BalanceHistoryPublished,
    /// Both offline stores match the paired manifest.
    Complete,
}

/// Crash-recovery journal for one paired install operation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairedInstallJournal {
    /// Journal format version.
    pub version: u32,
    /// Operation ID from the checkpoint manifest.
    pub operation_id: String,
    /// SHA-256 of the signed checkpoint manifest.
    pub checkpoint_manifest_sha256: String,
    /// Canonical indexer service root selected by the operator.
    pub indexer_root: PathBuf,
    /// Canonical balance-history service root selected by the operator.
    pub balance_history_root: PathBuf,
    /// Current durable install stage.
    pub stage: PairedInstallStage,
    /// Unix timestamp of the latest durable transition.
    pub updated_at: u64,
}

/// Durable proof written only after both restarted services recompute the expected state refs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PairedCheckpointRecoveryMarker {
    /// Marker format version.
    pub version: u32,
    /// Paired checkpoint operation ID.
    pub operation_id: String,
    /// Recomputed exact checkpoint height.
    pub checkpoint_height: u32,
    /// Recomputed upstream snapshot ID.
    pub snapshot_id: String,
    /// Recomputed local state commitment.
    pub local_state_commit: String,
    /// Recomputed top-level system state ID.
    pub system_state_id: String,
    /// Unix timestamp at which post-restart verification completed.
    pub verified_at: u64,
}
