use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};
use usdb_util::{
    ActiveVersionSet, CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION,
    ConsensusQueryContext, ConsensusSnapshotIdentity, ConsensusStateReference,
    LOCAL_STATE_COMMIT_HASH_ALGO, LocalStateActiveBalanceSnapshot, LocalStateCommitIdentity,
    LocalStatePassCommitIdentity, SYSTEM_STATE_ID_HASH_ALGO, SYSTEM_STATE_ID_VERSION,
    SystemStateIdentity, USDB_CANDIDATE_SET_SELECTION_RULE,
};
pub use usdb_util::{
    USDB_ECONOMIC_PAGE_MAX_LIMIT, USDB_ECONOMIC_STATE_VIEW_VERSION, USDB_INDEXER_API_VERSION,
    USDB_INDEXER_FEATURE_CANDIDATE_SET_VIEW, USDB_INDEXER_FEATURE_COLLAB_BREAKDOWN,
    USDB_INDEXER_FEATURE_HISTORICAL_STATE_REF, USDB_INDEXER_FEATURE_MINER_ECONOMIC_AGGREGATE,
    USDB_INDEXER_FEATURE_PASS_ECONOMIC_PROFILE,
};

/// Business error code returned when the requested height is above local durable sync progress.
pub const ERR_HEIGHT_NOT_SYNCED: i64 = -32010;
/// Business error code returned when a pass snapshot cannot be found at the requested height.
pub const ERR_PASS_NOT_FOUND: i64 = -32011;
/// Business error code returned when no energy record can be resolved for the requested pass/height.
pub const ERR_ENERGY_NOT_FOUND: i64 = -32012;
/// Business error code returned when an exact active-balance snapshot is missing.
pub const ERR_SNAPSHOT_NOT_FOUND: i64 = -32013;
/// Business error code returned when history invariants imply more than one active pass per owner.
pub const ERR_DUPLICATE_ACTIVE_OWNER: i64 = -32014;
/// Business error code returned when pagination arguments are invalid.
pub const ERR_INVALID_PAGINATION: i64 = -32015;
/// Business error code returned when a closed height range is malformed.
pub const ERR_INVALID_HEIGHT_RANGE: i64 = -32016;
/// Business error code returned when internal state invariants are violated during RPC resolution.
pub const ERR_INTERNAL_INVARIANT_BROKEN: i64 = -32017;
/// Deterministic candidate-set ordering rule for the first UIP-0006 view.
pub const CANDIDATE_SET_SELECTION_RULE: &str = USDB_CANDIDATE_SET_SELECTION_RULE;
/// Hash algorithm name used when deriving `IndexerSnapshotInfo.snapshot_id`.
pub const SNAPSHOT_ID_HASH_ALGO: &str = CONSENSUS_SNAPSHOT_ID_HASH_ALGO;
/// Version tag of the consensus snapshot-id derivation rule exposed by the RPC layer.
pub const SNAPSHOT_ID_VERSION: &str = CONSENSUS_SNAPSHOT_ID_VERSION;
/// Hash algorithm name used when deriving `LocalStateCommitInfo.local_state_commit`.
pub const LOCAL_STATE_HASH_ALGO: &str = LOCAL_STATE_COMMIT_HASH_ALGO;
/// Hash algorithm name used when deriving `SystemStateInfo.system_state_id`.
pub const SYSTEM_STATE_HASH_ALGO: &str = SYSTEM_STATE_ID_HASH_ALGO;
/// Version tag of the system-state id derivation rule exposed by the RPC layer.
pub const SYSTEM_STATE_VERSION: &str = SYSTEM_STATE_ID_VERSION;

/// Service metadata returned by `get_rpc_info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcInfo {
    /// Fixed service name, currently `usdb-indexer`.
    pub service: String,
    /// Public API version, for example `1.0.0`.
    pub api_version: String,
    /// Bitcoin network type, for example `mainnet` or `testnet`.
    pub network: String,
    /// Advertised capability list supported by this server instance.
    pub features: Vec<String>,
    /// UIP-0006 economic state view contract accepted by this server.
    pub economic_state_view_version: String,
    /// Deterministic candidate ordering rule implemented by this server.
    pub candidate_set_selection_rule: String,
    /// Maximum `limit` accepted by UIP-0006 cursor-paged methods.
    pub economic_page_max_limit: usize,
    /// Canonical identity of the activation registry embedded in this binary.
    pub activation_registry_id: String,
}

/// Runtime synchronization status of the indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSyncStatus {
    /// First block height included by protocol indexing.
    pub genesis_block_height: u32,
    /// Last block height fully committed by the indexer.
    pub synced_block_height: Option<u32>,
    /// Stable height currently exposed by balance-history and used as the indexer sync ceiling.
    pub balance_history_stable_height: Option<u32>,
    /// Current progress position for status display.
    pub current: u32,
    /// Total progress target for status display.
    pub total: u32,
    /// Optional human-readable status message.
    pub message: Option<String>,
}

/// Upstream snapshot metadata plus the local commit point that adopted it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSnapshotInfo {
    /// Local durable commit height in usdb-indexer when this anchor was adopted.
    /// This is local progress metadata only and is intentionally excluded from
    /// `snapshot_id`, which must stay stable across nodes observing the same
    /// upstream consensus snapshot.
    pub local_synced_block_height: u32,
    /// Upstream stable height reported by balance-history for the current upstream snapshot.
    /// This is the external snapshot ceiling, not a local usdb-indexer progress field.
    pub balance_history_stable_height: u32,
    /// Stable BTC block hash returned by balance-history for the current upstream snapshot.
    pub stable_block_hash: String,
    /// Latest logical block commit returned by balance-history for the current upstream snapshot.
    pub latest_block_commit: String,
    /// Shared consensus identity derived only from globally reproducible fields.
    pub consensus_identity: ConsensusSnapshotIdentity,
    /// Balance-history commit protocol version used for `latest_block_commit`.
    pub commit_protocol_version: String,
    /// Hash algorithm used by both upstream block commit and local snapshot id.
    pub commit_hash_algo: String,
    /// Canonical consensus snapshot id derived from `consensus_identity`.
    pub snapshot_id: String,
    /// Hash algorithm used to derive `snapshot_id`.
    pub snapshot_id_hash_algo: String,
    /// Version tag of the consensus snapshot id derivation rule.
    pub snapshot_id_version: String,
}

/// Normalized inputs required to derive one `IndexerSnapshotInfo`.
///
/// Keeping this seed separate avoids repeating the same consensus-identity and
/// snapshot-id assembly logic at multiple call sites.
#[derive(Debug, Clone)]
pub struct IndexerSnapshotInfoSeed {
    pub network: String,
    pub local_synced_block_height: u32,
    pub balance_history_stable_height: u32,
    pub stable_block_hash: String,
    pub latest_block_commit: String,
    pub stable_lag: u32,
    pub commit_protocol_version: String,
    pub commit_hash_algo: String,
}

impl From<IndexerSnapshotInfoSeed> for IndexerSnapshotInfo {
    fn from(seed: IndexerSnapshotInfoSeed) -> Self {
        let consensus_identity = ConsensusSnapshotIdentity {
            source_chain: usdb_util::CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
            network: seed.network,
            stable_height: seed.balance_history_stable_height,
            stable_block_hash: seed.stable_block_hash.clone(),
            stable_lag: seed.stable_lag,
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.to_string(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .to_string(),
        };
        let snapshot_id = usdb_util::build_consensus_snapshot_id(&consensus_identity);

        Self {
            local_synced_block_height: seed.local_synced_block_height,
            balance_history_stable_height: seed.balance_history_stable_height,
            stable_block_hash: seed.stable_block_hash,
            latest_block_commit: seed.latest_block_commit,
            consensus_identity,
            commit_protocol_version: seed.commit_protocol_version,
            commit_hash_algo: seed.commit_hash_algo,
            snapshot_id,
            snapshot_id_hash_algo: SNAPSHOT_ID_HASH_ALGO.to_string(),
            snapshot_id_version: SNAPSHOT_ID_VERSION.to_string(),
        }
    }
}

impl From<&IndexerSnapshotInfo> for ConsensusStateReference {
    fn from(snapshot: &IndexerSnapshotInfo) -> Self {
        Self {
            snapshot_id: Some(snapshot.snapshot_id.clone()),
            stable_height: Some(snapshot.balance_history_stable_height),
            stable_block_hash: Some(snapshot.stable_block_hash.clone()),
            balance_history_api_version: Some(
                snapshot
                    .consensus_identity
                    .balance_history_api_version
                    .clone(),
            ),
            balance_history_semantics_version: Some(
                snapshot
                    .consensus_identity
                    .balance_history_semantics_version
                    .clone(),
            ),
            activation_registry_id: None,
            active_version_set_id: None,
            local_state_commit: None,
            system_state_id: None,
        }
    }
}

/// Parameters for `get_pass_block_commit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassBlockCommitParams {
    /// Optional query height; `None` resolves to the current local synced height.
    pub block_height: Option<u32>,
}

/// Local pass block commit metadata resolved at one exact height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassBlockCommitInfo {
    /// Final query height resolved by the server.
    pub block_height: u32,
    /// Upstream balance-history height used as the external anchor.
    /// Pass commit v1 requires this to equal `block_height`; both are exposed so clients can see
    /// that the local commit is anchored to a specific upstream protocol source.
    pub balance_history_block_height: u32,
    /// Upstream balance-history logical block commit captured at the anchor height.
    /// This already commits to the upstream BTC block hash, so the pass commit RPC does not
    /// separately expose that hash unless a future protocol revision needs cross-height anchoring.
    pub balance_history_block_commit: String,
    /// Hash of the normalized local pass mutation stream for this block.
    pub mutation_root: String,
    /// Rolling local pass block commit chained from previous local commit and upstream anchor.
    pub block_commit: String,
    /// Local pass commit protocol version used to interpret this row.
    pub commit_protocol_version: String,
    /// Hash algorithm used by both `mutation_root` and `block_commit`.
    pub commit_hash_algo: String,
}

/// Locally durable core-state commit anchored to one upstream snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalStateCommitInfo {
    /// Canonical identity of the activation registry embedded in this node.
    pub activation_registry_id: String,
    /// Full version set selected at `local_synced_block_height`.
    pub active_version_set: ActiveVersionSet,
    /// Canonical identity of `active_version_set`.
    pub active_version_set_id: String,
    /// Local durable synced height represented by this state commit.
    pub local_synced_block_height: u32,
    /// Upstream consensus snapshot id used when deriving this local state.
    pub upstream_snapshot_id: String,
    /// Latest pass block commit at or before `local_synced_block_height`.
    pub latest_pass_block_commit: Option<LocalStatePassCommitIdentity>,
    /// Exact active-balance snapshot at `local_synced_block_height`, when present.
    pub latest_active_balance_snapshot: Option<LocalStateActiveBalanceSnapshot>,
    /// Shared identity struct used as the canonical hash input.
    pub local_state_identity: LocalStateCommitIdentity,
    /// Canonical local-state commit derived from `local_state_identity`.
    pub local_state_commit: String,
    /// Hash algorithm used to derive `local_state_commit`.
    pub local_state_commit_hash_algo: String,
    /// Version tag of the local-state commit derivation rule.
    pub local_state_commit_version: String,
}

/// Normalized inputs required to derive one `LocalStateCommitInfo`.
#[derive(Debug, Clone)]
pub struct LocalStateCommitInfoSeed {
    pub activation_registry_id: String,
    pub active_version_set: ActiveVersionSet,
    pub commit_protocol_version: String,
    pub local_synced_block_height: u32,
    pub upstream_snapshot_id: String,
    pub latest_pass_block_commit: Option<LocalStatePassCommitIdentity>,
    pub latest_active_balance_snapshot: Option<LocalStateActiveBalanceSnapshot>,
}

impl From<LocalStateCommitInfoSeed> for LocalStateCommitInfo {
    fn from(seed: LocalStateCommitInfoSeed) -> Self {
        let active_version_set_id = seed.active_version_set.active_version_set_id();
        let local_state_identity = LocalStateCommitIdentity {
            commit_protocol_version: seed.commit_protocol_version.clone(),
            upstream_snapshot_id: seed.upstream_snapshot_id.clone(),
            active_version_set_id: active_version_set_id.clone(),
            local_synced_block_height: seed.local_synced_block_height,
            latest_pass_block_commit: seed.latest_pass_block_commit.clone(),
            latest_active_balance_snapshot: seed.latest_active_balance_snapshot.clone(),
        };
        let local_state_commit = usdb_util::build_local_state_commit(&local_state_identity);

        Self {
            activation_registry_id: seed.activation_registry_id,
            active_version_set: seed.active_version_set,
            active_version_set_id,
            local_synced_block_height: seed.local_synced_block_height,
            upstream_snapshot_id: seed.upstream_snapshot_id,
            latest_pass_block_commit: seed.latest_pass_block_commit,
            latest_active_balance_snapshot: seed.latest_active_balance_snapshot,
            local_state_identity,
            local_state_commit,
            local_state_commit_hash_algo: LOCAL_STATE_HASH_ALGO.to_string(),
            local_state_commit_version: seed.commit_protocol_version,
        }
    }
}

impl From<&LocalStateCommitInfo> for ConsensusStateReference {
    fn from(local_state: &LocalStateCommitInfo) -> Self {
        Self {
            snapshot_id: Some(local_state.upstream_snapshot_id.clone()),
            stable_height: None,
            stable_block_hash: None,
            balance_history_api_version: None,
            balance_history_semantics_version: None,
            activation_registry_id: Some(local_state.activation_registry_id.clone()),
            active_version_set_id: Some(local_state.active_version_set_id.clone()),
            local_state_commit: Some(local_state.local_state_commit.clone()),
            system_state_id: None,
        }
    }
}

/// Single top-level system-state id for downstream USDB-chain consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStateInfo {
    /// Canonical identity of the activation registry embedded in this node.
    pub activation_registry_id: String,
    /// Full version set selected at `local_synced_block_height`.
    pub active_version_set: ActiveVersionSet,
    /// Canonical identity of `active_version_set`.
    pub active_version_set_id: String,
    /// Local durable synced height represented by this system state.
    pub local_synced_block_height: u32,
    /// Upstream consensus snapshot id currently adopted by the node.
    pub upstream_snapshot_id: String,
    /// Current locally durable core-state commit anchored to that upstream snapshot.
    pub local_state_commit: String,
    /// Shared identity struct used as the canonical system-state hash input.
    pub system_state_identity: SystemStateIdentity,
    /// Canonical system-state id derived from `system_state_identity`.
    pub system_state_id: String,
    /// Hash algorithm used to derive `system_state_id`.
    pub system_state_id_hash_algo: String,
    /// Version tag of the system-state id derivation rule.
    pub system_state_id_version: String,
}

impl From<&LocalStateCommitInfo> for SystemStateInfo {
    fn from(local_state: &LocalStateCommitInfo) -> Self {
        let system_state_identity = SystemStateIdentity {
            upstream_snapshot_id: local_state.upstream_snapshot_id.clone(),
            local_state_commit: local_state.local_state_commit.clone(),
        };
        let system_state_id = usdb_util::build_system_state_id(&system_state_identity);

        Self {
            activation_registry_id: local_state.activation_registry_id.clone(),
            active_version_set: local_state.active_version_set.clone(),
            active_version_set_id: local_state.active_version_set_id.clone(),
            local_synced_block_height: local_state.local_synced_block_height,
            upstream_snapshot_id: local_state.upstream_snapshot_id.clone(),
            local_state_commit: local_state.local_state_commit.clone(),
            system_state_identity,
            system_state_id,
            system_state_id_hash_algo: SYSTEM_STATE_HASH_ALGO.to_string(),
            system_state_id_version: SYSTEM_STATE_VERSION.to_string(),
        }
    }
}

impl From<&SystemStateInfo> for ConsensusStateReference {
    fn from(system_state: &SystemStateInfo) -> Self {
        Self {
            snapshot_id: Some(system_state.upstream_snapshot_id.clone()),
            stable_height: None,
            stable_block_hash: None,
            balance_history_api_version: None,
            balance_history_semantics_version: None,
            activation_registry_id: Some(system_state.activation_registry_id.clone()),
            active_version_set_id: Some(system_state.active_version_set_id.clone()),
            local_state_commit: Some(system_state.local_state_commit.clone()),
            system_state_id: Some(system_state.system_state_id.clone()),
        }
    }
}

/// Parameters for resolving the historical state reference at one exact BTC height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetStateRefAtHeightParams {
    /// Exact BTC block height whose locally durable historical state should be returned.
    pub block_height: u32,
    /// Optional consensus selectors pinned by the caller for exact historical validation.
    ///
    /// This lets downstream validators re-check a historical `(height, state)`
    /// tuple recorded in a USDB block instead of accepting any state ref that
    /// happens to be reconstructable for the same height.
    pub context: Option<ConsensusQueryContext>,
}

/// Historical `usdb-indexer` state reference reconstructed for one exact BTC height.
///
/// This object intentionally separates current-head introspection from
/// historical validation: downstream consumers can pin validation to the exact
/// upstream snapshot, local-state commit, and system-state id observed at one
/// durable height without depending on the node's current head.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalStateRefInfo {
    /// Exact BTC block height whose durable local state is being described.
    pub block_height: u32,
    /// Historical upstream snapshot view adopted at `block_height`.
    pub snapshot_info: IndexerSnapshotInfo,
    /// Historical locally durable core-state commit at `block_height`.
    pub local_state_commit_info: LocalStateCommitInfo,
    /// Historical top-level system-state id at `block_height`.
    pub system_state_info: SystemStateInfo,
}

/// Normalized inputs required to derive one `HistoricalStateRefInfo`.
///
/// Keeping this seed explicit makes it easier to audit which three exact
/// historical sub-views are bundled into one validator-facing state ref.
#[derive(Debug, Clone)]
pub struct HistoricalStateRefInfoSeed {
    pub block_height: u32,
    pub snapshot_info: IndexerSnapshotInfo,
    pub local_state_commit_info: LocalStateCommitInfo,
    pub system_state_info: SystemStateInfo,
}

impl From<HistoricalStateRefInfoSeed> for HistoricalStateRefInfo {
    fn from(seed: HistoricalStateRefInfoSeed) -> Self {
        Self {
            block_height: seed.block_height,
            snapshot_info: seed.snapshot_info,
            local_state_commit_info: seed.local_state_commit_info,
            system_state_info: seed.system_state_info,
        }
    }
}

impl From<&HistoricalStateRefInfo> for ConsensusStateReference {
    fn from(state_ref: &HistoricalStateRefInfo) -> Self {
        let mut reference = ConsensusStateReference::from(&state_ref.snapshot_info);
        reference.activation_registry_id = Some(
            state_ref
                .local_state_commit_info
                .activation_registry_id
                .clone(),
        );
        reference.active_version_set_id = Some(
            state_ref
                .local_state_commit_info
                .active_version_set_id
                .clone(),
        );
        reference.local_state_commit =
            Some(state_ref.local_state_commit_info.local_state_commit.clone());
        reference.system_state_id = Some(state_ref.system_state_info.system_state_id.clone());
        reference
    }
}

/// Exact historical state identity attached to every UIP-0006 economic view.
///
/// All fields are required so downstream validators can reconstruct a
/// `ConsensusQueryContext` without consulting the current service head.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EconomicExternalState {
    /// Exact BTC height used by the economic query.
    pub btc_height: u32,
    /// Historical balance-history consensus snapshot id.
    pub snapshot_id: String,
    /// Stable BTC block hash committed by `snapshot_id`.
    pub stable_block_hash: String,
    /// Historical usdb-indexer local durable state commit.
    pub local_state_commit: String,
    /// Historical top-level system state id.
    pub system_state_id: String,
    /// Historical balance-history public API version.
    pub balance_history_api_version: String,
    /// Historical balance query semantics version.
    pub balance_history_semantics_version: String,
    /// Canonical identity of the activation registry embedded in this node.
    pub activation_registry_id: String,
    /// Full version set selected at `btc_height`.
    pub active_version_set: ActiveVersionSet,
    /// Canonical identity of `active_version_set`.
    pub active_version_set_id: String,
}

impl From<&HistoricalStateRefInfo> for EconomicExternalState {
    fn from(state_ref: &HistoricalStateRefInfo) -> Self {
        let identity = &state_ref.snapshot_info.consensus_identity;
        Self {
            btc_height: state_ref.block_height,
            snapshot_id: state_ref.snapshot_info.snapshot_id.clone(),
            stable_block_hash: state_ref.snapshot_info.stable_block_hash.clone(),
            local_state_commit: state_ref.local_state_commit_info.local_state_commit.clone(),
            system_state_id: state_ref.system_state_info.system_state_id.clone(),
            balance_history_api_version: identity.balance_history_api_version.clone(),
            balance_history_semantics_version: identity.balance_history_semantics_version.clone(),
            activation_registry_id: state_ref
                .local_state_commit_info
                .activation_registry_id
                .clone(),
            active_version_set: state_ref.local_state_commit_info.active_version_set.clone(),
            active_version_set_id: state_ref
                .local_state_commit_info
                .active_version_set_id
                .clone(),
        }
    }
}

impl From<&EconomicExternalState> for ConsensusStateReference {
    fn from(external_state: &EconomicExternalState) -> Self {
        Self {
            snapshot_id: Some(external_state.snapshot_id.clone()),
            stable_height: Some(external_state.btc_height),
            stable_block_hash: Some(external_state.stable_block_hash.clone()),
            balance_history_api_version: Some(external_state.balance_history_api_version.clone()),
            balance_history_semantics_version: Some(
                external_state.balance_history_semantics_version.clone(),
            ),
            activation_registry_id: Some(external_state.activation_registry_id.clone()),
            active_version_set_id: Some(external_state.active_version_set_id.clone()),
            local_state_commit: Some(external_state.local_state_commit.clone()),
            system_state_id: Some(external_state.system_state_id.clone()),
        }
    }
}

impl From<&EconomicExternalState> for ConsensusQueryContext {
    fn from(external_state: &EconomicExternalState) -> Self {
        Self {
            requested_height: Some(external_state.btc_height),
            expected_state: ConsensusStateReference::from(external_state),
        }
    }
}

/// Machine-readable blockers that keep usdb-indexer from a stricter ready state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadinessBlocker {
    /// RPC listener is not yet serving requests, so even liveness is not established.
    RpcNotListening,
    /// Shutdown has been requested and the node is draining toward exit.
    ShutdownRequested,
    /// No durable local synced height exists yet.
    SyncedHeightMissing,
    /// Local durable state is behind the latest upstream stable snapshot.
    CatchingUp,
    /// Upstream balance-history readiness has not been observed yet.
    UpstreamReadinessUnknown,
    /// Upstream balance-history is reachable but not consensus-ready.
    UpstreamConsensusNotReady,
    /// No adopted upstream snapshot anchor is currently available locally.
    UpstreamSnapshotMissing,
    /// Adopted upstream snapshot exists but is not aligned with the local durable height.
    UpstreamSnapshotHeightMismatch,
    /// Node is inside resumable upstream reorg recovery.
    ReorgRecoveryPending,
    /// Local core-state commit could not be built for the current durable height.
    LocalStateCommitMissing,
    /// Top-level system-state id could not be built for the current durable height.
    SystemStateMissing,
}

/// Structured readiness state for liveness, local queries, and downstream consensus use.
///
/// `rpc_alive` is plain liveness. `query_ready` means local RPC queries are
/// allowed against the node's current durable state. `consensus_ready` is
/// stricter and only becomes true when the node has a complete upstream
/// snapshot anchor, complete local/system commits, and no transient recovery
/// work is still pending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessInfo {
    /// Fixed service identifier, currently `usdb-indexer`.
    pub service: String,
    /// True once the RPC server is listening and able to answer requests.
    pub rpc_alive: bool,
    /// True when ordinary local query traffic is allowed.
    pub query_ready: bool,
    /// True only when the current system state is safe for downstream consensus use.
    pub consensus_ready: bool,
    /// Local durable synced height, when available.
    pub synced_block_height: Option<u32>,
    /// Latest upstream stable height observed from balance-history, when available.
    pub balance_history_stable_height: Option<u32>,
    /// Current adopted upstream snapshot id, when available.
    pub upstream_snapshot_id: Option<String>,
    /// Current locally durable core-state commit, when available.
    pub local_state_commit: Option<String>,
    /// Current top-level system-state id, when available.
    pub system_state_id: Option<String>,
    /// Current progress counter mirrored from sync status.
    pub current: u32,
    /// Total progress target mirrored from sync status.
    pub total: u32,
    /// Optional human-readable status message.
    pub message: Option<String>,
    /// Machine-readable reasons keeping the service from a stricter ready state.
    pub blockers: Vec<ReadinessBlocker>,
}

/// Parameters for `get_pass_snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassSnapshotParams {
    /// Target inscription id, for example `txidi0`.
    pub inscription_id: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    ///
    /// When present, the service validates the historical state reference at
    /// the resolved height before returning the pass snapshot.
    pub context: Option<ConsensusQueryContext>,
}

/// Pass snapshot resolved at a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassSnapshot {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Global inscription number from ordinals.
    pub inscription_number: i32,
    /// Mint transaction id.
    pub mint_txid: String,
    /// Mint block height.
    pub mint_block_height: u32,
    /// Mint owner script hash.
    pub mint_owner: String,
    /// Mint schema version parsed from the inscription payload.
    pub mint_version: u32,
    /// Pass kind parsed from UIP-0001 v1 fields.
    pub pass_kind: String,
    /// Primary USDB-chain account address declared in mint content.
    pub usdb_main: String,
    /// Fixed Leader pass binding for collab passes.
    pub leader_pass_id: Option<String>,
    /// Leader BTC address binding for collab passes.
    pub leader_btc_addr: Option<String>,
    /// Previous pass references used for inheritance.
    pub prev: Vec<String>,
    /// Invalid error code when pass is marked invalid.
    pub invalid_code: Option<String>,
    /// Human-readable invalid reason.
    pub invalid_reason: Option<String>,
    /// Owner script hash at resolved height.
    pub owner: String,
    /// Pass state at resolved height.
    pub state: String,
    /// Pass satpoint at resolved height.
    pub satpoint: String,
    /// Last history event id used to derive this snapshot.
    pub last_event_id: i64,
    /// Last history event type used to derive this snapshot.
    pub last_event_type: String,
    /// Final query height resolved by the server.
    pub resolved_height: u32,
}

/// Parameters for `get_active_passes_at_height`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetActivePassesAtHeightParams {
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// Single active pass item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePassItem {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Current owner script hash.
    pub owner: String,
}

/// Paged active-pass response for a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePassesAtHeight {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total number of active passes at this height.
    pub total: u64,
    /// Active pass rows in the requested page.
    pub items: Vec<ActivePassItem>,
}

/// Parameters for `get_pass_stats_at_height`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassStatsAtHeightParams {
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
}

/// Aggregated pass-state statistics resolved at a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassStatsAtHeight {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total number of passes visible at this height.
    pub total_count: u64,
    /// Number of passes in `active` state.
    pub active_count: u64,
    /// Number of passes in `dormant` state.
    pub dormant_count: u64,
    /// Number of passes in `consumed` state.
    pub consumed_count: u64,
    /// Number of passes in `burned` state.
    pub burned_count: u64,
    /// Number of passes in `invalid` state.
    pub invalid_count: u64,
}

/// Parameters for `get_pass_history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassHistoryParams {
    /// Target inscription id.
    pub inscription_id: String,
    /// Inclusive range start height.
    pub from_height: u32,
    /// Inclusive range end height.
    pub to_height: u32,
    /// Optional order, `asc` or `desc`.
    pub order: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One pass history event row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassHistoryEvent {
    /// Monotonic history event id.
    pub event_id: i64,
    /// Pass inscription id.
    pub inscription_id: String,
    /// Block height where this event happened.
    pub block_height: u32,
    /// Event type, for example `mint` or `owner_transfer`.
    pub event_type: String,
    /// Pass state after this event is applied.
    pub state: String,
    /// Pass owner after this event is applied.
    pub owner: String,
    /// Pass satpoint after this event is applied.
    pub satpoint: String,
}

/// Paged pass history response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassHistoryPage {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total history rows in the requested closed range.
    pub total: u64,
    /// History rows in requested page.
    pub items: Vec<PassHistoryEvent>,
}

/// Parameters for `get_owner_active_pass_at_height`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetOwnerActivePassAtHeightParams {
    /// Target owner script hash.
    pub owner: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
}

/// Parameters for `get_owner_passes_at_height`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetOwnerPassesAtHeightParams {
    /// Target owner script hash.
    pub owner: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
    /// Optional state filter. Empty or absent means all pass states.
    pub states: Option<Vec<String>>,
    /// Optional order by latest event height, `desc` by default.
    pub order: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One pass currently owned by an owner at a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerPassItem {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Global inscription number.
    pub inscription_number: i32,
    /// Mint block height.
    pub mint_block_height: u32,
    /// Owner script hash at the resolved height.
    pub owner: String,
    /// Pass state at the resolved height.
    pub state: String,
    /// Latest event height that produced this state snapshot.
    pub latest_event_height: u32,
    /// Mint schema version parsed from the inscription payload.
    pub mint_version: u32,
    /// Pass kind parsed from UIP-0001 v1 fields.
    pub pass_kind: String,
    /// Primary USDB-chain account address declared by the pass mint content.
    pub usdb_main: String,
    /// Fixed Leader pass binding for collab passes.
    pub leader_pass_id: Option<String>,
    /// Leader BTC address binding for collab passes.
    pub leader_btc_addr: Option<String>,
    /// Current satpoint at the resolved height.
    pub satpoint: String,
}

/// Paged owner-pass response for a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerPassesAtHeight {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Target owner script hash.
    pub owner: String,
    /// Total number of matching passes.
    pub total: u64,
    /// Matching pass rows in the requested page.
    pub items: Vec<OwnerPassItem>,
}

/// Parameters for `get_recent_passes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRecentPassesParams {
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
    /// Optional state filter. Empty or absent means all pass states.
    pub states: Option<Vec<String>>,
    /// Optional order by mint height, `desc` by default.
    pub order: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One recently minted pass resolved at a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentPassItem {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Global inscription number.
    pub inscription_number: i32,
    /// Mint block height.
    pub mint_block_height: u32,
    /// Owner script hash at the resolved height.
    pub owner: String,
    /// Pass state at the resolved height.
    pub state: String,
    /// Latest event height that produced this state snapshot.
    pub latest_event_height: u32,
    /// Mint schema version parsed from the inscription payload.
    pub mint_version: u32,
    /// Pass kind parsed from UIP-0001 v1 fields.
    pub pass_kind: String,
    /// Primary USDB-chain account address declared by the pass mint content.
    pub usdb_main: String,
    /// Fixed Leader pass binding for collab passes.
    pub leader_pass_id: Option<String>,
    /// Leader BTC address binding for collab passes.
    pub leader_btc_addr: Option<String>,
    /// Current satpoint at the resolved height.
    pub satpoint: String,
}

/// Paged recent-pass response for a target height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentPassesPage {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total number of matching passes.
    pub total: u64,
    /// Matching recent pass rows in the requested page.
    pub items: Vec<RecentPassItem>,
}

/// Parameters for `get_pass_energy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyParams {
    /// Target inscription id.
    pub inscription_id: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub block_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    ///
    /// When present, the service validates the historical state reference at
    /// the resolved height before returning the energy snapshot.
    pub context: Option<ConsensusQueryContext>,
    /// Query mode:
    /// - `exact`: read only the record exactly at `block_height`.
    /// - `at_or_before`: read latest record at or before `block_height`,
    ///   then return projected latest energy at query height.
    pub mode: Option<String>,
}

/// Energy snapshot of one pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergySnapshot {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Height used by this query after resolution.
    pub query_block_height: u32,
    /// Height of the stored energy record returned.
    pub record_block_height: u32,
    /// Effective pass state at query height.
    /// For `mode=exact`, this equals record state.
    /// For `mode=at_or_before`, this is derived from latest record <= query height.
    pub state: String,
    /// Active base height used by energy formula.
    pub active_block_height: u32,
    /// Owner script hash in this energy record.
    pub owner_address: String,
    /// Owner BTC balance in satoshis for this record.
    pub owner_balance: u64,
    /// Balance delta in satoshis for this record.
    pub owner_delta: i64,
    /// Raw energy at query height, encoded as canonical decimal string.
    /// For `mode=exact`, this equals record energy.
    /// For `mode=at_or_before`, this is projected from the latest record <= query height.
    pub raw_energy: String,
    /// Collab contribution at query height, encoded as canonical decimal string.
    /// This is derived at query time and is not stored in the raw energy ledger.
    pub collab_contribution: String,
    /// Effective energy at query height, encoded as canonical decimal string.
    /// Active standard passes use `raw_energy + collab_contribution`; active collab
    /// and non-active passes resolve to `"0"`.
    pub effective_energy: String,
    /// UIP-0005 level derived from `effective_energy` at query height.
    pub level: u8,
    /// UIP-0005 difficulty factor derived from `level`, expressed in bps.
    pub difficulty_factor_bps: u64,
}

/// Parameters for `get_pass_energy_range`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyRangeParams {
    /// Target inscription id.
    pub inscription_id: String,
    /// Inclusive range start height.
    pub from_height: u32,
    /// Inclusive range end height.
    pub to_height: u32,
    /// Optional sort order, `asc` or `desc`. Defaults to `asc`.
    pub order: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One row in pass energy range response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyRangeItem {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Block height of this energy record.
    pub record_block_height: u32,
    /// Pass state in this record.
    pub state: String,
    /// Active base height used by formula.
    pub active_block_height: u32,
    /// Owner script hash in this record.
    pub owner_address: String,
    /// Owner balance in satoshis.
    pub owner_balance: u64,
    /// Owner balance delta in satoshis.
    pub owner_delta: i64,
    /// Raw energy value for this record, encoded as canonical decimal string.
    pub energy: String,
}

/// Paged pass energy range response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyRangePage {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total energy rows in the requested closed range.
    pub total: u64,
    /// Energy rows in requested page.
    pub items: Vec<PassEnergyRangeItem>,
}

/// Parameters for `get_pass_energy_leaderboard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyLeaderboardParams {
    /// Optional query height; `None` resolves to the current local synced height.
    pub at_height: Option<u32>,
    /// Optional leaderboard scope:
    /// - `active`: only active passes (default).
    /// - `active_dormant`: include active + dormant passes.
    /// - `all`: include all pass states.
    pub scope: Option<String>,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One row in energy leaderboard response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyLeaderboardItem {
    /// Pass inscription id.
    pub inscription_id: String,
    /// Owner script hash at resolved height.
    pub owner: String,
    /// Height of the latest energy record used for ranking.
    pub record_block_height: u32,
    /// Pass state in the latest energy record.
    pub state: String,
    /// Raw energy value used for ranking, encoded as canonical decimal string.
    pub energy: String,
}

/// Paged pass energy leaderboard response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyLeaderboardPage {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total number of ranked passes.
    pub total: u64,
    /// Leaderboard rows in requested page.
    pub items: Vec<PassEnergyLeaderboardItem>,
}

/// Parameters for `get_pass_economic_profile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEconomicProfileParams {
    /// Required UIP-0006 view contract version selector.
    pub view_version: String,
    /// Target pass inscription id.
    pub pass_id: String,
    /// Optional query height; `None` resolves from context or current local synced height.
    pub block_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    ///
    /// When present, the service validates the exact historical state identity
    /// before deriving any pass or energy field.
    pub context: Option<ConsensusQueryContext>,
}

/// One pass economic profile resolved under a UIP-0006 historical context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEconomicProfile {
    /// Pass inscription id.
    pub pass_id: String,
    /// Canonical owner script hash at the resolved height.
    pub owner_script_hash: String,
    /// Optional display BTC address when a reverse owner mapping is available.
    pub owner_btc_addr: Option<String>,
    /// Pass state at the resolved height.
    pub state: String,
    /// Pass kind at the resolved height.
    pub pass_kind: String,
    /// Standard-pass USDB reward address; absent for collab passes.
    pub usdb_main: Option<String>,
    /// UIP-0003 raw energy, encoded as canonical decimal string.
    pub raw_energy: String,
    /// UIP-0004 collab contribution, encoded as canonical decimal string.
    pub collab_contribution: String,
    /// UIP-0004 effective energy, encoded as canonical decimal string.
    pub effective_energy: String,
    /// UIP-0005 level derived from `effective_energy`.
    pub level: u8,
    /// UIP-0005 difficulty factor derived from `level`, expressed in bps.
    pub difficulty_factor_bps: u64,
    /// Number of collab passes included in `collab_contribution`.
    pub collab_breakdown_count: u64,
}

/// UIP-0006 economic state view for one pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEconomicProfileView {
    /// Economic state view version used by this response.
    pub view_version: String,
    /// Exact historical state identity used to derive the profile.
    pub external_state: EconomicExternalState,
    /// Pass snapshot and derived economic fields at `external_state.btc_height`.
    pub pass: PassEconomicProfile,
    /// Network-wide BTC miner aggregate from the same exact `external_state`.
    pub miner_aggregate: MinerEconomicAggregate,
}

/// Network-wide BTC miner aggregate at one UIP-0006 historical context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MinerEconomicAggregate {
    /// Sum of BTC balances for all unique Active Standard and Collab pass owners.
    pub total_miner_btc_sats: String,
    /// Number of unique Active Standard and Collab pass owners.
    pub active_miner_owner_count: u64,
}

/// Parameters for `get_miner_economic_aggregate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetMinerEconomicAggregateParams {
    /// Required UIP-0006 view contract version selector.
    pub view_version: String,
    /// Optional query height; `None` resolves from context or current local synced height.
    pub block_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    pub context: Option<ConsensusQueryContext>,
}

/// UIP-0006 historical view of the network-wide miner BTC aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerEconomicAggregateView {
    /// Economic state view version used by this response.
    pub view_version: String,
    /// Exact historical state identity used to derive the aggregate.
    pub external_state: EconomicExternalState,
    /// Aggregate committed by the same local state identity.
    pub miner_aggregate: MinerEconomicAggregate,
}

/// Parameters for `get_candidate_set_view`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetCandidateSetViewParams {
    /// Required UIP-0006 view contract version selector.
    pub view_version: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub block_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    ///
    /// When present, the service validates the historical state reference at
    /// the resolved height before returning the candidate set.
    pub context: Option<ConsensusQueryContext>,
    /// Optional selection rule. Defaults to `CANDIDATE_SET_SELECTION_RULE`.
    pub selection_rule: Option<String>,
    /// Opaque continuation returned by the preceding page.
    pub cursor: Option<String>,
    /// Positive page size, bound into the cursor for all subsequent pages.
    pub limit: usize,
}

/// One row in a UIP-0006 candidate set view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSetViewItem {
    /// Pass inscription id, used as the pass id in UIP-0006 ordering.
    pub pass_id: String,
    /// Owner script hash at the resolved height.
    pub owner_script_hash: String,
    /// Pass state at the resolved height. Candidate rows are always `active`.
    pub state: String,
    /// Pass kind at the resolved height. Candidate rows are always `standard`.
    pub pass_kind: String,
    /// Height of the raw energy record used for this candidate.
    pub record_block_height: u32,
    /// UIP-0003 raw energy at resolved height, encoded as canonical decimal string.
    pub raw_energy: String,
    /// UIP-0004 collab contribution at resolved height, encoded as canonical decimal string.
    pub collab_contribution: String,
    /// UIP-0004 effective energy at resolved height, encoded as canonical decimal string.
    pub effective_energy: String,
    /// UIP-0005 level derived from `effective_energy` at resolved height.
    pub level: u8,
    /// UIP-0005 difficulty factor derived from `level`, expressed in bps.
    pub difficulty_factor_bps: u64,
}

/// Paged UIP-0006 candidate set audit view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSetViewPage {
    /// Economic state view version used by this response.
    pub view_version: String,
    /// Exact historical state identity used to derive every row in this page.
    pub external_state: EconomicExternalState,
    /// Ordering contract used to derive the top-ranked audit item.
    pub selection_rule: String,
    /// Total number of active standard candidate passes.
    pub total: u64,
    /// Page size bound to this query and its continuation cursor.
    pub limit: usize,
    /// Maximum page size accepted by this server.
    pub max_limit: usize,
    /// Opaque continuation cursor, or `None` when this is the final page.
    pub next_cursor: Option<String>,
    /// Candidate rows in requested page.
    pub items: Vec<CandidateSetViewItem>,
}

/// Parameters for `get_collab_breakdown`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetCollabBreakdownParams {
    /// Required UIP-0006 view contract version selector.
    pub view_version: String,
    /// Leader standard pass inscription id.
    pub leader_pass_id: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub block_height: Option<u32>,
    /// Optional consensus selectors pinned by downstream validators.
    ///
    /// When present, the service validates the historical state reference at
    /// the resolved height before returning the breakdown.
    pub context: Option<ConsensusQueryContext>,
    /// Optional sort:
    /// - `collab_pass_id_asc`: canonical pass-id text ascending.
    /// - `contribution_desc_pass_id_asc`: contribution descending, then
    ///   canonical pass-id text ascending.
    pub sort: Option<String>,
    /// Opaque continuation returned by the preceding page.
    pub cursor: Option<String>,
    /// Positive page size, bound into the cursor for all subsequent pages.
    pub limit: usize,
}

/// One row in a collab contribution breakdown response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabBreakdownItem {
    /// Collab pass inscription id.
    pub collab_pass_id: String,
    /// Collab owner script hash at the resolved height.
    pub collab_owner_script_hash: String,
    /// Optional display BTC address when a reverse owner mapping is available.
    pub collab_owner_btc_addr: Option<String>,
    /// Height of the raw energy record used for this collab pass.
    pub record_block_height: u32,
    /// Collab raw energy at resolved height, encoded as canonical decimal string.
    pub collab_raw_energy: String,
    /// Collab weight in basis points.
    pub collab_weight_bps: u64,
    /// Collab contribution at resolved height, encoded as canonical decimal string.
    pub collab_contribution: String,
    /// Leader reference kind declared by the collab pass.
    pub leader_ref_kind: String,
    /// Original Leader reference value declared by the collab pass.
    pub leader_ref_value: String,
}

/// Paged collab contribution breakdown for one Leader pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollabBreakdownPage {
    /// Economic state view version used by this response.
    pub view_version: String,
    /// Exact historical state identity used to derive this breakdown.
    pub external_state: EconomicExternalState,
    /// Leader standard pass inscription id.
    pub leader_pass_id: String,
    /// Leader state at resolved height.
    pub leader_state: String,
    /// Leader pass kind at resolved height.
    pub leader_pass_kind: String,
    /// Sort rule used by this response.
    pub sort: String,
    /// Total number of collab passes contributing at this height.
    pub total: u64,
    /// Full aggregate collab contribution at resolved height.
    pub aggregate_collab_contribution: String,
    /// Page size bound to this query and its continuation cursor.
    pub limit: usize,
    /// Maximum page size accepted by this server.
    pub max_limit: usize,
    /// Opaque continuation cursor, or `None` when this is the final page.
    pub next_cursor: Option<String>,
    /// Breakdown rows in requested page.
    pub items: Vec<CollabBreakdownItem>,
}

/// Parameters for `get_invalid_passes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInvalidPassesParams {
    /// Optional invalid code filter.
    pub error_code: Option<String>,
    /// Inclusive range start height based on mint height.
    pub from_height: u32,
    /// Inclusive range end height based on mint height.
    pub to_height: u32,
    /// Zero-based page index.
    pub page: usize,
    /// Number of rows per page.
    pub page_size: usize,
}

/// One invalid pass row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidPassItem {
    /// Invalid pass inscription id.
    pub inscription_id: String,
    /// Global inscription number.
    pub inscription_number: i32,
    /// Mint transaction id.
    pub mint_txid: String,
    /// Mint block height.
    pub mint_block_height: u32,
    /// Mint owner script hash.
    pub mint_owner: String,
    /// Mint schema version parsed from the inscription payload.
    pub mint_version: u32,
    /// Pass kind parsed from UIP-0001 v1 fields.
    pub pass_kind: String,
    /// Primary USDB-chain account address in mint content.
    pub usdb_main: String,
    /// Fixed Leader pass binding for collab passes.
    pub leader_pass_id: Option<String>,
    /// Leader BTC address binding for collab passes.
    pub leader_btc_addr: Option<String>,
    /// Previous pass references from mint content.
    pub prev: Vec<String>,
    /// Invalid error code.
    pub invalid_code: Option<String>,
    /// Invalid reason message.
    pub invalid_reason: Option<String>,
    /// Current owner script hash.
    pub owner: String,
    /// Current state, expected to be `invalid`.
    pub state: String,
    /// Current satpoint.
    pub satpoint: String,
}

/// Paged invalid-pass response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidPassesPage {
    /// Final query height resolved by the server.
    pub resolved_height: u32,
    /// Total invalid-pass rows in the requested closed range.
    pub total: u64,
    /// Invalid pass rows in requested page.
    pub items: Vec<InvalidPassItem>,
}

/// JSON-RPC interface exposed by `usdb-indexer`.
#[rpc(server)]
pub trait UsdbIndexerRpc {
    /// Returns service metadata and feature flags.
    #[rpc(name = "get_rpc_info")]
    fn get_rpc_info(&self) -> JsonResult<RpcInfo>;

    /// Returns current network type.
    #[rpc(name = "get_network_type")]
    fn get_network_type(&self) -> JsonResult<String>;

    /// Returns indexer sync progress, local durable height, and upstream stable-height status.
    #[rpc(name = "get_sync_status")]
    fn get_sync_status(&self) -> JsonResult<IndexerSyncStatus>;

    /// Returns latest fully committed sync height.
    #[rpc(name = "get_synced_block_height")]
    fn get_synced_block_height(&self) -> JsonResult<Option<u64>>;

    /// Returns the current upstream snapshot metadata.
    ///
    /// Returns shared consensus error `SNAPSHOT_NOT_READY` when no adopted
    /// upstream snapshot anchor is currently available.
    #[rpc(name = "get_snapshot_info")]
    fn get_snapshot_info(&self) -> JsonResult<Option<IndexerSnapshotInfo>>;

    /// Returns local pass block commit metadata at one exact height.
    #[rpc(name = "get_pass_block_commit")]
    fn get_pass_block_commit(
        &self,
        params: GetPassBlockCommitParams,
    ) -> JsonResult<Option<PassBlockCommitInfo>>;

    /// Returns the current locally durable core-state commit.
    ///
    /// Returns shared consensus error `SNAPSHOT_NOT_READY` when the node does
    /// not yet have a complete adopted upstream snapshot anchor from which the
    /// current local state can be derived.
    #[rpc(name = "get_local_state_commit_info")]
    fn get_local_state_commit_info(&self) -> JsonResult<Option<LocalStateCommitInfo>>;

    /// Returns the top-level system-state id for downstream consumers.
    ///
    /// Returns shared consensus error `SNAPSHOT_NOT_READY` when the node does
    /// not yet have a complete current local/system state to expose.
    #[rpc(name = "get_system_state_info")]
    fn get_system_state_info(&self) -> JsonResult<Option<SystemStateInfo>>;

    /// Returns the exact historical upstream/local/system state reference at one BTC height.
    ///
    /// This endpoint is intended for downstream validators that need to
    /// re-check a previously produced block against the historical BTC-side
    /// state at `block_height`, not against the node's current head.
    ///
    /// Returns shared consensus error `HEIGHT_NOT_SYNCED` when `block_height`
    /// exceeds current durable sync progress.
    #[rpc(name = "get_state_ref_at_height")]
    fn get_state_ref_at_height(
        &self,
        params: GetStateRefAtHeightParams,
    ) -> JsonResult<HistoricalStateRefInfo>;

    /// Returns structured readiness state for liveness, local queries, and consensus use.
    ///
    /// Downstream callers must use `consensus_ready` instead of inferring
    /// readiness from `get_network_type` or from free-form sync messages.
    #[rpc(name = "get_readiness")]
    fn get_readiness(&self) -> JsonResult<ReadinessInfo>;

    /// Returns one pass snapshot at a target height.
    #[rpc(name = "get_pass_snapshot")]
    fn get_pass_snapshot(&self, params: GetPassSnapshotParams) -> JsonResult<Option<PassSnapshot>>;

    /// Returns active pass list at a target height with pagination.
    #[rpc(name = "get_active_passes_at_height")]
    fn get_active_passes_at_height(
        &self,
        params: GetActivePassesAtHeightParams,
    ) -> JsonResult<ActivePassesAtHeight>;

    /// Returns pass-state aggregate stats at a target height.
    #[rpc(name = "get_pass_stats_at_height")]
    fn get_pass_stats_at_height(
        &self,
        params: GetPassStatsAtHeightParams,
    ) -> JsonResult<PassStatsAtHeight>;

    /// Returns pass history events in a height range.
    #[rpc(name = "get_pass_history")]
    fn get_pass_history(&self, params: GetPassHistoryParams) -> JsonResult<PassHistoryPage>;

    /// Returns owner's active pass snapshot at a target height.
    #[rpc(name = "get_owner_active_pass_at_height")]
    fn get_owner_active_pass_at_height(
        &self,
        params: GetOwnerActivePassAtHeightParams,
    ) -> JsonResult<Option<PassSnapshot>>;

    /// Returns pass snapshots currently owned by an owner at a target height.
    #[rpc(name = "get_owner_passes_at_height")]
    fn get_owner_passes_at_height(
        &self,
        params: GetOwnerPassesAtHeightParams,
    ) -> JsonResult<OwnerPassesAtHeight>;

    /// Returns recently minted pass snapshots at a target height.
    #[rpc(name = "get_recent_passes")]
    fn get_recent_passes(&self, params: GetRecentPassesParams) -> JsonResult<RecentPassesPage>;

    /// Returns one pass energy snapshot.
    #[rpc(name = "get_pass_energy")]
    fn get_pass_energy(&self, params: GetPassEnergyParams) -> JsonResult<PassEnergySnapshot>;

    /// Returns pass energy timeline records in a height range.
    #[rpc(name = "get_pass_energy_range")]
    fn get_pass_energy_range(
        &self,
        params: GetPassEnergyRangeParams,
    ) -> JsonResult<PassEnergyRangePage>;

    /// Returns pass energy leaderboard at a target height.
    #[rpc(name = "get_pass_energy_leaderboard")]
    fn get_pass_energy_leaderboard(
        &self,
        params: GetPassEnergyLeaderboardParams,
    ) -> JsonResult<PassEnergyLeaderboardPage>;

    /// Returns one UIP-0006 pass economic profile at a historical context.
    #[rpc(name = "get_pass_economic_profile")]
    fn get_pass_economic_profile(
        &self,
        params: GetPassEconomicProfileParams,
    ) -> JsonResult<PassEconomicProfileView>;

    /// Returns the network-wide UIP-0006 miner BTC aggregate at a historical context.
    #[rpc(name = "get_miner_economic_aggregate")]
    fn get_miner_economic_aggregate(
        &self,
        params: GetMinerEconomicAggregateParams,
    ) -> JsonResult<MinerEconomicAggregateView>;

    /// Returns the UIP-0006 USDB-side candidate-set audit view at a target height.
    #[rpc(name = "get_candidate_set_view")]
    fn get_candidate_set_view(
        &self,
        params: GetCandidateSetViewParams,
    ) -> JsonResult<CandidateSetViewPage>;

    /// Returns a paged collab contribution breakdown for one Leader pass.
    #[rpc(name = "get_collab_breakdown")]
    fn get_collab_breakdown(
        &self,
        params: GetCollabBreakdownParams,
    ) -> JsonResult<CollabBreakdownPage>;

    /// Returns invalid passes with optional code filter.
    #[rpc(name = "get_invalid_passes")]
    fn get_invalid_passes(&self, params: GetInvalidPassesParams) -> JsonResult<InvalidPassesPage>;

    /// Triggers graceful shutdown of the indexer process.
    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}
