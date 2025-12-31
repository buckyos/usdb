use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use usdb_util::{BTCConfig, ElectrsConfig, OrdConfig, BALANCE_HISTORY_SERVICE_HTTP_PORT};

fn default_batch_size() -> usize {
    64
}

fn default_utxo_cache_bytes() -> usize {
    1024 * 1024 * 1024 * 6 // 6 GB
}

fn default_balance_cache_bytes() -> usize {
    1024 * 1024 * 1024 * 6 // 6 GB
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexConfig {
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    // UTXO cache size in bytes in memory
    #[serde(default = "default_utxo_cache_bytes")]
    pub utxo_cache_bytes: usize,


    // Balance cache size in bytes in memory
    #[serde(default = "default_balance_cache_bytes")]
    pub balance_cache_bytes: usize,
}


impl Default for IndexConfig {
    fn default() -> Self {
        IndexConfig {
            batch_size: default_batch_size(),
            utxo_cache_bytes: default_utxo_cache_bytes(),
            balance_cache_bytes: default_balance_cache_bytes(),
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
        RpcServer { port: default_rpc_port() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceHistoryConfig {
    pub btc: BTCConfig,
    pub ordinals: OrdConfig,
    pub electrs: ElectrsConfig,

    pub sync: IndexConfig,
    pub rpc_server: RpcServer,
}

impl Default for BalanceHistoryConfig {
    fn default() -> Self {
        Self {
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
}

pub type BalanceHistoryConfigRef = Arc<BalanceHistoryConfig>;
