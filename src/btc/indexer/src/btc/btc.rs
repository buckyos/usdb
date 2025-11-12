use bitcoincore_rpc::Error as BTCError;
use bitcoincore_rpc::bitcoin::Txid;
use bitcoincore_rpc::bitcoincore_rpc_json::GetRawTransactionResult;
use bitcoincore_rpc::json::GetBlockResult;
use bitcoincore_rpc::{Auth, Client, RpcApi};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type TxCache = LruCache<String, GetRawTransactionResult>;

const MAX_CACHE_ENTRIES: usize = 1024 * 10;
const RETRY_COUNT: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(2);

pub struct BTCClient {
    tx_cache: Mutex<TxCache>,
    client: Client,
}

impl BTCClient {
    pub fn new(rpc_url: String, auth: Auth) -> Result<Self, String> {
        let client = Client::new(&rpc_url, auth).map_err(|e| {
            let msg = format!("Failed to create BTC RPC client: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let tx_cache = Mutex::new(LruCache::new(NonZeroUsize::new(MAX_CACHE_ENTRIES).unwrap()));
        let ret = Self { tx_cache, client };

        Ok(ret)
    }

    fn should_retry(error: &BTCError) -> bool {
        match error {
            BTCError::JsonRpc(_) => {
                // TODO we should retry for all jsonrpc errors?
                true
            }
            BTCError::Io(_) => true,
            _ => false,
        }
    }

    async fn sleep_for_retry() {
        tokio::time::sleep(RETRY_DELAY).await;
    }

    pub async fn get_latest_block_height(&self) -> Result<u64, String> {
        for i in 0..RETRY_COUNT {
            match self.client.get_block_count() {
                Ok(height) => return Ok(height),
                Err(error) => {
                    if Self::should_retry(&error) {
                        warn!(
                            "get_block_count failed (attempt {} of {}): {}. Retrying...",
                            i + 1,
                            RETRY_COUNT,
                            error
                        );
                        Self::sleep_for_retry().await;
                        continue;
                    } else {
                        let msg = format!("get_block_count failed: {}", error);
                        error!("{}", msg);
                        return Err(msg);
                    }
                }
            }
        }

        let msg = format!("get_block_count failed after {} attempts", RETRY_COUNT);
        error!("{}", msg);
        Err(msg)
    }

    pub async fn get_block(&self, height: u64) -> Result<GetBlockResult, String> {
        for i in 0..RETRY_COUNT {
            match self.get_block_inner(height) {
                Ok(block) => return Ok(block),
                Err(error) => {
                    if Self::should_retry(&error) {
                        warn!(
                            "get_block failed (attempt {} of {}): {}. Retrying...",
                            i + 1,
                            RETRY_COUNT,
                            error
                        );
                        Self::sleep_for_retry().await;
                        continue;
                    } else {
                        let msg = format!("get_block failed: {}", error);
                        error!("{}", msg);
                        return Err(msg);
                    }
                }
            }
        }

        let msg = format!("get_block failed after {} attempts", RETRY_COUNT);
        error!("{}", msg);
        Err(msg)
    }

    fn get_block_inner(&self, height: u64) -> Result<GetBlockResult, BTCError> {
        let hash = self.client.get_block_hash(height)?;
        self.client.get_block_info(&hash)
    }

    pub async fn get_transaction(&self, txid: &Txid) -> Result<GetRawTransactionResult, String> {
        for i in 0..RETRY_COUNT {
            match self.get_transaction_inner(txid) {
                Ok(tx) => return Ok(tx),
                Err(error) => {
                    if Self::should_retry(&error) {
                        warn!(
                            "get_transaction failed (attempt {} of {}): {}. Retrying...",
                            i + 1,
                            RETRY_COUNT,
                            error
                        );
                        Self::sleep_for_retry().await;
                        continue;
                    } else {
                        let msg = format!("get_transaction failed: {}", error);
                        error!("{}", msg);
                        return Err(msg);
                    }
                }
            }
        }

        let msg = format!("get_transaction failed after {} attempts", RETRY_COUNT);
        error!("{}", msg);
        Err(msg)
    }

    fn get_transaction_inner(&self, txid: &Txid) -> Result<GetRawTransactionResult, BTCError> {
        // First check the cache
        {
            let txid_str = txid.to_string();
            let mut cache = self.tx_cache.lock().unwrap();
            if let Some(cached_tx) = cache.get(&txid_str) {
                return Ok(cached_tx.clone());
            }
        }

        let tx = self.client.get_raw_transaction_info(txid, None)?;
        self.tx_cache
            .lock()
            .unwrap()
            .put(txid.to_string(), tx.clone());

        Ok(tx)
    }

    // Get multiple transactions in batch
    pub async fn get_transactions(
        self: &BTCClientRef,
        txids: &[Txid],
    ) -> Result<Vec<GetRawTransactionResult>, String> {
        const BATCH_SIZE: usize = 64;
        let mut results = Vec::with_capacity(txids.len());

        for chunk in txids.chunks(BATCH_SIZE) {
            let mut handles = Vec::with_capacity(chunk.len());

            for txid in chunk {
                let client_clone = self.clone();
                let txid_clone = *txid;
                let handle =
                    tokio::spawn(async move { client_clone.get_transaction(&txid_clone).await });
                handles.push(handle);
            }

            for handle in handles {
                match handle.await {
                    Ok(Ok(tx)) => results.push(tx),
                    Ok(Err(e)) => return Err(e),
                    Err(e) => {
                        let msg = format!("Task join error: {}", e);
                        error!("{}", msg);
                        return Err(msg);
                    }
                }
            }
        }

        assert_eq!(
            results.len(),
            txids.len(),
            "Mismatch in number of transactions fetched"
        );

        Ok(results)
    }
}

pub type BTCClientRef = Arc<BTCClient>;
