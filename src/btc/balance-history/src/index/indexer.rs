use super::block::BatchBlockProcessor;
use crate::btc::{BTCClientRef, create_btc_client};
use crate::cache::{AddressBalanceCache, AddressBalanceCacheRef, MemoryCacheMonitor, MemoryCacheMonitorRef};
use crate::cache::{UTXOCache, UTXOCacheRef};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use crate::output::IndexOutputRef;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use usdb_util::USDBScriptHash;

// Use to keep the balance history result for a block
type BlockHistoryResult = HashMap<USDBScriptHash, BalanceHistoryEntry>;

#[derive(Clone)]
pub struct BalanceHistoryIndexer {
    config: BalanceHistoryConfigRef,
    btc_client: BTCClientRef,
    utxo_cache: UTXOCacheRef,
    balance_cache: AddressBalanceCacheRef,
    cache_monitor: MemoryCacheMonitorRef,
    db: BalanceHistoryDBRef,
    batch_block_processor: BatchBlockProcessor,
    output: IndexOutputRef,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    shutdown_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl BalanceHistoryIndexer {
    pub fn new(
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Result<Self, String> {
        // Init btc client
        let last_synced_block_height = db.get_btc_block_height()?;
        let btc_client = create_btc_client(
            &config,
            output.clone(),
            db.clone(),
            last_synced_block_height,
        )?;

        // Init UTXO cache
        let utxo_cache = Arc::new(UTXOCache::new(&config));

        // Init Address Balance Cache
        let balance_cache = Arc::new(AddressBalanceCache::new(&config));

        let cache_monitor = Arc::new(MemoryCacheMonitor::new(
            config.clone(),
            utxo_cache.clone(),
            balance_cache.clone(),
        ));

        let batch_block_processor = BatchBlockProcessor::new(
            btc_client.clone(),
            db.clone(),
            utxo_cache.clone(),
            balance_cache.clone(),
        );

        Ok(Self {
            config,
            btc_client,
            utxo_cache,
            balance_cache,
            cache_monitor,
            db,
            batch_block_processor,
            output,
            shutdown_tx: Arc::new(Mutex::new(None)),
            shutdown_rx: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn run(&self) -> Result<(), String> {
        // Set up shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx.lock().unwrap().replace(shutdown_tx);
        self.shutdown_rx.lock().unwrap().replace(shutdown_rx);

        // Start cache monitor
        self.cache_monitor.start();

        // Run the sync loop in a separate thread
        let indexer = self.clone();
        let handle = tokio::task::spawn_blocking(move || {
            // First initialize the BTC client
            // This step may take some time for local loader to load blk files
            let ret = match indexer.btc_client.init() {
                Ok(_) => {
                    info!("BTC client initialized successfully.");

                    indexer.run_loop();
                    info!("Balance History Indexer run loop exited.");
                    Ok::<(), String>(())
                }
                Err(e) => {
                    let msg = format!("Failed to initialize BTC client: {}", e);
                    error!("{}", msg);
                    Err(msg)
                }
            };

            // Take the shutdown channel back
            indexer.shutdown_tx.lock().unwrap().take();
            indexer.shutdown_rx.lock().unwrap().take();

            info!("Balance History Indexer thread exiting.");
            ret
        });

        let ret = match handle.await {
            Ok(ret) => match ret {
                Ok(_) => {
                    info!("Balance History Indexer thread exited successfully");
                    Ok(())
                }
                Err(e) => {
                    let msg = format!("Balance History Indexer encountered an error: {}", e);
                    error!("{}", msg);
                    Err(msg)
                }
            },
            Err(e) => {
                let msg = format!("Balance History Indexer thread panicked: {:?}", e);
                error!("{}", msg);
                Err(msg)
            }
        };

        ret
    }

    // Gracefully shutdown the indexer and wait for the run loop to exit
    pub async fn shutdown(&self) {
        if let Err(e) = self.btc_client.stop() {
            error!("Error while stopping BTC client: {}", e);
        }

        let tx = self.shutdown_tx.lock().unwrap().take();
        if let Some(tx) = tx {
            let _ = tx.send(());
            info!("Shutdown signal sent to Balance History Indexer");

            // Wait the run loop to exit
            loop {
                if self.shutdown_rx.lock().unwrap().is_none() {
                    info!("Balance History Indexer has shut down");
                    break;
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        } else {
            warn!("Shutdown signal already sent or indexer not running");
        }
    }

    fn run_loop(&self) {
        info!("Starting Balance History Indexer...");

        let mut failed_attempts = 0;
        loop {
            match self.sync_once() {
                Ok(latest_height) => {
                    failed_attempts = 0;
                    info!(
                        "Sync iteration completed successfully. Latest synced height: {}",
                        latest_height
                    );
                    let msg = format!("Synced up to block height {}", latest_height);
                    self.output.update_current_height(latest_height as u64);
                    self.output.set_index_message(&msg);

                    // Check for shutdown signal before waiting for new blocks
                    if self.check_shutdown() {
                        info!("Indexer shutdown requested. Exiting sync loop");
                        break;
                    }

                    // Clear some caches after sync is complete
                    self.cache_monitor.on_sync_complete();
                    self.btc_client.on_sync_complete(latest_height).unwrap_or_else(|e| {
                        error!("Error during BTC client on_sync_complete: {}", e);
                    });

                    // Wait for new blocks
                    match self.wait_for_new_blocks(latest_height) {
                        Ok(new_height) => {
                            info!(
                                "New block detected at height {}. Continuing sync...",
                                new_height
                            );

                            let msg = format!("New block detected at height {}", new_height);
                            self.output.set_index_message(&msg);
                        }
                        Err(e) => {
                            error!(
                                "Error while waiting for new blocks: {}. Retrying in 10 seconds...",
                                e
                            );
                            let msg = format!(
                                "Error while waiting for new blocks: {}. Retrying in 10 seconds...",
                                e
                            );
                            self.output.set_index_message(&msg);

                            std::thread::sleep(std::time::Duration::from_secs(10));

                            // Check for shutdown signal after error
                            if self.check_shutdown() {
                                info!("Indexer shutdown requested. Exiting sync loop");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    failed_attempts += 1;

                    error!(
                        "Error during sync with attempt {}: {}. Retrying in 10 seconds...",
                        failed_attempts, e
                    );

                    let msg = format!(
                        "Error during sync with attempt {}: {}. Retrying in 10 seconds...",
                        failed_attempts, e
                    );
                    self.output.set_index_message(&msg);

                    std::thread::sleep(std::time::Duration::from_secs(10));

                    // Check for shutdown signal after error
                    if self.check_shutdown() {
                        info!("Indexer shutdown requested. Exiting sync loop");
                        break;
                    }
                }
            }
        }

        info!("Balance History Indexer shut down gracefully.");
        self.output
            .set_index_message("Indexer shut down gracefully");
    }

    fn check_shutdown(&self) -> bool {
        let mut rx_lock = self.shutdown_rx.lock().unwrap();
        if let Some(rx) = rx_lock.as_mut() {
            match rx.try_recv() {
                Ok(_) | Err(oneshot::error::TryRecvError::Closed) => {
                    info!("Shutdown signal received. Stopping indexer...");
                    self.output.set_index_message("Indexer shutting down...");
                    return true;
                }
                Err(oneshot::error::TryRecvError::Empty) => {
                    // No shutdown signal yet
                    return false;
                }
            }
        }
        false
    }

    fn wait_for_new_blocks(&self, last_height: u32) -> Result<u32, String> {
        loop {
            let latest_height = self.btc_client.get_latest_block_height()? as u32;
            if latest_height > last_height {
                info!("New block detected: {} > {}", latest_height, last_height);
                return Ok(latest_height);
            }

            std::thread::sleep(std::time::Duration::from_secs(1));

            // Check for shutdown signal while waiting
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting wait for new blocks.");
                return Ok(last_height);
            }
        }
    }

    // Return the last synced block height
    fn sync_once(&self) -> Result<u32, String> {
        // Get latest block height from BTC node
        let latest_btc_height = self.btc_client.get_latest_block_height()? as u32;
        info!("Latest BTC block height: {}", latest_btc_height);

        // Get last synced block height from DB
        let last_synced_height = self.db.get_btc_block_height()?;
        info!("Last synced block height: {}", last_synced_height);

        // Update output to current status
        if !self.output.is_index_started() {
            self.output.set_index_message("Starting indexer...");
            self.output
                .start_index(latest_btc_height as u64, last_synced_height as u64);
        } else {
            self.output
                .update_total_block_height(latest_btc_height as u64);
            self.output.update_current_height(last_synced_height as u64);
        }

        if latest_btc_height <= last_synced_height {
            info!(
                "No new blocks to sync. Latest BTC height: {}, Last synced height: {}",
                latest_btc_height, last_synced_height
            );

            return Ok(last_synced_height);
        }

        let msg = format!(
            "Syncing blocks {} to {}",
            last_synced_height + 1,
            latest_btc_height
        );
        self.output.set_index_message(&msg);

        // Process blocks in batches
        let batch_size = self.config.sync.batch_size;
        let mut current_height = last_synced_height + 1;

        while current_height <= latest_btc_height {
            let end_height =
                std::cmp::min(current_height + batch_size as u32 - 1, latest_btc_height);
            info!("Processing blocks [{} - {}]", current_height, end_height);
            let last_height = self.process_block_batch(current_height..(end_height + 1))?;
            current_height = last_height + 1;

            // Check for shutdown signal between batches
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting sync once loop.");
                break;
            }
        }

        Ok(current_height - 1)
    }

    // Process a batch of blocks from height_range.start() to height_range.end() (not included)
    // Return the last processed block height
    fn process_block_batch(&self, height_range: std::ops::Range<u32>) -> Result<u32, String> {
        assert!(!height_range.is_empty(), "Height range should not be empty");

        self.batch_block_processor
            .process_blocks(height_range.clone())?;

        // self.db.flush_all()?;

        let last_height = height_range.end - 1;
        self.output.update_current_height(last_height as u64);

        info!(
            "Finished processing blocks [{} - {}]",
            height_range.start, last_height,
        );

        Ok(last_height)
    }
}
