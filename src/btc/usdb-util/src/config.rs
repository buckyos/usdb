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

impl Default for BTCConfig {
    fn default() -> Self {
        BTCConfig {
            network: Some(Network::Bitcoin),
            data_dir: None,
            rpc_url: None,
            auth: None,
        }
    }
}
