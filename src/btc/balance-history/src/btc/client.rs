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
    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String>;
    fn get_block_by_height(&self, block_height: u32) -> Result<Block, String>;
    async fn get_blocks(&self, start_height: u32, end_height: u32) -> Result<Vec<Block>, String>;
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String>;
}

pub type BTCClientRef = Arc<Box<dyn BTCClient>>;
