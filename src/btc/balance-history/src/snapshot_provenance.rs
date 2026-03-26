use crate::config::SnapshotTrustMode;
use serde::{Deserialize, Serialize};

/// Origin of the current durable balance-history DB.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotInstallOrigin {
    /// The DB was populated by snapshot install instead of full live sync.
    SnapshotInstall,
}

/// Verification status of a snapshot-installed DB.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotVerificationState {
    /// Snapshot install completed without any manifest-backed provenance check.
    ManifestMissing,
    /// Snapshot install matched a manifest and the staged state-ref before swap.
    ManifestVerified,
    /// Snapshot install matched a manifest and a trusted detached signature.
    SignatureVerified,
}

/// Structured provenance recorded for a DB populated via snapshot install.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotInstallProvenance {
    /// Provenance origin of the currently persisted DB.
    pub origin: SnapshotInstallOrigin,
    /// Verification mode requested by the local installer configuration.
    pub trust_mode: SnapshotTrustMode,
    /// Effective verification state recorded for the installed snapshot.
    pub verification_state: SnapshotVerificationState,
    /// Whether a sidecar manifest was present during installation.
    pub manifest_present: bool,
    /// Whether the staged DB matched the manifest-backed state reference.
    pub manifest_verified: bool,
    /// Whether a detached signature file was present during installation.
    pub signature_present: bool,
    /// Whether a trusted detached signature was verified during installation.
    pub signature_verified: bool,
    /// Manifest schema version, when a manifest was present.
    pub manifest_version: Option<String>,
    /// Detached signature scheme, when present in the manifest.
    pub signature_scheme: Option<String>,
    /// Signer identifier recorded in the manifest, if any.
    pub signing_key_id: Option<String>,
    /// Snapshot DB file hash from the manifest, if any.
    pub snapshot_file_sha256: Option<String>,
    /// Expected installed snapshot id from the manifest, if any.
    pub snapshot_id: Option<String>,
    /// Installed BTC block height of the snapshot DB.
    pub installed_block_height: u32,
}

impl SnapshotInstallProvenance {
    /// Returns true when the installed snapshot is safe for downstream consensus use.
    pub fn is_consensus_verified(&self) -> bool {
        matches!(
            self.verification_state,
            SnapshotVerificationState::ManifestVerified
                | SnapshotVerificationState::SignatureVerified
        )
    }

    /// Returns the legacy manifest-verified boolean expected by older readiness logic.
    pub fn legacy_manifest_verified(&self) -> bool {
        self.manifest_verified
    }
}
