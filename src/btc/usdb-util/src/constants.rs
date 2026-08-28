// Service names
pub const USDB_INDEXER_SERVICE_NAME: &str = "usdb-indexer";
pub const BALANCE_HISTORY_SERVICE_NAME: &str = "balance-history";
pub const USDB_CONTROL_PLANE_SERVICE_NAME: &str = "usdb-control-plane";
pub const USDB_INDEXER_CLI_TOOL_NAME: &str = "usdb-indexer-cli";
pub const BALANCE_HISTORY_CLI_TOOL_NAME: &str = "balance-history-cli";

/// Public JSON-RPC API version advertised by `usdb-indexer`.
pub const USDB_INDEXER_API_VERSION: &str = "1.0.0";
/// UIP-0006 economic state view contract accepted by the current indexer.
pub const USDB_ECONOMIC_STATE_VIEW_VERSION: &str = "uip-0006-usdb-economic-state-view:v1";
/// Deterministic ordering rule used by the UIP-0006 candidate set.
pub const USDB_CANDIDATE_SET_SELECTION_RULE: &str = "uip-0006:effective-energy-desc-pass-id-asc:v1";
/// Maximum number of rows accepted by UIP-0006 cursor-paged queries.
pub const USDB_ECONOMIC_PAGE_MAX_LIMIT: usize = 500;

/// `get_rpc_info.features` name for exact historical state references.
pub const USDB_INDEXER_FEATURE_HISTORICAL_STATE_REF: &str = "historical_state_ref";
/// `get_rpc_info.features` name for single-pass UIP-0006 profiles.
pub const USDB_INDEXER_FEATURE_PASS_ECONOMIC_PROFILE: &str = "pass_economic_profile";
/// `get_rpc_info.features` name for the canonical validator candidate set.
pub const USDB_INDEXER_FEATURE_CANDIDATE_SET_VIEW: &str = "candidate_set_view";
/// `get_rpc_info.features` name for auditable collab contribution pages.
pub const USDB_INDEXER_FEATURE_COLLAB_BREAKDOWN: &str = "collab_breakdown";
/// `get_rpc_info.features` name for the selector-bound miner BTC aggregate.
pub const USDB_INDEXER_FEATURE_MINER_ECONOMIC_AGGREGATE: &str = "miner_economic_aggregate";
/// Capability flag for atomically selecting one active standard pass by `usdb_main`.
pub const USDB_INDEXER_FEATURE_MINER_CANDIDATE_RESOLUTION: &str = "miner_candidate_resolution";
/// Complete feature set required for UIP-0006 economic state view consumers.
pub const USDB_INDEXER_ECONOMIC_STATE_VIEW_FEATURES: [&str; 6] = [
    USDB_INDEXER_FEATURE_HISTORICAL_STATE_REF,
    USDB_INDEXER_FEATURE_PASS_ECONOMIC_PROFILE,
    USDB_INDEXER_FEATURE_CANDIDATE_SET_VIEW,
    USDB_INDEXER_FEATURE_COLLAB_BREAKDOWN,
    USDB_INDEXER_FEATURE_MINER_ECONOMIC_AGGREGATE,
    USDB_INDEXER_FEATURE_MINER_CANDIDATE_RESOLUTION,
];

// Directory constants
pub const USDB_ROOT_DIR: &str = ".usdb";

// Port base ranges for USDB-managed local services.
pub const USDB_MAINNET_PORT_BASE: u16 = 28_000;
pub const USDB_REGTEST_PORT_BASE: u16 = 28_100;
pub const USDB_TESTNET_PORT_BASE: u16 = 28_200;
pub const USDB_SIGNET_PORT_BASE: u16 = 28_300;
pub const USDB_TESTNET4_PORT_BASE: u16 = 28_400;

// Shared per-service offsets from each network base.
pub const PORT_OFFSET_BALANCE_HISTORY_RPC: u16 = 10;
pub const PORT_OFFSET_USDB_INDEXER_RPC: u16 = 20;
pub const PORT_OFFSET_ORD_HTTP: u16 = 30;
pub const PORT_OFFSET_BITCOIND_RPC: u16 = 32;
pub const PORT_OFFSET_BITCOIND_P2P: u16 = 33;
pub const PORT_OFFSET_CONTROL_PLANE_HTTP: u16 = 40;

// Mainnet service ports (USDB-managed local services) plus standard bitcoind defaults.
pub const BALANCE_HISTORY_SERVICE_HTTP_PORT: u16 = 28_010; // base 28000 + offset 10
pub const USDB_INDEXER_SERVICE_HTTP_PORT: u16 = 28_020; // base 28000 + offset 20
pub const ORD_SERVICE_HTTP_PORT: u16 = 28_030; // base 28000 + offset 30
pub const USDB_CONTROL_PLANE_HTTP_PORT: u16 = 28_040; // base 28000 + offset 40
pub const BITCOIND_MAINNET_RPC_PORT: u16 = 8332;
pub const BITCOIND_MAINNET_P2P_PORT: u16 = 8333;

// Regtest default ports (explicit values for quick lookup).
pub const REGTEST_BALANCE_HISTORY_SERVICE_HTTP_PORT: u16 = 28_110; // base 28100 + offset 10
pub const REGTEST_USDB_INDEXER_SERVICE_HTTP_PORT: u16 = 28_120; // base 28100 + offset 20
pub const REGTEST_ORD_SERVICE_HTTP_PORT: u16 = 28_130; // base 28100 + offset 30
pub const REGTEST_USDB_CONTROL_PLANE_HTTP_PORT: u16 = 28_140; // base 28100 + offset 40
pub const BITCOIND_REGTEST_RPC_PORT: u16 = 28_132; // base 28100 + offset 32
pub const BITCOIND_REGTEST_P2P_PORT: u16 = 28_133; // base 28100 + offset 33

// Additional network defaults use the standard bitcoind ports.
pub const BITCOIND_TESTNET_RPC_PORT: u16 = 18_332;
pub const BITCOIND_TESTNET_P2P_PORT: u16 = 18_333;
pub const BITCOIND_SIGNET_RPC_PORT: u16 = 38_332;
pub const BITCOIND_SIGNET_P2P_PORT: u16 = 38_333;
pub const BITCOIND_TESTNET4_RPC_PORT: u16 = 48_332;
pub const BITCOIND_TESTNET4_P2P_PORT: u16 = 48_333;
