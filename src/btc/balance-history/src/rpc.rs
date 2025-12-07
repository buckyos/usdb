
use serde::{Deserialize, Serialize};
use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use std::ops::Range;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBalanceParams {
    pub address: String,

    // Optional param 1: Specific block height
    pub block_height: Option<u64>,

    // Optional param 2: Specific block range
    pub block_range: Option<Range<u64>>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBalance {
    pub block_height: u64,
    pub balance: u64, // in Satoshi
}

#[rpc(server)]
pub trait BalanceHistoryRpc {
    /// Gets the current synced block height in the database
    #[rpc(name = "get_block_height")]
    fn get_block_height(&self) -> JsonResult<u64>;

    /// Gets the current balance for the specified address
    #[rpc(name = "get_address_balance")]
    fn get_address_balance(&self, address: String) -> JsonResult<Vec<AddressBalance>>;
}