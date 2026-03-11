use super::block::BatchBlockProcessor;
use crate::btc::{BTCClientRef, BTCClientType, create_btc_rpc_client, create_local_btc_client};
use crate::cache::{
    AddressBalanceCache, AddressBalanceCacheRef, MemoryCacheMonitor, MemoryCacheMonitorRef,
};
use crate::cache::{UTXOCache, UTXOCacheRef};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDB, BalanceHistoryDBMode, BalanceHistoryDBRef, BalanceHistoryEntry};
use crate::output::IndexOutputRef;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use usdb_util::USDBScriptHash;

// Use to keep the balance history result for a block
type BlockHistoryResult = HashMap<USDBScriptHash, BalanceHistoryEntry>;

// Find the highest local height that still matches the current canonical BTC chain.
fn find_reorg_common_ancestor_height(
    db: &BalanceHistoryDBRef,
    btc_client: &BTCClientRef,
    current_height: u32,
    latest_btc_height: u32,
) -> Result<Option<u32>, String> {
    if current_height == 0 {
        return Ok(None);
    }

    if latest_btc_height >= current_height {
        let local_tip_commit = db.get_block_commit(current_height)?.ok_or_else(|| {
            let msg = format!(
                "Missing local block commit at synced height {} while checking for reorg",
                current_height
            );
            error!("{}", msg);
            msg
        })?;
        let canonical_tip_hash = btc_client.get_block_hash(current_height)?;
        if local_tip_commit.btc_block_hash == canonical_tip_hash {
            return Ok(None);
        }

        warn!(
            "Detected local/canonical tip mismatch: local_height={}, local_hash={}, canonical_hash={}",
            current_height, local_tip_commit.btc_block_hash, canonical_tip_hash
        );
    }

    let search_start_height = current_height.min(latest_btc_height);
    if latest_btc_height < current_height {
        warn!(
            "Canonical BTC tip moved below local synced height: local_height={}, canonical_height={}",
            current_height, latest_btc_height
        );
    }

    for height in (1..=search_start_height).rev() {
        let local_commit = db.get_block_commit(height)?.ok_or_else(|| {
            let msg = format!(
                "Missing local block commit at height {} while searching reorg common ancestor",
                height
            );
            error!("{}", msg);
            msg
        })?;
        let canonical_hash = btc_client.get_block_hash(height)?;
        if local_commit.btc_block_hash == canonical_hash {
            return Ok(Some(height));
        }
    }

    Ok(Some(0))
}

// Wake the sync loop when height changes or the watched tip hash no longer matches local state.
fn should_wake_for_chain_update(
    db: &BalanceHistoryDBRef,
    btc_client: &BTCClientRef,
    last_height: u32,
    latest_height: u32,
) -> Result<bool, String> {
    if latest_height != last_height || last_height == 0 {
        return Ok(latest_height != last_height);
    }

    let local_tip_commit = db.get_block_commit(last_height)?.ok_or_else(|| {
        let msg = format!(
            "Missing local block commit at synced height {} while waiting for chain updates",
            last_height
        );
        error!("{}", msg);
        msg
    })?;
    let canonical_tip_hash = btc_client.get_block_hash(last_height)?;

    if canonical_tip_hash != local_tip_commit.btc_block_hash {
        warn!(
            "Detected local/canonical tip mismatch while waiting for chain update: height={}, local_hash={}, canonical_hash={}",
            last_height, local_tip_commit.btc_block_hash, canonical_tip_hash
        );
    }

    Ok(local_tip_commit.btc_block_hash != canonical_tip_hash)
}

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
    pub fn new(config: BalanceHistoryConfigRef, output: IndexOutputRef) -> Result<Self, String> {
        // First open in normal mode to get last synced block height
        output.println("Initializing database in normal mode... this may take a while.");
        let db = match BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal) {
            Ok(database) => database,
            Err(e) => {
                let msg = format!("Failed to initialize database: {}", e);
                error!("{}", msg);
                output.println(&msg);
                return Err(msg);
            }
        };
        output.println("Database initialized.");

        // Check synced block height
        let last_synced_block_height = db.get_btc_block_height()?;
        let btc_rpc_client = create_btc_rpc_client(&config)?;
        let rpc_latest_block_height = btc_rpc_client.get_latest_block_height().map_err(|e| {
            let msg = format!("Failed to get latest block height from BTC client: {}", e);
            error!("{}", msg);
            msg
        })?;
        let latest_block_height = config
            .sync
            .max_sync_block_height
            .min(rpc_latest_block_height);

        output.println(&format!(
            "Latest BTC block height: {}, Last synced block height: {}",
            latest_block_height, last_synced_block_height
        ));

        let (db, btc_client) = if latest_block_height - last_synced_block_height
            > config.sync.local_loader_threshold as u32
        {
            let msg = format!(
                "Using LocalLoader BTC client as we are behind by more than {} blocks",
                config.sync.local_loader_threshold
            );
            output.println(&msg);

            drop(db); // Close the normal mode db

            output.println("Reinitializing database in best effort mode... this may take a while.");
            let db = match BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::BestEffort)
            {
                Ok(database) => database,
                Err(e) => {
                    let msg = format!("Failed to initialize database: {}", e);
                    output.eprintln(&msg);

                    return Err(msg);
                }
            };
            let db = Arc::new(db);
            output.println("Database reinitialized.");

            let btc_client = create_local_btc_client(
                btc_rpc_client.clone(),
                &config,
                output.clone(),
                db.clone(),
            )?;

            (db, btc_client)
        } else {
            info!("Using RPC BTC client");
            (Arc::new(db), btc_rpc_client)
        };

        let cache_strategy = match btc_client.get_type() {
            BTCClientType::LocalLoader => {
                info!("Using BestEffort cache strategy for Local Loader BTC client");
                crate::cache::CacheStrategy::BestEffort
            }
            BTCClientType::RPC => {
                info!("Using Normal cache strategy for RPC BTC client");
                crate::cache::CacheStrategy::Normal
            }
        };

        // Init UTXO cache
        let utxo_cache = Arc::new(UTXOCache::new(config.clone(), cache_strategy));

        // Init Address Balance Cache
        let balance_cache = Arc::new(AddressBalanceCache::new(config.clone(), cache_strategy));

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

    pub fn get_latest_block_height(&self) -> Result<u32, String> {
        let rpc_latest_block_height = self
            .btc_client
            .get_latest_block_height()
            .map(|h| h as u32)?;
        let latest_block_height = self
            .config
            .sync
            .max_sync_block_height
            .min(rpc_latest_block_height);

        Ok(latest_block_height)
    }

    pub fn db(&self) -> &BalanceHistoryDBRef {
        &self.db
    }

    fn resume_pending_rollback_if_needed(&self) -> Result<bool, String> {
        let resumed = self.db.resume_rollback_if_needed()?;
        if resumed {
            warn!("Resumed pending balance-history rollback from persisted meta state");
            self.output
                .set_index_message("Resumed pending rollback from persisted meta state");
            self.utxo_cache.clear();
            self.balance_cache.clear();
        }
        Ok(resumed)
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
        let mut output_mode_warning = false;
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
                    self.btc_client
                        .on_sync_complete(latest_height)
                        .unwrap_or_else(|e| {
                            error!("Error during BTC client on_sync_complete: {}", e);
                        });

                    /*
                    // We just don't need to switch back to normal mode here, which may cause extra overhead or deadlock
                    // We will stay in best effort mode until the indexer is restarted
                    if let Err(e) = self.db.switch_mode(BalanceHistoryDBMode::Normal) {
                        error!("Error switching DB to Normal mode: {}", e);
                        break;
                    }
                    */

                    if self.db.get_mode() == BalanceHistoryDBMode::BestEffort
                        && !output_mode_warning
                    {
                        self.output.println("Staying in Best Effort mode until indexer restart(you can restart the indexer when close to sync to switch back to Normal mode).");
                        output_mode_warning = true;
                    }

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
            let latest_height = self.get_latest_block_height()?;
            if should_wake_for_chain_update(&self.db, &self.btc_client, last_height, latest_height)?
            {
                info!(
                    "BTC chain update detected while waiting: local_height={}, canonical_height={}",
                    last_height, latest_height
                );
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

    fn reconcile_reorg_if_needed(
        &self,
        current_height: u32,
        latest_btc_height: u32,
    ) -> Result<u32, String> {
        let ancestor_height = match find_reorg_common_ancestor_height(
            &self.db,
            &self.btc_client,
            current_height,
            latest_btc_height,
        )? {
                Some(height) => height,
                None => return Ok(current_height),
            };

        warn!(
            "BTC reorg detected, rolling back local balance-history state: from_height={}, to_height={}",
            current_height, ancestor_height
        );
        self.output.set_index_message(&format!(
            "BTC reorg detected, rolling back from block height {} to {}",
            current_height, ancestor_height
        ));

        self.db.rollback_to_block_height(ancestor_height)?;
        self.utxo_cache.clear();
        self.balance_cache.clear();

        Ok(ancestor_height)
    }

    // Return the last synced block height
    fn sync_once(&self) -> Result<u32, String> {
        // Check and resume pending rollback if needed before getting latest block height, to ensure we are checking the reorg against the correct local state
        self.resume_pending_rollback_if_needed()?;

        // Get latest block height from BTC node
        let latest_btc_height = self.get_latest_block_height()?;
        info!("Latest BTC block height: {}", latest_btc_height);

        // Get last synced block height from DB
        let last_synced_height = self.db.get_btc_block_height()?;

        // Check for reorg and reconcile local state if needed. This will also update the last_synced_height to the reconciled height.
        let last_synced_height =
            self.reconcile_reorg_if_needed(last_synced_height, latest_btc_height)?;
        info!("Last synced block height: {}", last_synced_height);

        // Update output to current status
        if !self.output.is_index_started() {
            self.output.println("Starting indexer...");
            self.output.println(&format!(
                "Latest BTC block height: {}, Last synced block height: {}",
                latest_btc_height, last_synced_height
            ));
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
            let last_height =
                self.process_block_batch(current_height..(end_height + 1), latest_btc_height)?;
            current_height = last_height + 1;

            // Check for shutdown signal between batches
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting sync once loop.");
                break;
            }
        }

        // Finally flush the db to ensure all data is persisted
        self.db.flush_all()?;

        Ok(current_height - 1)
    }

    // Process a batch of blocks from height_range.start() to height_range.end() (not included)
    // Return the last processed block height
    fn process_block_batch(
        &self,
        height_range: std::ops::Range<u32>,
        latest_btc_height: u32,
    ) -> Result<u32, String> {
        assert!(!height_range.is_empty(), "Height range should not be empty");
        let batch_start_height = height_range.start;

        self.output.set_index_message(&format!(
            "Processing block batch [{} - {})",
            height_range.start, height_range.end
        ));
        self.batch_block_processor
            .process_blocks(
                height_range.clone(),
                latest_btc_height,
                self.config.sync.undo_retention_blocks,
            )?;

        // self.db.flush_all()?;

        let last_height = height_range.end - 1;

        self.prune_undo_journal_if_needed(batch_start_height, last_height)?;

        self.output.update_current_height(last_height as u64);

        info!(
            "Finished processing blocks [{} - {}]",
            height_range.start, last_height,
        );

        Ok(last_height)
    }

    fn prune_undo_journal_if_needed(
        &self,
        batch_start_height: u32,
        last_height: u32,
    ) -> Result<(), String> {
        let retention_blocks = self.config.sync.undo_retention_blocks;
        let cleanup_interval_blocks = self.config.sync.undo_cleanup_interval_blocks;

        if retention_blocks == 0 || cleanup_interval_blocks == 0 {
            return Ok(());
        }

        let previous_height = batch_start_height.saturating_sub(1);
        let previous_bucket = previous_height / cleanup_interval_blocks;
        let current_bucket = last_height / cleanup_interval_blocks;
        if current_bucket == previous_bucket {
            return Ok(());
        }

        let latest_btc_height = self.get_latest_block_height()?;
        let hot_window_start =
            latest_btc_height.saturating_sub(retention_blocks.saturating_sub(1));
        if last_height < hot_window_start {
            return Ok(());
        }

        let min_retained_height = last_height.saturating_sub(retention_blocks.saturating_sub(1));
        let pruned = self.db.prune_undo_before_height(min_retained_height)?;
        if pruned > 0 {
            info!(
                "Undo retention prune finished: pruned_blocks={}, min_retained_height={}",
                pruned, min_retained_height
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btc::BTCClient;
    use crate::config::BalanceHistoryConfig;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf};
    use std::collections::BTreeMap;

    struct FakeBTCClient {
        block_hashes: BTreeMap<u32, BlockHash>,
    }

    #[async_trait::async_trait]
    impl BTCClient for FakeBTCClient {
        fn get_type(&self) -> BTCClientType {
            BTCClientType::RPC
        }

        fn init(&self) -> Result<(), String> {
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            Ok(())
        }

        fn on_sync_complete(&self, _block_height: u32) -> Result<(), String> {
            Ok(())
        }

        fn get_latest_block_height(&self) -> Result<u32, String> {
            self.block_hashes
                .keys()
                .next_back()
                .copied()
                .ok_or_else(|| "No block hashes configured".to_string())
        }

        fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
            self.block_hashes
                .get(&block_height)
                .copied()
                .ok_or_else(|| format!("Missing fake block hash at height {}", block_height))
        }

        fn get_block_by_hash(&self, _block_hash: &BlockHash) -> Result<Block, String> {
            Err("not implemented in FakeBTCClient".to_string())
        }

        fn get_block_by_height(&self, _block_height: u32) -> Result<Block, String> {
            Err("not implemented in FakeBTCClient".to_string())
        }

        async fn get_blocks(
            &self,
            _start_height: u32,
            _end_height: u32,
        ) -> Result<Vec<Block>, String> {
            Err("not implemented in FakeBTCClient".to_string())
        }

        fn get_utxo(&self, _outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
            Err("not implemented in FakeBTCClient".to_string())
        }
    }

    fn temp_db(test_name: &str) -> BalanceHistoryDBRef {
        let mut config = BalanceHistoryConfig::default();
        let temp_dir = std::env::temp_dir().join(test_name);
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = Arc::new(config);
        Arc::new(BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap())
    }

    #[test]
    fn test_find_reorg_common_ancestor_height_returns_none_when_tip_matches() {
        let db = temp_db("balance_history_reorg_detect_no_mismatch");
        let commits = vec![
            crate::db::BlockCommitEntry {
                block_height: 1,
                btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
                balance_delta_root: [1u8; 32],
                block_commit: [1u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 2,
                btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
                balance_delta_root: [2u8; 32],
                block_commit: [2u8; 32],
            },
        ];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(2).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([
                (1, BlockHash::from_slice(&[1u8; 32]).unwrap()),
                (2, BlockHash::from_slice(&[2u8; 32]).unwrap()),
            ]),
        }));

        let ancestor = find_reorg_common_ancestor_height(&db, &client, 2, 2).unwrap();
        assert_eq!(ancestor, None);
    }

    #[test]
    fn test_find_reorg_common_ancestor_height_returns_latest_matching_height() {
        let db = temp_db("balance_history_reorg_detect_with_mismatch");
        let commits = vec![
            crate::db::BlockCommitEntry {
                block_height: 1,
                btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
                balance_delta_root: [1u8; 32],
                block_commit: [1u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 2,
                btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
                balance_delta_root: [2u8; 32],
                block_commit: [2u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 3,
                btc_block_hash: BlockHash::from_slice(&[3u8; 32]).unwrap(),
                balance_delta_root: [3u8; 32],
                block_commit: [3u8; 32],
            },
        ];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(3).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([
                (1, BlockHash::from_slice(&[1u8; 32]).unwrap()),
                (2, BlockHash::from_slice(&[2u8; 32]).unwrap()),
                (3, BlockHash::from_slice(&[9u8; 32]).unwrap()),
            ]),
        }));

        let ancestor = find_reorg_common_ancestor_height(&db, &client, 3, 3).unwrap();
        assert_eq!(ancestor, Some(2));
    }

    #[test]
    fn test_find_reorg_common_ancestor_height_handles_canonical_tip_rollback() {
        let db = temp_db("balance_history_reorg_detect_tip_rollback");
        let commits = vec![
            crate::db::BlockCommitEntry {
                block_height: 1,
                btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
                balance_delta_root: [1u8; 32],
                block_commit: [1u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 2,
                btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
                balance_delta_root: [2u8; 32],
                block_commit: [2u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 3,
                btc_block_hash: BlockHash::from_slice(&[3u8; 32]).unwrap(),
                balance_delta_root: [3u8; 32],
                block_commit: [3u8; 32],
            },
        ];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(3).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([
                (1, BlockHash::from_slice(&[1u8; 32]).unwrap()),
                (2, BlockHash::from_slice(&[9u8; 32]).unwrap()),
            ]),
        }));

        let ancestor = find_reorg_common_ancestor_height(&db, &client, 3, 2).unwrap();
        assert_eq!(ancestor, Some(1));
    }

    #[test]
    fn test_should_wake_for_chain_update_on_same_height_hash_change() {
        let db = temp_db("balance_history_wait_hash_change");
        let commits = vec![crate::db::BlockCommitEntry {
            block_height: 2,
            btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
            balance_delta_root: [2u8; 32],
            block_commit: [2u8; 32],
        }];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(2).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([(2, BlockHash::from_slice(&[7u8; 32]).unwrap())]),
        }));

        let should_wake = should_wake_for_chain_update(&db, &client, 2, 2).unwrap();
        assert!(should_wake);
    }

    #[test]
    fn test_should_wake_for_chain_update_returns_false_when_chain_is_unchanged() {
        let db = temp_db("balance_history_wait_no_change");
        let commits = vec![crate::db::BlockCommitEntry {
            block_height: 2,
            btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
            balance_delta_root: [2u8; 32],
            block_commit: [2u8; 32],
        }];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(2).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([(2, BlockHash::from_slice(&[2u8; 32]).unwrap())]),
        }));

        let should_wake = should_wake_for_chain_update(&db, &client, 2, 2).unwrap();
        assert!(!should_wake);
    }

    #[test]
    fn test_should_wake_for_chain_update_on_canonical_height_drop() {
        let db = temp_db("balance_history_wait_height_drop");
        let commits = vec![crate::db::BlockCommitEntry {
            block_height: 3,
            btc_block_hash: BlockHash::from_slice(&[3u8; 32]).unwrap(),
            balance_delta_root: [3u8; 32],
            block_commit: [3u8; 32],
        }];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(3).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([
                (1, BlockHash::from_slice(&[1u8; 32]).unwrap()),
                (2, BlockHash::from_slice(&[2u8; 32]).unwrap()),
            ]),
        }));

        let should_wake = should_wake_for_chain_update(&db, &client, 3, 2).unwrap();
        assert!(should_wake);
    }

    #[test]
    fn test_find_reorg_common_ancestor_height_returns_genesis_when_no_match_remains() {
        let db = temp_db("balance_history_reorg_detect_no_common_match");
        let commits = vec![
            crate::db::BlockCommitEntry {
                block_height: 1,
                btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
                balance_delta_root: [1u8; 32],
                block_commit: [1u8; 32],
            },
            crate::db::BlockCommitEntry {
                block_height: 2,
                btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
                balance_delta_root: [2u8; 32],
                block_commit: [2u8; 32],
            },
        ];
        db.put_block_commits_async(&commits).unwrap();
        db.put_btc_block_height(2).unwrap();

        let client: BTCClientRef = Arc::new(Box::new(FakeBTCClient {
            block_hashes: BTreeMap::from([
                (1, BlockHash::from_slice(&[7u8; 32]).unwrap()),
                (2, BlockHash::from_slice(&[8u8; 32]).unwrap()),
            ]),
        }));

        let ancestor = find_reorg_common_ancestor_height(&db, &client, 2, 2).unwrap();
        assert_eq!(ancestor, Some(0));
    }
}
