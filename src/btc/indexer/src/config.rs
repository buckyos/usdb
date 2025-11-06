use bitcoincore_rpc::Auth;
use bitcoincore_rpc::bitcoin::Network;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BTCAuth {
    None,
    UserPass(String, String),
    CookieFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BTCConfig {
    pub network: Option<Network>,
    pub data_dir: Option<PathBuf>,
    pub rpc_url: Option<String>,
    pub auth: Option<BTCAuth>,
}

impl BTCConfig {
    pub fn network(&self) -> Network {
        if let Some(network) = self.network {
            network
        } else {
            // Default is main
            Network::Bitcoin
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        if let Some(ref dir) = self.data_dir {
            dir.clone()
        } else {
            let base_dir = dirs::home_dir().expect("Could not determine data directory");
            match self.network() {
                Network::Bitcoin => base_dir.join(".bitcoin"),
                Network::Testnet => base_dir.join(".bitcoin/testnet3"),
                Network::Regtest => base_dir.join(".bitcoin/regtest"),
                Network::Signet => base_dir.join(".bitcoin/signet"),
                Network::Testnet4 => base_dir.join(".bitcoin/testnet4"),
            }
        }
    }

    pub fn rpc_url(&self) -> String {
        if let Some(ref url) = self.rpc_url {
            url.clone()
        } else {
            /*
            # Listen for JSON-RPC connections on <port> (default: 8332, testnet3:
            # 18332, testnet4: 48332, signet: 38332, regtest: 18443)
             */

            let port = match self.network() {
                Network::Bitcoin => 8332,
                Network::Testnet => 18332,
                Network::Regtest => 18443,
                Network::Signet => 38332,
                Network::Testnet4 => 48332,
            };

            format!("http://127.0.0.1:{}", port)
        }
    }

    pub fn auth(&self) -> Auth {
        if let Some(ref auth) = self.auth {
            match auth {
                BTCAuth::None => Auth::None,
                BTCAuth::UserPass(user, pass) => Auth::UserPass(user.clone(), pass.clone()),
                BTCAuth::CookieFile(path) => Auth::CookieFile(path.clone()),
            }
        } else {
            // Default to cookie file
            let cookie_path = self.data_dir().join(".cookie");
            Auth::CookieFile(cookie_path)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrdConfig {
    pub rpc_url: Option<String>,
}

impl OrdConfig {
    pub fn rpc_url(&self) -> &str {
        if let Some(ref url) = self.rpc_url {
            url.as_str()
        } else {
            "http://127.0.0.1:8070"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexerConfig {
    // Used for store data and logs separately, default is "formal",
    pub isolate: String,

    pub bitcoin: BTCConfig,

    pub ordinals: OrdConfig,
}

pub struct ConfigManager {
    root_dir: PathBuf,
    config: IndexerConfig,
}

impl ConfigManager {
    pub fn new(root_dir: Option<PathBuf>) -> Result<Self, String> {
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

    pub fn config(&self) -> &IndexerConfig {
        &self.config
    }
}

pub type ConfigManagerRef = Arc<ConfigManager>;
