use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BTCClientType {
    RPC,
    LocalLoader,
}

#[async_trait::async_trait]
pub trait BTCClient: Send + Sync {
    fn get_type(&self) -> BTCClientType;

    fn init(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;

    // Called when sync is complete
    // This may be called multiple times
    fn on_sync_complete(&self, block_height: u32) -> Result<(), String>;

    fn get_latest_block_height(&self) -> Result<u32, String>;
    fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String>;
    /// Poll height without nesting the data-read retry policy.
    fn poll_block_height(&self) -> Result<u32, String> {
        self.get_latest_block_height()
    }
    /// Poll the canonical anchor once so idle failures use outer service backoff.
    fn poll_block_hash(&self, height: u32) -> Result<BlockHash, String> {
        self.get_block_hash(height)
    }
    /// Cumulative RPC read counters; non-RPC fixtures report zero.
    fn read_stats(&self) -> usdb_util::RpcReadStats {
        usdb_util::RpcReadStats::default()
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String>;
    fn get_block_by_height(&self, block_height: u32) -> Result<Block, String>;
    async fn get_blocks(&self, start_height: u32, end_height: u32) -> Result<Vec<Block>, String>;
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String>;
}

pub type BTCClientRef = Arc<Box<dyn BTCClient>>;
