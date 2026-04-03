use crate::models::{BalanceHistoryReadiness, UsdbIndexerReadiness};
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
}

pub fn decode_hex_quantity(value: &str) -> Result<u64, String> {
    let raw = value.trim_start_matches("0x");
    if raw.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(raw, 16).map_err(|e| format!("Invalid hex quantity {}: {}", value, e))
}
