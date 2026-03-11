use crate::status::SyncStatus;
use bitcoincore_rpc::bitcoin::OutPoint;
use jsonrpc_core::Result as JsonResult;
use jsonrpc_derive::rpc;
use serde::{Deserialize, Serialize};
use std::ops::Range;
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
    pub delta: i64,   // in Satoshi
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// Current stable height that balance-history exposes to downstream services.
    pub stable_height: u32,
    /// BTC block hash paired with `stable_height`, if a block commit exists for that height.
    pub stable_block_hash: Option<String>,
    /// Latest logical block commit at `stable_height`, encoded as lowercase hex.
    pub latest_block_commit: Option<String>,
    /// Version of the balance-history commit protocol exposed by this service.
    pub commit_protocol_version: String,
    /// Hash algorithm used to build `latest_block_commit`.
    pub commit_hash_algo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockCommitInfo {
    pub block_height: u32,
    pub btc_block_hash: String,
    pub balance_delta_root: String,
    pub block_commit: String,
    pub commit_protocol_version: String,
    pub commit_hash_algo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoInfo {
    pub txid: String,
    pub vout: u32,
    pub script_hash: String,
    pub value: u64,
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

    /// Gets the current stable snapshot metadata
    #[rpc(name = "get_snapshot_info")]
    fn get_snapshot_info(&self) -> JsonResult<SnapshotInfo>;

    /// Gets the logical block commit metadata at one exact block height.
    #[rpc(name = "get_block_commit")]
    fn get_block_commit(&self, block_height: u32) -> JsonResult<Option<BlockCommitInfo>>;

    /// Gets the current balance for the specified address
    #[rpc(name = "get_address_balance")]
    fn get_address_balance(&self, params: GetBalanceParams) -> JsonResult<Vec<AddressBalance>>;

    #[rpc(name = "get_addresses_balances")]
    fn get_addresses_balances(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<AddressBalance>>>;

    #[rpc(name = "get_address_balance_delta")]
    fn get_address_balance_delta(
        &self,
        params: GetBalanceParams,
    ) -> JsonResult<Vec<Option<AddressBalance>>>;

    #[rpc(name = "get_addresses_balances_delta")]
    fn get_addresses_balances_delta(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<Option<AddressBalance>>>>;

    #[rpc(name = "get_utxo")]
    fn get_utxo(&self, outpoint: OutPoint) -> JsonResult<Option<UtxoInfo>>;

    #[rpc(name = "stop")]
    fn stop(&self) -> JsonResult<()>;
}
