use super::{
    ActiveScriptRegistryPointer, SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION,
    ScriptRegistryActivationState, ScriptRegistryState,
};
use crate::config::BalanceHistoryConfig;
use crate::db::{ScriptRegistrySnapshotDb, SnapshotHash};
use crate::index::verify_snapshot_artifact_manifest_signature;
use crate::snapshot_contract::{CoreSnapshotManifest, ScriptRegistryManifest};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CORE_INSTALL_MARKER_VERSION: &str = "balance-history-core-install-marker:v1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreInstallMarker {
    schema_version: String,
    snapshot_mode: String,
    snapshot_file: String,
    snapshot_manifest: String,
    snapshot_manifest_sha256: String,
    installed_at: String,
}

/// Result of validating and atomically activating one immutable registry sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptRegistryActivationReport {
    /// True when the selected artifact was already active.
    pub already_active: bool,
    /// File-specific registry artifact identity.
    pub registry_artifact_id: String,
    /// Consensus snapshot identity shared with the installed core artifact.
    pub core_snapshot_id: String,
    /// Inclusive BTC height represented by the sidecar.
    pub base_height: u32,
    /// Exact verified mapping count.
    pub entry_count: u64,
    /// Active manifest digest committed by `state.json`.
    pub manifest_sha256: String,
}

/// Fully verifies an installed sidecar and publishes its active pointer.
///
/// The artifact directory must already be the immutable
/// `auxiliary/script-registry/bases/<registry_artifact_id>` destination. This
/// operation never opens or mutates core RocksDB, so it can run beside the live
/// balance-history service after the core install marker has been published.
pub fn activate_script_registry_sidecar(
    config: &BalanceHistoryConfig,
    manifest_path: &Path,
    core_manifest_path: &Path,
    trusted_keys_path: &Path,
) -> Result<ScriptRegistryActivationReport, String> {
    let state_path = config.script_registry_sidecar_dir().join("state.json");
    let previous_ready = ScriptRegistryActivationState::load(&state_path)
        .ok()
        .filter(|state| state.state == ScriptRegistryState::Ready);
    let result = activate_script_registry_sidecar_inner(
        config,
        manifest_path,
        core_manifest_path,
        trusted_keys_path,
        &state_path,
    );
    if let Err(error) = &result {
        log::error!(
            "Script-registry sidecar activation failed: manifest={}, core_manifest={}, error={}",
            manifest_path.display(),
            core_manifest_path.display(),
            error
        );
        if previous_ready.is_none() {
            let failed = ScriptRegistryActivationState {
                schema_version: SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
                state: ScriptRegistryState::Failed,
                active: None,
                last_error: Some(error.clone()),
                updated_at: unix_timestamp(),
            };
            if let Err(state_error) = failed.save_atomic(&state_path) {
                log::error!(
                    "Failed to persist script-registry activation failure: state={}, error={}",
                    state_path.display(),
                    state_error
                );
            }
        } else {
            log::warn!(
                "Preserving the previously active script-registry sidecar after replacement failure"
            );
        }
    }
    result
}

fn activate_script_registry_sidecar_inner(
    config: &BalanceHistoryConfig,
    manifest_path: &Path,
    core_manifest_path: &Path,
    trusted_keys_path: &Path,
    state_path: &Path,
) -> Result<ScriptRegistryActivationReport, String> {
    let manifest = ScriptRegistryManifest::load(manifest_path)?;
    let core_manifest = CoreSnapshotManifest::load(core_manifest_path)?;
    manifest.validate_against_core(&core_manifest)?;
    validate_core_install_marker(config, core_manifest_path, &core_manifest)?;

    let manifest_sha256 = SnapshotHash::calc_hash(manifest_path)?;
    let pointer = ActiveScriptRegistryPointer {
        registry_artifact_id: manifest.registry_artifact_id.clone(),
        manifest_file: file_name(manifest_path)?,
        manifest_sha256: manifest_sha256.clone(),
    };
    let expected_dir = config
        .script_registry_sidecar_dir()
        .join("bases")
        .join(&manifest.registry_artifact_id);
    require_same_directory(&expected_dir, manifest_path)?;

    if let Ok(state) = ScriptRegistryActivationState::load(state_path)
        && state.state == ScriptRegistryState::Ready
        && state.active.as_ref() == Some(&pointer)
    {
        log::info!(
            "Script-registry sidecar is already active: artifact_id={}, base_height={}",
            manifest.registry_artifact_id,
            manifest.base.base_height
        );
        return Ok(report(&manifest, manifest_sha256, true));
    }

    let has_active = ScriptRegistryActivationState::load(state_path)
        .ok()
        .is_some_and(|state| state.state == ScriptRegistryState::Ready);
    if !has_active {
        ScriptRegistryActivationState {
            schema_version: SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
            state: ScriptRegistryState::Verifying,
            active: None,
            last_error: None,
            updated_at: unix_timestamp(),
        }
        .save_atomic(state_path)?;
    }

    let signing_key_id = verify_snapshot_artifact_manifest_signature(
        manifest.signature_scheme.as_deref(),
        manifest.signing_key_id.as_deref(),
        manifest_path,
        &manifest.signature_payload()?,
        trusted_keys_path,
    )?;
    let db_path = manifest_path
        .parent()
        .ok_or_else(|| "Script-registry manifest has no parent directory".to_string())?
        .join(&manifest.file_name);
    let actual_file_sha256 = SnapshotHash::calc_hash(&db_path)?;
    if actual_file_sha256 != manifest.file_sha256 {
        return Err(format!(
            "Script-registry file hash mismatch: expected {}, got {}",
            manifest.file_sha256, actual_file_sha256
        ));
    }
    let db = ScriptRegistrySnapshotDb::open_for_verification(
        &db_path,
        config.script_registry.cache_size_kib,
    )?;
    db.verify_integrity()?;
    db.verify_schema()?;
    let meta = db.read_meta()?;
    if meta.base != manifest.base
        || meta.entry_count != manifest.entry_count
        || meta.generated_at != manifest.generated_at
    {
        return Err("Script-registry SQLite metadata does not match its manifest".to_string());
    }
    let actual_count = db.entry_count()?;
    if actual_count != manifest.entry_count {
        return Err(format!(
            "Script-registry entry count mismatch: expected {}, got {}",
            manifest.entry_count, actual_count
        ));
    }

    ScriptRegistryActivationState {
        schema_version: SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
        state: ScriptRegistryState::Ready,
        active: Some(pointer),
        last_error: None,
        updated_at: unix_timestamp(),
    }
    .save_atomic(state_path)?;
    log::info!(
        "Activated verified script-registry sidecar: artifact_id={}, base_height={}, entry_count={}, signing_key_id={}",
        manifest.registry_artifact_id,
        manifest.base.base_height,
        manifest.entry_count,
        signing_key_id
    );
    Ok(report(&manifest, manifest_sha256, false))
}

fn validate_core_install_marker(
    config: &BalanceHistoryConfig,
    core_manifest_path: &Path,
    core_manifest: &CoreSnapshotManifest,
) -> Result<(), String> {
    let marker_path = config
        .root_dir
        .join("bootstrap")
        .join("snapshot-loader.done.json");
    let marker_text = std::fs::read_to_string(&marker_path).map_err(|error| {
        format!(
            "Failed to read installed core marker {}: {error}",
            marker_path.display()
        )
    })?;
    let marker: CoreInstallMarker = usdb_util::parse_json_strict(&marker_text)
        .map_err(|error| format!("Failed to parse installed core marker: {error}"))?;
    if marker.schema_version != CORE_INSTALL_MARKER_VERSION {
        return Err(format!(
            "Unsupported core install marker schema {}",
            marker.schema_version
        ));
    }
    if marker.snapshot_mode != "balance-history" && marker.snapshot_mode != "paired-checkpoint" {
        return Err(format!(
            "Core install marker has unsupported snapshot mode {}",
            marker.snapshot_mode
        ));
    }
    let canonical_expected = core_manifest_path.canonicalize().map_err(|error| {
        format!(
            "Failed to canonicalize core manifest {}: {error}",
            core_manifest_path.display()
        )
    })?;
    let canonical_marker = PathBuf::from(&marker.snapshot_manifest)
        .canonicalize()
        .map_err(|error| {
            format!(
                "Failed to canonicalize core marker manifest {}: {error}",
                marker.snapshot_manifest
            )
        })?;
    if canonical_marker != canonical_expected {
        return Err(
            "Registry activation core manifest differs from the installed marker".to_string(),
        );
    }
    let manifest_sha256 = SnapshotHash::calc_hash(core_manifest_path)?;
    if marker.snapshot_manifest_sha256 != manifest_sha256 {
        return Err("Installed core marker manifest digest mismatch".to_string());
    }
    if marker.installed_at.trim().is_empty() {
        return Err("Installed core marker has no installation timestamp".to_string());
    }
    let snapshot_file = PathBuf::from(&marker.snapshot_file);
    let canonical_snapshot = snapshot_file.canonicalize().map_err(|error| {
        format!(
            "Failed to canonicalize installed core snapshot {}: {error}",
            snapshot_file.display()
        )
    })?;
    let expected_snapshot = core_manifest_path
        .parent()
        .ok_or_else(|| "Core manifest has no parent directory".to_string())?
        .join(&core_manifest.file_name)
        .canonicalize()
        .map_err(|error| {
            format!(
                "Failed to canonicalize core manifest snapshot {}: {error}",
                core_manifest.file_name
            )
        })?;
    if canonical_snapshot != expected_snapshot {
        return Err("Installed core marker file does not match the core manifest".to_string());
    }
    Ok(())
}

fn require_same_directory(expected: &Path, file: &Path) -> Result<(), String> {
    let expected = expected.canonicalize().map_err(|error| {
        format!(
            "Failed to canonicalize expected registry directory {}: {error}",
            expected.display()
        )
    })?;
    let actual = file
        .parent()
        .ok_or_else(|| "Script-registry manifest has no parent directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Failed to canonicalize registry artifact directory: {error}"))?;
    if actual != expected {
        return Err(format!(
            "Script-registry artifact directory mismatch: expected {}, got {}",
            expected.display(),
            actual.display()
        ));
    }
    Ok(())
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Path has no UTF-8 basename: {}", path.display()))
}

fn report(
    manifest: &ScriptRegistryManifest,
    manifest_sha256: String,
    already_active: bool,
) -> ScriptRegistryActivationReport {
    ScriptRegistryActivationReport {
        already_active,
        registry_artifact_id: manifest.registry_artifact_id.clone(),
        core_snapshot_id: manifest.base.core_snapshot_id.clone(),
        base_height: manifest.base.base_height,
        entry_count: manifest.entry_count,
        manifest_sha256,
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{BalanceHistoryDBIdentity, ScriptRegistryEntry, ScriptRegistrySnapshotMeta};
    use crate::index::{SnapshotTrustedKeySet, SnapshotTrustedPublicKey};
    use crate::service::{COMMIT_HASH_ALGO, COMMIT_PROTOCOL_VERSION, HistoricalSnapshotStateRef};
    use crate::snapshot_contract::ScriptRegistryBaseIdentity;
    use base64::Engine;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use ed25519_dalek::{Signer, SigningKey};
    use std::sync::atomic::{AtomicU64, Ordering};
    use usdb_util::{
        CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, CONSENSUS_SOURCE_CHAIN_BTC,
        ConsensusSnapshotIdentity, ToBtcScriptHash, build_consensus_snapshot_id,
    };

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestArtifact {
        config: BalanceHistoryConfig,
        manifest_path: PathBuf,
        core_manifest_path: PathBuf,
        trusted_keys_path: PathBuf,
    }

    impl Drop for TestArtifact {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.config.root_dir);
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "balance-history-registry-installer-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_signed_artifact(name: &str) -> TestArtifact {
        let root = temp_root(name);
        let config = BalanceHistoryConfig {
            root_dir: root.clone(),
            ..BalanceHistoryConfig::default()
        };
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let key_id = "registry-test-key";
        let trusted_keys_path = root.join("trusted-keys.json");
        std::fs::write(
            &trusted_keys_path,
            serde_json::to_vec_pretty(&SnapshotTrustedKeySet {
                keys: vec![SnapshotTrustedPublicKey {
                    key_id: key_id.to_string(),
                    public_key_base64: base64::engine::general_purpose::STANDARD
                        .encode(signing_key.verifying_key().to_bytes()),
                }],
            })
            .unwrap(),
        )
        .unwrap();

        let consensus_identity = ConsensusSnapshotIdentity {
            source_chain: CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
            network: "regtest".to_string(),
            stable_height: 42,
            stable_block_hash: "22".repeat(32),
            stable_lag: 5,
            balance_history_api_version: "1.0.0".to_string(),
            balance_history_semantics_version: "balance-snapshot-at-or-before:v1".to_string(),
        };
        let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
        let state_ref = HistoricalSnapshotStateRef {
            block_height: 42,
            stable_block_hash: consensus_identity.stable_block_hash.clone(),
            latest_block_commit: "33".repeat(32),
            consensus_identity,
            snapshot_id: snapshot_id.clone(),
            snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
            snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
        };
        let db_identity = BalanceHistoryDBIdentity {
            identity_version: "balance-history-db-identity:v1".to_string(),
            service: "balance-history".to_string(),
            schema_version: "balance-history-rocksdb:v1".to_string(),
            data_model_version: "balance-history-data-model:v1".to_string(),
            btc_network: "regtest".to_string(),
            btc_genesis_hash: "44".repeat(32),
        };
        let core_manifest = CoreSnapshotManifest::build(
            "balance_history_core_42.db".to_string(),
            "55".repeat(32),
            state_ref,
            db_identity,
            Some(key_id.to_string()),
            100,
        )
        .unwrap();
        let core_dir = root.join("core");
        std::fs::create_dir_all(&core_dir).unwrap();
        std::fs::write(core_dir.join(&core_manifest.file_name), b"core").unwrap();
        let core_manifest_path = core_dir.join("balance_history_core_42.manifest.json");
        core_manifest.save(&core_manifest_path).unwrap();
        std::fs::create_dir_all(root.join("bootstrap")).unwrap();
        std::fs::write(
            root.join("bootstrap/snapshot-loader.done.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": CORE_INSTALL_MARKER_VERSION,
                "snapshot_mode": "balance-history",
                "snapshot_file": core_dir.join(&core_manifest.file_name),
                "snapshot_manifest": core_manifest_path,
                "snapshot_manifest_sha256": SnapshotHash::calc_hash(&core_manifest_path).unwrap(),
                "installed_at": "2026-09-05T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();

        let base = ScriptRegistryBaseIdentity {
            btc_network: "regtest".to_string(),
            btc_genesis_hash: "44".repeat(32),
            base_height: 42,
            base_block_hash: "22".repeat(32),
            core_snapshot_id: snapshot_id,
        };
        let work = root.join("registry-work.db");
        let script = ScriptBuf::from_bytes(vec![0x51]);
        let mut db = ScriptRegistrySnapshotDb::create(&work).unwrap();
        db.put_entries(&[ScriptRegistryEntry {
            script_hash: script.to_btc_script_hash(),
            script_pubkey: script,
        }])
        .unwrap();
        db.write_meta(&ScriptRegistrySnapshotMeta {
            base: base.clone(),
            entry_count: 1,
            generated_at: 101,
        })
        .unwrap();
        db.finalize_for_distribution().unwrap();
        let manifest = ScriptRegistryManifest::build(
            "script_registry_42.db".to_string(),
            SnapshotHash::calc_hash(&work).unwrap(),
            base,
            1,
            Some(key_id.to_string()),
            101,
        )
        .unwrap();
        let artifact_dir = config
            .script_registry_sidecar_dir()
            .join("bases")
            .join(&manifest.registry_artifact_id);
        std::fs::create_dir_all(&artifact_dir).unwrap();
        std::fs::rename(&work, artifact_dir.join(&manifest.file_name)).unwrap();
        let manifest_path = artifact_dir.join("script_registry_42.manifest.json");
        manifest.save(&manifest_path).unwrap();
        let signature = signing_key.sign(&manifest.signature_payload().unwrap());
        std::fs::write(
            manifest_path.with_extension("sig"),
            base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
        )
        .unwrap();

        TestArtifact {
            config,
            manifest_path,
            core_manifest_path,
            trusted_keys_path,
        }
    }

    #[test]
    fn activation_verifies_and_publishes_an_idempotent_pointer() {
        let artifact = write_signed_artifact("ready");
        let first = activate_script_registry_sidecar(
            &artifact.config,
            &artifact.manifest_path,
            &artifact.core_manifest_path,
            &artifact.trusted_keys_path,
        )
        .unwrap();
        assert!(!first.already_active);
        assert_eq!(first.entry_count, 1);

        let state = ScriptRegistryActivationState::load(
            &artifact
                .config
                .script_registry_sidecar_dir()
                .join("state.json"),
        )
        .unwrap();
        assert_eq!(state.state, ScriptRegistryState::Ready);
        assert_eq!(
            state.active.unwrap().registry_artifact_id,
            first.registry_artifact_id
        );

        let replay = activate_script_registry_sidecar(
            &artifact.config,
            &artifact.manifest_path,
            &artifact.core_manifest_path,
            &artifact.trusted_keys_path,
        )
        .unwrap();
        assert!(replay.already_active);
    }

    #[test]
    fn activation_failure_preserves_no_unverified_pointer() {
        let artifact = write_signed_artifact("bad-signature");
        std::fs::write(artifact.manifest_path.with_extension("sig"), "invalid").unwrap();
        let error = activate_script_registry_sidecar(
            &artifact.config,
            &artifact.manifest_path,
            &artifact.core_manifest_path,
            &artifact.trusted_keys_path,
        )
        .unwrap_err();
        assert!(error.contains("signature"));

        let state = ScriptRegistryActivationState::load(
            &artifact
                .config
                .script_registry_sidecar_dir()
                .join("state.json"),
        )
        .unwrap();
        assert_eq!(state.state, ScriptRegistryState::Failed);
        assert!(state.active.is_none());
        assert!(state.last_error.unwrap().contains("signature"));
    }

    #[test]
    fn replacement_failure_preserves_the_active_pointer() {
        let artifact = write_signed_artifact("replacement-failure");
        activate_script_registry_sidecar(
            &artifact.config,
            &artifact.manifest_path,
            &artifact.core_manifest_path,
            &artifact.trusted_keys_path,
        )
        .unwrap();
        let state_path = artifact
            .config
            .script_registry_sidecar_dir()
            .join("state.json");
        let active = ScriptRegistryActivationState::load(&state_path).unwrap();

        let mut replacement = ScriptRegistryManifest::load(&artifact.manifest_path).unwrap();
        replacement.generated_at += 1;
        replacement.save(&artifact.manifest_path).unwrap();
        let error = activate_script_registry_sidecar(
            &artifact.config,
            &artifact.manifest_path,
            &artifact.core_manifest_path,
            &artifact.trusted_keys_path,
        )
        .unwrap_err();
        assert!(error.contains("signature"));
        assert_eq!(
            ScriptRegistryActivationState::load(&state_path).unwrap(),
            active
        );
    }
}
