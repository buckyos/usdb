use bitcoincore_rpc::Auth;
use bitcoincore_rpc::bitcoin::Network;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BTCAuth {
    None,
    UserPass(String, String),
    CookieFile(PathBuf),
}

fn default_network() -> Network {
    Network::Bitcoin
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BTCConfig {
    #[serde(default = "default_network")]
    pub network: Network,

    #[serde(default)]
    pub data_dir: Option<PathBuf>,

    #[serde(default)]
    pub rpc_url: Option<String>,

    #[serde(default)]
    pub auth: Option<BTCAuth>,

    #[serde(default)]
    pub block_magic: Option<u32>,
}

impl BTCConfig {
    pub fn network(&self) -> Network {
        self.network
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

    // FIXME: Magic number for test net is not verified, need to check when running a testnet node
    pub fn block_magic(&self) -> u32 {
        if let Some(magic) = self.block_magic {
            magic
        } else {
            match self.network() {
                Network::Bitcoin => 0xD9B4BEF9,
                Network::Testnet => 0xDAB5BFFA,
                Network::Regtest => 0xDAB5BFFA,
                Network::Signet => 0x0A03CF40,
                Network::Testnet4 => 0x07110907,
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

impl Default for BTCConfig {
    fn default() -> Self {
        BTCConfig {
            network: default_network(),
            data_dir: None,
            rpc_url: None,
            auth: None,
            block_magic: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrdConfig {
    #[serde(default = "default_ord_rpc_url")]
    pub rpc_url: String,
}

fn default_ord_rpc_url() -> String {
    "http://127.0.0.1:8070".to_string()
}

impl OrdConfig {
    pub fn rpc_url(&self) -> &str {
        self.rpc_url.as_str()
    }
}

impl Default for OrdConfig {
    fn default() -> Self {
        Self { rpc_url: default_ord_rpc_url() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElectrsConfig {
    #[serde(default = "default_electrs_rpc_url")]
    pub rpc_url: String,
}

fn default_electrs_rpc_url() -> String {
    "tcp://127.0.0.1:50001".to_string()
}

impl ElectrsConfig {
    pub fn rpc_url(&self) -> &str {
        self.rpc_url.as_str()
    }
}

impl Default for ElectrsConfig {
    fn default() -> Self {
        Self { rpc_url: default_electrs_rpc_url() }
    }
}


fn default_balance_history_rpc_url() -> String {
    format!("http://127.0.0.1:{}", crate::constants::BALANCE_HISTORY_SERVICE_HTTP_PORT)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceHistoryConfig {
    #[serde(default = "default_balance_history_rpc_url")]
    pub rpc_url: String,
}

impl Default for BalanceHistoryConfig {
    fn default() -> Self {
        Self { rpc_url: default_balance_history_rpc_url() }
    }
}