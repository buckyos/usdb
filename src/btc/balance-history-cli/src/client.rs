use balance_history::SyncStatus;
use bitcoincore_rpc::bitcoin::ScriptHash;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::ops::Range;

pub struct RpcClient {
    url: String,
    client: Client,
}

impl RpcClient {
    pub fn new(url: &str) -> Result<Self, String> {
        let client = Client::builder().build().map_err(|e| {
            let msg = format!("Failed to build HTTP client: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        Ok(Self {
            url: url.to_string(),
            client,
        })
    }

    pub async fn get_network_type(&self) -> Result<String, String> {
        self.rpc_call::<String>(&self.url, "get_network_type", json!([]))
            .await
    }

    pub async fn get_block_height(&self) -> Result<u64, String> {
        self.rpc_call::<u64>(&self.url, "get_block_height", json!([]))
            .await
    }

    pub async fn get_sync_status(&self) -> Result<SyncStatus, String> {
        self.rpc_call::<SyncStatus>(&self.url, "get_sync_status", json!([]))
            .await
    }

    pub async fn get_address_balance(
        &self,
        script_hash: ScriptHash,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<balance_history::AddressBalance>, String> {
        let params = json!({
            "script_hash": script_hash,
            "block_height": block_height,
            "block_range": block_range,
        });

        self.rpc_call::<Vec<balance_history::AddressBalance>>(
            &self.url,
            "get_address_balance",
            params,
        )
        .await
    }

    pub async fn get_addresses_balances(
        &self,
        script_hashes: Vec<ScriptHash>,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<Vec<balance_history::AddressBalance>>, String> {
        let params = json!({
            "script_hashes": script_hashes,
            "block_height": block_height,
            "block_range": block_range,
        });

        self.rpc_call::<Vec<Vec<balance_history::AddressBalance>>>(
            &self.url,
            "get_addresses_balances",
            params,
        )
        .await
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        let resp: Value = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let msg = format!("Failed to send RPC request: {}", e);
                log::error!("{}", msg);
                msg
            })?
            .json()
            .await
            .map_err(|e| {
                let msg = format!("Failed to parse RPC response: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        if let Some(err) = resp.get("error") {
            let msg = format!("RPC Error: {:?}", err);
            log::error!("{}", msg);
            return Err(msg);
        }

        Ok(serde_json::from_value(resp["result"].clone()).map_err(|e| {
            let msg = format!("Failed to parse RPC result: {}", e);
            log::error!("{}", msg);
            msg
        })?)
    }
}
