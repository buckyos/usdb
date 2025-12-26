use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf};
use std::sync::Arc;

#[async_trait::async_trait]
pub trait BTCClient: Send + Sync {
    fn init(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    
    fn get_latest_block_height(&self) -> Result<u64, String>;
    fn get_block_hash(&self, block_height: u64) -> Result<BlockHash, String>;
    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String>;
    fn get_block_by_height(&self, block_height: u64) -> Result<Block, String>;
    async fn get_blocks(&self, start_height: u64, end_height: u64) -> Result<Vec<Block>, String>;
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String>;
}

pub type BTCClientRef = Arc<Box<dyn BTCClient>>;
