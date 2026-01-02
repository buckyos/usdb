use super::client::BTCClient;
use super::client::BTCClientType;
use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf};
use usdb_util::BTCRpcClient;


#[async_trait::async_trait]
impl BTCClient for BTCRpcClient {
    fn get_type(&self) -> BTCClientType {
        BTCClientType::RPC
    }

    fn init(&self) -> Result<(), String> {
        // Just try to get latest block height to verify the connection
        let height = self.get_latest_block_height()?;
        info!(
            "BTC RPC client initialized, latest block height: {}",
            height
        );

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        // No specific stop action needed for the RPC client
        info!("BTC RPC client stopped.");

        Ok(())
    }

    fn on_sync_complete(&self, _block_height: u32) -> Result<(), String> {
        // Do nothing for RPC client
        Ok(())
    }

    fn get_latest_block_height(&self) -> Result<u32, String> {
        self.get_latest_block_height()
    }

    fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
        self.get_block_hash(block_height)
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        self.get_block_by_hash(block_hash)
    }

    fn get_block_by_height(&self, block_height: u32) -> Result<Block, String> {
        self.get_block(block_height)
    }

    async fn get_blocks(&self, start_height: u32, end_height: u32) -> Result<Vec<Block>, String> {
        self.get_blocks(start_height, end_height).await
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        self.get_utxo(outpoint)
    }
}
