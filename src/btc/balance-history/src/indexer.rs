use std::collections::HashMap;
use crate::btc::{BTCClient, BTCClientRef};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use crate::utxo::{CacheTxOut, UTXOCache, UTXOCacheRef};
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};
use std::sync::Arc;
use crate::output::IndexOutputRef;

type BlockHistoryCache = HashMap<ScriptHash, BalanceHistoryEntry>;

#[derive(Clone)]
pub struct BalanceHistoryIndexer {
    config: BalanceHistoryConfigRef,
    btc_client: BTCClientRef,
    utxo_cache: UTXOCacheRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
}

impl BalanceHistoryIndexer {
    pub fn new(config: BalanceHistoryConfigRef, db: BalanceHistoryDBRef, output: IndexOutputRef) -> Result<Self, String> {
        // Init btc client
        let rpc_url = config.btc.rpc_url();
        let auth = config.btc.auth();
        let btc_client = BTCClient::new(rpc_url, auth).map_err(|e| {
            let msg = format!("Failed to create BTC client: {}", e);
            log::error!("{}", msg);
            msg
        })?;
        let btc_client = Arc::new(btc_client);

        // Init UTXO cache
        let utxo_cache = Arc::new(UTXOCache::new(db.clone()));

        Ok(Self {
            config,
            btc_client,
            utxo_cache,
            db,
            output,
        })
    }

    pub async fn run(&self) -> Result<(), String> {
        // Run the sync loop in a separate thread
        let indexer = self.clone();
        let handle = tokio::task::spawn_blocking(move ||{
            indexer.run_loop();
        });

        match handle.await {
            Ok(_) => {
                unreachable!("Indexer thread should run indefinitely");
            }
            Err(e) => {
                let msg = format!("Balance History Indexer thread panicked: {:?}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    fn run_loop(&self) -> ! {
        info!("Starting Balance History Indexer...");
        self.output.set_message("Starting indexer");

        loop {
            match self.sync_once() {
                Ok(latest_height) => {
                    info!(
                        "Sync iteration completed successfully. Latest synced height: {}",
                        latest_height
                    );
                    self.output.update_current_height(latest_height);
                    self.output.set_message(&format!("Synced up to block height {}", latest_height));

                    // Wait for new blocks
                    match self.wait_for_new_blocks(latest_height) {
                        Ok(new_height) => {
                            info!(
                                "New block detected at height {}. Continuing sync...",
                                new_height
                            );
                            self.output.set_message(&format!("New block detected at height {}", new_height));
                        }
                        Err(e) => {
                            error!(
                                "Error while waiting for new blocks: {}. Retrying in 10 seconds...",
                                e
                            );
                            self.output.set_message(&format!("Error while waiting for new blocks: {}. Retrying in 10 seconds...", e));
                            std::thread::sleep(std::time::Duration::from_secs(10));
                        }
                    }
                }
                Err(e) => {
                    error!("Error during sync: {}. Retrying in 10 seconds...", e);
                    self.output.set_message(&format!("Error during sync: {}. Retrying in 10 seconds...", e));
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            }
        }
    }

    fn wait_for_new_blocks(&self, last_height: u64) -> Result<u64, String> {
        loop {
            let latest_height = self.btc_client.get_latest_block_height()?;
            if latest_height > last_height {
                info!("New block detected: {} > {}", latest_height, last_height);
                return Ok(latest_height);
            }

            std::thread::sleep(std::time::Duration::from_secs(1));
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

        self.output.set_message(format!(
            "Syncing blocks {} to {}",
            last_synced_height + 1,
            latest_btc_height
        ).as_str());


        // Process blocks in batches
        let batch_size = self.config.sync.batch_size;
        let mut current_height = last_synced_height as u64 + 1;

        while current_height <= latest_btc_height {
            let end_height =
                std::cmp::min(current_height + batch_size as u64 - 1, latest_btc_height);
            info!("Processing blocks [{} - {}]", current_height, end_height);
            self.process_block_batch(current_height..=end_height)?;
            current_height = end_height + 1;
        }

        Ok(latest_btc_height)
    }

    fn process_block_batch(
        &self,
        height_range: std::ops::RangeInclusive<u64>,
    ) -> Result<(), String> {
        for height in height_range.clone() {
            debug!("Processing block at height {}", height);

            let block_history = self.process_block(height)?;

            // Store to DB
            let entries = block_history.into_values().collect::<Vec<_>>();
            self.db.put_address_history(&entries)?;
            self.db.put_btc_block_height(height as u32)?;

            self.output.update_current_height(height);
            self.output.set_message(&format!("Synced block at height {}", height));
        }

        // Flush storage
        self.db.flush_balance_history()?;

        info!(
            "Finished processing blocks [{} - {}]",
            height_range.start(),
            height_range.end()
        );
        Ok(())
    }

    fn process_block(&self, block_height: u64) -> Result<BlockHistoryCache, String> {
        // Fetch the block
        let block = self.btc_client.get_block(block_height)?;

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
                            let latest_entry = self.db.get_latest_balance(utxo.script_hash)?;
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
                                "Latest entry block height should be less than current block height"
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
            for (n, vout) in tx.output.iter().enumerate() {
                let script_hash = vout.script_pubkey.script_hash();
                let value = vout.value.to_sat();

                match history.entry(script_hash) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        // Load latest record from DB to get current balance
                        let latest_entry = self.db.get_latest_balance(script_hash)?;
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
                self.utxo_cache.insert(
                    OutPoint {
                        txid,
                        vout: n as u32,
                    },
                    script_hash,
                    value,
                );
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