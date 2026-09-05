use crate::snapshot_contract::SCRIPT_REGISTRY_POLICY;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// On-disk schema for the optional sidecar lifecycle and active pointer.
pub const SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION: &str =
    "balance-history-script-registry-activation:v1";

/// Verified immutable artifact selected by the sidecar installer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActiveScriptRegistryPointer {
    /// Artifact identity used as the immutable directory name.
    pub registry_artifact_id: String,
    /// Manifest basename inside the immutable artifact directory.
    pub manifest_file: String,
    /// Lowercase SHA-256 of the manifest bytes validated before activation.
    pub manifest_sha256: String,
}

/// Atomic installer-to-runtime contract stored at `auxiliary/script-registry/state.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptRegistryActivationState {
    /// Fixed state-file schema version.
    pub schema_version: String,
    /// Current optional-sidecar lifecycle state.
    pub state: ScriptRegistryState,
    /// Active immutable pointer, required only when state is ready.
    pub active: Option<ActiveScriptRegistryPointer>,
    /// Last installer or validation failure for failed/conflict states.
    pub last_error: Option<String>,
    /// Unix timestamp of the last atomic state transition.
    pub updated_at: u64,
}

impl ScriptRegistryActivationState {
    /// Validates the strict active-pointer and lifecycle invariants.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION {
            return Err(format!(
                "Unsupported script-registry activation state version {}",
                self.schema_version
            ));
        }
        if self.state == ScriptRegistryState::Ready && self.active.is_none() {
            return Err("Ready script-registry state requires an active pointer".to_string());
        }
        if !matches!(
            self.state,
            ScriptRegistryState::Ready
                | ScriptRegistryState::Failed
                | ScriptRegistryState::Conflict
        ) && self.active.is_some()
        {
            return Err(format!(
                "Script-registry state {:?} must not retain an active pointer",
                self.state
            ));
        }
        let failure_active = matches!(
            self.state,
            ScriptRegistryState::Failed | ScriptRegistryState::Conflict
        );
        if failure_active && self.last_error.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "Script-registry state {:?} requires last_error",
                self.state
            ));
        }
        if !failure_active && self.last_error.is_some() {
            return Err(format!(
                "Script-registry state {:?} must not retain last_error",
                self.state
            ));
        }
        if let Some(active) = &self.active {
            validate_lower_hex_32("active registry_artifact_id", &active.registry_artifact_id)?;
            validate_lower_hex_32("active manifest_sha256", &active.manifest_sha256)?;
            validate_safe_basename("active manifest_file", &active.manifest_file)?;
        }
        Ok(())
    }

    /// Loads one bounded, strict activation-state file.
    pub fn load(path: &Path) -> Result<Self, String> {
        const MAX_STATE_BYTES: u64 = 64 * 1024;
        let metadata = std::fs::metadata(path).map_err(|error| {
            format!(
                "Failed to inspect script-registry activation state {}: {error}",
                path.display()
            )
        })?;
        if metadata.len() > MAX_STATE_BYTES {
            return Err(format!(
                "Script-registry activation state {} exceeds {} bytes",
                path.display(),
                MAX_STATE_BYTES
            ));
        }
        let data = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "Failed to read script-registry activation state {}: {error}",
                path.display()
            )
        })?;
        let state: Self = usdb_util::parse_json_strict(&data).map_err(|error| {
            format!(
                "Failed to parse script-registry activation state {}: {error}",
                path.display()
            )
        })?;
        state.validate()?;
        Ok(state)
    }

    /// Durably replaces one activation-state file through an atomic rename.
    pub fn save_atomic(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let parent = path.parent().ok_or_else(|| {
            format!(
                "Script-registry activation state path has no parent: {}",
                path.display()
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create script-registry state directory {}: {error}",
                parent.display()
            )
        })?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "Script-registry activation state path is not UTF-8".to_string())?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("System clock is before Unix epoch: {error}"))?
            .as_nanos();
        let temp_path = parent.join(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));
        let result = (|| {
            let data = serde_json::to_vec_pretty(self).map_err(|error| {
                format!("Failed to serialize script-registry activation state: {error}")
            })?;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)
                .map_err(|error| {
                    format!(
                        "Failed to create script-registry temporary state {}: {error}",
                        temp_path.display()
                    )
                })?;
            file.write_all(&data)
                .and_then(|_| file.sync_all())
                .map_err(|error| {
                    format!(
                        "Failed to persist script-registry temporary state {}: {error}",
                        temp_path.display()
                    )
                })?;
            std::fs::rename(&temp_path, path).map_err(|error| {
                format!(
                    "Failed to activate script-registry state {}: {error}",
                    path.display()
                )
            })?;
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    format!(
                        "Failed to sync script-registry state directory {}: {error}",
                        parent.display()
                    )
                })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result
    }
}

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

/// Storage layer that produced one successful script-hash resolution.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScriptRegistrySource {
    /// Writable RocksDB mappings observed by this node.
    Overlay,
    /// Immutable historical SQLite sidecar.
    BaseSidecar,
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

fn validate_safe_basename(field: &str, value: &str) -> Result<(), String> {
    let path = std::path::Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || value == "."
        || value == ".."
    {
        return Err(format!("{field} must be a safe file basename"));
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

        assert_eq!(
            serde_json::to_value(ScriptRegistrySource::Overlay).unwrap(),
            serde_json::Value::String("overlay".to_string())
        );
        assert_eq!(
            serde_json::to_value(ScriptRegistrySource::BaseSidecar).unwrap(),
            serde_json::Value::String("base_sidecar".to_string())
        );
    }

    #[test]
    fn activation_state_rejects_unsafe_or_inconsistent_pointers() {
        let mut state = ScriptRegistryActivationState {
            schema_version: SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
            state: ScriptRegistryState::Ready,
            active: Some(ActiveScriptRegistryPointer {
                registry_artifact_id: "11".repeat(32),
                manifest_file: "script_registry_10.manifest.json".to_string(),
                manifest_sha256: "22".repeat(32),
            }),
            last_error: None,
            updated_at: 1,
        };
        state.validate().unwrap();

        state.active.as_mut().unwrap().manifest_file = "../manifest.json".to_string();
        assert!(state.validate().unwrap_err().contains("safe file basename"));

        state.active = None;
        assert!(
            state
                .validate()
                .unwrap_err()
                .contains("requires an active pointer")
        );
    }

    #[test]
    fn activation_state_atomic_round_trip_is_strict() {
        let root = std::env::temp_dir().join(format!(
            "script_registry_activation_state_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("state.json");
        let state = ScriptRegistryActivationState {
            schema_version: SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
            state: ScriptRegistryState::Disabled,
            active: None,
            last_error: None,
            updated_at: 7,
        };
        state.save_atomic(&path).unwrap();
        assert_eq!(ScriptRegistryActivationState::load(&path).unwrap(), state);

        std::fs::write(
            &path,
            format!(
                "{{\"schema_version\":\"{}\",\"state\":\"disabled\",\"active\":null,\"last_error\":null,\"updated_at\":7,\"legacy\":true}}",
                SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION
            ),
        )
        .unwrap();
        assert!(ScriptRegistryActivationState::load(&path).is_err());
        std::fs::remove_dir_all(root).unwrap();
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
