// Service names
pub const USDB_INDEXER_SERVICE_NAME: &str = "usdb-indexer";
pub const BALANCE_HISTORY_SERVICE_NAME: &str = "balance-history";
pub const USDB_INDEXER_CLI_TOOL_NAME: &str = "usdb-indexer-cli";
pub const BALANCE_HISTORY_CLI_TOOL_NAME: &str = "balance-history-cli";

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

// Mainnet default ports (explicit values for quick lookup).
pub const BALANCE_HISTORY_SERVICE_HTTP_PORT: u16 = 28_010; // base 28000 + offset 10
pub const USDB_INDEXER_SERVICE_HTTP_PORT: u16 = 28_020; // base 28000 + offset 20
pub const ORD_SERVICE_HTTP_PORT: u16 = 28_030; // base 28000 + offset 30
pub const BITCOIND_MAINNET_RPC_PORT: u16 = 28_032; // base 28000 + offset 32
pub const BITCOIND_MAINNET_P2P_PORT: u16 = 28_033; // base 28000 + offset 33

// Regtest default ports (explicit values for quick lookup).
pub const REGTEST_BALANCE_HISTORY_SERVICE_HTTP_PORT: u16 = 28_110; // base 28100 + offset 10
pub const REGTEST_USDB_INDEXER_SERVICE_HTTP_PORT: u16 = 28_120; // base 28100 + offset 20
pub const REGTEST_ORD_SERVICE_HTTP_PORT: u16 = 28_130; // base 28100 + offset 30
pub const BITCOIND_REGTEST_RPC_PORT: u16 = 28_132; // base 28100 + offset 32
pub const BITCOIND_REGTEST_P2P_PORT: u16 = 28_133; // base 28100 + offset 33

// Additional network default ports (explicit values).
pub const BITCOIND_TESTNET_RPC_PORT: u16 = 28_232; // base 28200 + offset 32
pub const BITCOIND_TESTNET_P2P_PORT: u16 = 28_233; // base 28200 + offset 33
pub const BITCOIND_SIGNET_RPC_PORT: u16 = 28_332; // base 28300 + offset 32
pub const BITCOIND_SIGNET_P2P_PORT: u16 = 28_333; // base 28300 + offset 33
pub const BITCOIND_TESTNET4_RPC_PORT: u16 = 28_432; // base 28400 + offset 32
pub const BITCOIND_TESTNET4_P2P_PORT: u16 = 28_433; // base 28400 + offset 33
