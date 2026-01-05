use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use usdb_util::{BALANCE_HISTORY_SERVICE_HTTP_PORT, BTCConfig, ElectrsConfig, OrdConfig};

fn default_batch_size() -> usize {
    128
}

fn get_cache_size() -> usize {
    let available_memory = usdb_util::get_smart_memory_limit();
    info!("Available memory: {} bytes", available_memory);

    // Leave 8 GB free
    let cache_size = available_memory.saturating_sub(1024 * 1024 * 1024 * 8);
    info!("Calculated cache size: {} bytes", cache_size);

    cache_size as usize
}

// 1/4 of total cache size, at least 1 GB
fn default_utxo_cache_bytes() -> usize {
    let size = get_cache_size() / 4;
    size.max(1024 * 1024 * 1024)
}

// 3/4 of total cache size, at least 3 GB
fn default_balance_cache_bytes() -> usize {
    let size = get_cache_size() * 3 / 4;
    size.max(3 * 1024 * 1024 * 1024)
}

// When memory percent is not specified, default to 90%
// That is when used memory percent is up to 90%, we will start shrinking caches
fn default_max_memory_percent() -> usize {
    90
}

fn default_local_loader_threshold() -> usize {
    500
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Threshold of blocks behind to switch to LocalLoader client
    #[serde(default = "default_local_loader_threshold")]
    pub local_loader_threshold: usize,

    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    // UTXO cache size in bytes in memory
    #[serde(default = "default_utxo_cache_bytes")]
    pub utxo_max_cache_bytes: usize,

    // Balance cache size in bytes in memory
    #[serde(default = "default_balance_cache_bytes")]
    pub balance_max_cache_bytes: usize,

    // Maximum percent of system memory to use for caches
    // Value can be 10-100
    #[serde(default = "default_max_memory_percent")]
    pub max_memory_percent: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            local_loader_threshold: default_local_loader_threshold(),
            batch_size: default_batch_size(),
            utxo_max_cache_bytes: default_utxo_cache_bytes(),
            balance_max_cache_bytes: default_balance_cache_bytes(),
            max_memory_percent: default_max_memory_percent(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcServer {
    #[serde(default = "default_rpc_port")]
    pub port: u16,
}

fn default_rpc_port() -> u16 {
    BALANCE_HISTORY_SERVICE_HTTP_PORT
}

impl Default for RpcServer {
    fn default() -> Self {
        RpcServer {
            port: default_rpc_port(),
        }
    }
}

fn get_default_root_dir() -> PathBuf {
    let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    root_dir
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceHistoryConfig {
    #[serde(default = "get_default_root_dir")]
    pub root_dir: PathBuf,

    pub btc: BTCConfig,
    pub ordinals: OrdConfig,
    pub electrs: ElectrsConfig,

    pub sync: IndexConfig,
    pub rpc_server: RpcServer,
}

impl Default for BalanceHistoryConfig {
    fn default() -> Self {
        Self {
            root_dir: get_default_root_dir(),
            btc: BTCConfig::default(),
            ordinals: OrdConfig::default(),
            electrs: ElectrsConfig::default(),
            sync: IndexConfig::default(),
            rpc_server: RpcServer::default(),
        }
    }
}

impl BalanceHistoryConfig {
    pub fn load(root_dir: &Path) -> Result<Self, String> {
        let path = root_dir.join("config.toml");
        if !path.exists() {
            let default_config = BalanceHistoryConfig::default();
            info!(
                "Config file {} does not exist. Using default configuration.",
                path.display()
            );
            info!(
                "Default config: {}",
                toml::to_string_pretty(&default_config).unwrap()
            );
            Ok(default_config)
        } else {
            info!("Loading config from {}", path.display());
            let config_data = std::fs::read_to_string(&path).map_err(|e| {
                let msg = format!("Failed to read config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;
            info!("Config data: {}", config_data);

            let config: BalanceHistoryConfig = toml::from_str(&config_data).map_err(|e| {
                let msg = format!("Failed to parse config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;

            Ok(config)
        }
    }

    pub fn db_dir(&self) -> PathBuf {
        self.root_dir.join("db")
    }
}

pub type BalanceHistoryConfigRef = Arc<BalanceHistoryConfig>;
