use crate::snapshot_contract::SCRIPT_REGISTRY_POLICY;
use serde::{Deserialize, Serialize};

/// Lifecycle state of the optional historical registry sidecar.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRegistryState {
    /// No sidecar is configured or installed.
    Absent,
    /// Sidecar acquisition was explicitly disabled.
    Disabled,
    /// Sidecar bytes are being downloaded.
    Downloading,
    /// Downloaded bytes are undergoing hash, signature, and SQLite checks.
    Verifying,
    /// The active sidecar is verified and queryable.
    Ready,
    /// Sidecar acquisition or validation failed.
    Failed,
    /// Integrity or overlap validation found conflicting mappings.
    Conflict,
}

/// Historical range covered by the active registry sources.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRegistryCoverageMode {
    /// RocksDB contains all mappings observed from Bitcoin height zero.
    FullReplay,
    /// SQLite covers the snapshot base and RocksDB covers later observations.
    SnapshotPlusSidecar,
    /// RocksDB only covers observations made after snapshot installation.
    PostSnapshotOnly,
}

/// Explicit lookup capabilities advertised independently of core readiness.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptRegistryCapabilities {
    /// At least one registry source can answer point lookups.
    pub script_registry_lookup: bool,
    /// A miss can be interpreted as a definitive not-found result.
    pub script_registry_complete_coverage: bool,
}

/// Final readiness and provenance contract for the auxiliary registry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptRegistryReadiness {
    /// Current optional-sidecar lifecycle state.
    pub state: ScriptRegistryState,
    /// Coverage represented by the active overlay and optional sidecar.
    pub coverage_mode: ScriptRegistryCoverageMode,
    /// Machine-readable lookup capabilities.
    pub capabilities: ScriptRegistryCapabilities,
    /// Approximate number of mappings in the writable RocksDB overlay.
    pub overlay_estimated_count: Option<u64>,
    /// Inclusive historical height expected from or covered by the sidecar.
    pub base_height: Option<u32>,
    /// Canonical BTC block hash paired with base_height.
    pub base_block_hash: Option<String>,
    /// Consensus snapshot identity expected from or paired with the sidecar.
    pub core_snapshot_id: Option<String>,
    /// File-specific identity of the active sidecar.
    pub registry_artifact_id: Option<String>,
    /// Exact manifest count expected in the active sidecar.
    pub expected_count: Option<u64>,
    /// Machine-readable append-like registry policy.
    pub policy: String,
    /// Last sidecar failure, omitted when no failure is active.
    pub last_error: Option<String>,
}

/// Per-item result state returned by layered script-hash resolution.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptHashResolutionStatus {
    /// The writable RocksDB overlay contained the mapping.
    FoundOverlay,
    /// The immutable SQLite base sidecar contained the mapping.
    FoundBase,
    /// Complete declared coverage did not contain the mapping.
    NotFound,
    /// Coverage is incomplete, so an overlay miss is not definitive.
    Unresolved,
    /// The stored value failed hash validation or is recorded as conflicting.
    Conflict,
}

impl ScriptHashResolutionStatus {
    /// Returns true when the result carries a valid scriptPubKey.
    pub fn is_found(self) -> bool {
        matches!(self, Self::FoundOverlay | Self::FoundBase)
    }

    /// Maps a layered lookup miss according to the advertised coverage.
    pub fn for_miss(complete_coverage: bool) -> Self {
        if complete_coverage {
            Self::NotFound
        } else {
            Self::Unresolved
        }
    }
}

impl ScriptRegistryReadiness {
    /// Validates cross-field capability and provenance invariants.
    pub fn validate(&self) -> Result<(), String> {
        if self.policy != SCRIPT_REGISTRY_POLICY {
            return Err(format!(
                "Unsupported script registry policy {}; expected {}",
                self.policy, SCRIPT_REGISTRY_POLICY
            ));
        }
        let coverage_is_complete = !matches!(
            self.coverage_mode,
            ScriptRegistryCoverageMode::PostSnapshotOnly
        );
        if self.capabilities.script_registry_complete_coverage != coverage_is_complete {
            return Err(format!(
                "script_registry_complete_coverage={} conflicts with coverage_mode={:?}",
                self.capabilities.script_registry_complete_coverage, self.coverage_mode
            ));
        }
        if coverage_is_complete && !self.capabilities.script_registry_lookup {
            return Err("Complete registry coverage requires lookup capability".to_string());
        }
        match self.coverage_mode {
            ScriptRegistryCoverageMode::FullReplay => {
                if !matches!(
                    self.state,
                    ScriptRegistryState::Absent | ScriptRegistryState::Disabled
                ) {
                    return Err(format!(
                        "full_replay coverage does not require a sidecar, got state={:?}",
                        self.state
                    ));
                }
                if self.base_height.is_some()
                    || self.base_block_hash.is_some()
                    || self.core_snapshot_id.is_some()
                    || self.registry_artifact_id.is_some()
                    || self.expected_count.is_some()
                {
                    return Err(
                        "full_replay coverage must not advertise sidecar provenance".to_string()
                    );
                }
            }
            ScriptRegistryCoverageMode::SnapshotPlusSidecar => {
                if self.state != ScriptRegistryState::Ready {
                    return Err(format!(
                        "snapshot_plus_sidecar coverage requires state=ready, got {:?}",
                        self.state
                    ));
                }
                if self.base_height.is_none()
                    || self.base_block_hash.is_none()
                    || self.core_snapshot_id.is_none()
                    || self.registry_artifact_id.is_none()
                    || self.expected_count.is_none()
                {
                    return Err(
                        "snapshot_plus_sidecar coverage requires complete sidecar provenance"
                            .to_string(),
                    );
                }
            }
            ScriptRegistryCoverageMode::PostSnapshotOnly => {
                if self.state == ScriptRegistryState::Ready {
                    return Err(
                        "post_snapshot_only coverage cannot advertise state=ready".to_string()
                    );
                }
            }
        }

        for (field, value) in [
            ("base_block_hash", self.base_block_hash.as_deref()),
            ("core_snapshot_id", self.core_snapshot_id.as_deref()),
            ("registry_artifact_id", self.registry_artifact_id.as_deref()),
        ] {
            if let Some(value) = value {
                validate_lower_hex_32(field, value)?;
            }
        }
        if self.base_height == Some(0) {
            return Err("Registry base_height 0 is unsupported".to_string());
        }

        let failure_active = matches!(
            self.state,
            ScriptRegistryState::Failed | ScriptRegistryState::Conflict
        );
        if failure_active && self.last_error.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "Registry state {:?} requires last_error",
                self.state
            ));
        }
        if !failure_active && self.last_error.is_some() {
            return Err(format!(
                "Registry state {:?} must not retain last_error",
                self.state
            ));
        }
        Ok(())
    }
}

fn validate_lower_hex_32(field: &str, value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must be a 64-character lowercase hexadecimal value"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_enums_have_stable_json_names() {
        let states = [
            (ScriptRegistryState::Absent, "absent"),
            (ScriptRegistryState::Disabled, "disabled"),
            (ScriptRegistryState::Downloading, "downloading"),
            (ScriptRegistryState::Verifying, "verifying"),
            (ScriptRegistryState::Ready, "ready"),
            (ScriptRegistryState::Failed, "failed"),
            (ScriptRegistryState::Conflict, "conflict"),
        ];
        for (value, expected) in states {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::Value::String(expected.to_string())
            );
        }

        let coverage_modes = [
            (ScriptRegistryCoverageMode::FullReplay, "full_replay"),
            (
                ScriptRegistryCoverageMode::SnapshotPlusSidecar,
                "snapshot_plus_sidecar",
            ),
            (
                ScriptRegistryCoverageMode::PostSnapshotOnly,
                "post_snapshot_only",
            ),
        ];
        for (value, expected) in coverage_modes {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::Value::String(expected.to_string())
            );
        }

        let resolution_states = [
            (ScriptHashResolutionStatus::FoundOverlay, "found_overlay"),
            (ScriptHashResolutionStatus::FoundBase, "found_base"),
            (ScriptHashResolutionStatus::NotFound, "not_found"),
            (ScriptHashResolutionStatus::Unresolved, "unresolved"),
            (ScriptHashResolutionStatus::Conflict, "conflict"),
        ];
        for (value, expected) in resolution_states {
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::Value::String(expected.to_string())
            );
        }
    }

    #[test]
    fn only_found_states_report_values() {
        assert!(ScriptHashResolutionStatus::FoundOverlay.is_found());
        assert!(ScriptHashResolutionStatus::FoundBase.is_found());
        assert!(!ScriptHashResolutionStatus::NotFound.is_found());
        assert!(!ScriptHashResolutionStatus::Unresolved.is_found());
        assert!(!ScriptHashResolutionStatus::Conflict.is_found());
        assert_eq!(
            ScriptHashResolutionStatus::for_miss(true),
            ScriptHashResolutionStatus::NotFound
        );
        assert_eq!(
            ScriptHashResolutionStatus::for_miss(false),
            ScriptHashResolutionStatus::Unresolved
        );
    }

    #[test]
    fn readiness_validates_coverage_and_provenance() {
        let full_replay = ScriptRegistryReadiness {
            state: ScriptRegistryState::Disabled,
            coverage_mode: ScriptRegistryCoverageMode::FullReplay,
            capabilities: ScriptRegistryCapabilities {
                script_registry_lookup: true,
                script_registry_complete_coverage: true,
            },
            overlay_estimated_count: Some(2),
            base_height: None,
            base_block_hash: None,
            core_snapshot_id: None,
            registry_artifact_id: None,
            expected_count: None,
            policy: "auxiliary_seen_scripts_non_consensus_v1".to_string(),
            last_error: None,
        };
        full_replay.validate().unwrap();

        let mut invalid = full_replay.clone();
        invalid.capabilities.script_registry_complete_coverage = false;
        assert!(invalid.validate().unwrap_err().contains("conflicts"));

        let mut invalid = full_replay.clone();
        invalid.capabilities.script_registry_lookup = false;
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("lookup capability")
        );

        let mut invalid = full_replay.clone();
        invalid.state = ScriptRegistryState::Ready;
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("does not require a sidecar")
        );

        let mut invalid = full_replay;
        invalid.base_height = Some(100);
        assert!(
            invalid
                .validate()
                .unwrap_err()
                .contains("must not advertise sidecar")
        );

        let post_snapshot_only = ScriptRegistryReadiness {
            state: ScriptRegistryState::Absent,
            coverage_mode: ScriptRegistryCoverageMode::PostSnapshotOnly,
            capabilities: ScriptRegistryCapabilities {
                script_registry_lookup: true,
                script_registry_complete_coverage: false,
            },
            overlay_estimated_count: Some(2),
            base_height: Some(963_800),
            base_block_hash: Some("22".repeat(32)),
            core_snapshot_id: Some("33".repeat(32)),
            registry_artifact_id: None,
            expected_count: Some(1_541_365_559),
            policy: "auxiliary_seen_scripts_non_consensus_v1".to_string(),
            last_error: None,
        };
        post_snapshot_only.validate().unwrap();
    }

    #[test]
    fn readiness_rejects_unknown_fields() {
        let value = serde_json::json!({
            "state": "ready",
            "coverage_mode": "full_replay",
            "capabilities": {
                "script_registry_lookup": true,
                "script_registry_complete_coverage": true
            },
            "overlay_estimated_count": 2,
            "base_height": null,
            "base_block_hash": null,
            "core_snapshot_id": null,
            "registry_artifact_id": null,
            "expected_count": null,
            "policy": "auxiliary_seen_scripts_non_consensus_v1",
            "last_error": null,
            "legacy_available": true
        });
        assert!(serde_json::from_value::<ScriptRegistryReadiness>(value).is_err());
    }
}
