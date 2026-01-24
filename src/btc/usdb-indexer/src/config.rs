use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::{BTCConfig, OrdConfig, BalanceHistoryConfig};

fn default_genesis_block_height() -> u32 {
    900000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct USDBConfig {
    #[serde(default = "default_genesis_block_height")]
    pub genesis_block_height: u32,
}

impl Default for USDBConfig {
    fn default() -> Self {
        USDBConfig {
            genesis_block_height: default_genesis_block_height(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexerConfig {
    // Used for store data and logs separately, default is none,
    // So data and logs are stored in the root dir directly, or {isolate}/data and {isolate}/logs if set
    pub isolate: Option<String>,

    pub bitcoin: BTCConfig,

    pub ordinals: OrdConfig,

    pub balance_history: BalanceHistoryConfig,
    
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
             
            info!("Config file not found at {}. Using default config.", config_path.display());
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
