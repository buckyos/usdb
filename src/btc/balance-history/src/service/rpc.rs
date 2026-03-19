use crate::status::{SyncPhase, SyncStatus};
use bitcoincore_rpc::bitcoin::OutPoint;
use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use usdb_util::{ConsensusSnapshotIdentity, USDBScriptHash};

/// Public RPC/API version of balance-history.
///
/// Bump this when the externally visible JSON-RPC contract changes in an
/// incompatible way, such as response-shape changes or renamed fields.
pub const BALANCE_HISTORY_API_VERSION: &str = "1.0.0";
/// Version tag of the balance-history query semantics contract.
///
/// This describes how callers should interpret historical balance lookups.
/// The current value explicitly means:
/// - balance snapshot queries use at-or-before semantics
/// - delta queries use exact-height semantics
pub const BALANCE_HISTORY_SEMANTICS_VERSION: &str = "balance-snapshot-at-or-before:v1";
/// Fixed protocol stable lag used by balance-history.
///
/// This is not a local tuning knob. Changing it changes the externally visible
/// stable-view rule and therefore must be treated as a protocol/versioned
/// change across all nodes on the same network.
pub const BALANCE_HISTORY_STABLE_LAG: u32 = 0;

/// Query parameters for a single script-hash balance request.
///
/// The request supports exactly one of the optional selectors below in normal
/// usage:
/// - `block_height`: query one logical point-in-time view
/// - `block_range`: query an ordered range of persisted entries
/// - neither: query the latest persisted balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceParams {
    /// Target script hash in balance-history's canonical internal format.
    pub script_hash: USDBScriptHash,

    /// Optional exact query height.
    ///
    /// For balance lookups this uses at-or-before semantics: the service returns
    /// the latest persisted balance record whose block height is `<= block_height`.
    /// For delta lookups this means the delta exactly stored at `block_height`.
    pub block_height: Option<u32>,

    /// Optional half-open range `[start, end)` of block heights.
    ///
    /// When present, the service returns all persisted entries whose heights are
    /// covered by this range, ordered by block height.
    pub block_range: Option<Range<u32>>,
}

/// Query parameters for a batch script-hash balance request.
///
/// The selector semantics match `GetBalanceParams`, but apply to every script
/// hash in `script_hashes` and preserve the input order in the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalancesParams {
    /// Ordered list of target script hashes.
    pub script_hashes: Vec<USDBScriptHash>,

    /// Optional exact query height shared by all requested script hashes.
    pub block_height: Option<u32>,

    /// Optional half-open range `[start, end)` shared by all requested script hashes.
    pub block_range: Option<Range<u32>>,
}

/// One persisted balance record returned by balance-history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBalance {
    /// Height at which this persisted record was written.
    pub block_height: u32,
    /// Balance after applying the change at `block_height`, in satoshi.
    pub balance: u64,
    /// Signed balance delta recorded at `block_height`, in satoshi.
    pub delta: i64,
}

/// Stable snapshot metadata exposed to downstream consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Current stable height that balance-history exposes to downstream services.
    pub stable_height: u32,
    /// BTC block hash paired with `stable_height`, if a block commit exists for that height.
    pub stable_block_hash: Option<String>,
    /// Latest logical block commit at `stable_height`, encoded as lowercase hex.
    pub latest_block_commit: Option<String>,
    /// Fixed stable lag promised by this balance-history instance for the current network.
    ///
    /// Downstream services must treat this as part of the stable-view identity,
    /// not as a local tuning parameter.
    pub stable_lag: u32,
    /// Public API version of balance-history snapshot/query RPCs.
    ///
    /// This tracks response-contract compatibility, not commit-hash rules.
    pub balance_history_api_version: String,
    /// Version of the balance-history query semantics contract.
    ///
    /// This tells downstream consumers how to interpret point-in-time balance
    /// queries and whether the service uses exact-height or at-or-before rules.
    pub balance_history_semantics_version: String,
    /// Version of the balance-history commit protocol exposed by this service.
    pub commit_protocol_version: String,
    /// Hash algorithm used to build `latest_block_commit`.
    pub commit_hash_algo: String,
}

/// Parameters for resolving the exact historical consensus state reference at one BTC height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetStateRefAtHeightParams {
    /// Exact committed BTC block height whose historical state reference should be returned.
    pub block_height: u32,
}

/// Historical consensus state reference for one exact balance-history height.
///
/// This is distinct from `get_snapshot_info`, which only reports the current
/// stable head. ETHW-style validators use this structure to pin validation to
/// one historical BTC state instead of whatever the current head happens to be.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalSnapshotStateRef {
    /// Exact BTC block height whose stable state is being described.
    pub block_height: u32,
    /// Canonical stable BTC block hash recorded at `block_height`.
    pub stable_block_hash: String,
    /// Logical balance-history block commit recorded at `block_height`.
    pub latest_block_commit: String,
    /// Canonical consensus snapshot identity for this exact height.
    pub consensus_identity: ConsensusSnapshotIdentity,
    /// Canonical snapshot id derived from `consensus_identity`.
    pub snapshot_id: String,
    /// Hash algorithm used to derive `snapshot_id`.
    pub snapshot_id_hash_algo: String,
    /// Version tag of the consensus snapshot-id derivation rule.
    pub snapshot_id_version: String,
    /// Version of the balance-history block commit protocol.
    pub commit_protocol_version: String,
    /// Hash algorithm used by `latest_block_commit`.
    pub commit_hash_algo: String,
}

/// Machine-readable blockers that keep balance-history from being consensus-ready.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadinessBlocker {
    /// RPC listener is not yet serving requests, so even liveness is not established.
    RpcNotListening,
    /// Service is still in early bootstrap before any query-capable state exists.
    Initializing,
    /// Service is running a load/install path that should not be treated as ready.
    Loading,
    /// Service is still catching up to the current stable target height.
    CatchingUp,
    /// Durable state is being rolled back or resumed after an interrupted rollback.
    RollbackInProgress,
    /// Shutdown has been requested and the node is draining toward exit.
    ShutdownRequested,
    /// Stable height exists but its canonical BTC block hash is not yet available.
    StableBlockHashMissing,
    /// Stable height exists but the logical block commit at that height is not yet available.
    LatestBlockCommitMissing,
}

/// Structured readiness state for both local monitoring and downstream gating.
///
/// `rpc_alive` is plain liveness. `query_ready` means the service is in a state
/// where ordinary DB-backed queries are expected to work. `consensus_ready`
/// is stricter and only becomes true when the currently advertised stable
/// snapshot is complete and the service is not in a transient recovery state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadinessInfo {
    /// Fixed service identifier, currently `balance-history`.
    pub service: String,
    /// True once the RPC server is listening and able to answer requests.
    pub rpc_alive: bool,
    /// True when ordinary query traffic is allowed.
    pub query_ready: bool,
    /// True only when the current stable snapshot is safe for downstream consensus use.
    pub consensus_ready: bool,
    /// Current sync phase from the high-level sync status tracker.
    pub phase: SyncPhase,
    /// Current progress counter mirrored from sync status.
    pub current: u64,
    /// Total progress target mirrored from sync status.
    pub total: u64,
    /// Optional human-readable status message.
    pub message: Option<String>,
    /// Current stable height, when it can be read from the local DB.
    pub stable_height: Option<u32>,
    /// Stable BTC block hash at `stable_height`, when available.
    pub stable_block_hash: Option<String>,
    /// Latest logical block commit at `stable_height`, when available.
    pub latest_block_commit: Option<String>,
    /// Machine-readable reasons keeping the service from a stricter ready state.
    pub blockers: Vec<ReadinessBlocker>,
}

/// Logical block-commit metadata recorded for one exact BTC block height.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockCommitInfo {
    /// BTC block height associated with this logical commit.
    pub block_height: u32,
    /// Canonical BTC block hash recorded at `block_height`, lowercase hex.
    pub btc_block_hash: String,
    /// Root hash of the balance delta set for this block, lowercase hex.
    pub balance_delta_root: String,
    /// Rolling logical block commit at this height, lowercase hex.
    pub block_commit: String,
    /// Version of the logical commit protocol.
    pub commit_protocol_version: String,
    /// Hash algorithm used by `balance_delta_root` and `block_commit`.
    pub commit_hash_algo: String,
}

/// One currently-live UTXO entry stored by balance-history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoInfo {
    /// Transaction id of the live outpoint, lowercase hex.
    pub txid: String,
    /// Output index inside `txid`.
    pub vout: u32,
    /// Script hash corresponding to the output script, lowercase hex.
    pub script_hash: String,
    /// Output value in satoshi.
    pub value: u64,
}

#[rpc(server)]
pub trait BalanceHistoryRpc {
    /// Returns the BTC network configured for the current service instance.
    ///
    /// Example values include `bitcoin`, `testnet`, `regtest`, or `signet`,
    /// depending on the configured upstream node.
    #[rpc(name = "get_network_type")]
    fn get_network_type(&self) -> JsonResult<String>;

    /// Returns the latest BTC block height that has been committed into the local DB.
    ///
    /// This is the service's current stable height, not necessarily the upstream
    /// node's best tip if indexing is still catching up.
    #[rpc(name = "get_block_height")]
    fn get_block_height(&self) -> JsonResult<u64>;

    /// Returns the current high-level sync status snapshot.
    ///
    /// Downstream callers can use this endpoint to distinguish between fully
    /// caught-up, actively syncing, or recovering states before issuing more
    /// specific data queries.
    #[rpc(name = "get_sync_status")]
    fn get_sync_status(&self) -> JsonResult<SyncStatus>;

    /// Returns snapshot metadata for the service's current stable view.
    ///
    /// The response includes the stable block height, the canonical block hash
    /// recorded at that height, and the latest logical block-commit metadata.
    ///
    /// Returns shared consensus error `SNAPSHOT_NOT_READY` when the local DB
    /// has a height but the advertised stable snapshot is still incomplete.
    #[rpc(name = "get_snapshot_info")]
    fn get_snapshot_info(&self) -> JsonResult<SnapshotInfo>;

    /// Returns structured readiness state for liveness, query serving, and consensus use.
    ///
    /// This endpoint is intentionally stricter than a simple "RPC reachable"
    /// probe: callers must use `consensus_ready` instead of inferring readiness
    /// from `get_network_type` or from free-form sync messages.
    #[rpc(name = "get_readiness")]
    fn get_readiness(&self) -> JsonResult<ReadinessInfo>;

    /// Returns the exact historical consensus state reference at one BTC height.
    ///
    /// This endpoint is intended for downstream validators that must re-check a
    /// block against the historical BTC state observed at height `block_height`,
    /// not against the service's current head.
    ///
    /// Returns shared consensus error `HEIGHT_NOT_SYNCED` when `block_height`
    /// is above the current stable height, and `SNAPSHOT_NOT_READY` when the
    /// current stable view is not yet safe for consensus use.
    #[rpc(name = "get_state_ref_at_height")]
    fn get_state_ref_at_height(
        &self,
        params: GetStateRefAtHeightParams,
    ) -> JsonResult<HistoricalSnapshotStateRef>;

    /// Returns logical block-commit metadata for one exact BTC block height.
    ///
    /// Returns `None` when no block commit has been persisted for the requested
    /// height yet.
    #[rpc(name = "get_block_commit")]
    fn get_block_commit(&self, block_height: u32) -> JsonResult<Option<BlockCommitInfo>>;

    /// Returns balance records for one script hash.
    ///
    /// Semantics depend on the selector in `params`:
    /// - with `block_height`: returns one-element vector containing the latest
    ///   persisted balance record at or before that height
    /// - with `block_range`: returns all persisted balance records within the
    ///   half-open range `[start, end)`
    /// - with neither selector: returns one-element vector containing the latest
    ///   persisted balance record overall
    ///
    /// Returns shared consensus error `HEIGHT_NOT_SYNCED` when the requested
    /// height or range exceeds the current stable height.
    #[rpc(name = "get_address_balance")]
    fn get_address_balance(&self, params: GetBalanceParams) -> JsonResult<Vec<AddressBalance>>;

    /// Returns balance records for multiple script hashes.
    ///
    /// The output order matches `params.script_hashes`. Each element uses the
    /// same selector semantics as `get_address_balance`, including shared
    /// consensus error `HEIGHT_NOT_SYNCED` for future heights/ranges.
    #[rpc(name = "get_addresses_balances")]
    fn get_addresses_balances(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<AddressBalance>>>;

    /// Returns balance delta records for one script hash.
    ///
    /// This endpoint requires an explicit selector:
    /// - with `block_height`: returns a one-element vector containing the delta
    ///   record stored exactly at that height, or `None` when no delta was
    ///   recorded for the script hash at that height
    /// - with `block_range`: returns ordered delta records within `[start, end)`
    /// - with neither selector: returns `InvalidParams`
    #[rpc(name = "get_address_balance_delta")]
    fn get_address_balance_delta(
        &self,
        params: GetBalanceParams,
    ) -> JsonResult<Vec<Option<AddressBalance>>>;

    /// Returns balance delta records for multiple script hashes.
    ///
    /// The output order matches `params.script_hashes`. Each element uses the
    /// same selector semantics as `get_address_balance_delta`.
    #[rpc(name = "get_addresses_balances_delta")]
    fn get_addresses_balances_delta(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<Option<AddressBalance>>>>;

    /// Gets one currently-live UTXO from balance-history's persisted UTXO view.
    ///
    /// This endpoint only reads the service's own DB state and returns `None`
    /// when the outpoint is already spent, pruned from the live UTXO set, or
    /// has never been committed into the local balance-history state.
    ///
    /// Unlike the internal indexer preload path, this RPC does not fall back to
    /// bitcoind RPC when the outpoint is missing from the local DB/cache.
    #[rpc(name = "get_live_utxo")]
    fn get_live_utxo(&self, outpoint: OutPoint) -> JsonResult<Option<UtxoInfo>>;

    /// Requests graceful shutdown of the balance-history process.
    ///
    /// The service sends its internal shutdown signal and starts tearing down the
    /// RPC server. Callers should treat a successful response as acknowledgement
    /// that shutdown has started, not as a guarantee that the process has fully
    /// exited at the moment the HTTP response is returned.
    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}
