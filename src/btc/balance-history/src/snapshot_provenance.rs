use crate::config::SnapshotTrustMode;
use crate::snapshot_contract::{
    CORE_SNAPSHOT_MANIFEST_VERSION, CORE_SNAPSHOT_SCHEMA_VERSION,
    SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519, SnapshotArtifactType,
};
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
    /// Core install matched its manifest, artifact bytes, SQLite contents, and staged state-ref.
    ManifestVerified,
    /// Core install additionally matched a trusted detached signature.
    SignatureVerified,
}

/// Structured provenance recorded for a DB populated via snapshot install.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotInstallProvenance {
    /// Provenance origin of the currently persisted DB.
    pub origin: SnapshotInstallOrigin,
    /// Verification mode requested by the local installer configuration.
    pub trust_mode: SnapshotTrustMode,
    /// Effective verification state recorded for the installed snapshot.
    pub verification_state: SnapshotVerificationState,
    /// Whether a trusted detached signature was verified during installation.
    pub signature_verified: bool,
    /// Fixed kind of artifact used to populate this DB.
    pub artifact_type: SnapshotArtifactType,
    /// Manifest schema version accepted by the installer.
    pub manifest_version: String,
    /// SQLite schema version accepted by the installer.
    pub snapshot_schema_version: String,
    /// Detached signature scheme, when present in the manifest.
    pub signature_scheme: Option<String>,
    /// Signer identifier recorded in the manifest, if any.
    pub signing_key_id: Option<String>,
    /// SHA-256 of the installed core SQLite file.
    pub snapshot_file_sha256: String,
    /// Consensus state identity restored from the core artifact.
    pub core_snapshot_id: String,
    /// File-specific identity of the installed core artifact.
    pub core_artifact_id: String,
    /// Installed BTC block height of the snapshot DB.
    pub installed_block_height: u32,
    /// Earliest height for which at-or-before point balance queries are complete.
    pub balance_query_floor: u32,
    /// Earliest height for which exact deltas and history ranges are complete.
    pub history_query_floor: u32,
}

impl SnapshotInstallProvenance {
    /// Validates the durable core-install identity before storage or readiness use.
    pub fn validate(&self) -> Result<(), String> {
        if self.origin != SnapshotInstallOrigin::SnapshotInstall {
            return Err("Invalid snapshot install provenance origin".to_string());
        }
        if self.artifact_type != SnapshotArtifactType::BalanceHistoryCore {
            return Err("Snapshot install provenance must identify a core artifact".to_string());
        }
        if self.manifest_version != CORE_SNAPSHOT_MANIFEST_VERSION {
            return Err(format!(
                "Unsupported installed core manifest version {}",
                self.manifest_version
            ));
        }
        if self.snapshot_schema_version != CORE_SNAPSHOT_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported installed core snapshot schema {}",
                self.snapshot_schema_version
            ));
        }
        for (field, value) in [
            ("snapshot_file_sha256", &self.snapshot_file_sha256),
            ("core_snapshot_id", &self.core_snapshot_id),
            ("core_artifact_id", &self.core_artifact_id),
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(format!(
                    "Snapshot install provenance {field} must be 64 lowercase hex characters"
                ));
            }
        }
        if self.balance_query_floor != self.installed_block_height
            || self.history_query_floor != self.installed_block_height.saturating_add(1)
        {
            return Err(
                "Snapshot install provenance retention floors are inconsistent".to_string(),
            );
        }
        if matches!(self.trust_mode, SnapshotTrustMode::Signed)
            && (!self.signature_verified
                || self.verification_state != SnapshotVerificationState::SignatureVerified)
        {
            return Err(
                "Signed snapshot trust mode requires signature-verified provenance".to_string(),
            );
        }
        if self.signature_verified
            != (self.verification_state == SnapshotVerificationState::SignatureVerified)
        {
            return Err(
                "Snapshot signature flag and verification state are inconsistent".to_string(),
            );
        }
        match (&self.signature_scheme, &self.signing_key_id) {
            (None, None) if !self.signature_verified => {}
            (Some(scheme), Some(key_id))
                if scheme == SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519 && !key_id.is_empty() => {}
            (Some(scheme), Some(_)) => {
                return Err(format!(
                    "Unsupported snapshot provenance signature scheme {scheme}"
                ));
            }
            _ => {
                return Err(
                    "Snapshot provenance signature scheme and signing key ID must be present together"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    /// Returns true when the installed snapshot is safe for downstream consensus use.
    pub fn is_consensus_verified(&self) -> bool {
        self.validate().is_ok()
            && matches!(
                self.verification_state,
                SnapshotVerificationState::ManifestVerified
                    | SnapshotVerificationState::SignatureVerified
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_verified_provenance() -> SnapshotInstallProvenance {
        SnapshotInstallProvenance {
            origin: SnapshotInstallOrigin::SnapshotInstall,
            trust_mode: SnapshotTrustMode::Manifest,
            verification_state: SnapshotVerificationState::ManifestVerified,
            signature_verified: false,
            artifact_type: SnapshotArtifactType::BalanceHistoryCore,
            manifest_version: CORE_SNAPSHOT_MANIFEST_VERSION.to_string(),
            snapshot_schema_version: CORE_SNAPSHOT_SCHEMA_VERSION.to_string(),
            signature_scheme: None,
            signing_key_id: None,
            snapshot_file_sha256: "11".repeat(32),
            core_snapshot_id: "22".repeat(32),
            core_artifact_id: "33".repeat(32),
            installed_block_height: 7,
            balance_query_floor: 7,
            history_query_floor: 8,
        }
    }

    #[test]
    fn accepts_consistent_manifest_and_signature_states() {
        let manifest = manifest_verified_provenance();
        assert!(manifest.validate().is_ok());
        assert!(manifest.is_consensus_verified());

        let mut signed = manifest;
        signed.trust_mode = SnapshotTrustMode::Signed;
        signed.verification_state = SnapshotVerificationState::SignatureVerified;
        signed.signature_verified = true;
        signed.signature_scheme = Some(SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519.to_string());
        signed.signing_key_id = Some("release-key".to_string());
        assert!(signed.validate().is_ok());
        assert!(signed.is_consensus_verified());
    }

    #[test]
    fn rejects_inconsistent_signature_state_and_identity() {
        let mut provenance = manifest_verified_provenance();
        provenance.verification_state = SnapshotVerificationState::SignatureVerified;
        assert!(provenance.validate().is_err());

        let mut provenance = manifest_verified_provenance();
        provenance.signature_scheme = Some("unsupported".to_string());
        provenance.signing_key_id = Some("release-key".to_string());
        assert!(provenance.validate().is_err());

        let mut provenance = manifest_verified_provenance();
        provenance.signature_scheme = Some(SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519.to_string());
        assert!(provenance.validate().is_err());
    }

    #[test]
    fn rejects_invalid_core_identity_and_retention_floors() {
        let mut provenance = manifest_verified_provenance();
        provenance.core_snapshot_id = "AA".repeat(32);
        assert!(provenance.validate().is_err());

        let mut provenance = manifest_verified_provenance();
        provenance.history_query_floor = provenance.installed_block_height;
        assert!(provenance.validate().is_err());
    }
}
