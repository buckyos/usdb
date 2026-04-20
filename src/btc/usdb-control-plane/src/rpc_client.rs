use crate::config::{BitcoinAuthMode, ControlPlaneConfig};
use crate::models::{
    BalanceHistoryReadiness, BitcoinBlockHeader, BitcoinBlockchainInfo, EthBlockHeader,
    UsdbIndexerReadiness,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(Debug, serde::Deserialize)]
struct JsonRpcEnvelope<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

#[derive(Clone)]
pub struct RpcClient {
    client: Client,
}

impl RpcClient {
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|e| {
                let msg = format!("Failed to build HTTP client: {}", e);
                error!("{}", msg);
                msg
            })?;
        Ok(Self { client })
    }

    pub async fn balance_history_network(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "get_network_type", json!([])).await
    }

    pub async fn balance_history_readiness(
        &self,
        url: &str,
    ) -> Result<BalanceHistoryReadiness, String> {
        self.json_rpc_call(url, "get_readiness", json!([])).await
    }

    pub async fn usdb_indexer_network(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "get_network_type", json!([])).await
    }

    pub async fn usdb_indexer_readiness(&self, url: &str) -> Result<UsdbIndexerReadiness, String> {
        self.json_rpc_call(url, "get_readiness", json!([])).await
    }

    pub async fn balance_history_proxy(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.json_rpc_call(url, method, params).await
    }

    pub async fn usdb_indexer_proxy(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.json_rpc_call(url, method, params).await
    }

    pub async fn ethw_client_version(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "web3_clientVersion", json!([]))
            .await
    }

    pub async fn ethw_chain_id(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "eth_chainId", json!([])).await
    }

    pub async fn ethw_network_id(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "net_version", json!([])).await
    }

    pub async fn ethw_block_number(&self, url: &str) -> Result<String, String> {
        self.json_rpc_call(url, "eth_blockNumber", json!([])).await
    }

    pub async fn ethw_syncing(&self, url: &str) -> Result<Value, String> {
        self.json_rpc_call(url, "eth_syncing", json!([])).await
    }

    pub async fn ethw_latest_block(&self, url: &str) -> Result<Option<EthBlockHeader>, String> {
        self.json_rpc_call(url, "eth_getBlockByNumber", json!(["latest", false]))
            .await
    }

    pub async fn http_probe(&self, url: &str) -> Result<u16, String> {
        let response = self.client.get(url).send().await.map_err(|e| {
            let msg = format!("Failed to probe HTTP endpoint {}: {}", url, e);
            warn!("{}", msg);
            msg
        })?;

        Ok(response.status().as_u16())
    }

    pub async fn http_text(&self, url: &str) -> Result<String, String> {
        let response = self.client.get(url).send().await.map_err(|e| {
            let msg = format!("Failed to fetch HTTP endpoint {}: {}", url, e);
            warn!("{}", msg);
            msg
        })?;
        let status = response.status();
        if !status.is_success() {
            let msg = format!(
                "HTTP endpoint {} returned non-success status {}",
                url, status
            );
            warn!("{}", msg);
            return Err(msg);
        }

        response.text().await.map_err(|e| {
            let msg = format!("Failed to read HTTP response body from {}: {}", url, e);
            warn!("{}", msg);
            msg
        })
    }

    pub async fn bitcoin_blockchain_info(
        &self,
        config: &ControlPlaneConfig,
    ) -> Result<BitcoinBlockchainInfo, String> {
        self.bitcoin_json_rpc_call(config, "getblockchaininfo", json!([]))
            .await
    }

    pub async fn bitcoin_block_header(
        &self,
        config: &ControlPlaneConfig,
        block_hash: &str,
    ) -> Result<BitcoinBlockHeader, String> {
        self.bitcoin_json_rpc_call(config, "getblockheader", json!([block_hash]))
            .await
    }

    async fn json_rpc_call<T: DeserializeOwned>(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to send RPC request to {} (method={}): {}",
                    url, method, e
                );
                warn!("{}", msg);
                msg
            })?;

        let status = response.status();
        let envelope: JsonRpcEnvelope<T> = response.json().await.map_err(|e| {
            let msg = format!(
                "Failed to decode RPC response from {} (method={}, status={}): {}",
                url, method, status, e
            );
            warn!("{}", msg);
            msg
        })?;

        if let Some(error) = envelope.error {
            let msg = if let Some(data) = error.data {
                format!(
                    "RPC {} returned error {} ({}): {}",
                    method, error.code, error.message, data
                )
            } else {
                format!(
                    "RPC {} returned error {}: {}",
                    method, error.code, error.message
                )
            };
            warn!("{}", msg);
            return Err(msg);
        }

        envelope.result.ok_or_else(|| {
            let msg = format!("RPC {} returned neither result nor error", method);
            warn!("{}", msg);
            msg
        })
    }

    async fn bitcoin_json_rpc_call<T: DeserializeOwned>(
        &self,
        config: &ControlPlaneConfig,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "1.0",
            "id": "usdb-control-plane",
            "method": method,
            "params": params,
        });

        let mut builder = self.client.post(&config.bitcoin.url).json(&request);
        match config.bitcoin.auth_mode {
            BitcoinAuthMode::None => {}
            BitcoinAuthMode::Userpass => {
                let user =
                    config.bitcoin.rpc_user.as_deref().ok_or_else(|| {
                        "BTC RPC auth_mode=userpass requires rpc_user".to_string()
                    })?;
                let password = config.bitcoin.rpc_password.as_deref().ok_or_else(|| {
                    "BTC RPC auth_mode=userpass requires rpc_password".to_string()
                })?;
                builder = builder.basic_auth(user, Some(password));
            }
            BitcoinAuthMode::Cookie => {
                let cookie_file =
                    config.bitcoin.cookie_file.as_ref().ok_or_else(|| {
                        "BTC RPC auth_mode=cookie requires cookie_file".to_string()
                    })?;
                let cookie_path = config.resolve_runtime_path(cookie_file)?;
                let cookie = std::fs::read_to_string(&cookie_path).map_err(|e| {
                    let msg = format!(
                        "Failed to read BTC cookie file {}: {}",
                        cookie_path.display(),
                        e
                    );
                    warn!("{}", msg);
                    msg
                })?;
                let trimmed = cookie.trim();
                let (user, password) = trimmed.split_once(':').ok_or_else(|| {
                    format!(
                        "Invalid BTC cookie file format at {}",
                        cookie_path.display()
                    )
                })?;
                builder = builder.basic_auth(user, Some(password));
            }
        }

        let response = builder.send().await.map_err(|e| {
            let msg = format!(
                "Failed to send BTC RPC request to {} (method={}): {}",
                config.bitcoin.url, method, e
            );
            warn!("{}", msg);
            msg
        })?;

        let status = response.status();
        let envelope: JsonRpcEnvelope<T> = response.json().await.map_err(|e| {
            let msg = format!(
                "Failed to decode BTC RPC response from {} (method={}, status={}): {}",
                config.bitcoin.url, method, status, e
            );
            warn!("{}", msg);
            msg
        })?;

        if let Some(error) = envelope.error {
            let msg = if let Some(data) = error.data {
                format!(
                    "BTC RPC {} returned error {} ({}): {}",
                    method, error.code, error.message, data
                )
            } else {
                format!(
                    "BTC RPC {} returned error {}: {}",
                    method, error.code, error.message
                )
            };
            warn!("{}", msg);
            return Err(msg);
        }

        envelope.result.ok_or_else(|| {
            let msg = format!("BTC RPC {} returned neither result nor error", method);
            warn!("{}", msg);
            msg
        })
    }
}

pub fn decode_hex_quantity(value: &str) -> Result<u64, String> {
    let raw = value.trim_start_matches("0x");
    if raw.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(raw, 16).map_err(|e| format!("Invalid hex quantity {}: {}", value, e))
}
