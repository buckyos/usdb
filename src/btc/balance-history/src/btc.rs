use bitcoincore_rpc::bitcoin::Block;
use bitcoincore_rpc::{Client, Auth, RpcApi};

pub struct BTCClient {
    client: Client,
}

impl BTCClient {
    pub fn new(rpc_url: String, auth: Auth) -> Result<Self, String> {
        let client = Client::new(&rpc_url, auth).map_err(|e| {
            let msg = format!("Failed to create BTC RPC client: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let ret = Self { client };

        Ok(ret)
    }

    pub fn get_block(&self, block_height: u64) -> Result<Block, String> {
        // First get the block hash for the given height
        let hash = self.client.get_block_hash(block_height).map_err(|error| {
            let msg = format!("get_block_hash failed: {}", error);
            error!("{}", msg);
            msg
        })?;

        // Now get the block using the hash
        self.client.get_block(&hash).map_err(|error| {
            let msg = format!("get_block failed: {}", error);
            error!("{}", msg);
            msg
        })
    }
}