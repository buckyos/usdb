use serde::{Deserialize, Serialize};
use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use std::ops::Range;
use crate::status::SyncStatus;
use usdb_util::USDBScriptHash;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceParams {
    pub script_hash: USDBScriptHash,

    // Optional param 1: Specific block height
    pub block_height: Option<u32>,

    // Optional param 2: Specific block range
    pub block_range: Option<Range<u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalancesParams {
    pub script_hashes: Vec<USDBScriptHash>,

    // Optional param 1: Specific block height
    pub block_height: Option<u32>,

    // Optional param 2: Specific block range
    pub block_range: Option<Range<u32>>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBalance {
    pub block_height: u32,
    pub balance: u64, // in Satoshi
    pub delta: i64, // in Satoshi
}

#[rpc(server)]
pub trait BalanceHistoryRpc {
    /// Gets the current bitcoin chain network type
    #[rpc(name = "get_network_type")]
    fn get_network_type(&self) -> JsonResult<String>;
    
    /// Gets the current synced block height in the database
    #[rpc(name = "get_block_height")]
    fn get_block_height(&self) -> JsonResult<u64>;

    /// Gets the current sync status
    #[rpc(name = "get_sync_status")]
    fn get_sync_status(&self) -> JsonResult<SyncStatus>;

    /// Gets the current balance for the specified address
    #[rpc(name = "get_address_balance")]
    fn get_address_balance(&self, params: GetBalanceParams) -> JsonResult<Vec<AddressBalance>>;

    #[rpc(name = "get_addresses_balances")]
    fn get_addresses_balances(&self, params: GetBalancesParams) -> JsonResult<Vec<Vec<AddressBalance>>>;

    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}