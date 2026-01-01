use bitcoincore_rpc::bitcoin::OutPoint;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

#[derive(Debug, Clone)]
pub struct UTXOEntry {
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

pub type UTXOEntryRef = Arc<UTXOEntry>;
pub type OutPointRef = Arc<OutPoint>;

#[derive(Debug, Clone)]
pub struct BalanceHistoryData {
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub type BalanceHistoryDataRef = Arc<BalanceHistoryData>;