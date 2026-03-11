use super::MintValidationErrorCode;
use super::energy::{PassEnergyManager, PassEnergyManagerRef};
use super::pass::{
    InvalidPassMintInscriptionInfo, MinerPassManager, MinerPassManagerRef, PassMintInscriptionInfo,
};
use super::pass_commit::{PassBlockCommitEntry, PassBlockMutationCollector};
use super::transfer::{InscriptionTransferTracker, TransferTrackSeed};
use crate::balance::BalanceMonitor;
use crate::config::ConfigManagerRef;
use crate::inscription::{
    BitcoindInscriptionSource, CompareInscriptionSource, FixtureInscriptionSource,
    InscriptionNewItem, InscriptionSource, InscriptionTransferItem, OrdInscriptionSource,
};
use crate::status::StatusManagerRef;
use crate::storage::{MinePassStorageSavePointGuard, MinerPassStorage, MinerPassStorageRef};
use balance_history::{
    RpcClient as BalanceHistoryRpcClient, SnapshotInfo as BalanceHistorySnapshotInfo,
};
use bitcoincore_rpc::bitcoin::{Block, Txid};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use usdb_util::{BTCRpcClient, BTCRpcClientRef};

#[path = "indexer/block_events.rs"]
mod block_events;
#[path = "indexer/traits.rs"]
mod traits;

use block_events::{BlockEventExecutor, BlockEventPlanner, BlockProcessEvent};
use traits::RpcBlockHintProvider;
pub(crate) use traits::{
    BalanceHistoryCommitApi, BlockHintProvider, IndexStatusApi, TransferTrackerApi,
};

pub struct InscriptionIndexer {
    config: ConfigManagerRef,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    inscription_source: Arc<dyn InscriptionSource>,

    transfer_tracker: Arc<dyn TransferTrackerApi>,
    miner_pass_storage: MinerPassStorageRef,
    balance_monitor: BalanceMonitor,

    pass_energy_manager: PassEnergyManagerRef,
    miner_pass_manager: MinerPassManagerRef,
    balance_history_client: Arc<dyn BalanceHistoryCommitApi>,

    status: Arc<dyn IndexStatusApi>,

    // Shutdown signal
    should_stop: Arc<AtomicBool>,
}

struct CollectedMintItems {
    valid_items: Vec<InscriptionNewItem>,
    invalid_items: Vec<InvalidPassMintInscriptionInfo>,
}

struct BlockMutationCollectionGuard<'a> {
    manager: &'a MinerPassManagerRef,
    active: bool,
}

impl<'a> BlockMutationCollectionGuard<'a> {
    fn begin(manager: &'a MinerPassManagerRef, block_height: u32) -> Result<Self, String> {
        manager.begin_block_mutation_collection(block_height)?;
        Ok(Self {
            manager,
            active: true,
        })
    }

    fn take(mut self, block_height: u32) -> Result<PassBlockMutationCollector, String> {
        let collector = self.manager.take_block_mutation_collector(block_height)?;
        self.active = false;
        Ok(collector)
    }
}

impl Drop for BlockMutationCollectionGuard<'_> {
    fn drop(&mut self) {
        if self.active {
            self.manager.clear_block_mutation_collection();
        }
    }
}

impl InscriptionIndexer {
    pub fn new(config: ConfigManagerRef, status: StatusManagerRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = Arc::new(BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?);
        let inscription_source =
            Self::build_inscription_source(config.clone(), btc_client.clone())?;
        let block_hint_provider: Arc<dyn BlockHintProvider> =
            Arc::new(RpcBlockHintProvider::new(btc_client.clone()));
        let balance_history_client: Arc<dyn BalanceHistoryCommitApi> = Arc::new(
            BalanceHistoryRpcClient::new(&config.config().balance_history.rpc_url)?,
        );

        // Init pass energy manager
        let pass_energy_manager = Arc::new(PassEnergyManager::new(config.clone())?);

        // Init pass storage
        let miner_pass_storage = MinerPassStorage::new(&config.data_dir())?;
        let miner_pass_storage = Arc::new(miner_pass_storage);

        let miner_pass_manager = Arc::new(MinerPassManager::new(
            config.clone(),
            miner_pass_storage.clone(),
            pass_energy_manager.clone(),
        )?);

        let transfer_tracker = InscriptionTransferTracker::new(
            config.clone(),
            miner_pass_manager.miner_pass_storage().clone(),
        )?;
        let transfer_tracker: Arc<dyn TransferTrackerApi> = Arc::new(transfer_tracker);

        let balance_monitor = BalanceMonitor::new(config.clone(), miner_pass_storage.clone())?;
        let status: Arc<dyn IndexStatusApi> = status;

        let ret = Self {
            config,
            block_hint_provider,
            inscription_source,

            transfer_tracker,

            pass_energy_manager,
            miner_pass_manager,
            balance_history_client,
            miner_pass_storage,
            balance_monitor,
            status,

            should_stop: Arc::new(AtomicBool::new(false)),
        };

        Ok(ret)
    }

    #[cfg(test)]
    pub(crate) fn new_with_deps_for_test(
        config: ConfigManagerRef,
        block_hint_provider: Arc<dyn BlockHintProvider>,
        inscription_source: Arc<dyn InscriptionSource>,
        transfer_tracker: Arc<dyn TransferTrackerApi>,
        miner_pass_storage: MinerPassStorageRef,
        balance_monitor: BalanceMonitor,
        pass_energy_manager: PassEnergyManagerRef,
        miner_pass_manager: MinerPassManagerRef,
        balance_history_client: Arc<dyn BalanceHistoryCommitApi>,
        status: Arc<dyn IndexStatusApi>,
    ) -> Self {
        Self {
            config,
            block_hint_provider,
            inscription_source,
            transfer_tracker,
            miner_pass_storage,
            balance_monitor,
            pass_energy_manager,
            miner_pass_manager,
            balance_history_client,
            status,
            should_stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn create_inscription_source_by_name(
        source_name: &str,
        config: ConfigManagerRef,
        btc_client: BTCRpcClientRef,
    ) -> Result<Arc<dyn InscriptionSource>, String> {
        match source_name {
            "ord" => Ok(Arc::new(OrdInscriptionSource::new(config)?)),
            "bitcoind" => Ok(Arc::new(BitcoindInscriptionSource::new(btc_client))),
            "fixture" => Ok(Arc::new(FixtureInscriptionSource::new(config)?)),
            _ => Err(format!(
                "Unsupported inscription source: {} (supported: ord, bitcoind, fixture)",
                source_name
            )),
        }
    }

    fn build_inscription_source(
        config: ConfigManagerRef,
        btc_client: BTCRpcClientRef,
    ) -> Result<Arc<dyn InscriptionSource>, String> {
        let source_name = config
            .config()
            .usdb
            .inscription_source
            .trim()
            .to_ascii_lowercase();
        let primary = Self::create_inscription_source_by_name(
            &source_name,
            config.clone(),
            btc_client.clone(),
        )?;

        if !config.config().usdb.inscription_source_shadow_compare {
            info!(
                "Inscription source selected: module=indexer, source={}",
                source_name
            );
            return Ok(primary);
        }

        let shadow_source_name = if source_name == "ord" {
            "bitcoind"
        } else {
            "ord"
        };
        let shadow = Self::create_inscription_source_by_name(
            shadow_source_name,
            config.clone(),
            btc_client.clone(),
        )?;

        let fail_fast = config.config().usdb.inscription_source_shadow_fail_fast;
        info!(
            "Inscription source shadow compare enabled: module=indexer, primary_source={}, shadow_source={}, fail_fast={}",
            source_name, shadow_source_name, fail_fast
        );

        Ok(Arc::new(CompareInscriptionSource::new(
            primary, shadow, fail_fast,
        )))
    }

    pub async fn init(&self) -> Result<(), String> {
        self.transfer_tracker.init().await?;

        info!("Inscription transfer tracker initialized");

        Ok(())
    }

    pub fn stop(&self) {
        let prev_value = self.should_stop.swap(true, Ordering::SeqCst);
        if !prev_value {
            info!("Shutdown signal sent to InscriptionIndexer");
        }
    }

    pub fn miner_pass_storage(&self) -> &MinerPassStorageRef {
        &self.miner_pass_storage
    }

    pub fn pass_energy_manager(&self) -> &PassEnergyManagerRef {
        &self.pass_energy_manager
    }

    fn check_shutdown(&self) -> bool {
        self.should_stop.load(Ordering::SeqCst)
    }

    pub async fn run(&self) -> Result<(), String> {
        loop {
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting run loop.");
                break;
            }

            match self.sync_once().await {
                Ok(last_synced_height) => {
                    // Successfully synced once, and sleep for a while before next sync
                    let new_height = self.wait_for_new_blocks(last_synced_height).await;
                    info!(
                        "New blocks detected. Last synced height: {}, new height: {}",
                        last_synced_height, new_height
                    );
                }
                Err(e) => {
                    error!("Failed to sync inscriptions: {}", e);

                    // Sleep and retry
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            }
        }

        Ok(())
    }

    // Get latest stable height from balance-history, which is the only sync height dependency.
    fn get_balance_history_stable_height(&self) -> Result<u32, String> {
        self.status.balance_history_stable_height().ok_or_else(|| {
            let msg = "Balance-history stable height is not ready yet".to_string();
            error!("{}", msg);
            msg
        })
    }

    fn current_balance_history_snapshot(&self) -> Result<BalanceHistorySnapshotInfo, String> {
        self.status.balance_history_snapshot().ok_or_else(|| {
            let msg = "Balance-history snapshot is not ready yet".to_string();
            error!("{}", msg);
            msg
        })
    }

    fn persist_balance_history_snapshot_anchor(
        &self,
        synced_height: u32,
        snapshot: &BalanceHistorySnapshotInfo,
    ) -> Result<(), String> {
        // Only adopt an upstream snapshot anchor when the local durable state has
        // fully caught up to the same stable height.
        if snapshot.stable_height != synced_height {
            let msg = format!(
                "Balance-history snapshot height mismatch when persisting anchor: synced_height={}, snapshot_height={}",
                synced_height, snapshot.stable_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.miner_pass_storage
            .upsert_balance_history_snapshot_anchor(snapshot)
            .map_err(|e| {
                let msg = format!(
                    "Failed to persist adopted balance-history snapshot anchor: synced_height={}, error={}",
                    synced_height, e
                );
                error!("{}", msg);
                msg
            })
    }

    async fn wait_for_new_blocks(&self, last_synced_height: u32) -> u32 {
        loop {
            let msg = format!(
                "Waiting for new blocks... Last synced height: {}",
                last_synced_height
            );
            self.status.update_index_status(None, None, Some(msg));

            let latest_height = match self.status.balance_history_stable_height() {
                Some(height) => height,
                None => last_synced_height,
            };
            if latest_height > last_synced_height {
                info!(
                    "New block detected: {} > {}",
                    latest_height, last_synced_height
                );
                return latest_height;
            }

            // Sleep for a while before checking again
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            // Check for shutdown signal while waiting
            if self.check_shutdown() {
                info!("Indexer shutdown requested. Exiting wait for new blocks.");
                return last_synced_height;
            }
        }
    }

    // Returns the latest synced block height after this sync
    async fn sync_once(&self) -> Result<u32, String> {
        let balance_history_snapshot = self.current_balance_history_snapshot()?;
        let latest_height = self.get_balance_history_stable_height()?;

        // Ensure we don't go below genesis block height
        let genesis_block_height = self.config.config().usdb.genesis_block_height;
        if latest_height < genesis_block_height {
            let msg = format!(
                "Latest block height {} is below genesis block height {}",
                latest_height, genesis_block_height
            );
            self.status.update_index_status(
                Some(latest_height),
                Some(latest_height),
                Some(msg.clone()),
            );
            return Ok(latest_height);
        }

        // Get current synced height, ensure it's at least genesis_block_height - 1
        let mut current_height = self
            .miner_pass_storage
            .get_synced_btc_block_height()?
            .unwrap_or(0);
        if current_height < genesis_block_height - 1 {
            current_height = genesis_block_height - 1;
        }

        self.miner_pass_storage
            .assert_no_data_after_block_height(current_height)
            .map_err(|e| {
                // Guard historical replay safety: any data above synced height means state drift.
                let msg = format!(
                    "Data consistency check failed before syncing: module=indexer, synced_height={}, error={}. Please clean data directory and resync from genesis.",
                    current_height, e
                );
                error!("{}", msg);
                msg
            })?;

        self.miner_pass_storage
            .assert_balance_snapshot_consistency(current_height, genesis_block_height)
            .map_err(|e| {
                // Snapshot consistency is mandatory because balance settlement is block-height keyed.
                let msg = format!(
                    "Balance snapshot consistency check failed before syncing: module=indexer, synced_height={}, genesis_block_height={}, error={}. Please clean data directory and resync from genesis.",
                    current_height, genesis_block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        // Reconcile energy store against pass synced height before scanning new blocks.
        self.pass_energy_manager
            .reconcile_with_pass_synced_height(current_height)
            .map_err(|e| {
                // Energy storage must not run ahead/behind pass storage before new block processing.
                let msg = format!(
                    "Energy consistency check failed before syncing: module=indexer, synced_height={}, error={}",
                    current_height, e
                );
                error!("{}", msg);
                msg
            })?;

        if current_height >= latest_height {
            // Even on a no-op sync loop, persist the adopted upstream snapshot anchor.
            // This backfills metadata for already-synced data directories and keeps
            // get_snapshot_info consistent after restart.
            self.persist_balance_history_snapshot_anchor(
                current_height,
                &balance_history_snapshot,
            )?;
            let msg = format!(
                "No new blocks to sync. Current height: {}, Latest height: {}",
                current_height, latest_height
            );
            self.status.update_index_status(
                Some(current_height),
                Some(latest_height),
                Some(msg.clone()),
            );
            return Ok(current_height);
        }

        self.status.update_index_status(
            Some(current_height),
            Some(latest_height),
            Some("Syncing inscriptions...".to_string()),
        );

        let next_height = current_height + 1;
        let block_range = next_height..=latest_height;
        let ret = self.sync_blocks(block_range.clone()).await;
        if let Err(e) = ret {
            let msg = format!(
                "Failed to sync inscriptions from block range {:?}: {}",
                block_range, e
            );
            error!("{}", msg);
            self.status
                .update_index_status(None, None, Some(msg.clone()));

            return Err(msg);
        }

        let current_height = ret.unwrap();

        self.persist_balance_history_snapshot_anchor(current_height, &balance_history_snapshot)?;

        Ok(current_height)
    }

    // Sync blocks in range, returns the latest synced block height
    async fn sync_blocks(&self, block_range: std::ops::RangeInclusive<u32>) -> Result<u32, String> {
        assert!(
            !block_range.is_empty(),
            "Block range should not be empty {:?}",
            block_range
        );

        let mut current_height = *block_range.start();
        for height in block_range {
            info!("Syncing inscriptions at block height {}", height);
            let sync_single_block_begin = Instant::now();

            // Use savepoint to keep pass+balance sqlite state atomic at per-block granularity.
            let savepoint_guard = MinePassStorageSavePointGuard::new(&self.miner_pass_storage)?;

            let msg = format!("Syncing block {}", height);
            self.status.update_index_status(None, None, Some(msg));

            // Any error in sync_block should abort this block and keep previous committed height intact.
            self.sync_block(height).await?;

            // Persist synced height before committing savepoint so crash-recovery starts from durable progress.
            let update_synced_height_begin = Instant::now();
            self.miner_pass_storage
                .update_synced_btc_block_height(height)?;
            let update_synced_height_elapsed_ms = update_synced_height_begin.elapsed().as_millis();

            // Commit only after all block writes and synced-height update succeed.
            let commit_savepoint_begin = Instant::now();
            savepoint_guard.commit()?;
            let commit_savepoint_elapsed_ms = commit_savepoint_begin.elapsed().as_millis();
            let sync_single_block_elapsed_ms = sync_single_block_begin.elapsed().as_millis();

            current_height = height;
            self.status
                .update_index_status(Some(current_height), None, None);
            info!(
                "Block sync progress saved: module=indexer, block_height={}, update_synced_height_elapsed_ms={}, commit_savepoint_elapsed_ms={}, sync_single_block_elapsed_ms={}",
                height,
                update_synced_height_elapsed_ms,
                commit_savepoint_elapsed_ms,
                sync_single_block_elapsed_ms
            );
        }

        Ok(current_height)
    }

    #[cfg(test)]
    pub(crate) async fn sync_blocks_for_test(
        &self,
        block_range: std::ops::RangeInclusive<u32>,
    ) -> Result<u32, String> {
        let current_height = self
            .miner_pass_storage
            .get_synced_btc_block_height()?
            .unwrap_or(0);
        self.pass_energy_manager
            .reconcile_with_pass_synced_height(current_height)?;
        self.sync_blocks(block_range).await
    }

    #[cfg(test)]
    pub(crate) fn has_active_block_mutation_collection_for_test(&self) -> bool {
        self.miner_pass_manager.has_active_block_mutation_collection()
    }

    async fn sync_block(&self, height: u32) -> Result<(), String> {
        info!("Processing inscriptions at block height {}", height);
        let sync_block_begin = Instant::now();

        // Mark energy sync as pending first so crashes can be detected and repaired on restart.
        self.pass_energy_manager.begin_block_sync(height)?;
        let mutation_collection_guard =
            BlockMutationCollectionGuard::begin(&self.miner_pass_manager, height)?;
        let block_hint = self.block_hint_provider.load_block_hint(height)?;
        let block_hint = block_hint.ok_or_else(|| {
            let msg = format!(
                "Missing required block hint at block height {}. Aborting for protocol safety.",
                height
            );
            error!("{}", msg);
            msg
        })?;

        // Collect mint events and transfer events first, then apply in tx order.
        let process_inscriptions_begin = Instant::now();
        let collected_mints = self
            .collect_block_inscription_mints(height, Some(block_hint.clone()))
            .await?;
        let process_inscriptions_elapsed_ms = process_inscriptions_begin.elapsed().as_millis();

        let transfer_track_seeds = Self::build_transfer_track_seeds(&collected_mints.valid_items);
        let process_transfers_begin = Instant::now();
        let transfer_items = self
            .collect_block_inscription_transfer_items(
                height,
                Some(block_hint.clone()),
                transfer_track_seeds,
            )
            .await?;
        let process_transfers_elapsed_ms = process_transfers_begin.elapsed().as_millis();

        let process_events_begin = Instant::now();
        let ordered_events = match self.plan_block_events(
            height,
            block_hint.clone(),
            collected_mints.valid_items,
            transfer_items,
        ) {
            Ok(value) => value,
            Err(e) => {
                // Drop staged transfer mutations when event planning fails to avoid stale staged state.
                self.transfer_tracker.rollback_staged_block(height).await?;
                return Err(e);
            }
        };
        let (new_inscriptions_count, transfer_count) =
            match self.execute_block_events(ordered_events).await {
                Ok(value) => value,
                Err(e) => {
                    // Event execution failed after transfer staging; staged state must be discarded.
                    self.transfer_tracker.rollback_staged_block(height).await?;
                    return Err(e);
                }
            };
        let process_events_elapsed_ms = process_events_begin.elapsed().as_millis();

        let process_invalid_mints_begin = Instant::now();
        let invalid_mints_count = match self
            .process_invalid_mints(collected_mints.invalid_items)
            .await
        {
            Ok(value) => value,
            Err(e) => {
                // Invalid mint recording is part of the same block transaction boundary.
                self.transfer_tracker.rollback_staged_block(height).await?;
                return Err(e);
            }
        };
        let process_invalid_mints_elapsed_ms = process_invalid_mints_begin.elapsed().as_millis();

        let settle_balance_begin = Instant::now();
        let balance_settlement = match self
            .balance_monitor
            .settle_active_balance_with_details(height)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(e) => {
                // Balance settlement failure means block is not complete; rollback staged transfer state.
                self.transfer_tracker.rollback_staged_block(height).await?;
                return Err(e);
            }
        };
        let apply_energy_begin = Instant::now();
        let mut energy_update_count = 0usize;
        for row in &balance_settlement.active_pass_balances {
            if row.delta == 0 {
                continue;
            }
            let changed = match self.pass_energy_manager.apply_active_balance_change(
                &row.inscription_id,
                &row.owner,
                row.block_height,
                row.balance,
                row.delta,
            ) {
                Ok(changed) => changed,
                Err(e) => {
                    self.transfer_tracker.rollback_staged_block(height).await?;
                    return Err(e);
                }
            };
            if changed {
                energy_update_count += 1;
            }
        }
        let apply_energy_elapsed_ms = apply_energy_begin.elapsed().as_millis();
        if let Err(e) = self.pass_energy_manager.finalize_block_sync(height) {
            // Energy finalize must succeed before transfer staging commit to keep cross-store ordering.
            self.transfer_tracker.rollback_staged_block(height).await?;
            return Err(e);
        }
        let mutation_collector = mutation_collection_guard.take(height)?;
        if let Err(e) = self
            .persist_pass_block_commit(height, &mutation_collector)
            .await
        {
            self.transfer_tracker.rollback_staged_block(height).await?;
            return Err(e);
        }
        // Commit transfer tracker staged state only after energy metadata finalize succeeds.
        self.transfer_tracker.commit_staged_block(height).await?;
        let settle_balance_elapsed_ms = settle_balance_begin.elapsed().as_millis();
        let total_elapsed_ms = sync_block_begin.elapsed().as_millis();

        if new_inscriptions_count == 0 && transfer_count == 0 {
            info!(
                "No unknown inscriptions and transfers found at block height {}",
                height
            );
        }

        info!(
            "Finished block processing: module=indexer, block_height={}, new_inscriptions={}, invalid_mints={}, transfers={}, active_address_count={}, total_active_balance={}, energy_update_count={}, process_inscriptions_elapsed_ms={}, process_transfers_elapsed_ms={}, process_events_elapsed_ms={}, process_invalid_mints_elapsed_ms={}, settle_balance_elapsed_ms={}, apply_energy_elapsed_ms={}, total_elapsed_ms={}",
            height,
            new_inscriptions_count,
            invalid_mints_count,
            transfer_count,
            balance_settlement.snapshot.active_address_count,
            balance_settlement.snapshot.total_balance,
            energy_update_count,
            process_inscriptions_elapsed_ms,
            process_transfers_elapsed_ms,
            process_events_elapsed_ms,
            process_invalid_mints_elapsed_ms,
            settle_balance_elapsed_ms,
            apply_energy_elapsed_ms,
            total_elapsed_ms
        );

        Ok(())
    }

    async fn persist_pass_block_commit(
        &self,
        block_height: u32,
        collector: &super::pass_commit::PassBlockMutationCollector,
    ) -> Result<(), String> {
        let upstream_commit = self
            .balance_history_client
            .get_block_commit(block_height)
            .await?
            .ok_or_else(|| {
                let msg = format!(
                    "Balance-history block commit is missing for synced block height {}",
                    block_height
                );
                error!("{}", msg);
                msg
            })?;

        let prev_local_commit = if block_height == 0 {
            None
        } else {
            self.miner_pass_storage
                .get_pass_block_commit(block_height - 1)?
        };
        let prev_local_commit = prev_local_commit.map(|entry| PassBlockCommitEntry {
            block_height: entry.block_height,
            balance_history_block_height: entry.balance_history_block_height,
            balance_history_block_commit: entry.balance_history_block_commit,
            mutation_root: entry.mutation_root,
            block_commit: entry.block_commit,
            commit_protocol_version: entry.commit_protocol_version,
            commit_hash_algo: entry.commit_hash_algo,
        });

        let entry = collector.build_commit_entry(&upstream_commit, prev_local_commit.as_ref())?;
        self.miner_pass_storage.upsert_pass_block_commit(&entry)
    }

    #[cfg(test)]
    pub(crate) async fn sync_block_for_test(&self, height: u32) -> Result<(), String> {
        self.sync_block(height).await
    }

    fn build_block_tx_position_map(block: &Block) -> HashMap<Txid, usize> {
        let mut tx_positions = HashMap::with_capacity(block.txdata.len());
        for (tx_position, tx) in block.txdata.iter().enumerate() {
            tx_positions.insert(tx.compute_txid(), tx_position);
        }
        tx_positions
    }

    fn plan_block_events(
        &self,
        block_height: u32,
        block_hint: Arc<Block>,
        mint_items: Vec<InscriptionNewItem>,
        transfer_items: Vec<InscriptionTransferItem>,
    ) -> Result<Vec<BlockProcessEvent>, String> {
        BlockEventPlanner::new(block_height, block_hint, mint_items, transfer_items).plan()
    }

    async fn execute_block_events(
        &self,
        ordered_events: Vec<BlockProcessEvent>,
    ) -> Result<(usize, usize), String> {
        BlockEventExecutor::new(self).execute(ordered_events).await
    }

    fn build_transfer_track_seeds(mint_items: &[InscriptionNewItem]) -> Vec<TransferTrackSeed> {
        mint_items
            .iter()
            .map(|item| TransferTrackSeed {
                inscription_id: item.inscription_id.clone(),
                owner: item.address.clone(),
                satpoint: item.satpoint.clone(),
            })
            .collect()
    }

    async fn collect_block_inscription_mints(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> Result<CollectedMintItems, String> {
        let discovered_batch = self
            .inscription_source
            .load_block_mint_batch(block_height, block_hint)
            .await?;
        if discovered_batch.valid_mints.is_empty() && discovered_batch.invalid_mints.is_empty() {
            info!("No inscriptions found at block height {}", block_height);
            return Ok(CollectedMintItems {
                valid_items: Vec::new(),
                invalid_items: Vec::new(),
            });
        }

        // Build create-info upfront so we can run block-level validation on reveal inputs.
        let mut valid_candidates = Vec::with_capacity(discovered_batch.valid_mints.len());
        let mut reveal_input_to_inscriptions = HashMap::new();
        for mint in discovered_batch.valid_mints {
            let create_info = self
                .transfer_tracker
                .calc_create_satpoint(&mint.inscription_id)
                .await?;

            // Creator address is required to build pass ownership; missing address is unrecoverable.
            if create_info.address.is_none() {
                let msg = format!(
                    "Inscription {} at block {} has no creator address",
                    mint.inscription_id, block_height
                );
                error!("{}", msg);
                return Err(msg);
            }

            if let Some(source_satpoint) = mint.satpoint {
                if source_satpoint != create_info.satpoint {
                    warn!(
                        "Inscription satpoint mismatch between source and local calc: module=indexer, source={}, block_height={}, inscription_id={}, source_satpoint={}, calc_satpoint={}",
                        self.inscription_source.source_name(),
                        block_height,
                        mint.inscription_id,
                        source_satpoint,
                        create_info.satpoint
                    );
                }
            }

            // Index by reveal input outpoint so we can detect ambiguous mint ownership later.
            // Under USDB protocol assumptions, one reveal input must not produce multiple USDB mints.
            reveal_input_to_inscriptions
                .entry(create_info.commit_outpoint)
                .or_insert_with(Vec::new)
                .push(mint.inscription_id.clone());

            valid_candidates.push((mint, create_info));
        }

        let mut new_inscription_items = Vec::with_capacity(valid_candidates.len());
        let mut invalid_items = Vec::with_capacity(discovered_batch.invalid_mints.len());
        for (mint, create_info) in valid_candidates {
            let conflicted_inscriptions = reveal_input_to_inscriptions
                .get(&create_info.commit_outpoint)
                .cloned()
                .unwrap_or_default();

            // Reject ambiguous reveal-input groups to avoid non-deterministic ownership mapping.
            // If this check is removed, multiple mints could incorrectly inherit from the same origin sat.
            if conflicted_inscriptions.len() > 1 {
                let reason = format!(
                    "Multiple usdb mints share the same reveal input outpoint {} in block {}, inscription_id={}, conflicted_inscriptions={:?}",
                    create_info.commit_outpoint,
                    block_height,
                    mint.inscription_id,
                    conflicted_inscriptions
                );
                warn!(
                    "Ambiguous reveal input detected for usdb mint: module=indexer, block_height={}, inscription_id={}, reveal_input_outpoint={}, conflict_size={}",
                    block_height,
                    mint.inscription_id,
                    create_info.commit_outpoint,
                    conflicted_inscriptions.len()
                );
                invalid_items.push(InvalidPassMintInscriptionInfo {
                    inscription_id: mint.inscription_id,
                    inscription_number: mint.inscription_number,
                    mint_txid: create_info.satpoint.outpoint.txid,
                    mint_block_height: mint.block_height,
                    mint_owner: create_info.address.unwrap(),
                    satpoint: create_info.satpoint,
                    error_code: MintValidationErrorCode::AmbiguousRevealInput
                        .as_str()
                        .to_string(),
                    error_reason: reason,
                });
                continue;
            }

            let op = mint.content.op();
            let inscription_new_item = InscriptionNewItem {
                inscription_id: mint.inscription_id.clone(),
                inscription_number: mint.inscription_number,
                block_height: mint.block_height,
                timestamp: mint.timestamp,
                address: create_info.address.unwrap(), // The creator address
                satpoint: create_info.satpoint,
                value: create_info.value,

                op,
                content: mint.content,
                content_string: mint.content_string,

                commit_txid: create_info.commit_txid,
            };

            new_inscription_items.push(inscription_new_item);
        }

        for invalid_mint in discovered_batch.invalid_mints {
            let create_info = self
                .transfer_tracker
                .calc_create_satpoint(&invalid_mint.inscription_id)
                .await?;

            if create_info.address.is_none() {
                let msg = format!(
                    "Invalid inscription {} at block {} has no creator address",
                    invalid_mint.inscription_id, block_height
                );
                error!("{}", msg);
                return Err(msg);
            }

            let invalid_item = InvalidPassMintInscriptionInfo {
                inscription_id: invalid_mint.inscription_id,
                inscription_number: invalid_mint.inscription_number,
                mint_txid: create_info.satpoint.outpoint.txid,
                mint_block_height: invalid_mint.block_height,
                mint_owner: create_info.address.unwrap(),
                satpoint: create_info.satpoint,
                error_code: invalid_mint.error_code.as_str().to_string(),
                error_reason: invalid_mint.error_reason,
            };
            invalid_items.push(invalid_item);
        }

        Ok(CollectedMintItems {
            valid_items: new_inscription_items,
            invalid_items,
        })
    }

    async fn on_new_inscription(&self, item: &InscriptionNewItem) -> Result<(), String> {
        // If it's a mint operation, process the pass minting
        let mint_content = item.content.as_mint().unwrap();
        let mint_info = PassMintInscriptionInfo {
            inscription_id: item.inscription_id.clone(),
            inscription_number: item.inscription_number,
            mint_txid: item.txid().clone(),
            mint_block_height: item.block_height,
            mint_owner: item.address.clone(),
            satpoint: item.satpoint.clone(),
            eth_main: mint_content.eth_main.clone(),
            eth_collab: mint_content.eth_collab.clone(),
            prev: mint_content.prev_inscription_ids().map_err(|e| {
                let msg = format!(
                    "Failed to parse prev inscription ids for inscription {}: {}",
                    item.inscription_id, e
                );
                error!("{}", msg);
                msg
            })?,
        };
        self.miner_pass_manager.on_mint_pass(&mint_info).await?;

        // Transfer tracking is handled by block-level staged state. We do not mutate
        // tracker cache directly here to keep commit/rollback consistent with DB savepoints.
        Ok(())
    }

    async fn process_invalid_mints(
        &self,
        invalid_mints: Vec<InvalidPassMintInscriptionInfo>,
    ) -> Result<usize, String> {
        let mut processed = 0usize;
        for item in invalid_mints {
            self.miner_pass_manager.on_invalid_mint_pass(&item).await?;
            processed += 1;
        }
        Ok(processed)
    }

    async fn collect_block_inscription_transfer_items(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        extra_tracked_inscriptions: Vec<TransferTrackSeed>,
    ) -> Result<Vec<InscriptionTransferItem>, String> {
        let transfer_items = self
            .transfer_tracker
            .process_block_with_hint(block_height, block_hint, extra_tracked_inscriptions)
            .await?;
        if transfer_items.is_empty() {
            info!(
                "No inscription transfers found at block height {}",
                block_height
            );
            return Ok(Vec::new());
        }

        Ok(transfer_items)
    }
}
