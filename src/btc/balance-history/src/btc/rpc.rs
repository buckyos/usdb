use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, ScriptBuf, BlockHash};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use jsonrpsee::core::client::{self, BatchResponse, ClientT};
use std::sync::{Arc, RwLock};
use super::client::BTCClient;

pub struct BTCRpcClient {
    rpc_url: String,
    auth: Auth,
    client: RwLock<Option<Arc<Client>>>,
}

impl BTCRpcClient {
    pub fn new(rpc_url: String, auth: Auth) -> Result<Self, String> {
        /*
        // We should not create the client here, as the auth cookie file may not exists because bitcoind not started yet
        // We should create the client on demand and update it when error occurs
        let client = Client::new(&rpc_url, auth.clone()).map_err(|e| {
            let msg = format!("Failed to create BTC RPC client: {}", e);
            error!("{}", msg);
            msg
        })?;
        */

        let ret = Self {
            rpc_url,
            auth,
            client: RwLock::new(None),
        };

        Ok(ret)
    }

    fn update_client(&self) -> Result<(), String> {
        let new_client = Client::new(&self.rpc_url, self.auth.clone()).map_err(|e| {
            let msg = format!("Failed to update BTC RPC client: {}", e);
            error!("{}", msg);
            msg
        })?;

        let arc_client = Arc::new(new_client);

        let mut write_guard = self.client.write().unwrap();
        *write_guard = Some(arc_client);

        info!("BTC RPC client updated successfully.");
        Ok(())
    }

    fn client(&self) -> Result<Arc<Client>, String> {
        {
            let read_guard = self.client.read().unwrap();
            if let Some(client) = &*read_guard {
                return Ok(client.clone());
            }
        }

        // In some case the client will create multiple times, it's ok
        let msg = "BTC RPC client is not initialized, attempting to update.".to_string();
        warn!("{}", msg);
        self.update_client()?;

        let read_guard = self.client.read().unwrap();
        if let Some(client) = &*read_guard {
            return Ok(client.clone());
        }

        Err("Failed to initialize BTC RPC client.".to_string())
    }

    fn is_auth_cookie(&self) -> bool {
        matches!(self.auth, Auth::CookieFile(_))
    }

    fn on_error(&self, error: &bitcoincore_rpc::Error) {
        match error {
            bitcoincore_rpc::Error::JsonRpc(rpc_err) => {
                match rpc_err {
                    bitcoincore_rpc::jsonrpc::Error::Transport(_transport_err) => {
                        // Transport error occurred, the bitcoind node might be restart with new auth cookie file
                        // So we try to update the client to use the new auth info
                        if self.is_auth_cookie() {
                            let _ = self.update_client();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn get_latest_block_height(&self) -> Result<u64, String> {
        self.client()?.get_block_count().map_err(|error| {
            self.on_error(&error);

            let msg = format!("get_block_count failed: {}", error);
            error!("{}", msg);
            msg
        })
    }

    pub fn get_block_hash(&self, block_height: u64) -> Result<BlockHash, String> {
        self.client()?
            .get_block_hash(block_height)
            .map_err(|error| {
                self.on_error(&error);

                let msg = format!("get_block_hash failed: {}", error);
                error!("{}", msg);
                msg
            })
    }

    pub fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        self.client()?.get_block(block_hash).map_err(|error| {
            self.on_error(&error);

            let msg = format!("get_block failed: {}", error);
            error!("{}", msg);
            msg
        })
    }

    pub fn get_block(&self, block_height: u64) -> Result<Block, String> {
        // First get the block hash for the given height
        let hash = self
            .client()?
            .get_block_hash(block_height)
            .map_err(|error| {
                self.on_error(&error);

                let msg = format!("get_block_hash failed: {}", error);
                error!("{}", msg);
                msg
            })?;

        // Now get the block using the hash
        self.client()?.get_block(&hash).map_err(|error| {
            self.on_error(&error);

            let msg = format!("get_block failed: {}", error);
            error!("{}", msg);
            msg
        })
    }

    pub async fn get_blocks(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<Block>, String> {
        assert!(end_height >= start_height);
        let count = (end_height - start_height + 1) as usize;
        let mut handles = Vec::with_capacity(count);

        let client = self.client()?;
        for height in start_height..=end_height {
            let handle = tokio::task::spawn_blocking({
                let client = client.clone();
                move || {
                    client
                        .get_block_hash(height)
                        .and_then(|hash| client.get_block(&hash))
                }
            });
            handles.push(handle);
        }

        let results_of_handles = futures::future::join_all(handles).await;

        let mut blocks = Vec::with_capacity(count);
        for result in results_of_handles {
            match result {
                Ok(Ok(block)) => {
                    blocks.push(block);
                }
                Ok(Err(e)) => {
                    self.on_error(&e);

                    let msg = format!("Failed to get block: {}", e);
                    error!("{}", msg);
                    return Err(msg);
                }
                Err(e) => {
                    let msg = format!("Task join error: {}", e);
                    error!("{}", msg);
                    return Err(msg);
                }
            }
        }

        Ok(blocks)
    }

    // Get UTXO details for a given outpoint, maybe spent already
    // So we should get it from transaction and then parse it
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        let ret = self
            .client()?
            .get_raw_transaction(&outpoint.txid, None)
            .map_err(|e| {
                self.on_error(&e);

                let msg = format!(
                    "Failed to get raw transaction for outpoint: {} {}",
                    outpoint, e
                );
                error!("{}", msg);
                msg
            })?;

        if outpoint.vout as usize >= ret.output.len() {
            let msg = format!("Invalid vout index for outpoint: {}", outpoint);
            error!("{}", msg);
            return Err(msg);
        }

        let tx_out = ret.output.get(outpoint.vout as usize).unwrap();
        Ok((tx_out.script_pubkey.clone(), tx_out.value))
    }
}

pub type BTCRpcClientRef = std::sync::Arc<BTCRpcClient>;

#[async_trait::async_trait]
impl BTCClient for BTCRpcClient {
    fn init(&self) -> Result<(), String> {
        // Just try to get latest block height to verify the connection
        let height = self.get_latest_block_height()?;
        info!("BTC RPC client initialized, latest block height: {}", height);

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        // No specific stop action needed for the RPC client
        info!("BTC RPC client stopped.");
        
        Ok(())
    }

    fn get_latest_block_height(&self) -> Result<u64, String> {
        self.get_latest_block_height()
    }

    fn get_block_hash(&self, block_height: u64) -> Result<BlockHash, String> {
        self.get_block_hash(block_height)
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        self.get_block_by_hash(block_hash)
    }

    fn get_block_by_height(&self, block_height: u64) -> Result<Block, String> {
        self.get_block(block_height)
    }

    async fn get_blocks(&self, start_height: u64, end_height: u64) -> Result<Vec<Block>, String> {
        self.get_blocks(start_height, end_height).await
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        self.get_utxo(outpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Address;
    use bitcoincore_rpc::bitcoin::Txid;
    use std::str::FromStr;
    use usdb_util::BTCConfig;

    #[test]
    fn test_btc_client() {
        let config = BTCConfig::default();
        let rpc_url = config.rpc_url();
        let auth = config.auth();

        let client_result = BTCRpcClient::new(rpc_url, auth);
        assert!(client_result.is_ok());

        let client = client_result.unwrap();
        let height_result = client.get_latest_block_height();
        assert!(height_result.is_ok());
        let height = height_result.unwrap();
        println!("Latest block height: {}", height);

        // Test get utxo with a known outpoint (this may fail if the outpoint doesn't exist in the test environment)
        let txid =
            Txid::from_str("adc4b0b0dd51518d5246ecf6aa91550a19b8d86b9dfca525b97bce18dabffc05")
                .unwrap();
        let outpoint = OutPoint::new(txid, 0);
        let utxo_result = client.get_utxo(&outpoint);
        match utxo_result {
            Ok((script, amount)) => {
                println!("UTXO Script: {:?}", script);
                let address =
                    Address::from_script(&script, bitcoincore_rpc::bitcoin::Network::Bitcoin)
                        .expect("Invalid script");
                println!("UTXO Address: {}", address);
                assert_eq!(address.to_string(), "1CMb4HTBRQtweVanz79nfZmKXTDBcJC7Uu");

                println!("UTXO Amount: {}", amount);
                assert_eq!(amount.to_sat(), 61550000);
            }
            Err(e) => {
                println!("Failed to get UTXO: {}", e);
            }
        }

        // Get another utxo in current tx but out of range
        let outpoint = OutPoint::new(txid, 3);
        let utxo_result = client.get_utxo(&outpoint);
        assert!(utxo_result.is_err());

        // Test get_blocks
        let start_height = 790000;
        let end_height = 790256;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(256)
            .build()
            .unwrap();

        rt.block_on(async {
            let begin_tick = std::time::Instant::now();
            let blocks_result = client.get_blocks(start_height, end_height).await;
            assert!(blocks_result.is_ok());
            let blocks = blocks_result.unwrap();
            assert_eq!(blocks.len() as u64, end_height - start_height + 1);
            let end_tick = std::time::Instant::now();
            println!("Time taken to get blocks: {:?}", end_tick - begin_tick);
        });

        // Use get_block to verify
        let begin_tick = std::time::Instant::now();
        for height in start_height..=end_height {
            let block = client.get_block(height).unwrap();
            //assert_eq!(block, blocks[(height - start_height) as usize]);
        }
        let end_tick = std::time::Instant::now();
        println!(
            "Time taken to get blocks individually: {:?}",
            end_tick - begin_tick
        );
    }
}
