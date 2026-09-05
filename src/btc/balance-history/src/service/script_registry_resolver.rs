use super::{
    ScriptHashResolutionStatus, ScriptRegistryActivationState, ScriptRegistryCapabilities,
    ScriptRegistryCoverageMode, ScriptRegistryReadiness, ScriptRegistrySource, ScriptRegistryState,
};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDBRef, ScriptRegistrySnapshotDb};
use crate::snapshot_contract::{SCRIPT_REGISTRY_POLICY, ScriptRegistryManifest};
use bitcoincore_rpc::bitcoin::ScriptBuf;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use usdb_util::{BtcScriptHash, ToBtcScriptHash};

const SCRIPT_REGISTRY_STATE_FILE: &str = "state.json";
const SCRIPT_REGISTRY_BASES_DIR: &str = "bases";

/// Internal layered result before display-specific address conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedScriptRegistryEntry {
    pub status: ScriptHashResolutionStatus,
    pub source: Option<ScriptRegistrySource>,
    pub script_pubkey: Option<ScriptBuf>,
}

struct ActiveSidecar {
    manifest: ScriptRegistryManifest,
    db: ScriptRegistrySnapshotDb,
}

struct ResolverCache {
    loaded: bool,
    state_digest: Option<String>,
    state: ScriptRegistryState,
    active: Option<ActiveSidecar>,
    last_manifest: Option<ScriptRegistryManifest>,
    last_error: Option<String>,
}

impl Default for ResolverCache {
    fn default() -> Self {
        Self {
            loaded: false,
            state_digest: None,
            state: ScriptRegistryState::Absent,
            active: None,
            last_manifest: None,
            last_error: None,
        }
    }
}

/// Combines the writable RocksDB registry overlay with an optional immutable SQLite base.
pub(crate) struct ScriptRegistryResolver {
    config: BalanceHistoryConfigRef,
    db: BalanceHistoryDBRef,
    cache: Mutex<ResolverCache>,
}

impl ScriptRegistryResolver {
    /// Creates a lazy resolver. Optional sidecar failures never fail service startup.
    pub fn new(config: BalanceHistoryConfigRef, db: BalanceHistoryDBRef) -> Self {
        Self {
            config,
            db,
            cache: Mutex::new(ResolverCache::default()),
        }
    }

    /// Returns current layered coverage without affecting core readiness.
    pub fn readiness(&self) -> ScriptRegistryReadiness {
        self.refresh();
        self.readiness_from_cache()
    }

    /// Resolves all hashes through overlay first and sidecar only for overlay misses.
    pub fn resolve(
        &self,
        script_hashes: &[BtcScriptHash],
    ) -> Result<(Vec<ResolvedScriptRegistryEntry>, ScriptRegistryReadiness), String> {
        self.refresh();
        let overlay_started = Instant::now();
        let overlay = self.db.get_script_registry_entries(script_hashes)?;
        let overlay_elapsed = overlay_started.elapsed();
        if overlay_elapsed.as_millis() >= u128::from(self.config.script_registry.slow_query_ms) {
            log::warn!(
                "Slow script-registry overlay lookup: item_count={}, elapsed_ms={}",
                script_hashes.len(),
                overlay_elapsed.as_millis()
            );
        }

        let mut resolved = Vec::with_capacity(script_hashes.len());
        let mut misses = Vec::new();
        let mut miss_positions = Vec::new();
        for (position, (script_hash, value)) in
            script_hashes.iter().zip(overlay.into_iter()).enumerate()
        {
            match value {
                Some(script_pubkey) if script_pubkey.to_btc_script_hash() == *script_hash => {
                    resolved.push(ResolvedScriptRegistryEntry {
                        status: ScriptHashResolutionStatus::FoundOverlay,
                        source: Some(ScriptRegistrySource::Overlay),
                        script_pubkey: Some(script_pubkey),
                    });
                }
                Some(script_pubkey) => {
                    log::error!(
                        "Script-registry overlay hash mismatch: requested={}, calculated={}",
                        script_hash,
                        script_pubkey.to_btc_script_hash()
                    );
                    resolved.push(ResolvedScriptRegistryEntry {
                        status: ScriptHashResolutionStatus::Conflict,
                        source: None,
                        script_pubkey: None,
                    });
                }
                None => {
                    miss_positions.push(position);
                    misses.push(*script_hash);
                    resolved.push(ResolvedScriptRegistryEntry {
                        status: ScriptHashResolutionStatus::Unresolved,
                        source: None,
                        script_pubkey: None,
                    });
                }
            }
        }

        if !misses.is_empty() {
            self.resolve_base_misses(&misses, &miss_positions, &mut resolved);
        }
        let readiness = self.readiness_from_cache();
        let miss_status = ScriptHashResolutionStatus::for_miss(
            readiness.capabilities.script_registry_complete_coverage,
        );
        for item in &mut resolved {
            if item.status == ScriptHashResolutionStatus::Unresolved {
                item.status = miss_status;
            }
        }
        Ok((resolved, readiness))
    }

    fn resolve_base_misses(
        &self,
        misses: &[BtcScriptHash],
        positions: &[usize],
        resolved: &mut [ResolvedScriptRegistryEntry],
    ) {
        let mut cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(error) => {
                log::error!("Script-registry resolver cache lock is poisoned: {error}");
                return;
            }
        };
        if cache.state != ScriptRegistryState::Ready {
            return;
        }
        let Some(active) = cache.active.as_ref() else {
            return;
        };

        let started = Instant::now();
        let values = active
            .db
            .get_entries(misses, self.config.script_registry.query_batch_size);
        let elapsed = started.elapsed();
        if elapsed.as_millis() >= u128::from(self.config.script_registry.slow_query_ms) {
            log::warn!(
                "Slow script-registry sidecar lookup: artifact_id={}, item_count={}, batch_size={}, elapsed_ms={}",
                active.manifest.registry_artifact_id,
                misses.len(),
                self.config.script_registry.query_batch_size,
                elapsed.as_millis()
            );
        }

        let values = match values {
            Ok(values) => values,
            Err(error) => {
                let message = format!(
                    "Active script-registry sidecar query failed for artifact {}: {error}",
                    active.manifest.registry_artifact_id
                );
                log::error!("{message}");
                cache.state = ScriptRegistryState::Failed;
                cache.last_error = Some(message);
                cache.active = None;
                return;
            }
        };

        let mut conflict = None;
        for ((script_hash, position), value) in misses.iter().zip(positions).zip(values) {
            let Some(script_pubkey) = value else {
                continue;
            };
            let calculated = script_pubkey.to_btc_script_hash();
            if calculated != *script_hash {
                let message = format!(
                    "Script-registry sidecar hash mismatch: requested={script_hash}, calculated={calculated}"
                );
                log::error!("{message}");
                resolved[*position] = ResolvedScriptRegistryEntry {
                    status: ScriptHashResolutionStatus::Conflict,
                    source: None,
                    script_pubkey: None,
                };
                conflict = Some(message);
                continue;
            }
            resolved[*position] = ResolvedScriptRegistryEntry {
                status: ScriptHashResolutionStatus::FoundBase,
                source: Some(ScriptRegistrySource::BaseSidecar),
                script_pubkey: Some(script_pubkey),
            };
        }
        if let Some(error) = conflict {
            cache.state = ScriptRegistryState::Conflict;
            cache.last_error = Some(error);
            cache.active = None;
        }
    }

    fn refresh(&self) {
        let state_path = self
            .config
            .script_registry_sidecar_dir()
            .join(SCRIPT_REGISTRY_STATE_FILE);
        let state_bytes = match std::fs::metadata(&state_path) {
            Ok(metadata) if metadata.len() > 64 * 1024 => {
                self.record_refresh_failure(
                    Some(format!("oversized:{}", metadata.len())),
                    format!(
                        "Script-registry activation state {} exceeds 65536 bytes",
                        state_path.display()
                    ),
                );
                return;
            }
            Ok(_) => match std::fs::read(&state_path) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    self.record_refresh_failure(
                        None,
                        format!(
                            "Failed to read script-registry activation state {}: {error}",
                            state_path.display()
                        ),
                    );
                    return;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                self.record_refresh_failure(
                    None,
                    format!(
                        "Failed to read script-registry activation state {}: {error}",
                        state_path.display()
                    ),
                );
                return;
            }
        };
        let digest = Some(match state_bytes.as_deref() {
            Some(bytes) => format!("sha256:{:x}", Sha256::digest(bytes)),
            None => "absent".to_string(),
        });
        if self
            .cache
            .lock()
            .map(|cache| cache.loaded && cache.state_digest == digest)
            .unwrap_or(false)
        {
            return;
        }

        let snapshot_install = match self.db.get_snapshot_install_provenance() {
            Ok(value) => value,
            Err(error) => {
                self.record_refresh_failure(
                    digest,
                    format!("Failed to read core snapshot provenance for registry: {error}"),
                );
                return;
            }
        };
        if snapshot_install.is_none() {
            self.replace_cache(digest, ScriptRegistryState::Disabled, None, None, None);
            return;
        }

        let Some(state_bytes) = state_bytes else {
            self.replace_cache(digest, ScriptRegistryState::Absent, None, None, None);
            return;
        };
        let state = match std::str::from_utf8(&state_bytes)
            .map_err(|error| format!("Activation state is not UTF-8: {error}"))
            .and_then(|text| {
                usdb_util::parse_json_strict::<ScriptRegistryActivationState>(text)
                    .map_err(|error| format!("Failed to parse activation state: {error}"))
            })
            .and_then(|state| {
                state.validate()?;
                Ok(state)
            }) {
            Ok(state) => state,
            Err(error) => {
                self.record_refresh_failure(digest, error);
                return;
            }
        };
        if state.state != ScriptRegistryState::Ready {
            self.replace_cache(digest, state.state, None, None, state.last_error);
            return;
        }
        let active_pointer = state
            .active
            .expect("validated ready state has active pointer");
        match self.open_active_sidecar(
            &active_pointer.registry_artifact_id,
            &active_pointer.manifest_file,
            &active_pointer.manifest_sha256,
        ) {
            Ok(active) => {
                log::info!(
                    "Activated immutable script-registry sidecar: artifact_id={}, base_height={}, entry_count={}, cache_size_kib={}",
                    active.manifest.registry_artifact_id,
                    active.manifest.base.base_height,
                    active.manifest.entry_count,
                    self.config.script_registry.cache_size_kib
                );
                let manifest = active.manifest.clone();
                self.replace_cache(
                    digest,
                    ScriptRegistryState::Ready,
                    Some(active),
                    Some(manifest),
                    None,
                );
            }
            Err(error) => self.record_refresh_failure(digest, error),
        }
    }

    fn open_active_sidecar(
        &self,
        artifact_id: &str,
        manifest_file: &str,
        manifest_sha256: &str,
    ) -> Result<ActiveSidecar, String> {
        let registry_root = self.config.script_registry_sidecar_dir();
        let bases_root = registry_root.join(SCRIPT_REGISTRY_BASES_DIR);
        let artifact_dir = bases_root.join(artifact_id);
        let manifest_path = artifact_dir.join(manifest_file);
        require_descendant(&bases_root, &manifest_path, "script-registry manifest")?;
        let manifest_size = std::fs::metadata(&manifest_path)
            .map_err(|error| {
                format!(
                    "Failed to inspect active script-registry manifest {}: {error}",
                    manifest_path.display()
                )
            })?
            .len();
        if manifest_size > 1024 * 1024 {
            return Err(format!(
                "Active script-registry manifest {} exceeds 1048576 bytes",
                manifest_path.display()
            ));
        }
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| {
            format!(
                "Failed to read active script-registry manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        let actual_manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
        if actual_manifest_sha256 != manifest_sha256 {
            return Err(format!(
                "Active script-registry manifest hash mismatch: expected {manifest_sha256}, got {actual_manifest_sha256}"
            ));
        }
        let manifest: ScriptRegistryManifest = std::str::from_utf8(&manifest_bytes)
            .map_err(|error| format!("Script-registry manifest is not UTF-8: {error}"))
            .and_then(|text| {
                usdb_util::parse_json_strict(text)
                    .map_err(|error| format!("Failed to parse script-registry manifest: {error}"))
            })?;
        manifest.validate()?;
        if manifest.registry_artifact_id != artifact_id {
            return Err(format!(
                "Active script-registry directory ID {} does not match manifest {}",
                artifact_id, manifest.registry_artifact_id
            ));
        }
        self.validate_manifest_against_live_core(&manifest)?;

        let db_path = artifact_dir.join(&manifest.file_name);
        require_descendant(&artifact_dir, &db_path, "script-registry database")?;
        let db = ScriptRegistrySnapshotDb::open_for_lookup(
            &db_path,
            self.config.script_registry.cache_size_kib,
        )?;
        db.verify_schema()?;
        let meta = db.read_meta()?;
        if meta.base != manifest.base
            || meta.entry_count != manifest.entry_count
            || meta.generated_at != manifest.generated_at
        {
            return Err(
                "Active script-registry SQLite metadata does not match its manifest".to_string(),
            );
        }
        Ok(ActiveSidecar { manifest, db })
    }

    fn validate_manifest_against_live_core(
        &self,
        manifest: &ScriptRegistryManifest,
    ) -> Result<(), String> {
        let provenance = self
            .db
            .get_snapshot_install_provenance()?
            .ok_or_else(|| "A full-replay DB must not activate a registry sidecar".to_string())?;
        provenance.validate()?;
        let db_identity = self
            .db
            .get_db_identity()?
            .ok_or_else(|| "Balance-history DB identity is missing".to_string())?;
        if manifest.base.core_snapshot_id != provenance.core_snapshot_id
            || manifest.base.base_height != provenance.installed_block_height
            || manifest.base.btc_network != db_identity.btc_network
            || manifest.base.btc_genesis_hash != db_identity.btc_genesis_hash
        {
            return Err(
                "Script-registry manifest does not match the installed core snapshot identity"
                    .to_string(),
            );
        }
        let commit = self
            .db
            .get_block_commit(manifest.base.base_height)?
            .ok_or_else(|| {
                format!(
                    "Installed core DB has no block commit at registry base height {}",
                    manifest.base.base_height
                )
            })?;
        if commit.btc_block_hash.to_string() != manifest.base.base_block_hash {
            return Err(
                "Script-registry base block hash does not match installed core DB".to_string(),
            );
        }
        Ok(())
    }

    fn readiness_from_cache(&self) -> ScriptRegistryReadiness {
        let overlay_estimated_count = match self.db.get_estimated_script_registry_count() {
            Ok(value) => Some(value),
            Err(error) => {
                log::warn!("Failed to estimate script-registry overlay rows: {error}");
                None
            }
        };
        let snapshot_provenance = self.db.get_snapshot_install_provenance().ok().flatten();
        let cache = match self.cache.lock() {
            Ok(cache) => cache,
            Err(error) => {
                log::error!("Script-registry resolver cache lock is poisoned: {error}");
                if snapshot_provenance.is_none() {
                    return ScriptRegistryReadiness {
                        state: ScriptRegistryState::Disabled,
                        coverage_mode: ScriptRegistryCoverageMode::FullReplay,
                        capabilities: ScriptRegistryCapabilities {
                            script_registry_lookup: true,
                            script_registry_complete_coverage: true,
                        },
                        overlay_estimated_count,
                        base_height: None,
                        base_block_hash: None,
                        core_snapshot_id: None,
                        registry_artifact_id: None,
                        expected_count: None,
                        policy: SCRIPT_REGISTRY_POLICY.to_string(),
                        last_error: None,
                    };
                }
                return ScriptRegistryReadiness {
                    state: ScriptRegistryState::Failed,
                    coverage_mode: ScriptRegistryCoverageMode::PostSnapshotOnly,
                    capabilities: ScriptRegistryCapabilities {
                        script_registry_lookup: true,
                        script_registry_complete_coverage: false,
                    },
                    overlay_estimated_count,
                    base_height: snapshot_provenance
                        .as_ref()
                        .map(|value| value.installed_block_height),
                    base_block_hash: None,
                    core_snapshot_id: snapshot_provenance
                        .as_ref()
                        .map(|value| value.core_snapshot_id.clone()),
                    registry_artifact_id: None,
                    expected_count: None,
                    policy: SCRIPT_REGISTRY_POLICY.to_string(),
                    last_error: Some(format!("Registry resolver cache lock is poisoned: {error}")),
                };
            }
        };
        if snapshot_provenance.is_none() {
            return ScriptRegistryReadiness {
                state: ScriptRegistryState::Disabled,
                coverage_mode: ScriptRegistryCoverageMode::FullReplay,
                capabilities: ScriptRegistryCapabilities {
                    script_registry_lookup: true,
                    script_registry_complete_coverage: true,
                },
                overlay_estimated_count,
                base_height: None,
                base_block_hash: None,
                core_snapshot_id: None,
                registry_artifact_id: None,
                expected_count: None,
                policy: SCRIPT_REGISTRY_POLICY.to_string(),
                last_error: None,
            };
        }

        let provenance = snapshot_provenance.expect("checked above");
        let manifest = cache.last_manifest.as_ref();
        let ready = cache.state == ScriptRegistryState::Ready && cache.active.is_some();
        ScriptRegistryReadiness {
            state: cache.state,
            coverage_mode: if ready {
                ScriptRegistryCoverageMode::SnapshotPlusSidecar
            } else {
                ScriptRegistryCoverageMode::PostSnapshotOnly
            },
            capabilities: ScriptRegistryCapabilities {
                script_registry_lookup: true,
                script_registry_complete_coverage: ready,
            },
            overlay_estimated_count,
            base_height: Some(provenance.installed_block_height),
            base_block_hash: manifest
                .map(|value| value.base.base_block_hash.clone())
                .or_else(|| {
                    self.db
                        .get_block_commit(provenance.installed_block_height)
                        .ok()
                        .flatten()
                        .map(|commit| commit.btc_block_hash.to_string())
                }),
            core_snapshot_id: Some(provenance.core_snapshot_id),
            registry_artifact_id: manifest.map(|value| value.registry_artifact_id.clone()),
            expected_count: manifest.map(|value| value.entry_count),
            policy: SCRIPT_REGISTRY_POLICY.to_string(),
            last_error: cache.last_error.clone(),
        }
    }

    fn record_refresh_failure(&self, digest: Option<String>, error: String) {
        let should_log = self
            .cache
            .lock()
            .map(|cache| {
                !cache.loaded
                    || cache.state_digest != digest
                    || cache.last_error.as_deref() != Some(&error)
            })
            .unwrap_or(true);
        if should_log {
            log::error!("Script-registry sidecar activation failed: {error}");
        }
        self.replace_cache(digest, ScriptRegistryState::Failed, None, None, Some(error));
    }

    fn replace_cache(
        &self,
        digest: Option<String>,
        state: ScriptRegistryState,
        active: Option<ActiveSidecar>,
        last_manifest: Option<ScriptRegistryManifest>,
        last_error: Option<String>,
    ) {
        match self.cache.lock() {
            Ok(mut cache) => {
                *cache = ResolverCache {
                    loaded: true,
                    state_digest: digest,
                    state,
                    active,
                    last_manifest,
                    last_error,
                };
            }
            Err(error) => {
                log::error!("Failed to update poisoned script-registry cache: {error}");
            }
        }
    }
}

fn require_descendant(root: &Path, path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical_root = root.canonicalize().map_err(|error| {
        format!(
            "Failed to canonicalize script-registry root {}: {error}",
            root.display()
        )
    })?;
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("Failed to canonicalize {label} {}: {error}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(format!(
            "Resolved {label} path escapes script-registry root: {}",
            canonical_path.display()
        ));
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BalanceHistoryConfig, SnapshotTrustMode};
    use crate::db::{
        BalanceHistoryDB, BalanceHistoryDBMode, BlockCommitEntry, ScriptRegistryEntry,
        ScriptRegistrySnapshotMeta, SnapshotHash,
    };
    use crate::snapshot_contract::{ScriptRegistryBaseIdentity, ScriptRegistryManifest};
    use crate::snapshot_provenance::{
        SnapshotInstallOrigin, SnapshotInstallProvenance, SnapshotVerificationState,
    };
    use bitcoincore_rpc::bitcoin::{BlockHash, ScriptBuf};
    use rusqlite::Connection;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestResolver {
        root: PathBuf,
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        resolver: ScriptRegistryResolver,
    }

    impl Drop for TestResolver {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn test_resolver(tag: &str, snapshot: bool) -> TestResolver {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("registry_resolver_{tag}_{nanos}"));
        std::fs::create_dir_all(&root).unwrap();
        let mut raw_config = BalanceHistoryConfig {
            root_dir: root.clone(),
            ..BalanceHistoryConfig::default()
        };
        raw_config.script_registry.query_batch_size = 1;
        let config = Arc::new(raw_config);
        let db =
            Arc::new(BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap());
        if snapshot {
            let commit = BlockCommitEntry {
                block_height: 10,
                btc_block_hash: BlockHash::from_byte_array([7; 32]),
                balance_delta_root: [8; 32],
                block_commit: [9; 32],
            };
            db.put_block_commits_async(std::slice::from_ref(&commit))
                .unwrap();
            db.put_snapshot_install_provenance(&SnapshotInstallProvenance {
                origin: SnapshotInstallOrigin::SnapshotInstall,
                trust_mode: SnapshotTrustMode::Manifest,
                verification_state: SnapshotVerificationState::ManifestVerified,
                signature_verified: false,
                artifact_type: crate::snapshot_contract::SnapshotArtifactType::BalanceHistoryCore,
                manifest_version: crate::snapshot_contract::CORE_SNAPSHOT_MANIFEST_VERSION
                    .to_string(),
                snapshot_schema_version: crate::snapshot_contract::CORE_SNAPSHOT_SCHEMA_VERSION
                    .to_string(),
                signature_scheme: None,
                signing_key_id: None,
                snapshot_file_sha256: "11".repeat(32),
                core_snapshot_id: "22".repeat(32),
                core_artifact_id: "33".repeat(32),
                installed_block_height: 10,
                balance_query_floor: 10,
                history_query_floor: 11,
            })
            .unwrap();
            db.put_query_retention_floors(10, 11).unwrap();
        }
        let resolver = ScriptRegistryResolver::new(config.clone(), db.clone());
        TestResolver {
            root,
            config,
            db,
            resolver,
        }
    }

    fn script(seed: u8) -> ScriptBuf {
        ScriptBuf::from(vec![0x51, seed])
    }

    fn activate_sidecar(
        test: &TestResolver,
        entries: &[ScriptRegistryEntry],
        corrupt_value: Option<(&BtcScriptHash, ScriptBuf)>,
    ) -> ScriptRegistryManifest {
        let identity = test.db.get_db_identity().unwrap().unwrap();
        let commit = test.db.get_block_commit(10).unwrap().unwrap();
        let base = ScriptRegistryBaseIdentity {
            btc_network: identity.btc_network,
            btc_genesis_hash: identity.btc_genesis_hash,
            base_height: 10,
            base_block_hash: commit.btc_block_hash.to_string(),
            core_snapshot_id: "22".repeat(32),
        };
        let work = test.root.join("registry-work.db");
        let mut writer = ScriptRegistrySnapshotDb::create(&work).unwrap();
        writer.put_entries(entries).unwrap();
        writer
            .write_meta(&ScriptRegistrySnapshotMeta {
                base: base.clone(),
                entry_count: entries.len() as u64,
                generated_at: 100,
            })
            .unwrap();
        writer.finalize_for_distribution().unwrap();
        if let Some((script_hash, replacement)) = corrupt_value {
            let connection = Connection::open(&work).unwrap();
            connection
                .execute(
                    "UPDATE script_registry SET script_pubkey=?1 WHERE script_hash=?2",
                    (replacement.as_bytes(), script_hash.as_ref() as &[u8]),
                )
                .unwrap();
        }
        let file_sha256 = SnapshotHash::calc_hash(&work).unwrap();
        let manifest = ScriptRegistryManifest::build(
            "script_registry_10.db".to_string(),
            file_sha256,
            base,
            entries.len() as u64,
            None,
            100,
        )
        .unwrap();
        let artifact_dir = test
            .config
            .script_registry_sidecar_dir()
            .join(SCRIPT_REGISTRY_BASES_DIR)
            .join(&manifest.registry_artifact_id);
        std::fs::create_dir_all(&artifact_dir).unwrap();
        std::fs::rename(&work, artifact_dir.join(&manifest.file_name)).unwrap();
        let manifest_file = "script_registry_10.manifest.json";
        let manifest_path = artifact_dir.join(manifest_file);
        manifest.save(&manifest_path).unwrap();
        let manifest_sha256 = SnapshotHash::calc_hash(&manifest_path).unwrap();
        ScriptRegistryActivationState {
            schema_version: super::super::SCRIPT_REGISTRY_ACTIVATION_STATE_VERSION.to_string(),
            state: ScriptRegistryState::Ready,
            active: Some(super::super::ActiveScriptRegistryPointer {
                registry_artifact_id: manifest.registry_artifact_id.clone(),
                manifest_file: manifest_file.to_string(),
                manifest_sha256,
            }),
            last_error: None,
            updated_at: 101,
        }
        .save_atomic(
            &test
                .config
                .script_registry_sidecar_dir()
                .join(SCRIPT_REGISTRY_STATE_FILE),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn full_replay_uses_overlay_and_returns_definitive_misses() {
        let test = test_resolver("full_replay", false);
        let overlay_script = script(1);
        let overlay_hash = overlay_script.to_btc_script_hash();
        let missing_hash = script(2).to_btc_script_hash();
        test.db
            .put_script_registry_entries(&[ScriptRegistryEntry {
                script_hash: overlay_hash,
                script_pubkey: overlay_script.clone(),
            }])
            .unwrap();

        let (items, readiness) = test
            .resolver
            .resolve(&[missing_hash, overlay_hash, missing_hash])
            .unwrap();
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::FullReplay
        );
        assert_eq!(items[0].status, ScriptHashResolutionStatus::NotFound);
        assert_eq!(items[1].status, ScriptHashResolutionStatus::FoundOverlay);
        assert_eq!(items[1].script_pubkey.as_ref(), Some(&overlay_script));
        assert_eq!(items[2].status, ScriptHashResolutionStatus::NotFound);
    }

    #[test]
    fn snapshot_misses_change_from_unresolved_to_found_without_restart() {
        let test = test_resolver("late_activation", true);
        let base_script = script(3);
        let base_hash = base_script.to_btc_script_hash();
        let overlay_script = script(4);
        let overlay_hash = overlay_script.to_btc_script_hash();
        test.db
            .put_script_registry_entries(&[ScriptRegistryEntry {
                script_hash: overlay_hash,
                script_pubkey: overlay_script,
            }])
            .unwrap();

        let (before, readiness) = test.resolver.resolve(&[base_hash]).unwrap();
        assert_eq!(before[0].status, ScriptHashResolutionStatus::Unresolved);
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::PostSnapshotOnly
        );

        let manifest = activate_sidecar(
            &test,
            &[ScriptRegistryEntry {
                script_hash: base_hash,
                script_pubkey: base_script.clone(),
            }],
            None,
        );
        let (after, readiness) = test
            .resolver
            .resolve(&[base_hash, overlay_hash, script(5).to_btc_script_hash()])
            .unwrap();
        assert_eq!(after[0].status, ScriptHashResolutionStatus::FoundBase);
        assert_eq!(after[0].source, Some(ScriptRegistrySource::BaseSidecar));
        assert_eq!(after[0].script_pubkey.as_ref(), Some(&base_script));
        assert_eq!(after[1].status, ScriptHashResolutionStatus::FoundOverlay);
        assert_eq!(after[2].status, ScriptHashResolutionStatus::NotFound);
        assert_eq!(readiness.state, ScriptRegistryState::Ready);
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::SnapshotPlusSidecar
        );
        assert_eq!(
            readiness.registry_artifact_id.as_deref(),
            Some(manifest.registry_artifact_id.as_str())
        );
        readiness.validate().unwrap();
    }

    #[test]
    fn overlay_hit_shadows_matching_base_without_changing_coverage() {
        let test = test_resolver("overlay_shadows_base", true);
        let shared_script = script(9);
        let shared_hash = shared_script.to_btc_script_hash();
        test.db
            .put_script_registry_entries(&[ScriptRegistryEntry {
                script_hash: shared_hash,
                script_pubkey: shared_script.clone(),
            }])
            .unwrap();
        activate_sidecar(
            &test,
            &[ScriptRegistryEntry {
                script_hash: shared_hash,
                script_pubkey: shared_script.clone(),
            }],
            None,
        );

        let (items, readiness) = test.resolver.resolve(&[shared_hash, shared_hash]).unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            assert_eq!(item.status, ScriptHashResolutionStatus::FoundOverlay);
            assert_eq!(item.source, Some(ScriptRegistrySource::Overlay));
            assert_eq!(item.script_pubkey.as_ref(), Some(&shared_script));
        }
        assert_eq!(readiness.state, ScriptRegistryState::Ready);
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::SnapshotPlusSidecar
        );
        readiness.validate().unwrap();
    }

    #[test]
    fn invalid_activation_isolated_as_auxiliary_failure() {
        let test = test_resolver("invalid_state", true);
        let state_path = test
            .config
            .script_registry_sidecar_dir()
            .join(SCRIPT_REGISTRY_STATE_FILE);
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        std::fs::write(&state_path, b"{\"schema_version\":\"wrong\"}").unwrap();

        let (items, readiness) = test
            .resolver
            .resolve(&[script(6).to_btc_script_hash()])
            .unwrap();
        assert_eq!(items[0].status, ScriptHashResolutionStatus::Unresolved);
        assert_eq!(readiness.state, ScriptRegistryState::Failed);
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::PostSnapshotOnly
        );
        assert!(readiness.last_error.is_some());
        readiness.validate().unwrap();
    }

    #[test]
    fn corrupted_base_mapping_returns_conflict_and_disables_base() {
        let test = test_resolver("corrupt_mapping", true);
        let expected_script = script(7);
        let expected_hash = expected_script.to_btc_script_hash();
        activate_sidecar(
            &test,
            &[ScriptRegistryEntry {
                script_hash: expected_hash,
                script_pubkey: expected_script,
            }],
            Some((&expected_hash, script(8))),
        );

        let (items, readiness) = test.resolver.resolve(&[expected_hash]).unwrap();
        assert_eq!(items[0].status, ScriptHashResolutionStatus::Conflict);
        assert_eq!(readiness.state, ScriptRegistryState::Conflict);
        assert_eq!(
            readiness.coverage_mode,
            ScriptRegistryCoverageMode::PostSnapshotOnly
        );
        readiness.validate().unwrap();
    }
}
