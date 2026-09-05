use crate::db::BalanceHistoryDBIdentity;
use crate::service::HistoricalSnapshotStateRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, build_consensus_snapshot_id,
};

/// Manifest version for the first registry-free core snapshot artifact.
pub const CORE_SNAPSHOT_MANIFEST_VERSION: &str = "balance-history-core-snapshot-manifest:v1";
/// SQLite schema version for the first registry-free core snapshot artifact.
pub const CORE_SNAPSHOT_SCHEMA_VERSION: &str = "balance-history-core-snapshot:v1";
/// Artifact-ID hash domain for core snapshot artifacts.
pub const CORE_SNAPSHOT_ARTIFACT_ID_DOMAIN: &str =
    "usdb.balance-history.core-snapshot-artifact-id:v1";
/// Detached-signature domain for core snapshot manifests.
pub const CORE_SNAPSHOT_SIGNATURE_DOMAIN: &str =
    "usdb.balance-history.core-snapshot-manifest-signature:v1";
/// Manifest version for the first standalone script-registry artifact.
pub const SCRIPT_REGISTRY_MANIFEST_VERSION: &str = "balance-history-script-registry-manifest:v1";
/// SQLite schema version for the first standalone script-registry artifact.
pub const SCRIPT_REGISTRY_SCHEMA_VERSION: &str = "balance-history-script-registry-sqlite:v1";
/// Artifact-ID hash domain for standalone script-registry artifacts.
pub const SCRIPT_REGISTRY_ARTIFACT_ID_DOMAIN: &str =
    "usdb.balance-history.script-registry-artifact-id:v1";
/// Detached-signature domain for standalone script-registry manifests.
pub const SCRIPT_REGISTRY_SIGNATURE_DOMAIN: &str =
    "usdb.balance-history.script-registry-manifest-signature:v1";
/// Policy identifier for the append-like, non-consensus script registry.
pub const SCRIPT_REGISTRY_POLICY: &str = "auxiliary_seen_scripts_non_consensus_v1";
/// Signature scheme accepted by the v1 artifact contracts.
pub const SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519: &str = "ed25519";
/// Frozen SQLite schema for the registry-free core snapshot.
pub const CORE_SNAPSHOT_SQL_SCHEMA_V1: &str = include_str!("db/core_snapshot_v1.sql");
/// Frozen SQLite schema for the standalone script-registry sidecar.
pub const SCRIPT_REGISTRY_SQL_SCHEMA_V1: &str = include_str!("db/script_registry_v1.sql");

/// Artifact kind covered by a split snapshot manifest.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotArtifactType {
    /// Consensus-relevant balance-history checkpoint without script-registry rows.
    BalanceHistoryCore,
    /// Optional historical script-hash lookup sidecar.
    BalanceHistoryScriptRegistry,
}

/// Manifest for a registry-free core snapshot artifact.
///
/// The `core_snapshot_id` is the consensus state identity from `state_ref`.
/// `core_artifact_id` additionally binds the concrete SQLite file hash and
/// schema, so rebuilding the same state into different bytes produces a
/// different artifact without changing consensus identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreSnapshotManifest {
    /// Fixed manifest schema version.
    pub manifest_version: String,
    /// Fixed artifact type.
    pub artifact_type: SnapshotArtifactType,
    /// Frozen SQLite schema version.
    pub snapshot_schema_version: String,
    /// Must remain false for every core artifact.
    pub registry_included: bool,
    /// Consensus snapshot identity, equal to `state_ref.snapshot_id`.
    pub core_snapshot_id: String,
    /// File-specific artifact identity derived by `Self::calculate_artifact_id`.
    pub core_artifact_id: String,
    /// SQLite file basename.
    pub file_name: String,
    /// Lowercase SHA-256 digest of the SQLite file.
    pub file_sha256: String,
    /// Exact historical consensus state restored by the artifact.
    pub state_ref: HistoricalSnapshotStateRef,
    /// RocksDB state-domain identity that exported the artifact.
    pub db_identity: BalanceHistoryDBIdentity,
    /// Earliest complete at-or-before balance query height.
    pub balance_query_floor: u32,
    /// Earliest complete exact-delta and history-range query height.
    pub history_query_floor: u32,
    /// Detached signature scheme, present only for signed artifacts.
    pub signature_scheme: Option<String>,
    /// Trusted signer identifier, present only for signed artifacts.
    pub signing_key_id: Option<String>,
    /// Manifest creation time as Unix seconds.
    pub generated_at: u64,
}

/// Immutable BTC and core-snapshot anchor covered by a registry sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptRegistryBaseIdentity {
    /// Canonical Bitcoin network name.
    pub btc_network: String,
    /// Genesis block hash for `btc_network`.
    pub btc_genesis_hash: String,
    /// Inclusive maximum BTC height represented by the sidecar.
    pub base_height: u32,
    /// Canonical BTC block hash at `base_height`.
    pub base_block_hash: String,
    /// Consensus snapshot identity of the paired core checkpoint.
    pub core_snapshot_id: String,
}

/// Manifest for an optional immutable script-registry sidecar.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptRegistryManifest {
    /// Fixed manifest schema version.
    pub manifest_version: String,
    /// Fixed artifact type.
    pub artifact_type: SnapshotArtifactType,
    /// Frozen SQLite schema version.
    pub registry_schema_version: String,
    /// Append-like, non-consensus registry policy.
    pub policy: String,
    /// File-specific identity derived by `Self::calculate_artifact_id`.
    pub registry_artifact_id: String,
    /// SQLite file basename.
    pub file_name: String,
    /// Lowercase SHA-256 digest of the SQLite file.
    pub file_sha256: String,
    /// BTC and core-snapshot identity represented by this sidecar.
    pub base: ScriptRegistryBaseIdentity,
    /// Exact number of mappings in the sidecar.
    pub entry_count: u64,
    /// Detached signature scheme, present only for signed artifacts.
    pub signature_scheme: Option<String>,
    /// Trusted signer identifier, present only for signed artifacts.
    pub signing_key_id: Option<String>,
    /// Manifest creation time as Unix seconds.
    pub generated_at: u64,
}

#[derive(Serialize)]
struct CoreArtifactIdentity<'a> {
    manifest_version: &'a str,
    artifact_type: SnapshotArtifactType,
    snapshot_schema_version: &'a str,
    registry_included: bool,
    file_sha256: &'a str,
    core_snapshot_id: &'a str,
    state_ref: &'a HistoricalSnapshotStateRef,
    db_identity: &'a BalanceHistoryDBIdentity,
    balance_query_floor: u32,
    history_query_floor: u32,
}

#[derive(Serialize)]
struct RegistryArtifactIdentity<'a> {
    manifest_version: &'a str,
    artifact_type: SnapshotArtifactType,
    registry_schema_version: &'a str,
    policy: &'a str,
    file_sha256: &'a str,
    base: &'a ScriptRegistryBaseIdentity,
    entry_count: u64,
}

impl CoreSnapshotManifest {
    /// Builds and validates a v1 core snapshot manifest.
    pub fn build(
        file_name: String,
        file_sha256: String,
        state_ref: HistoricalSnapshotStateRef,
        db_identity: BalanceHistoryDBIdentity,
        signing_key_id: Option<String>,
        generated_at: u64,
    ) -> Result<Self, String> {
        let mut manifest = Self {
            manifest_version: CORE_SNAPSHOT_MANIFEST_VERSION.to_string(),
            artifact_type: SnapshotArtifactType::BalanceHistoryCore,
            snapshot_schema_version: CORE_SNAPSHOT_SCHEMA_VERSION.to_string(),
            registry_included: false,
            core_snapshot_id: state_ref.snapshot_id.clone(),
            core_artifact_id: String::new(),
            file_name,
            file_sha256,
            balance_query_floor: state_ref.block_height,
            history_query_floor: state_ref.block_height.saturating_add(1),
            state_ref,
            db_identity,
            signature_scheme: signing_key_id
                .as_ref()
                .map(|_| SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519.to_string()),
            signing_key_id,
            generated_at,
        };
        manifest.core_artifact_id = manifest.calculate_artifact_id()?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Derives the file-specific artifact ID without signing or packaging fields.
    pub fn calculate_artifact_id(&self) -> Result<String, String> {
        hash_domain_separated_json(
            CORE_SNAPSHOT_ARTIFACT_ID_DOMAIN,
            &CoreArtifactIdentity {
                manifest_version: &self.manifest_version,
                artifact_type: self.artifact_type,
                snapshot_schema_version: &self.snapshot_schema_version,
                registry_included: self.registry_included,
                file_sha256: &self.file_sha256,
                core_snapshot_id: &self.core_snapshot_id,
                state_ref: &self.state_ref,
                db_identity: &self.db_identity,
                balance_query_floor: self.balance_query_floor,
                history_query_floor: self.history_query_floor,
            },
        )
    }

    /// Returns the domain-separated bytes covered by an Ed25519 signature.
    pub fn signature_payload(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        domain_separated_json(CORE_SNAPSHOT_SIGNATURE_DOMAIN, self)
    }

    /// Loads a strict v1 core manifest from disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let manifest: Self = load_manifest(path, "core snapshot")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Writes this validated manifest as formatted JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        save_manifest(path, self, "core snapshot")
    }

    /// Validates all fixed fields and cross-field identities.
    pub fn validate(&self) -> Result<(), String> {
        require_equal(
            "core manifest version",
            &self.manifest_version,
            CORE_SNAPSHOT_MANIFEST_VERSION,
        )?;
        if self.artifact_type != SnapshotArtifactType::BalanceHistoryCore {
            return Err("Core manifest has the wrong artifact_type".to_string());
        }
        require_equal(
            "core snapshot schema version",
            &self.snapshot_schema_version,
            CORE_SNAPSHOT_SCHEMA_VERSION,
        )?;
        if self.registry_included {
            return Err("Core snapshot manifest must set registry_included=false".to_string());
        }
        validate_safe_basename(&self.file_name)?;
        validate_lower_hex_32("core file_sha256", &self.file_sha256)?;
        validate_lower_hex_32("core_snapshot_id", &self.core_snapshot_id)?;
        validate_lower_hex_32("core_artifact_id", &self.core_artifact_id)?;
        if self.core_snapshot_id != self.state_ref.snapshot_id {
            return Err("core_snapshot_id does not match state_ref.snapshot_id".to_string());
        }
        let calculated_snapshot_id =
            build_consensus_snapshot_id(&self.state_ref.consensus_identity);
        if self.core_snapshot_id != calculated_snapshot_id {
            return Err(format!(
                "core_snapshot_id does not match consensus_identity: expected {}, got {}",
                calculated_snapshot_id, self.core_snapshot_id
            ));
        }
        require_equal(
            "snapshot ID hash algorithm",
            &self.state_ref.snapshot_id_hash_algo,
            CONSENSUS_SNAPSHOT_ID_HASH_ALGO,
        )?;
        require_equal(
            "snapshot ID version",
            &self.state_ref.snapshot_id_version,
            CONSENSUS_SNAPSHOT_ID_VERSION,
        )?;
        if self.state_ref.block_height != self.state_ref.consensus_identity.stable_height {
            return Err("state_ref height does not match its consensus identity".to_string());
        }
        if self.state_ref.stable_block_hash != self.state_ref.consensus_identity.stable_block_hash {
            return Err("state_ref block hash does not match its consensus identity".to_string());
        }
        if self.db_identity.btc_network != self.state_ref.consensus_identity.network {
            return Err("Core DB identity network does not match state_ref network".to_string());
        }
        validate_lower_hex_32(
            "core DB identity BTC genesis hash",
            &self.db_identity.btc_genesis_hash,
        )?;
        validate_lower_hex_32("core stable block hash", &self.state_ref.stable_block_hash)?;
        validate_lower_hex_32(
            "core latest block commit",
            &self.state_ref.latest_block_commit,
        )?;
        if self.balance_query_floor != self.state_ref.block_height {
            return Err("Core balance_query_floor must equal snapshot height".to_string());
        }
        if self.history_query_floor != self.state_ref.block_height.saturating_add(1) {
            return Err("Core history_query_floor must equal snapshot height + 1".to_string());
        }
        validate_signing_fields(
            self.signature_scheme.as_deref(),
            self.signing_key_id.as_deref(),
        )?;
        let expected_artifact_id = self.calculate_artifact_id()?;
        if self.core_artifact_id != expected_artifact_id {
            return Err(format!(
                "Core artifact ID mismatch: expected {}, got {}",
                expected_artifact_id, self.core_artifact_id
            ));
        }
        Ok(())
    }
}

impl ScriptRegistryManifest {
    /// Builds and validates a v1 script-registry manifest.
    pub fn build(
        file_name: String,
        file_sha256: String,
        base: ScriptRegistryBaseIdentity,
        entry_count: u64,
        signing_key_id: Option<String>,
        generated_at: u64,
    ) -> Result<Self, String> {
        let mut manifest = Self {
            manifest_version: SCRIPT_REGISTRY_MANIFEST_VERSION.to_string(),
            artifact_type: SnapshotArtifactType::BalanceHistoryScriptRegistry,
            registry_schema_version: SCRIPT_REGISTRY_SCHEMA_VERSION.to_string(),
            policy: SCRIPT_REGISTRY_POLICY.to_string(),
            registry_artifact_id: String::new(),
            file_name,
            file_sha256,
            base,
            entry_count,
            signature_scheme: signing_key_id
                .as_ref()
                .map(|_| SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519.to_string()),
            signing_key_id,
            generated_at,
        };
        manifest.registry_artifact_id = manifest.calculate_artifact_id()?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Derives the file-specific artifact ID without signing or packaging fields.
    pub fn calculate_artifact_id(&self) -> Result<String, String> {
        hash_domain_separated_json(
            SCRIPT_REGISTRY_ARTIFACT_ID_DOMAIN,
            &RegistryArtifactIdentity {
                manifest_version: &self.manifest_version,
                artifact_type: self.artifact_type,
                registry_schema_version: &self.registry_schema_version,
                policy: &self.policy,
                file_sha256: &self.file_sha256,
                base: &self.base,
                entry_count: self.entry_count,
            },
        )
    }

    /// Returns the domain-separated bytes covered by an Ed25519 signature.
    pub fn signature_payload(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        domain_separated_json(SCRIPT_REGISTRY_SIGNATURE_DOMAIN, self)
    }

    /// Loads a strict v1 script-registry manifest from disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let manifest: Self = load_manifest(path, "script-registry")?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Writes this validated manifest as formatted JSON.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        save_manifest(path, self, "script-registry")
    }

    /// Validates all fixed fields and cross-field identities.
    pub fn validate(&self) -> Result<(), String> {
        require_equal(
            "registry manifest version",
            &self.manifest_version,
            SCRIPT_REGISTRY_MANIFEST_VERSION,
        )?;
        if self.artifact_type != SnapshotArtifactType::BalanceHistoryScriptRegistry {
            return Err("Registry manifest has the wrong artifact_type".to_string());
        }
        require_equal(
            "registry schema version",
            &self.registry_schema_version,
            SCRIPT_REGISTRY_SCHEMA_VERSION,
        )?;
        require_equal("registry policy", &self.policy, SCRIPT_REGISTRY_POLICY)?;
        validate_safe_basename(&self.file_name)?;
        validate_lower_hex_32("registry file_sha256", &self.file_sha256)?;
        validate_lower_hex_32("registry_artifact_id", &self.registry_artifact_id)?;
        validate_lower_hex_32("registry BTC genesis hash", &self.base.btc_genesis_hash)?;
        validate_lower_hex_32("registry base block hash", &self.base.base_block_hash)?;
        validate_lower_hex_32("registry core_snapshot_id", &self.base.core_snapshot_id)?;
        if self.base.btc_network.is_empty() {
            return Err("Registry BTC network must not be empty".to_string());
        }
        if self.base.base_height == 0 {
            return Err("Registry base height 0 is unsupported".to_string());
        }
        validate_signing_fields(
            self.signature_scheme.as_deref(),
            self.signing_key_id.as_deref(),
        )?;
        let expected_artifact_id = self.calculate_artifact_id()?;
        if self.registry_artifact_id != expected_artifact_id {
            return Err(format!(
                "Registry artifact ID mismatch: expected {}, got {}",
                expected_artifact_id, self.registry_artifact_id
            ));
        }
        Ok(())
    }

    /// Validates that this registry sidecar was built for one exact core checkpoint.
    pub fn validate_against_core(&self, core: &CoreSnapshotManifest) -> Result<(), String> {
        self.validate()?;
        core.validate()?;
        if self.base.core_snapshot_id != core.core_snapshot_id {
            return Err("Registry core_snapshot_id does not match core manifest".to_string());
        }
        if self.base.base_height != core.state_ref.block_height {
            return Err("Registry base height does not match core snapshot height".to_string());
        }
        if self.base.base_block_hash != core.state_ref.stable_block_hash {
            return Err("Registry base block hash does not match core snapshot".to_string());
        }
        if self.base.btc_network != core.db_identity.btc_network {
            return Err("Registry BTC network does not match core DB identity".to_string());
        }
        if self.base.btc_genesis_hash != core.db_identity.btc_genesis_hash {
            return Err("Registry BTC genesis hash does not match core DB identity".to_string());
        }
        Ok(())
    }
}

fn hash_domain_separated_json<T: Serialize>(domain: &str, value: &T) -> Result<String, String> {
    let payload = domain_separated_json(domain, value)?;
    Ok(format!("{:x}", Sha256::digest(payload)))
}

fn load_manifest<T>(path: &Path, label: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let data = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read {label} manifest {}: {error}",
            path.display()
        )
    })?;
    usdb_util::parse_json_strict(&data).map_err(|error| {
        format!(
            "Failed to parse {label} manifest {}: {error}",
            path.display()
        )
    })
}

fn save_manifest<T: Serialize>(path: &Path, manifest: &T, label: &str) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("Failed to serialize {label} manifest: {error}"))?;
    std::fs::write(path, data).map_err(|error| {
        format!(
            "Failed to write {label} manifest {}: {error}",
            path.display()
        )
    })
}

fn domain_separated_json<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(value)
        .map_err(|error| format!("Failed to serialize artifact contract: {error}"))?;
    let domain_len =
        u32::try_from(domain.len()).map_err(|_| "Artifact hash domain is too large".to_string())?;
    let json_len =
        u64::try_from(json.len()).map_err(|_| "Artifact JSON is too large".to_string())?;
    let mut payload = Vec::with_capacity(4 + domain.len() + 8 + json.len());
    payload.extend_from_slice(&domain_len.to_be_bytes());
    payload.extend_from_slice(domain.as_bytes());
    payload.extend_from_slice(&json_len.to_be_bytes());
    payload.extend_from_slice(&json);
    Ok(payload)
}

fn validate_signing_fields(
    signature_scheme: Option<&str>,
    signing_key_id: Option<&str>,
) -> Result<(), String> {
    match (signature_scheme, signing_key_id) {
        (None, None) => Ok(()),
        (Some(SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519), Some(key_id)) if !key_id.is_empty() => {
            Ok(())
        }
        (Some(scheme), Some(_)) => Err(format!(
            "Unsupported artifact signature scheme {scheme}; expected {SNAPSHOT_ARTIFACT_SIGNATURE_SCHEME_ED25519}"
        )),
        _ => Err("signature_scheme and signing_key_id must be present together".to_string()),
    }
}

fn validate_safe_basename(value: &str) -> Result<(), String> {
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(format!(
            "Artifact file_name must be a safe basename: {value}"
        ));
    }
    Ok(())
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

fn require_equal(field: &str, actual: &str, expected: &str) -> Result<(), String> {
    if actual != expected {
        return Err(format!("Unsupported {field} {actual}; expected {expected}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{COMMIT_HASH_ALGO, COMMIT_PROTOCOL_VERSION};
    use rusqlite::Connection;
    use usdb_util::{
        CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, CONSENSUS_SOURCE_CHAIN_BTC,
        ConsensusSnapshotIdentity, build_consensus_snapshot_id,
    };

    fn state_ref() -> HistoricalSnapshotStateRef {
        let consensus_identity = ConsensusSnapshotIdentity {
            source_chain: CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
            network: "regtest".to_string(),
            stable_height: 963_800,
            stable_block_hash: "22".repeat(32),
            stable_lag: 10,
            balance_history_api_version: "1.0.0".to_string(),
            balance_history_semantics_version: "balance-snapshot-at-or-before:v1".to_string(),
        };
        let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
        HistoricalSnapshotStateRef {
            block_height: consensus_identity.stable_height,
            stable_block_hash: consensus_identity.stable_block_hash.clone(),
            latest_block_commit: "33".repeat(32),
            consensus_identity,
            snapshot_id,
            snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
            snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
        }
    }

    fn db_identity() -> BalanceHistoryDBIdentity {
        BalanceHistoryDBIdentity {
            identity_version: "balance-history-db-identity:v1".to_string(),
            service: "balance-history".to_string(),
            schema_version: "balance-history-rocksdb:v1".to_string(),
            data_model_version: "balance-history-data-model:v1".to_string(),
            btc_network: "regtest".to_string(),
            btc_genesis_hash: "44".repeat(32),
        }
    }

    fn core_manifest() -> CoreSnapshotManifest {
        CoreSnapshotManifest::build(
            "balance_history_core_963800.db".to_string(),
            "11".repeat(32),
            state_ref(),
            db_identity(),
            Some("test-release-key".to_string()),
            1_725_000_000,
        )
        .unwrap()
    }

    fn registry_manifest(core_snapshot_id: String) -> ScriptRegistryManifest {
        ScriptRegistryManifest::build(
            "script_registry_963800.db".to_string(),
            "55".repeat(32),
            ScriptRegistryBaseIdentity {
                btc_network: "regtest".to_string(),
                btc_genesis_hash: "44".repeat(32),
                base_height: 963_800,
                base_block_hash: "22".repeat(32),
                core_snapshot_id,
            },
            1_541_365_559,
            Some("test-release-key".to_string()),
            1_725_000_001,
        )
        .unwrap()
    }

    #[test]
    fn split_snapshot_schemas_have_disjoint_tables() {
        assert_eq!(
            format!("{:x}", Sha256::digest(CORE_SNAPSHOT_SQL_SCHEMA_V1)),
            "a0dfb24ad67b4958d04264760595e5b3ebf64426d5d31ab240e497f3ada0939d"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(SCRIPT_REGISTRY_SQL_SCHEMA_V1)),
            "7665afcdc6ad3d2996cac451880f96cd349b498a5846fd2e2d639d671f2dbb1f"
        );
        let core = Connection::open_in_memory().unwrap();
        core.execute_batch(CORE_SNAPSHOT_SQL_SCHEMA_V1).unwrap();
        let core_registry_count: u32 = core
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='script_registry'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(core_registry_count, 0);

        let registry = Connection::open_in_memory().unwrap();
        registry
            .execute_batch(SCRIPT_REGISTRY_SQL_SCHEMA_V1)
            .unwrap();
        let registry_sql: String = registry
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='script_registry'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(registry_sql.contains("WITHOUT ROWID"));
    }

    #[test]
    fn artifact_ids_and_signature_payloads_have_stable_vectors() {
        let core = core_manifest();
        let registry = registry_manifest(core.core_snapshot_id.clone());
        let core_signature_hash =
            format!("{:x}", Sha256::digest(core.signature_payload().unwrap()));
        let registry_signature_hash = format!(
            "{:x}",
            Sha256::digest(registry.signature_payload().unwrap())
        );

        assert_eq!(
            core.core_artifact_id,
            "6f582d284e59f53e8657555951487d3191e10459eda0cc5612110182582dc2a8"
        );
        assert_eq!(
            registry.registry_artifact_id,
            "7c5f96657dcd659b5ee68a1ec149843217799e5a2fca742b5b72055d68ce4393"
        );
        assert_eq!(
            core_signature_hash,
            "979ccd1b78741ebea6cf49b7e6fa5090a947493dc59f8dda0c73a1ebda566e16"
        );
        assert_eq!(
            registry_signature_hash,
            "eb58be3ded88b4cd841a71d68955a74462f0d7a760e3c130df9bdb71ad1c0830"
        );
    }

    #[test]
    fn manifests_reject_identity_and_signature_mismatches() {
        let mut core = core_manifest();
        core.registry_included = true;
        assert!(
            core.validate()
                .unwrap_err()
                .contains("registry_included=false")
        );

        let mut registry = registry_manifest(state_ref().snapshot_id);
        registry.signature_scheme = None;
        assert!(
            registry
                .validate()
                .unwrap_err()
                .contains("must be present together")
        );

        let mut registry = registry_manifest(state_ref().snapshot_id);
        registry.entry_count += 1;
        assert!(
            registry
                .validate()
                .unwrap_err()
                .contains("artifact ID mismatch")
        );
    }

    #[test]
    fn registry_manifest_is_bound_to_exact_core_checkpoint() {
        let core = core_manifest();
        let registry = registry_manifest(core.core_snapshot_id.clone());
        registry.validate_against_core(&core).unwrap();

        let mut wrong_height = registry.clone();
        wrong_height.base.base_height -= 1;
        wrong_height.registry_artifact_id = wrong_height.calculate_artifact_id().unwrap();
        assert!(
            wrong_height
                .validate_against_core(&core)
                .unwrap_err()
                .contains("base height")
        );

        let mut wrong_genesis = registry;
        wrong_genesis.base.btc_genesis_hash = "66".repeat(32);
        wrong_genesis.registry_artifact_id = wrong_genesis.calculate_artifact_id().unwrap();
        assert!(
            wrong_genesis
                .validate_against_core(&core)
                .unwrap_err()
                .contains("genesis hash")
        );
    }

    #[test]
    fn manifests_reject_unknown_json_fields() {
        let mut value = serde_json::to_value(core_manifest()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("legacy_registry_count".to_string(), 1.into());
        assert!(serde_json::from_value::<CoreSnapshotManifest>(value).is_err());
    }
}
