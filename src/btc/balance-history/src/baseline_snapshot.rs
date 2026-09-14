//! Source-independent business-genesis snapshots; production installation is a separate stage.

mod export;
mod job;
mod source;
pub(crate) mod storage;

pub use export::{BaselineExportOptions, export_baseline_snapshot};
pub use job::{
    BaselineJobInput, BaselineJobReport, BaselineJobSource, baseline_job_status,
    create_or_resume_baseline, verify_baseline_job,
};
pub use storage::verify_baseline_snapshot;

use std::collections::BTreeMap;

use bitcoincore_rpc::bitcoin::{BlockHash, Network};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bootstrap::NativeBootstrapState;
use crate::{
    BALANCE_HISTORY_DATA_MODEL_VERSION, BalanceHistoryDBIdentity, COMMIT_PROTOCOL_VERSION,
};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(crate) const SQL: &str = include_str!("db/baseline_snapshot_v1.sql");
/// Physical single-file schema; deliberately distinct from registry-free core v1.
pub const BASELINE_SCHEMA: &str = "balance-history-baseline-snapshot:v1";
/// Manifest and signature contract for the single-file baseline artifact.
pub const BASELINE_MANIFEST: &str = "balance-history-baseline-manifest:v1";
/// Coverage is the live set at G plus every output script of block G; later observations append.
pub const BASELINE_REGISTRY_POLICY: &str = "live_utxos_and_genesis_outputs:v1";

/// Business state identity shared by full replay and AssumeUTXO replay.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineIdentity {
    /// Exact Bitcoin network.
    pub network: Network,
    /// Snapshot height, which must also be the business genesis.
    pub height: u32,
    /// Canonical hash at the genesis height.
    pub block_hash: BlockHash,
    /// Frozen logical interpretation of balances and UTXOs.
    pub data_model_version: String,
    /// Existing rolling commitment protocol, never replaced by a state digest.
    pub commit_protocol_version: String,
    /// Explicit limited script coverage, independent of source history.
    pub registry_policy: String,
    /// Earliest complete at-or-before balance lookup.
    pub balance_query_floor: u32,
    /// Earliest complete delta/history lookup; G is a balance baseline.
    pub history_query_floor: u32,
}

impl BaselineIdentity {
    /// Construct the exact-genesis contract without claiming historical deltas at G.
    pub fn new(
        network: Network,
        height: u32,
        block_hash: BlockHash,
    ) -> std::result::Result<Self, String> {
        if height == 0 || height == u32::MAX {
            return Err("Baseline height must be in 1..u32::MAX".to_string());
        }
        Ok(Self {
            network,
            height,
            block_hash,
            data_model_version: BALANCE_HISTORY_DATA_MODEL_VERSION.to_string(),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            registry_policy: BASELINE_REGISTRY_POLICY.to_string(),
            balance_query_floor: height,
            history_query_floor: height + 1,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if *self != Self::new(self.network, self.height, self.block_hash)? {
            return Err("Unsupported baseline identity, coverage policy or query floors".into());
        }
        Ok(())
    }
}

/// Ordered table digest, excluding physical layout and source-specific metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineTableDigest {
    /// Exact number of canonical rows.
    pub rows: u64,
    /// SHA-256 of the documented canonical row stream.
    pub sha256: String,
}

/// Complete logical contents of a normalized baseline snapshot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BaselineState {
    /// Shared business identity.
    pub identity: BaselineIdentity,
    /// Digests of balances, UTXOs, G's commit, registry and the raw G block.
    pub tables: BTreeMap<String, BaselineTableDigest>,
}

impl BaselineState {
    /// Derive an audit identity; this hash is never installed as C(G).
    pub fn logical_sha256(&self) -> std::result::Result<String, String> {
        domain_bytes("balance-history-baseline-state:v1", self)
            .map(|payload| crate::assumeutxo::format::hex(&Sha256::digest(payload)))
            .map_err(|e| e.to_string())
    }
}

/// Honest producer provenance, excluded from logical equivalence but covered by the signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BaselineSource {
    /// Exact-height full replay, without imported legacy state or a registry sidecar.
    FullReplay {
        db_identity: BalanceHistoryDBIdentity,
    },
    /// Sealed native replay, including its independently verified original source/checkpoint.
    Assumeutxo { state: Box<NativeBootstrapState> },
    /// Verified legacy split artifacts, retained as provenance rather than native-import evidence.
    LegacySplit {
        /// Complete signed core identity.
        core: Box<crate::CoreSnapshotManifest>,
        /// Registry identity bound to that exact core.
        registry: Box<crate::ScriptRegistryManifest>,
    },
}

/// One signed manifest for the complete baseline DB. Trusted keys remain release-owned.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineManifest {
    /// Version and signature domain selector.
    pub manifest_version: String,
    /// Physical database schema version.
    pub snapshot_schema_version: String,
    /// File basename; paths are never accepted here.
    pub file_name: String,
    /// Complete file length.
    pub file_size: u64,
    /// SHA-256 of the finalized SQLite file.
    pub file_sha256: String,
    /// Canonical logical state.
    pub state: BaselineState,
    /// Source-independent audit hash, distinct from the block commit.
    pub logical_sha256: String,
    /// Producer provenance; never a private key or runtime config.
    pub source: BaselineSource,
    /// Existing signing algorithm.
    pub signature_scheme: String,
    /// Existing snapshot key ID.
    pub signing_key_id: String,
    /// Creation time, outside the logical state hash.
    pub generated_at: u64,
}

impl BaselineManifest {
    pub(crate) fn signature_payload(&self) -> Result<Vec<u8>> {
        domain_bytes("usdb.balance-history.baseline-manifest-signature:v1", self)
    }
}

// Match the existing length-delimited signature convention, with a distinct domain.
fn domain_bytes(domain: &str, value: &impl Serialize) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(value)?;
    let mut payload = Vec::new();
    payload.extend(u32::try_from(domain.len())?.to_be_bytes());
    payload.extend(domain.as_bytes());
    payload.extend(u64::try_from(json.len())?.to_be_bytes());
    payload.extend(json);
    Ok(payload)
}

#[cfg(test)]
#[path = "../../../../tests/baseline_snapshot.rs"]
mod tests;
