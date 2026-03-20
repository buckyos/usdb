use super::rpc::*;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use usdb_util::{ConsensusQueryContext, ConsensusRpcErrorData};

/// JSON-RPC client for querying and controlling `usdb-indexer`.
///
/// This client wraps all public RPC methods exposed by the indexer service and
/// returns strongly typed response structs defined in `service::rpc`.
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
    /// Creates a new RPC client bound to a target server URL.
    ///
    /// # Arguments
    /// * `url` - Full JSON-RPC endpoint, for example `http://127.0.0.1:28020`.
    ///
    /// # Returns
    /// * `Ok(Self)` if the underlying HTTP client is constructed successfully.
    /// * `Err(String)` with context if client construction fails.
    pub fn new(url: &str) -> Result<Self, String> {
        let client = Client::builder().build().map_err(|e| {
            let msg = format!("Failed to build HTTP client: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            url: url.to_string(),
            client,
        })
    }

    /// Returns service metadata such as API version and advertised feature list.
    ///
    /// # Returns
    /// * `Ok(RpcInfo)` on success.
    /// * `Err(String)` if the RPC call fails or response deserialization fails.
    pub async fn get_rpc_info(&self) -> Result<RpcInfo, String> {
        self.rpc_call::<RpcInfo>("get_rpc_info", json!([])).await
    }

    /// Returns the current Bitcoin network type of the running indexer.
    ///
    /// # Returns
    /// * `Ok(String)` such as `mainnet`, `testnet`, `signet`, or `regtest`.
    /// * `Err(String)` if the RPC call fails.
    pub async fn get_network_type(&self) -> Result<String, String> {
        self.rpc_call::<String>("get_network_type", json!([])).await
    }

    /// Returns indexer synchronization progress, local durable height, and upstream stable-height snapshot.
    ///
    /// # Returns
    /// * `Ok(IndexerSyncStatus)` on success.
    /// * `Err(String)` if the RPC call fails.
    pub async fn get_sync_status(&self) -> Result<IndexerSyncStatus, String> {
        self.rpc_call::<IndexerSyncStatus>("get_sync_status", json!([]))
            .await
    }

    /// Returns the latest block height fully committed by the indexer.
    ///
    /// # Returns
    /// * `Ok(Some(height))` when the indexer has synced at least one block.
    /// * `Ok(None)` when no synced height is available yet.
    /// * `Err(String)` if the RPC call fails.
    pub async fn get_synced_block_height(&self) -> Result<Option<u64>, String> {
        self.rpc_call::<Option<u64>>("get_synced_block_height", json!([]))
            .await
    }

    /// Returns the current upstream snapshot metadata.
    pub async fn get_snapshot_info(&self) -> Result<Option<IndexerSnapshotInfo>, String> {
        self.rpc_call::<Option<IndexerSnapshotInfo>>("get_snapshot_info", json!([]))
            .await
    }

    /// Returns local pass block commit metadata at a target height.
    ///
    /// # Arguments
    /// * `block_height` - Optional query height. `None` resolves to current local synced height.
    ///
    /// # Returns
    /// * `Ok(Some(PassBlockCommitInfo))` when a local pass block commit row exists at the height.
    /// * `Ok(None)` when the height is resolved successfully but no local pass commit row exists.
    /// * `Err(String)` if the RPC call fails.
    pub async fn get_pass_block_commit(
        &self,
        block_height: Option<u32>,
    ) -> Result<Option<PassBlockCommitInfo>, String> {
        self.rpc_call::<Option<PassBlockCommitInfo>>(
            "get_pass_block_commit",
            json!([{
                "block_height": block_height,
            }]),
        )
        .await
    }

    /// Returns the current locally durable core-state commit anchored to the current upstream snapshot.
    pub async fn get_local_state_commit_info(
        &self,
    ) -> Result<Option<LocalStateCommitInfo>, String> {
        self.rpc_call::<Option<LocalStateCommitInfo>>("get_local_state_commit_info", json!([]))
            .await
    }

    /// Returns the top-level system-state id for downstream consumers such as ETHW.
    pub async fn get_system_state_info(&self) -> Result<Option<SystemStateInfo>, String> {
        self.rpc_call::<Option<SystemStateInfo>>("get_system_state_info", json!([]))
            .await
    }

    /// Returns the exact historical upstream/local/system state reference at one BTC height.
    pub async fn get_state_ref_at_height(
        &self,
        block_height: u32,
    ) -> Result<HistoricalStateRefInfo, String> {
        self.get_state_ref_at_height_with_context(block_height, None)
            .await
    }

    /// Returns the exact historical state reference at one BTC height while
    /// also enforcing optional caller-supplied consensus selectors.
    pub async fn get_state_ref_at_height_with_context(
        &self,
        block_height: u32,
        context: Option<ConsensusQueryContext>,
    ) -> Result<HistoricalStateRefInfo, String> {
        self.rpc_call::<HistoricalStateRefInfo>(
            "get_state_ref_at_height",
            json!([GetStateRefAtHeightParams {
                block_height,
                context,
            }]),
        )
        .await
    }

    /// Returns structured readiness state for liveness, local queries, and consensus use.
    pub async fn get_readiness(&self) -> Result<ReadinessInfo, String> {
        self.rpc_call::<ReadinessInfo>("get_readiness", json!([]))
            .await
    }

    /// Returns a pass snapshot resolved at a target height.
    ///
    /// # Arguments
    /// * `inscription_id` - Pass inscription id (for example `txidi0`).
    /// * `at_height` - Optional query height. `None` resolves to current local synced height.
    ///
    /// # Returns
    /// * `Ok(Some(PassSnapshot))` if pass exists and has visible history at that height.
    /// * `Ok(None)` if pass does not exist or has no history at that height.
    /// * `Err(String)` for transport, RPC, or deserialization errors.
    pub async fn get_pass_snapshot(
        &self,
        inscription_id: &str,
        at_height: Option<u32>,
    ) -> Result<Option<PassSnapshot>, String> {
        self.rpc_call::<Option<PassSnapshot>>(
            "get_pass_snapshot",
            json!([{
                "inscription_id": inscription_id,
                "at_height": at_height,
            }]),
        )
        .await
    }

    /// Returns active passes snapshot at a target height with pagination.
    ///
    /// # Arguments
    /// * `at_height` - Optional query height. `None` resolves to current local synced height.
    /// * `page` - Zero-based page index.
    /// * `page_size` - Number of rows per page.
    ///
    /// # Returns
    /// * `Ok(ActivePassesAtHeight)` including `resolved_height` and page items.
    /// * `Err(String)` if the request is rejected or fails.
    pub async fn get_active_passes_at_height(
        &self,
        at_height: Option<u32>,
        page: usize,
        page_size: usize,
    ) -> Result<ActivePassesAtHeight, String> {
        self.rpc_call::<ActivePassesAtHeight>(
            "get_active_passes_at_height",
            json!([{
                "at_height": at_height,
                "page": page,
                "page_size": page_size,
            }]),
        )
        .await
    }

    /// Returns pass-state aggregate counts at a target height.
    ///
    /// # Arguments
    /// * `at_height` - Optional query height. `None` resolves to current local synced height.
    ///
    /// # Returns
    /// * `Ok(PassStatsAtHeight)` on success.
    /// * `Err(String)` if request fails.
    pub async fn get_pass_stats_at_height(
        &self,
        at_height: Option<u32>,
    ) -> Result<PassStatsAtHeight, String> {
        self.rpc_call::<PassStatsAtHeight>(
            "get_pass_stats_at_height",
            json!([{
                "at_height": at_height,
            }]),
        )
        .await
    }

    /// Returns pass history events in a closed height range with pagination.
    ///
    /// # Arguments
    /// * `inscription_id` - Target inscription id.
    /// * `from_height` - Inclusive range start.
    /// * `to_height` - Inclusive range end.
    /// * `order` - Optional sort order: `Some(\"asc\")` or `Some(\"desc\")`.
    /// * `page` - Zero-based page index.
    /// * `page_size` - Number of rows per page.
    ///
    /// # Returns
    /// * `Ok(PassHistoryPage)` on success.
    /// * `Err(String)` if parameters are invalid or request fails.
    pub async fn get_pass_history(
        &self,
        inscription_id: &str,
        from_height: u32,
        to_height: u32,
        order: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> Result<PassHistoryPage, String> {
        self.rpc_call::<PassHistoryPage>(
            "get_pass_history",
            json!([{
                "inscription_id": inscription_id,
                "from_height": from_height,
                "to_height": to_height,
                "order": order,
                "page": page,
                "page_size": page_size,
            }]),
        )
        .await
    }

    /// Returns the unique active pass owned by an address at a target height.
    ///
    /// # Arguments
    /// * `owner` - Owner script hash string.
    /// * `at_height` - Optional query height. `None` resolves to current local synced height.
    ///
    /// # Returns
    /// * `Ok(Some(PassSnapshot))` when exactly one active pass exists for owner.
    /// * `Ok(None)` when owner has no active pass at that height.
    /// * `Err(String)` when request fails or server detects invariant violation.
    pub async fn get_owner_active_pass_at_height(
        &self,
        owner: &str,
        at_height: Option<u32>,
    ) -> Result<Option<PassSnapshot>, String> {
        self.rpc_call::<Option<PassSnapshot>>(
            "get_owner_active_pass_at_height",
            json!([{
                "owner": owner,
                "at_height": at_height,
            }]),
        )
        .await
    }

    /// Returns pass energy snapshot at a target height.
    ///
    /// # Arguments
    /// * `inscription_id` - Target inscription id.
    /// * `block_height` - Optional query height. `None` resolves to current local synced height.
    /// * `mode` - Optional query mode: `exact` or `at_or_before`.
    ///
    /// # Returns
    /// * `Ok(PassEnergySnapshot)` on success.
    /// * `Err(String)` if the record is not found or request fails.
    pub async fn get_pass_energy(
        &self,
        inscription_id: &str,
        block_height: Option<u32>,
        mode: Option<&str>,
    ) -> Result<PassEnergySnapshot, String> {
        self.rpc_call::<PassEnergySnapshot>(
            "get_pass_energy",
            json!([{
                "inscription_id": inscription_id,
                "block_height": block_height,
                "mode": mode,
            }]),
        )
        .await
    }

    /// Returns paginated pass energy records inside a closed height range.
    ///
    /// # Arguments
    /// * `inscription_id` - Target inscription id.
    /// * `from_height` - Inclusive range start.
    /// * `to_height` - Inclusive range end.
    /// * `order` - Optional sort order: `Some("asc")` or `Some("desc")`.
    /// * `page` - Zero-based page index.
    /// * `page_size` - Number of rows per page.
    ///
    /// # Returns
    /// * `Ok(PassEnergyRangePage)` containing energy timeline records.
    /// * `Err(String)` if request fails.
    pub async fn get_pass_energy_range(
        &self,
        inscription_id: &str,
        from_height: u32,
        to_height: u32,
        order: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> Result<PassEnergyRangePage, String> {
        self.rpc_call::<PassEnergyRangePage>(
            "get_pass_energy_range",
            json!([{
                "inscription_id": inscription_id,
                "from_height": from_height,
                "to_height": to_height,
                "order": order,
                "page": page,
                "page_size": page_size,
            }]),
        )
        .await
    }

    /// Returns pass energy leaderboard at a target height.
    ///
    /// # Arguments
    /// * `at_height` - Optional query height. `None` resolves to current local synced height.
    /// * `scope` - Optional leaderboard scope: `active`, `active_dormant`, or `all`.
    /// * `page` - Zero-based page index.
    /// * `page_size` - Number of rows per page.
    ///
    /// # Returns
    /// * `Ok(PassEnergyLeaderboardPage)` on success.
    /// * `Err(String)` if request fails.
    pub async fn get_pass_energy_leaderboard(
        &self,
        at_height: Option<u32>,
        scope: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> Result<PassEnergyLeaderboardPage, String> {
        self.rpc_call::<PassEnergyLeaderboardPage>(
            "get_pass_energy_leaderboard",
            json!([{
                "at_height": at_height,
                "scope": scope,
                "page": page,
                "page_size": page_size,
            }]),
        )
        .await
    }

    /// Returns active-balance snapshot exactly at `block_height`.
    ///
    /// # Arguments
    /// * `block_height` - Exact snapshot height.
    ///
    /// # Returns
    /// * `Ok(RpcActiveBalanceSnapshot)` when snapshot exists.
    /// * `Err(String)` if snapshot does not exist or request fails.
    pub async fn get_active_balance_snapshot(
        &self,
        block_height: u32,
    ) -> Result<RpcActiveBalanceSnapshot, String> {
        self.rpc_call::<RpcActiveBalanceSnapshot>(
            "get_active_balance_snapshot",
            json!([{
                "block_height": block_height,
            }]),
        )
        .await
    }

    /// Returns the latest persisted active-balance snapshot.
    ///
    /// # Returns
    /// * `Ok(Some(RpcActiveBalanceSnapshot))` when at least one snapshot exists.
    /// * `Ok(None)` when snapshot table is empty.
    /// * `Err(String)` if the request fails.
    pub async fn get_latest_active_balance_snapshot(
        &self,
    ) -> Result<Option<RpcActiveBalanceSnapshot>, String> {
        self.rpc_call::<Option<RpcActiveBalanceSnapshot>>(
            "get_latest_active_balance_snapshot",
            json!([]),
        )
        .await
    }

    /// Returns invalid mint passes in a closed height range with pagination.
    ///
    /// # Arguments
    /// * `error_code` - Optional invalid code filter.
    /// * `from_height` - Inclusive range start.
    /// * `to_height` - Inclusive range end.
    /// * `page` - Zero-based page index.
    /// * `page_size` - Number of rows per page.
    ///
    /// # Returns
    /// * `Ok(InvalidPassesPage)` on success.
    /// * `Err(String)` if request fails.
    pub async fn get_invalid_passes(
        &self,
        error_code: Option<&str>,
        from_height: u32,
        to_height: u32,
        page: usize,
        page_size: usize,
    ) -> Result<InvalidPassesPage, String> {
        self.rpc_call::<InvalidPassesPage>(
            "get_invalid_passes",
            json!([{
                "error_code": error_code,
                "from_height": from_height,
                "to_height": to_height,
                "page": page,
                "page_size": page_size,
            }]),
        )
        .await
    }

    /// Sends a graceful stop signal to the running indexer service.
    ///
    /// # Returns
    /// * `Ok(())` if stop signal is accepted.
    /// * `Err(String)` if the request fails.
    pub async fn stop(&self) -> Result<(), String> {
        self.rpc_call::<()>("stop", json!([])).await
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
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
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to send usdb-indexer RPC request: method={}, url={}, error={}",
                    method, self.url, e
                );
                error!("{}", msg);
                msg
            })?
            .json()
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to parse usdb-indexer RPC response: method={}, url={}, error={}",
                    method, self.url, e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(err) = resp.error {
            let msg = self.format_rpc_error(method, &err);
            error!("{}", msg);
            return Err(msg);
        }

        resp.result.ok_or_else(|| {
            let msg = format!(
                "USDB indexer RPC response missing both result and error: method={}, url={}",
                method, self.url
            );
            error!("{}", msg);
            msg
        })
    }

    fn format_rpc_error(&self, method: &str, err: &RpcErrorPayload) -> String {
        let mut msg = format!(
            "USDB indexer RPC returned error: method={}, url={}, code={}, message={}",
            method, self.url, err.code, err.message
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

/// Shared `Arc` wrapper type for `RpcClient`.
pub type RpcClientRef = std::sync::Arc<RpcClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_rpc_error_with_structured_consensus_data() {
        let client = RpcClient {
            url: "http://127.0.0.1:28020".to_string(),
            client: Client::new(),
        };
        let msg = client.format_rpc_error(
            "get_system_state_info",
            &RpcErrorPayload {
                code: -32041,
                message: "SNAPSHOT_NOT_READY".to_string(),
                data: Some(json!({
                    "service": "usdb-indexer",
                    "requested_height": 120,
                    "local_synced_height": 120,
                    "upstream_stable_height": 120,
                    "consensus_ready": false,
                    "expected_state": {},
                    "actual_state": {
                        "snapshot_id": "aa"
                    },
                    "detail": "No adopted upstream snapshot anchor available"
                })),
            },
        );

        assert!(msg.contains("method=get_system_state_info"));
        assert!(msg.contains("code=-32041"));
        assert!(msg.contains("message=SNAPSHOT_NOT_READY"));
        assert!(msg.contains("service=usdb-indexer"));
        assert!(msg.contains("requested_height=Some(120)"));
    }

    #[test]
    fn test_format_rpc_error_with_unstructured_data_falls_back_to_json() {
        let client = RpcClient {
            url: "http://127.0.0.1:28020".to_string(),
            client: Client::new(),
        };
        let msg = client.format_rpc_error(
            "get_active_balance_snapshot",
            &RpcErrorPayload {
                code: -32047,
                message: "NO_RECORD".to_string(),
                data: Some(json!({"foo": "bar"})),
            },
        );

        assert!(msg.contains("method=get_active_balance_snapshot"));
        assert!(msg.contains("data={\"foo\":\"bar\"}"));
    }
}
