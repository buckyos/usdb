use crate::balance::{
    AddressBalanceCache, AddressBalanceCacheRef, AddressBalanceSyncCache,
};
use crate::btc::{BTCClient, BTCClientRef, create_btc_client};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use crate::output::IndexOutputRef;
use crate::utxo::{CacheTxOut, UTXOCache, UTXOCacheRef};
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

type BlockHistoryCache = HashMap<ScriptHash, BalanceHistoryEntry>;

#[derive(Clone)]
pub struct BalanceHistoryIndexer {
    config: BalanceHistoryConfigRef,
    btc_client: BTCClientRef,
    utxo_cache: UTXOCacheRef,
    balance_cache: AddressBalanceCacheRef,
    db: BalanceHistoryDBRef,
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
        let last_synced_block_height = db.get_btc_block_height()? as u64;
        let btc_client = create_btc_client(&config, output.clone(), last_synced_block_height)?;

        // Init UTXO cache
        let utxo_cache = Arc::new(UTXOCache::new(db.clone()));

        // Init Address Balance Cache
        let balance_cache = Arc::new(AddressBalanceCache::new(db.clone()));

        Ok(Self {
            config,
            btc_client,
            utxo_cache,
            balance_cache,
            db,
            output,
            shutdown_tx: Arc::new(Mutex::new(None)),
            shutdown_rx: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn run(&self) -> Result<(), String> {
        // First initialize the BTC client
        // This step may take some time for local loader to load blk files
        self.btc_client.init().await?;

        // Set up shutdown channel
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx.lock().unwrap().replace(shutdown_tx);
        self.shutdown_rx.lock().unwrap().replace(shutdown_rx);

        // Run the sync loop in a separate thread
        let indexer = self.clone();
        let handle = tokio::task::spawn_blocking(move || {
            indexer.run_loop();
            info!("Balance History Indexer run loop exited.");
        });

        let ret = match handle.await {
            Ok(_) => {
                info!("Balance History Indexer thread completed successfully");
                Ok(())
            }
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
        let mut tx_lock = self.shutdown_tx.lock().unwrap();
        if let Some(tx) = tx_lock.take() {
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
        self.output.set_message("Starting indexer");

        let mut failed_attempts = 0;
        loop {
            match self.sync_once() {
                Ok(latest_height) => {
                    failed_attempts = 0;
                    info!(
                        "Sync iteration completed successfully. Latest synced height: {}",
                        latest_height
                    );
                    self.output.update_current_height(latest_height);
                    self.output
                        .set_message(&format!("Synced up to block height {}", latest_height));

                    // Check for shutdown signal before waiting for new blocks
                    if self.check_shutdown() {
                        info!("Indexer shutdown requested. Exiting sync loop");
                        break;
                    }

                    // Wait for new blocks
                    match self.wait_for_new_blocks(latest_height) {
                        Ok(new_height) => {
                            info!(
                                "New block detected at height {}. Continuing sync...",
                                new_height
                            );
                            self.output.set_message(&format!(
                                "New block detected at height {}",
                                new_height
                            ));
                        }
                        Err(e) => {
                            error!(
                                "Error while waiting for new blocks: {}. Retrying in 10 seconds...",
                                e
                            );
                            self.output.set_message(&format!(
                                "Error while waiting for new blocks: {}. Retrying in 10 seconds...",
                                e
                            ));
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

                    self.output.set_message(&format!(
                        "Error during sync with attempt {}: {}. Retrying in 10 seconds...",
                        failed_attempts, e
                    ));
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
        self.output.set_message("Indexer shut down gracefully");

        // Take the shutdown channel back
        self.shutdown_rx.lock().unwrap().take();
        self.shutdown_tx.lock().unwrap().take();
    }

    fn check_shutdown(&self) -> bool {
        let mut rx_lock = self.shutdown_rx.lock().unwrap();
        if let Some(rx) = rx_lock.as_mut() {
            match rx.try_recv() {
                Ok(_) | Err(oneshot::error::TryRecvError::Closed) => {
                    info!("Shutdown signal received. Stopping indexer...");
                    self.output.set_message("Indexer shutting down...");
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

    fn wait_for_new_blocks(&self, last_height: u64) -> Result<u64, String> {
        loop {
            let latest_height = self.btc_client.get_latest_block_height()?;
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

    // Return the latest synced block height
    fn sync_once(&self) -> Result<u64, String> {
        // Get latest block height from BTC node
        let latest_btc_height = self.btc_client.get_latest_block_height()?;
        info!("Latest BTC block height: {}", latest_btc_height);

        // Get last synced block height from DB
        let last_synced_height = self.db.get_btc_block_height()?;
        info!("Last synced block height: {}", last_synced_height);

        // Update output to current status
        self.output.update_total_block_height(latest_btc_height);
        self.output.update_current_height(last_synced_height as u64);

        if latest_btc_height <= last_synced_height as u64 {
            info!(
                "No new blocks to sync. Latest BTC height: {}, Last synced height: {}",
                latest_btc_height, last_synced_height
            );

            return Ok(last_synced_height as u64);
        }

        self.output.set_message(
            format!(
                "Syncing blocks {} to {}",
                last_synced_height + 1,
                latest_btc_height
            )
            .as_str(),
        );

        // Process blocks in batches
        let batch_size = self.config.sync.batch_size;
        let mut current_height = last_synced_height as u64 + 1;

        while current_height <= latest_btc_height {
            let end_height =
                std::cmp::min(current_height + batch_size as u64 - 1, latest_btc_height);
            info!("Processing blocks [{} - {}]", current_height, end_height);
            let last_height = self.process_block_batch(current_height..=end_height)?;
            current_height = last_height + 1;

            // Check for shutdown signal between batches
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting sync once loop.");
                break;
            }
        }

        Ok(current_height - 1)
    }

    // Process a batch of blocks from height_range.start() to height_range.end()
    // Return the latest processed block height
    fn process_block_batch(
        &self,
        height_range: std::ops::RangeInclusive<u64>,
    ) -> Result<u64, String> {
        assert!(!height_range.is_empty(), "Height range should not be empty");

        let mut balance_sync_cache = AddressBalanceSyncCache::new(self.balance_cache.clone());
        let mut last_height = 0;
        let mut result = Vec::new();
        for height in height_range.clone() {
            debug!("Processing block at height {}", height);

            let block_history = self.process_block(height, &balance_sync_cache)?;

            // Convert block history map to vector for later processing
            let entries = block_history.into_values().collect::<Vec<_>>();

            // Cache balance updates for latest balance retrieval
            balance_sync_cache.on_block_synced(&entries);

            // Save for batch write later
            result.push((height, entries));

            self.output.update_current_height(height);
            self.output
                .set_message(&format!("Synced block at height {}", height));

            last_height = height;

            // Check for shutdown signal after processing each block
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting block processing loop.");
                break;
            }
        }

        // Save all balance entries to DB
        for (_height, entries) in result {
            self.db.put_address_history(&entries)?;
        }

        // Save all utxo cache write entries to DB
        self.utxo_cache.flush_write_cache()?;

        self.db.put_btc_block_height(last_height as u32)?;
        // Flush storage
        // FIXME: Should we flush all include utxo cache?
        self.db.flush_all()?;

        info!(
            "Finished processing blocks [{} - {}]",
            height_range.start(),
            last_height,
        );

        Ok(last_height)
    }

    fn process_block(&self, block_height: u64, balance_sync_cache: &AddressBalanceSyncCache) -> Result<BlockHistoryCache, String> {
        // Fetch the block
        let block = self.btc_client.get_block_by_height(block_height)?;

        let mut history = BlockHistoryCache::new();

        // Process transactions in the block
        for tx in block.txdata.iter() {
            if !tx.is_coinbase() {
                for vin in tx.input.iter() {
                    assert!(
                        !vin.previous_output.is_null(),
                        "Previous output should not be null {}",
                        tx.compute_txid()
                    );

                    let utxo = self.load_utxo(&vin.previous_output)?;

                    match history.entry(utxo.script_hash) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // Load latest record from DB to get current balance
                            let latest_entry = balance_sync_cache.get(utxo.script_hash)?;
                            if latest_entry.block_height == block_height as u32 {
                                // The block may have been synced, skip duplicate entry
                                warn!(
                                    "Skipping duplicate entry for script_hash {} at block height {}",
                                    utxo.script_hash, block_height
                                );
                                continue;
                            }

                            assert!(
                                latest_entry.block_height < block_height as u32,
                                "Latest entry block height should be less than current block height {} < {}",
                                latest_entry.block_height, block_height
                            );
                            assert!(
                                latest_entry.balance >= utxo.value,
                                "Insufficient balance for script_hash {}",
                                utxo.script_hash
                            );
                            e.insert(BalanceHistoryEntry {
                                script_hash: utxo.script_hash,
                                block_height: block_height as u32,
                                delta: -(utxo.value as i64),
                                balance: latest_entry.balance - utxo.value,
                            });
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            let entry = e.get_mut();

                            assert!(
                                entry.balance >= utxo.value,
                                "Insufficient balance for script_hash {}",
                                utxo.script_hash
                            );
                            entry.delta -= utxo.value as i64;
                            entry.balance -= utxo.value;
                        }
                    }
                }
            }

            let txid = tx.compute_txid();
            if self.utxo_cache.check_black_list_coinbase_tx(block_height, &txid) && tx.is_coinbase() {
                warn!(
                    "Skipping blacklisted coinbase tx {} at block height {}",
                    txid, block_height
                );
                continue;
            }
            
            for (n, vout) in tx.output.iter().enumerate() {
                let script_hash = vout.script_pubkey.script_hash();
                let value = vout.value.to_sat();

                match history.entry(script_hash) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        // Load latest record from DB to get current balance
                        let latest_entry = balance_sync_cache.get(script_hash)?;
                        if latest_entry.block_height == block_height as u32 {
                            // The block may have been synced, skip duplicate entry
                            warn!(
                                "Skipping duplicate entry for script_hash {} at block height {}",
                                script_hash, block_height
                            );
                            continue;
                        }

                        assert!(
                            latest_entry.block_height < block_height as u32,
                            "Latest entry block height should be less than current block height"
                        );
                        e.insert(BalanceHistoryEntry {
                            script_hash,
                            block_height: block_height as u32,
                            delta: value as i64,
                            balance: latest_entry.balance + value,
                        });
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        let entry = e.get_mut();
                        entry.delta += value as i64;
                        entry.balance += value;
                    }
                }

                // Cache the UTXO for future use
                self.utxo_cache.put(
                    OutPoint {
                        txid,
                        vout: n as u32,
                    },
                    script_hash,
                    value,
                )?;
            }
        }

        Ok(history)
    }

    fn load_utxo(&self, outpoint: &OutPoint) -> Result<CacheTxOut, String> {
        // First try to get from cache
        if let Some(cached) = self.utxo_cache.spend(outpoint)? {
            return Ok(cached);
        }

        // Load from RPC as needed
        let (script, amount) = self.btc_client.get_utxo(outpoint)?;
        Ok(CacheTxOut {
            script_hash: script.script_hash(),
            value: amount.to_sat(),
        })
    }
}
