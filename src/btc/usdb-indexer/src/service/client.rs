use super::rpc::*;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

/// JSON-RPC client for querying and controlling `usdb-indexer`.
///
/// This client wraps all public RPC methods exposed by the indexer service and
/// returns strongly typed response structs defined in `service::rpc`.
pub struct RpcClient {
    url: String,
    client: Client,
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

    /// Returns indexer synchronization progress and dependency status snapshot.
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

    /// Returns the currently adopted upstream snapshot metadata.
    pub async fn get_snapshot_info(&self) -> Result<Option<IndexerSnapshotInfo>, String> {
        self.rpc_call::<Option<IndexerSnapshotInfo>>("get_snapshot_info", json!([]))
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

        let resp: Value = self
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

        if let Some(err) = resp.get("error") {
            let msg = format!(
                "USDB indexer RPC returned error: method={}, url={}, error={:?}, response={:?}",
                method, self.url, err, resp
            );
            error!("{}", msg);
            return Err(msg);
        }

        serde_json::from_value(resp["result"].clone()).map_err(|e| {
            let msg = format!(
                "Failed to deserialize usdb-indexer RPC result: method={}, url={}, error={}, response={:?}",
                method, self.url, e, resp
            );
            error!("{}", msg);
            msg
        })
    }
}

/// Shared `Arc` wrapper type for `RpcClient`.
pub type RpcClientRef = std::sync::Arc<RpcClient>;
