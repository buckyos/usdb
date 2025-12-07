use base64::prelude::*;
use bitcoincore_rpc::Auth;
use bitcoincore_rpc::bitcoin::{Block, Transaction};
use jsonrpsee::core::client::{self, BatchResponse, ClientT};
use jsonrpsee::core::params::BatchRequestBuilder;
use jsonrpsee::http_client::{HeaderMap, HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde::Deserialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
pub struct BlockEx {
    tx: Vec<Transaction>,
}

pub struct BTCBatchClient {
    rpc_url: String,
    auth: Auth,
    client: RwLock<Option<Arc<HttpClient>>>,
}

impl BTCBatchClient {
    pub fn new(rpc_url: String, auth: Auth) -> Result<Self, String> {
        let ret = Self {
            rpc_url,
            auth,
            client: RwLock::new(None),
        };

        Ok(ret)
    }

    fn update_client(&self) -> Result<(), String> {
        let (user, pass) = self.auth.clone().get_user_pass().map_err(|e| {
            let msg = format!("Failed to get user/pass from auth: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut headers = HeaderMap::new();
        if let Some(user) = user {
            let credential = format!("{}:{}", user, pass.unwrap_or_default());
            let encoded = BASE64_STANDARD.encode(credential);
            headers.insert(
                "Authorization",
                format!("Basic {}", encoded).parse().unwrap(),
            );
        };

        let client = HttpClientBuilder::default()
            .request_timeout(Duration::from_secs(120))
            .max_concurrent_requests(100)
            .max_response_size(1024 * 1024 * 1000)
            .set_headers(headers)
            .build(self.rpc_url.as_str())
            .map_err(|e| {
                let msg = format!("Failed to create JSON-RPC client: {}", e);
                error!("{}", msg);
                msg
            })?;
        let arc_client = Arc::new(client);

        let mut write_guard = self.client.write().unwrap();
        *write_guard = Some(arc_client);

        info!("BTC Batch RPC client updated successfully.");
        Ok(())
    }

    fn client(&self) -> Result<Arc<HttpClient>, String> {
        {
            let read_guard = self.client.read().unwrap();
            if let Some(client) = &*read_guard {
                return Ok(client.clone());
            }
        }

        // In some case the client will create multiple times, it's ok
        let msg = "BTC Batch RPC client is not initialized, attempting to update.".to_string();
        warn!("{}", msg);
        self.update_client()?;

        let read_guard = self.client.read().unwrap();
        if let Some(client) = &*read_guard {
            return Ok(client.clone());
        }

        let msg = "BTC Batch RPC client is still not initialized after update.".to_string();
        error!("{}", msg);
        Err(msg)
    }

    pub async fn get_blocks(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let mut batch = BatchRequestBuilder::new();
        for height in start_height..=end_height {
            let params = rpc_params![height];
            batch.insert("getblockhash", params).map_err(|e| {
                let msg = format!("Failed to insert getblockhash into batch: {}", e);
                error!("{}", msg);
                msg
            })?;
        }
        let client = self.client()?;
        let resp = client.batch_request::<String>(batch).await.map_err(|e| {
            let msg = format!("Failed to get block hashes: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut hashes: Vec<String> = Vec::new();
        for item in resp.iter() {
            match item {
                Err(err) => {
                    let msg = format!("getblockhash call failed: {}", err);
                    error!("{}", msg);
                    return Err(msg);
                }
                Ok(value) => {
                    hashes.push(value.clone());
                }
            }
        }

        let mut height_to_idx = std::collections::HashMap::new();

        let mut batch2 = BatchRequestBuilder::new();
        for (i, height) in (start_height..=end_height).enumerate() {
            let block_hash = &hashes[i];
            let params = rpc_params![block_hash, 2i32];
            batch2.insert("getblock", params).map_err(|e| {
                let msg = format!("Failed to insert getblock into batch: {}", e);
                error!("{}", msg);
                msg
            })?;
            height_to_idx.insert(block_hash.clone(), height);
        }

        let resp = client
            .batch_request::<serde_json::Value>(batch2)
            .await
            .map_err(|e| {
                let msg = format!("Failed to get blocks: {}", e);
                error!("{}", msg);
                msg
            })?;
        let mut raw_blocks: Vec<serde_json::Value> =
            Vec::with_capacity((end_height - start_height + 1) as usize);
        for item in resp.iter() {
            match item {
                Err(err) => {
                    let msg = format!("getblock call failed: {}", err);
                    error!("{}", msg);
                    return Err(msg);
                }
                Ok(value) => {
                    // println!("Got block: {}", value);
                    raw_blocks.push(value.clone());
                }
            }
        }

        /*
        let mut blocks = Vec::with_capacity((end_height - start_height + 1) as usize);
        for block in raw_blocks {
            let block_hash = block.block_hash().to_string();
            if let Some(&height) = height_to_idx.get(&block_hash) {
                let idx = (height - start_height) as usize;
                blocks.push(block);
            }
        }
        */

        Ok(raw_blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::consensus::Decodable;
    use usdb_util::BTCConfig;

    #[tokio::test]
    async fn test_batch_get_blocks() {
        let config = BTCConfig::default();
        let rpc_url = config.rpc_url();
        let auth = config.auth();

        let client_result = BTCBatchClient::new(rpc_url, auth);
        assert!(client_result.is_ok());

        let client = client_result.unwrap();
        let start_height = 790000;
        let end_height = 790256;
        let begin_tick = std::time::Instant::now();
        let blocks_result: Result<Vec<serde_json::Value>, String> =
            client.get_blocks(start_height, end_height).await;
        match blocks_result {
            Ok(blocks) => {}
            Err(e) => {
                panic!("Failed to get blocks: {}", e);
            }
        }
        let end_tick = std::time::Instant::now();
        println!(
            "Time taken to get blocks in batch: {:?}",
            end_tick - begin_tick
        );
    }
}
