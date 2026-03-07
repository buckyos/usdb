use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcInfo {
    pub service: String,
    pub api_version: String,
    pub network: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSyncStatus {
    pub genesis_block_height: u32,
    pub synced_block_height: Option<u32>,
    pub latest_depend_synced_block_height: u32,
    pub current: u32,
    pub total: u32,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassSnapshotParams {
    pub inscription_id: String,
    pub at_height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassSnapshot {
    pub inscription_id: String,
    pub inscription_number: i32,
    pub mint_txid: String,
    pub mint_block_height: u32,
    pub mint_owner: String,
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<String>,
    pub invalid_code: Option<String>,
    pub invalid_reason: Option<String>,
    pub owner: String,
    pub state: String,
    pub satpoint: String,
    pub last_event_id: i64,
    pub last_event_type: String,
    pub resolved_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetActivePassesAtHeightParams {
    pub at_height: Option<u32>,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePassItem {
    pub inscription_id: String,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePassesAtHeight {
    pub resolved_height: u32,
    pub items: Vec<ActivePassItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassHistoryParams {
    pub inscription_id: String,
    pub from_height: u32,
    pub to_height: u32,
    pub order: Option<String>,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassHistoryEvent {
    pub event_id: i64,
    pub inscription_id: String,
    pub block_height: u32,
    pub event_type: String,
    pub state: String,
    pub owner: String,
    pub satpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassHistoryPage {
    pub resolved_height: u32,
    pub items: Vec<PassHistoryEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetOwnerActivePassAtHeightParams {
    pub owner: String,
    pub at_height: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyParams {
    pub inscription_id: String,
    pub block_height: Option<u32>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergySnapshot {
    pub inscription_id: String,
    pub query_block_height: u32,
    pub record_block_height: u32,
    pub state: String,
    pub active_block_height: u32,
    pub owner_address: String,
    pub owner_balance: u64,
    pub owner_delta: i64,
    pub energy: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPassEnergyRangeParams {
    pub inscription_id: String,
    pub from_height: u32,
    pub to_height: u32,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyRangeItem {
    pub inscription_id: String,
    pub record_block_height: u32,
    pub state: String,
    pub active_block_height: u32,
    pub owner_address: String,
    pub owner_balance: u64,
    pub owner_delta: i64,
    pub energy: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassEnergyRangePage {
    pub resolved_height: u32,
    pub items: Vec<PassEnergyRangeItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetActiveBalanceSnapshotParams {
    pub block_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcActiveBalanceSnapshot {
    pub block_height: u32,
    pub total_balance: u64,
    pub active_address_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInvalidPassesParams {
    pub error_code: Option<String>,
    pub from_height: u32,
    pub to_height: u32,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidPassItem {
    pub inscription_id: String,
    pub inscription_number: i32,
    pub mint_txid: String,
    pub mint_block_height: u32,
    pub mint_owner: String,
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<String>,
    pub invalid_code: Option<String>,
    pub invalid_reason: Option<String>,
    pub owner: String,
    pub state: String,
    pub satpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidPassesPage {
    pub resolved_height: u32,
    pub items: Vec<InvalidPassItem>,
}

#[rpc(server)]
pub trait UsdbIndexerRpc {
    #[rpc(name = "get_rpc_info")]
    fn get_rpc_info(&self) -> JsonResult<RpcInfo>;

    #[rpc(name = "get_network_type")]
    fn get_network_type(&self) -> JsonResult<String>;

    #[rpc(name = "get_sync_status")]
    fn get_sync_status(&self) -> JsonResult<IndexerSyncStatus>;

    #[rpc(name = "get_synced_block_height")]
    fn get_synced_block_height(&self) -> JsonResult<Option<u64>>;

    #[rpc(name = "get_pass_snapshot")]
    fn get_pass_snapshot(&self, params: GetPassSnapshotParams) -> JsonResult<Option<PassSnapshot>>;

    #[rpc(name = "get_active_passes_at_height")]
    fn get_active_passes_at_height(
        &self,
        params: GetActivePassesAtHeightParams,
    ) -> JsonResult<ActivePassesAtHeight>;

    #[rpc(name = "get_pass_history")]
    fn get_pass_history(&self, params: GetPassHistoryParams) -> JsonResult<PassHistoryPage>;

    #[rpc(name = "get_owner_active_pass_at_height")]
    fn get_owner_active_pass_at_height(
        &self,
        params: GetOwnerActivePassAtHeightParams,
    ) -> JsonResult<Option<PassSnapshot>>;

    #[rpc(name = "get_pass_energy")]
    fn get_pass_energy(&self, params: GetPassEnergyParams) -> JsonResult<PassEnergySnapshot>;

    #[rpc(name = "get_pass_energy_range")]
    fn get_pass_energy_range(
        &self,
        params: GetPassEnergyRangeParams,
    ) -> JsonResult<PassEnergyRangePage>;

    #[rpc(name = "get_invalid_passes")]
    fn get_invalid_passes(&self, params: GetInvalidPassesParams) -> JsonResult<InvalidPassesPage>;

    #[rpc(name = "get_active_balance_snapshot")]
    fn get_active_balance_snapshot(
        &self,
        params: GetActiveBalanceSnapshotParams,
    ) -> JsonResult<RpcActiveBalanceSnapshot>;

    #[rpc(name = "get_latest_active_balance_snapshot")]
    fn get_latest_active_balance_snapshot(&self) -> JsonResult<Option<RpcActiveBalanceSnapshot>>;

    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}
