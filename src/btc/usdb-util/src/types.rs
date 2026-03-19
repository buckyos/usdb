use crate::USDBScriptHash;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, Txid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct UTXOEntry {
    pub outpoint: OutPoint,
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

impl UTXOEntry {
    pub fn outpoint_vec(&self) -> [u8; 36] {
        OutPointCodec::encode(&self.outpoint)
    }
}

#[derive(Debug, Clone)]
pub struct UTXOValue {
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

impl UTXOValue {
    pub fn to_vec(&self) -> [u8; USDBScriptHash::LEN + 8] {
        Self::encode(&self.script_hash, self.value)
    }

    pub fn encode(script_hash: &USDBScriptHash, value: u64) -> [u8; USDBScriptHash::LEN + 8] {
        let mut data = [0u8; USDBScriptHash::LEN + 8];
        data[..USDBScriptHash::LEN].copy_from_slice(script_hash.as_ref() as &[u8]);
        data[USDBScriptHash::LEN..].copy_from_slice(&value.to_be_bytes());
        data
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, String> {
        if data.len() != USDBScriptHash::LEN + 8 {
            return Err("Invalid UTXOValue data length".to_string());
        }

        let script_hash = USDBScriptHash::from_slice(&data[0..USDBScriptHash::LEN])
            .map_err(|e| format!("Failed to parse script hash: {}", e))?;
        let value = u64::from_be_bytes(
            data[USDBScriptHash::LEN..USDBScriptHash::LEN + 8]
                .try_into()
                .map_err(|_| "Failed to parse value".to_string())?,
        );

        Ok(UTXOValue { script_hash, value })
    }
}

pub type UTXOEntryRef = Arc<UTXOValue>;
pub type OutPointRef = Arc<OutPoint>;

#[derive(Debug, Clone)]
pub struct BalanceHistoryData {
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub type BalanceHistoryDataRef = Arc<BalanceHistoryData>;

/// Fixed source-chain tag used by BTC-side consensus snapshot identifiers.
pub const CONSENSUS_SOURCE_CHAIN_BTC: &str = "BTC";
/// Hash algorithm used by canonical consensus snapshot ids.
pub const CONSENSUS_SNAPSHOT_ID_HASH_ALGO: &str = "sha256";
/// Version tag of the canonical consensus snapshot-id serialization rule.
pub const CONSENSUS_SNAPSHOT_ID_VERSION: &str = "btc-consensus-snapshot:v1";
/// Hash algorithm used by canonical usdb-index local-state commits.
pub const LOCAL_STATE_COMMIT_HASH_ALGO: &str = "sha256";
/// Version tag of the canonical usdb-index local-state commit serialization rule.
pub const LOCAL_STATE_COMMIT_VERSION: &str = "usdb-local-state:v1";
/// Hash algorithm used by canonical system-state ids consumed by downstream chains.
pub const SYSTEM_STATE_ID_HASH_ALGO: &str = "sha256";
/// Version tag of the canonical BTC-side system-state id serialization rule.
pub const SYSTEM_STATE_ID_VERSION: &str = "btc-system-state:v1";
/// Shared JSON-RPC error code returned when the requested height is not yet
/// covered by the service's current durable or stable view.
pub const CONSENSUS_RPC_ERR_HEIGHT_NOT_SYNCED: i64 = -32040;
/// Shared JSON-RPC error code returned when the service is alive but the
/// currently advertised snapshot is not safe for downstream consensus use.
pub const CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY: i64 = -32041;
/// Shared JSON-RPC error code returned when the caller's expected snapshot id
/// does not match the service's current snapshot identity.
pub const CONSENSUS_RPC_ERR_SNAPSHOT_ID_MISMATCH: i64 = -32042;
/// Shared JSON-RPC error code returned when the caller's expected stable BTC
/// block hash does not match the service's current stable anchor.
pub const CONSENSUS_RPC_ERR_BLOCK_HASH_MISMATCH: i64 = -32043;
/// Shared JSON-RPC error code returned when a versioned protocol or semantics
/// field required by the caller does not match the service's current value.
pub const CONSENSUS_RPC_ERR_VERSION_MISMATCH: i64 = -32044;
/// Shared JSON-RPC error code returned when the caller expects a different
/// locally durable core-state commit from the one currently exposed.
pub const CONSENSUS_RPC_ERR_LOCAL_STATE_COMMIT_MISMATCH: i64 = -32045;
/// Shared JSON-RPC error code returned when the caller expects a different
/// top-level system-state id from the one currently exposed.
pub const CONSENSUS_RPC_ERR_SYSTEM_STATE_ID_MISMATCH: i64 = -32046;
/// Shared JSON-RPC error code returned when the query is valid and within the
/// durable range, but no record exists for the requested object/key.
pub const CONSENSUS_RPC_ERR_NO_RECORD: i64 = -32047;

/// Shared consensus-layer JSON-RPC error contract used by BTC-side services.
///
/// These error names intentionally do not cover service-specific business
/// conditions such as `PASS_NOT_FOUND` or `INVALID_PAGINATION`. They are only
/// for cross-service, downstream-consumable readiness/anchor/version failures.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusRpcErrorCode {
    HeightNotSynced,
    SnapshotNotReady,
    SnapshotIdMismatch,
    BlockHashMismatch,
    VersionMismatch,
    LocalStateCommitMismatch,
    SystemStateIdMismatch,
    NoRecord,
}

impl ConsensusRpcErrorCode {
    /// Stable JSON-RPC server error integer used on the wire.
    pub fn code(self) -> i64 {
        match self {
            Self::HeightNotSynced => CONSENSUS_RPC_ERR_HEIGHT_NOT_SYNCED,
            Self::SnapshotNotReady => CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY,
            Self::SnapshotIdMismatch => CONSENSUS_RPC_ERR_SNAPSHOT_ID_MISMATCH,
            Self::BlockHashMismatch => CONSENSUS_RPC_ERR_BLOCK_HASH_MISMATCH,
            Self::VersionMismatch => CONSENSUS_RPC_ERR_VERSION_MISMATCH,
            Self::LocalStateCommitMismatch => CONSENSUS_RPC_ERR_LOCAL_STATE_COMMIT_MISMATCH,
            Self::SystemStateIdMismatch => CONSENSUS_RPC_ERR_SYSTEM_STATE_ID_MISMATCH,
            Self::NoRecord => CONSENSUS_RPC_ERR_NO_RECORD,
        }
    }

    /// Stable symbolic name used as the JSON-RPC `message` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HeightNotSynced => "HEIGHT_NOT_SYNCED",
            Self::SnapshotNotReady => "SNAPSHOT_NOT_READY",
            Self::SnapshotIdMismatch => "SNAPSHOT_ID_MISMATCH",
            Self::BlockHashMismatch => "BLOCK_HASH_MISMATCH",
            Self::VersionMismatch => "VERSION_MISMATCH",
            Self::LocalStateCommitMismatch => "LOCAL_STATE_COMMIT_MISMATCH",
            Self::SystemStateIdMismatch => "SYSTEM_STATE_ID_MISMATCH",
            Self::NoRecord => "NO_RECORD",
        }
    }
}

/// Optional consensus-state selectors that a downstream caller can pin when it
/// wants exact cross-service or cross-chain reproducibility.
///
/// This struct is intentionally broader than any single service. Unused fields
/// may remain `None` for services that do not expose that layer directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConsensusStateReference {
    /// Expected upstream consensus snapshot id.
    pub snapshot_id: Option<String>,
    /// Expected upstream stable BTC height.
    pub stable_height: Option<u32>,
    /// Expected upstream stable BTC block hash.
    pub stable_block_hash: Option<String>,
    /// Expected balance-history public API version.
    pub balance_history_api_version: Option<String>,
    /// Expected balance-history query semantics version.
    pub balance_history_semantics_version: Option<String>,
    /// Expected usdb-index public protocol version.
    pub usdb_index_protocol_version: Option<String>,
    /// Expected local-state commit from usdb-indexer.
    pub local_state_commit: Option<String>,
    /// Expected top-level system-state id from usdb-indexer.
    pub system_state_id: Option<String>,
}

impl ConsensusStateReference {
    /// Returns true when the caller did not pin any consensus-side selector.
    pub fn is_empty(&self) -> bool {
        self.snapshot_id.is_none()
            && self.stable_height.is_none()
            && self.stable_block_hash.is_none()
            && self.balance_history_api_version.is_none()
            && self.balance_history_semantics_version.is_none()
            && self.usdb_index_protocol_version.is_none()
            && self.local_state_commit.is_none()
            && self.system_state_id.is_none()
    }
}

/// Shared request-side context that downstream consumers can attach to
/// consensus-sensitive queries.
///
/// Phase 1 only standardizes this structure in `usdb-util`. Individual RPC
/// methods can adopt it incrementally without forcing every existing query to
/// change at once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConsensusQueryContext {
    /// Optional requested logical height associated with the query.
    pub requested_height: Option<u32>,
    /// Optional consensus-state selectors the caller expects the service to honor.
    pub expected_state: ConsensusStateReference,
}

/// Shared structured `data` payload for consensus-layer JSON-RPC errors.
///
/// `expected_state` mirrors the caller's pinned selectors, while `actual_state`
/// describes the service's current observed state at the time the error was
/// produced. This lets downstream verifiers distinguish real mismatches from
/// mere liveness/readiness issues without parsing free-form error strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConsensusRpcErrorData {
    /// Service emitting the error, for example `balance-history` or `usdb-indexer`.
    pub service: String,
    /// Optional requested logical height associated with the failed query.
    pub requested_height: Option<u32>,
    /// Optional locally durable synced height on the emitting service.
    pub local_synced_height: Option<u32>,
    /// Optional upstream stable height observed by the emitting service.
    pub upstream_stable_height: Option<u32>,
    /// Optional current consensus-ready flag at the time of failure.
    pub consensus_ready: Option<bool>,
    /// Optional request-side selectors supplied by the caller.
    pub expected_state: ConsensusStateReference,
    /// Service-side state observed when the error was raised.
    pub actual_state: ConsensusStateReference,
    /// Optional short detail string for operator debugging.
    pub detail: Option<String>,
}

impl ConsensusRpcErrorData {
    /// Builds an empty structured payload for one service.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsensusSnapshotIdentity {
    /// Fixed chain namespace, currently `BTC`.
    pub source_chain: String,
    /// Bitcoin network name, such as `mainnet` or `regtest`.
    pub network: String,
    /// Stable BTC height committed by the upstream balance-history snapshot.
    pub stable_height: u32,
    /// Stable BTC block hash paired with `stable_height`.
    pub stable_block_hash: String,
    /// Fixed lag rule used when interpreting `stable_height`.
    pub stable_lag: u32,
    /// Externally visible RPC/API version of balance-history.
    pub balance_history_api_version: String,
    /// Historical query semantics version of balance-history.
    pub balance_history_semantics_version: String,
    /// Version of the usdb-index derived-state formula set.
    pub usdb_index_formula_version: String,
    /// Version of the usdb-index external protocol contract.
    pub usdb_index_protocol_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalStatePassCommitIdentity {
    /// Block height of the latest local pass commit included in this state snapshot.
    pub block_height: u32,
    /// Rolling local pass block commit at `block_height`.
    pub block_commit: String,
    /// Version of the pass-commit protocol used to derive `block_commit`.
    pub commit_protocol_version: String,
    /// Hash algorithm used by the pass-commit protocol.
    pub commit_hash_algo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalStateActiveBalanceSnapshot {
    /// Exact block height of the committed active-balance snapshot.
    pub block_height: u32,
    /// Sum of balances of all active owners in satoshis.
    pub total_balance: u64,
    /// Number of active owners included in the snapshot.
    pub active_address_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalStateCommitIdentity {
    /// Upstream consensus snapshot id adopted by the local node.
    pub upstream_snapshot_id: String,
    /// Latest local durable height committed by usdb-indexer.
    pub local_synced_block_height: u32,
    /// Latest local pass commit at or before `local_synced_block_height`.
    pub latest_pass_block_commit: Option<LocalStatePassCommitIdentity>,
    /// Exact active-balance snapshot at `local_synced_block_height`, when present.
    pub latest_active_balance_snapshot: Option<LocalStateActiveBalanceSnapshot>,
    /// Version of the external usdb-index protocol contract.
    pub usdb_index_protocol_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemStateIdentity {
    /// Upstream consensus snapshot id currently adopted by usdb-indexer.
    pub upstream_snapshot_id: String,
    /// Canonical local-state commit currently durable on the node.
    pub local_state_commit: String,
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

fn update_string_component(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u32).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn update_optional_marker(hasher: &mut Sha256, present: bool) {
    hasher.update([if present { 1 } else { 0 }]);
}

pub fn build_consensus_snapshot_id(identity: &ConsensusSnapshotIdentity) -> String {
    let mut hasher = Sha256::new();
    update_string_component(&mut hasher, CONSENSUS_SNAPSHOT_ID_VERSION);
    update_string_component(&mut hasher, &identity.source_chain);
    update_string_component(&mut hasher, &identity.network);
    hasher.update(identity.stable_height.to_be_bytes());
    update_string_component(&mut hasher, &identity.stable_block_hash);
    hasher.update(identity.stable_lag.to_be_bytes());
    update_string_component(&mut hasher, &identity.balance_history_api_version);
    update_string_component(&mut hasher, &identity.balance_history_semantics_version);
    update_string_component(&mut hasher, &identity.usdb_index_formula_version);
    update_string_component(&mut hasher, &identity.usdb_index_protocol_version);
    encode_hex(&hasher.finalize())
}

/// Builds the canonical usdb-index local-state commit for one locally durable snapshot.
///
/// The commit intentionally binds together:
/// - the adopted upstream consensus snapshot id
/// - the local durable synced height
/// - the latest pass commit at or before that height
/// - the exact active-balance snapshot at that height
///
/// This keeps `snapshot_id` focused on upstream consensus, while `local_state_commit`
/// answers "what locally derived usdb-index state is durable on this node right now".
pub fn build_local_state_commit(identity: &LocalStateCommitIdentity) -> String {
    let mut hasher = Sha256::new();
    update_string_component(&mut hasher, LOCAL_STATE_COMMIT_VERSION);
    update_string_component(&mut hasher, &identity.upstream_snapshot_id);
    hasher.update(identity.local_synced_block_height.to_be_bytes());

    update_optional_marker(&mut hasher, identity.latest_pass_block_commit.is_some());
    if let Some(pass_commit) = &identity.latest_pass_block_commit {
        hasher.update(pass_commit.block_height.to_be_bytes());
        update_string_component(&mut hasher, &pass_commit.block_commit);
        update_string_component(&mut hasher, &pass_commit.commit_protocol_version);
        update_string_component(&mut hasher, &pass_commit.commit_hash_algo);
    }

    update_optional_marker(
        &mut hasher,
        identity.latest_active_balance_snapshot.is_some(),
    );
    if let Some(snapshot) = &identity.latest_active_balance_snapshot {
        hasher.update(snapshot.block_height.to_be_bytes());
        hasher.update(snapshot.total_balance.to_be_bytes());
        hasher.update(snapshot.active_address_count.to_be_bytes());
    }

    update_string_component(&mut hasher, &identity.usdb_index_protocol_version);
    encode_hex(&hasher.finalize())
}

/// Builds the canonical BTC-side system-state id consumed by downstream users such as ETHW.
///
/// This intentionally stays minimal: downstream systems only need one stable hash that binds
/// together the adopted upstream snapshot and the current local durable state derived from it.
pub fn build_system_state_id(identity: &SystemStateIdentity) -> String {
    let mut hasher = Sha256::new();
    update_string_component(&mut hasher, SYSTEM_STATE_ID_VERSION);
    update_string_component(&mut hasher, &identity.upstream_snapshot_id);
    update_string_component(&mut hasher, &identity.local_state_commit);
    encode_hex(&hasher.finalize())
}

pub struct OutPointCodec;

pub const OUTPOINT_SIZE: usize = 36;

impl OutPointCodec {
    pub fn encode(outpoint: &OutPoint) -> [u8; OUTPOINT_SIZE] {
        let mut key = [0u8; OUTPOINT_SIZE];
        key[..32].copy_from_slice(outpoint.txid.as_ref());
        key[32..36].copy_from_slice(&outpoint.vout.to_be_bytes());
        key
    }

    pub fn decode(data: &[u8]) -> Result<OutPoint, String> {
        if data.len() != OUTPOINT_SIZE {
            return Err("Invalid data length".to_string());
        }

        let txid = Txid::from_slice(&data[0..32]).map_err(|e| e.to_string())?;
        let vout = u32::from_be_bytes(
            data[32..36]
                .try_into()
                .map_err(|_| "Failed to parse vout".to_string())?,
        );
        Ok(OutPoint { txid, vout })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_consensus_snapshot_id_is_stable() {
        let identity = ConsensusSnapshotIdentity {
            source_chain: CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
            network: "regtest".to_string(),
            stable_height: 100,
            stable_block_hash: "aa".repeat(32),
            stable_lag: 0,
            balance_history_api_version: "1.0.0".to_string(),
            balance_history_semantics_version: "balance-snapshot-at-or-before:v1".to_string(),
            usdb_index_formula_version: "pass-energy-formula:v1".to_string(),
            usdb_index_protocol_version: "1.0.0".to_string(),
        };

        let a = build_consensus_snapshot_id(&identity);
        let b = build_consensus_snapshot_id(&identity);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn test_build_local_state_commit_changes_with_component_changes() {
        let base = LocalStateCommitIdentity {
            upstream_snapshot_id: "11".repeat(32),
            local_synced_block_height: 123,
            latest_pass_block_commit: Some(LocalStatePassCommitIdentity {
                block_height: 120,
                block_commit: "22".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            }),
            latest_active_balance_snapshot: Some(LocalStateActiveBalanceSnapshot {
                block_height: 123,
                total_balance: 5_000,
                active_address_count: 2,
            }),
            usdb_index_protocol_version: "1.0.0".to_string(),
        };

        let mut changed = base.clone();
        changed.latest_active_balance_snapshot = Some(LocalStateActiveBalanceSnapshot {
            block_height: 123,
            total_balance: 6_000,
            active_address_count: 2,
        });

        let base_commit = build_local_state_commit(&base);
        let changed_commit = build_local_state_commit(&changed);
        assert_eq!(base_commit.len(), 64);
        assert_eq!(changed_commit.len(), 64);
        assert_ne!(base_commit, changed_commit);
    }

    #[test]
    fn test_build_system_state_id_changes_with_local_state_commit() {
        let base = SystemStateIdentity {
            upstream_snapshot_id: "aa".repeat(32),
            local_state_commit: "bb".repeat(32),
        };
        let changed = SystemStateIdentity {
            upstream_snapshot_id: "aa".repeat(32),
            local_state_commit: "cc".repeat(32),
        };

        let base_id = build_system_state_id(&base);
        let changed_id = build_system_state_id(&changed);
        assert_eq!(base_id.len(), 64);
        assert_eq!(changed_id.len(), 64);
        assert_ne!(base_id, changed_id);
    }

    #[test]
    fn test_consensus_rpc_error_code_contract_is_stable() {
        assert_eq!(
            ConsensusRpcErrorCode::HeightNotSynced.code(),
            CONSENSUS_RPC_ERR_HEIGHT_NOT_SYNCED
        );
        assert_eq!(
            ConsensusRpcErrorCode::HeightNotSynced.as_str(),
            "HEIGHT_NOT_SYNCED"
        );
        assert_eq!(
            ConsensusRpcErrorCode::SnapshotNotReady.code(),
            CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY
        );
        assert_eq!(
            ConsensusRpcErrorCode::SnapshotIdMismatch.code(),
            CONSENSUS_RPC_ERR_SNAPSHOT_ID_MISMATCH
        );
        assert_eq!(
            ConsensusRpcErrorCode::BlockHashMismatch.code(),
            CONSENSUS_RPC_ERR_BLOCK_HASH_MISMATCH
        );
        assert_eq!(
            ConsensusRpcErrorCode::VersionMismatch.code(),
            CONSENSUS_RPC_ERR_VERSION_MISMATCH
        );
        assert_eq!(
            ConsensusRpcErrorCode::LocalStateCommitMismatch.code(),
            CONSENSUS_RPC_ERR_LOCAL_STATE_COMMIT_MISMATCH
        );
        assert_eq!(
            ConsensusRpcErrorCode::SystemStateIdMismatch.code(),
            CONSENSUS_RPC_ERR_SYSTEM_STATE_ID_MISMATCH
        );
        assert_eq!(
            ConsensusRpcErrorCode::NoRecord.code(),
            CONSENSUS_RPC_ERR_NO_RECORD
        );
        assert_eq!(ConsensusRpcErrorCode::NoRecord.as_str(), "NO_RECORD");
    }

    #[test]
    fn test_consensus_state_reference_is_empty() {
        let empty = ConsensusStateReference::default();
        assert!(empty.is_empty());

        let non_empty = ConsensusStateReference {
            snapshot_id: Some("aa".repeat(32)),
            ..Default::default()
        };
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_consensus_rpc_error_data_new_sets_service() {
        let data = ConsensusRpcErrorData::new("balance-history");
        assert_eq!(data.service, "balance-history");
        assert!(data.expected_state.is_empty());
        assert!(data.actual_state.is_empty());
    }
}
