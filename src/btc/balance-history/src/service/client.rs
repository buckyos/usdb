use super::rpc::{
    AddressBalance, BlockCommitInfo, GetStateRefAtHeightParams, HistoricalSnapshotStateRef,
    ReadinessInfo, SnapshotInfo, UtxoInfo,
};
use crate::status::SyncStatus;
use bitcoincore_rpc::bitcoin::OutPoint;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use std::ops::Range;
use usdb_util::{ConsensusQueryContext, ConsensusRpcErrorData, USDBScriptHash};

pub struct RpcClient {
    url: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope<T> {
    result: Option<T>,
    error: Option<RpcErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorPayload {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcClient {
    // Create a lightweight JSON-RPC client for the local balance-history service.
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

    // Read the current stable snapshot metadata, including the latest logical block commit.
    pub async fn get_snapshot_info(&self) -> Result<SnapshotInfo, String> {
        self.rpc_call::<SnapshotInfo>(&self.url, "get_snapshot_info", json!([]))
            .await
    }

    pub async fn get_readiness(&self) -> Result<ReadinessInfo, String> {
        self.rpc_call::<ReadinessInfo>(&self.url, "get_readiness", json!([]))
            .await
    }

    pub async fn get_state_ref_at_height(
        &self,
        block_height: u32,
    ) -> Result<HistoricalSnapshotStateRef, String> {
        self.get_state_ref_at_height_with_context(block_height, None)
            .await
    }

    pub async fn get_state_ref_at_height_with_context(
        &self,
        block_height: u32,
        context: Option<ConsensusQueryContext>,
    ) -> Result<HistoricalSnapshotStateRef, String> {
        self.rpc_call::<HistoricalSnapshotStateRef>(
            &self.url,
            "get_state_ref_at_height",
            json!([GetStateRefAtHeightParams {
                block_height,
                context,
            }]),
        )
        .await
    }

    pub async fn get_block_commit(
        &self,
        block_height: u32,
    ) -> Result<Option<BlockCommitInfo>, String> {
        self.rpc_call::<Option<BlockCommitInfo>>(
            &self.url,
            "get_block_commit",
            json!([block_height]),
        )
        .await
    }

    // Query the current live UTXO view persisted by balance-history itself.
    pub async fn get_live_utxo(&self, outpoint: OutPoint) -> Result<Option<UtxoInfo>, String> {
        let params = json!([outpoint]);
        self.rpc_call::<Option<UtxoInfo>>(&self.url, "get_live_utxo", params)
            .await
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.rpc_call::<()>(&self.url, "stop", json!([])).await
    }

    pub async fn get_address_balance(
        &self,
        script_hash: USDBScriptHash,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<AddressBalance>, String> {
        let params = json!([{
            "script_hash": script_hash,
            "block_height": block_height,
            "block_range": block_range,
        }]);
        self.rpc_call::<Vec<AddressBalance>>(&self.url, "get_address_balance", params)
            .await
    }

    pub async fn get_addresses_balances(
        &self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<Vec<AddressBalance>>, String> {
        let params = json!([{
            "script_hashes": script_hashes,
            "block_height": block_height,
            "block_range": block_range,
        }]);

        self.rpc_call::<Vec<Vec<AddressBalance>>>(&self.url, "get_addresses_balances", params)
            .await
    }

    pub async fn get_address_balance_delta(
        &self,
        script_hash: USDBScriptHash,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<Option<AddressBalance>>, String> {
        let params = json!([{
            "script_hash": script_hash,
            "block_height": block_height,
            "block_range": block_range,
        }]);
        self.rpc_call::<Vec<Option<AddressBalance>>>(&self.url, "get_address_balance_delta", params)
            .await
    }

    pub async fn get_addresses_balances_delta(
        &self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<Range<u32>>,
    ) -> Result<Vec<Vec<Option<AddressBalance>>>, String> {
        let params = json!([{
            "script_hashes": script_hashes,
            "block_height": block_height,
            "block_range": block_range,
        }]);

        self.rpc_call::<Vec<Vec<Option<AddressBalance>>>>(
            &self.url,
            "get_addresses_balances_delta",
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

        let resp: RpcEnvelope<T> = self
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

        if let Some(err) = resp.error {
            let msg = Self::format_rpc_error(method, &err);
            log::error!("{}", msg);
            return Err(msg);
        }

        resp.result.ok_or_else(|| {
            let msg = format!(
                "RPC response for method {} missing both result and error",
                method
            );
            log::error!("{}", msg);
            msg
        })
    }

    fn format_rpc_error(method: &str, err: &RpcErrorPayload) -> String {
        let mut msg = format!(
            "RPC Error: method={}, code={}, message={}",
            method, err.code, err.message
        );

        if let Some(data) = &err.data {
            if let Ok(structured) = serde_json::from_value::<ConsensusRpcErrorData>(data.clone()) {
                msg.push_str(&format!(
                    ", service={}, requested_height={:?}, local_synced_height={:?}, upstream_stable_height={:?}, consensus_ready={:?}, detail={:?}, actual_state={:?}",
                    structured.service,
                    structured.requested_height,
                    structured.local_synced_height,
                    structured.upstream_stable_height,
                    structured.consensus_ready,
                    structured.detail,
                    structured.actual_state,
                ));
            } else {
                msg.push_str(&format!(", data={}", data));
            }
        }

        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_rpc_error_with_structured_consensus_data() {
        let msg = RpcClient::format_rpc_error(
            "get_snapshot_info",
            &RpcErrorPayload {
                code: -32041,
                message: "SNAPSHOT_NOT_READY".to_string(),
                data: Some(json!({
                    "service": "balance-history",
                    "requested_height": 12,
                    "local_synced_height": null,
                    "upstream_stable_height": 10,
                    "consensus_ready": false,
                    "expected_state": {},
                    "actual_state": {
                        "stable_height": 10,
                        "stable_block_hash": "aa"
                    },
                    "detail": "stable snapshot incomplete"
                })),
            },
        );

        assert!(msg.contains("method=get_snapshot_info"));
        assert!(msg.contains("code=-32041"));
        assert!(msg.contains("message=SNAPSHOT_NOT_READY"));
        assert!(msg.contains("service=balance-history"));
        assert!(msg.contains("requested_height=Some(12)"));
        assert!(msg.contains("upstream_stable_height=Some(10)"));
    }

    #[test]
    fn test_format_rpc_error_with_unstructured_data_falls_back_to_json() {
        let msg = RpcClient::format_rpc_error(
            "get_address_balance",
            &RpcErrorPayload {
                code: -32040,
                message: "HEIGHT_NOT_SYNCED".to_string(),
                data: Some(json!({"foo": "bar"})),
            },
        );

        assert!(msg.contains("method=get_address_balance"));
        assert!(msg.contains("data={\"foo\":\"bar\"}"));
    }
}

pub type RpcClientRef = std::sync::Arc<RpcClient>;
