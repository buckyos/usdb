use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, ScriptBuf};
use bitcoincore_rpc::{Auth, Client, RpcApi};

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

     pub fn get_latest_block_height(&self) -> Result<u64, String> {
        self.client.get_block_count().map_err(|error| {
            let msg = format!("get_block_count failed: {}", error);
            error!("{}", msg);
            msg
        })
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

    pub fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        let ret = self
            .client
            .get_tx_out(&outpoint.txid, outpoint.vout, Some(false))
            .map_err(|error| {
                let msg = format!("get_tx_out failed: {}", error);
                error!("{}", msg);
                msg
            })?;

        if let Some(tx_out) = ret {
            let script = tx_out.script_pub_key.script().map_err(|e| {
                let msg = format!("Failed to get script from tx_out: {} {}", outpoint, e);
                error!("{}", msg);
                msg
            })?;

            Ok((script, tx_out.value))
        } else {
            let msg = format!("UTXO not found for outpoint: {}", outpoint);
            error!("{}", msg);
            Err(msg)
        }
    }
}

pub type BTCClientRef = std::sync::Arc<BTCClient>;