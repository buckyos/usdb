use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::path::Path;
use usdb_util::BTCConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexConfig {
    pub batch_size: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        IndexConfig { batch_size: 256 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcServer {
    pub port: u16,
}

impl Default for RpcServer {
    fn default() -> Self {
        RpcServer {
            port: 8099,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceHistoryConfig {
    pub btc: BTCConfig,
    pub sync: IndexConfig,
    pub rpc_server: RpcServer,
}

impl Default for BalanceHistoryConfig {
    fn default() -> Self {
        BalanceHistoryConfig {
            btc: BTCConfig::default(),
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