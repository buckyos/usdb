use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::{BTCConfig, BalanceHistoryConfig, OrdConfig, USDB_INDEXER_SERVICE_HTTP_PORT};

fn default_genesis_block_height() -> u32 {
    900000
}

fn default_active_address_page_size() -> usize {
    1024
}

fn default_balance_query_batch_size() -> usize {
    1024
}

fn default_balance_query_concurrency() -> usize {
    4
}

fn default_balance_query_timeout_ms() -> u64 {
    10_000
}

fn default_balance_query_max_retries() -> u32 {
    2
}

fn default_inscription_source() -> String {
    "ord".to_string()
}

fn default_inscription_fixture_file() -> Option<String> {
    None
}

fn default_inscription_source_shadow_compare() -> bool {
    false
}

fn default_inscription_source_shadow_fail_fast() -> bool {
    false
}

fn default_rpc_server_port() -> u16 {
    USDB_INDEXER_SERVICE_HTTP_PORT
}

fn default_rpc_server_enabled() -> bool {
    true
}

fn default_pass_energy_leaderboard_cache_enabled() -> bool {
    true
}

fn default_pass_energy_leaderboard_cache_top_k() -> usize {
    1000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct USDBConfig {
    // First BTC block height that the indexer should process for USDB protocol data.
    #[serde(default = "default_genesis_block_height")]
    pub genesis_block_height: u32,

    // Number of active pass records loaded per storage page when scanning owners.
    #[serde(default = "default_active_address_page_size")]
    pub active_address_page_size: usize,

    // Number of addresses included in each balance-history RPC request batch.
    #[serde(default = "default_balance_query_batch_size")]
    pub balance_query_batch_size: usize,

    // Maximum number of in-flight balance-history RPC batches.
    #[serde(default = "default_balance_query_concurrency")]
    pub balance_query_concurrency: usize,

    // Per-batch balance-history RPC timeout in milliseconds.
    #[serde(default = "default_balance_query_timeout_ms")]
    pub balance_query_timeout_ms: u64,

    // Maximum retry attempts for a failed balance-history RPC batch.
    #[serde(default = "default_balance_query_max_retries")]
    pub balance_query_max_retries: u32,

    // Primary inscription source backend: supported values are "ord" and "bitcoind".
    #[serde(default = "default_inscription_source")]
    pub inscription_source: String,

    // Optional fixture JSON file path used when inscription_source is "fixture".
    // Relative paths are resolved from usdb-indexer root directory.
    #[serde(default = "default_inscription_fixture_file")]
    pub inscription_fixture_file: Option<String>,

    // Enable primary-vs-shadow inscription source comparison for diagnostics.
    #[serde(default = "default_inscription_source_shadow_compare")]
    pub inscription_source_shadow_compare: bool,

    // Stop block processing immediately when shadow comparison finds mismatches.
    #[serde(default = "default_inscription_source_shadow_fail_fast")]
    pub inscription_source_shadow_fail_fast: bool,

    // JSON-RPC server listen port for usdb-indexer external query APIs.
    #[serde(default = "default_rpc_server_port")]
    pub rpc_server_port: u16,

    // Enable or disable JSON-RPC server startup.
    #[serde(default = "default_rpc_server_enabled")]
    pub rpc_server_enabled: bool,

    // Enable in-memory cache for latest-height pass energy leaderboard queries.
    #[serde(default = "default_pass_energy_leaderboard_cache_enabled")]
    pub pass_energy_leaderboard_cache_enabled: bool,

    // Maximum number of top-ranked leaderboard rows cached in memory.
    #[serde(default = "default_pass_energy_leaderboard_cache_top_k")]
    pub pass_energy_leaderboard_cache_top_k: usize,
}

impl Default for USDBConfig {
    fn default() -> Self {
        USDBConfig {
            genesis_block_height: default_genesis_block_height(),
            active_address_page_size: default_active_address_page_size(),
            balance_query_batch_size: default_balance_query_batch_size(),
            balance_query_concurrency: default_balance_query_concurrency(),
            balance_query_timeout_ms: default_balance_query_timeout_ms(),
            balance_query_max_retries: default_balance_query_max_retries(),
            inscription_source: default_inscription_source(),
            inscription_fixture_file: default_inscription_fixture_file(),
            inscription_source_shadow_compare: default_inscription_source_shadow_compare(),
            inscription_source_shadow_fail_fast: default_inscription_source_shadow_fail_fast(),
            rpc_server_port: default_rpc_server_port(),
            rpc_server_enabled: default_rpc_server_enabled(),
            pass_energy_leaderboard_cache_enabled: default_pass_energy_leaderboard_cache_enabled(),
            pass_energy_leaderboard_cache_top_k: default_pass_energy_leaderboard_cache_top_k(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexerConfig {
    // Optional namespace to isolate runtime data and logs under <root>/<isolate>/.
    pub isolate: Option<String>,

    // Bitcoin RPC connectivity settings.
    pub bitcoin: BTCConfig,

    // Ord service RPC settings.
    pub ordinals: OrdConfig,

    // Balance-history service RPC settings.
    pub balance_history: BalanceHistoryConfig,

    // USDB indexer behavior and performance tuning settings.
    pub usdb: USDBConfig,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        IndexerConfig {
            isolate: None,
            bitcoin: BTCConfig::default(),
            ordinals: OrdConfig::default(),
            balance_history: BalanceHistoryConfig::default(),
            usdb: USDBConfig::default(),
        }
    }
}

pub struct ConfigManager {
    root_dir: PathBuf,
    config: IndexerConfig,
}

impl ConfigManager {
    pub fn load(root_dir: Option<PathBuf>) -> Result<Self, String> {
        let root_dir = if let Some(dir) = root_dir {
            dir
        } else {
            let home = dirs::home_dir().expect("Could not determine home directory");
            home.join(".usdb")
        };

        if !root_dir.exists() {
            std::fs::create_dir_all(&root_dir).map_err(|e| {
                let msg = format!(
                    "Could not create root directory at {}: {}",
                    root_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        let config_path = root_dir.join("config.json");
        if !config_path.exists() {
            let default_config = IndexerConfig::default();

            info!(
                "Config file not found at {}. Using default config.",
                config_path.display()
            );
            info!(
                "Default config: {}",
                serde_json::to_string_pretty(&default_config).unwrap()
            );

            return Ok(Self {
                root_dir,
                config: default_config,
            });
        }

        let config_data = std::fs::read_to_string(&config_path).map_err(|e| {
            let msg = format!(
                "Could not read config file at {}: {}",
                config_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let config: IndexerConfig = serde_json::from_str(&config_data).map_err(|e| {
            let msg = format!(
                "Could not parse config file at {}: {}",
                config_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(Self { root_dir, config })
    }

    pub fn root_dir(&self) -> &PathBuf {
        &self.root_dir
    }

    pub fn data_dir(&self) -> PathBuf {
        let dir = match &self.config.isolate {
            Some(isolate) => self.root_dir.join(isolate).join("data"),
            None => self.root_dir.join("data"),
        };

        if !dir.exists() {
            std::fs::create_dir_all(&dir).expect("Could not create data directory");
        }

        dir
    }

    pub fn config(&self) -> &IndexerConfig {
        &self.config
    }
}

pub type ConfigManagerRef = Arc<ConfigManager>;
