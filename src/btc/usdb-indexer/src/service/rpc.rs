use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, ConsensusQueryContext,
    ConsensusSnapshotIdentity, LOCAL_STATE_COMMIT_HASH_ALGO, LOCAL_STATE_COMMIT_VERSION,
    LocalStateActiveBalanceSnapshot, LocalStateCommitIdentity, LocalStatePassCommitIdentity,
    SYSTEM_STATE_ID_HASH_ALGO, SYSTEM_STATE_ID_VERSION, SystemStateIdentity,
    USDB_INDEX_FORMULA_VERSION as UTIL_USDB_INDEX_FORMULA_VERSION,
    USDB_INDEX_PROTOCOL_VERSION as UTIL_USDB_INDEX_PROTOCOL_VERSION,
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

pub const USDB_INDEX_FORMULA_VERSION: &str = UTIL_USDB_INDEX_FORMULA_VERSION;
pub const USDB_INDEX_PROTOCOL_VERSION: &str = UTIL_USDB_INDEX_PROTOCOL_VERSION;
/// Hash algorithm name used when deriving `IndexerSnapshotInfo.snapshot_id`.
pub const SNAPSHOT_ID_HASH_ALGO: &str = CONSENSUS_SNAPSHOT_ID_HASH_ALGO;
/// Version tag of the consensus snapshot-id derivation rule exposed by the RPC layer.
pub const SNAPSHOT_ID_VERSION: &str = CONSENSUS_SNAPSHOT_ID_VERSION;
/// Hash algorithm name used when deriving `LocalStateCommitInfo.local_state_commit`.
pub const LOCAL_STATE_HASH_ALGO: &str = LOCAL_STATE_COMMIT_HASH_ALGO;
/// Version tag of the local-state commit derivation rule exposed by the RPC layer.
pub const LOCAL_STATE_VERSION: &str = LOCAL_STATE_COMMIT_VERSION;
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

/// Single top-level system-state id for downstream consumers such as ETHW.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStateInfo {
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

/// Parameters for resolving the historical state reference at one exact BTC height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetStateRefAtHeightParams {
    /// Exact BTC block height whose locally durable historical state should be returned.
    pub block_height: u32,
    /// Optional consensus selectors pinned by the caller for exact historical validation.
    ///
    /// This lets downstream validators re-check a historical `(height, state)`
    /// tuple recorded in an ETHW block instead of accepting any state ref that
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
    /// Primary ETH address declared in mint content.
    pub eth_main: String,
    /// Optional collaborator ETH address.
    pub eth_collab: Option<String>,
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

/// Parameters for `get_pass_energy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyParams {
    /// Target inscription id.
    pub inscription_id: String,
    /// Optional query height; `None` resolves to the current local synced height.
    pub block_height: Option<u32>,
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
    /// Effective energy at query height.
    /// For `mode=exact`, this equals record energy.
    /// For `mode=at_or_before`, this is projected from the latest record <= query height.
    pub energy: u64,
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
    /// Energy value for this record.
    pub energy: u64,
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
    /// Energy value used for ranking.
    pub energy: u64,
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

/// Parameters for `get_active_balance_snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetActiveBalanceSnapshotParams {
    /// Exact block height of the requested snapshot.
    pub block_height: u32,
}

/// Active address total-balance snapshot at one height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcActiveBalanceSnapshot {
    /// Snapshot block height.
    pub block_height: u32,
    /// Sum of balances of all active owners in satoshis.
    pub total_balance: u64,
    /// Number of active owners included in the snapshot.
    pub active_address_count: u32,
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
    /// Primary ETH address in mint content.
    pub eth_main: String,
    /// Optional collaborator ETH address.
    pub eth_collab: Option<String>,
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

    /// Returns invalid passes with optional code filter.
    #[rpc(name = "get_invalid_passes")]
    fn get_invalid_passes(&self, params: GetInvalidPassesParams) -> JsonResult<InvalidPassesPage>;

    /// Returns active-balance snapshot at exact height.
    ///
    /// Returns shared consensus error `HEIGHT_NOT_SYNCED` when `block_height`
    /// exceeds current local durable sync progress, and `NO_RECORD` when the
    /// height is valid but no exact active-balance snapshot exists there.
    #[rpc(name = "get_active_balance_snapshot")]
    fn get_active_balance_snapshot(
        &self,
        params: GetActiveBalanceSnapshotParams,
    ) -> JsonResult<RpcActiveBalanceSnapshot>;

    /// Returns latest available active-balance snapshot.
    #[rpc(name = "get_latest_active_balance_snapshot")]
    fn get_latest_active_balance_snapshot(&self) -> JsonResult<Option<RpcActiveBalanceSnapshot>>;

    /// Triggers graceful shutdown of the indexer process.
    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}
