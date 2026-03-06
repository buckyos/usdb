use super::energy::{PassEnergyManager, PassEnergyManagerRef};
use super::pass::{MinerPassManager, MinerPassManagerRef, PassMintInscriptionInfo};
use super::transfer::{InscriptionCreateInfo, InscriptionTransferTracker};
use crate::balance::BalanceMonitor;
use crate::config::ConfigManagerRef;
use crate::inscription::{
    BitcoindInscriptionSource, CompareInscriptionSource, InscriptionNewItem, InscriptionSource,
    InscriptionTransferItem, OrdInscriptionSource,
};
use crate::status::StatusManager;
use crate::status::StatusManagerRef;
use crate::storage::{MinePassStorageSavePointGuard, MinerPassStorage, MinerPassStorageRef};
use bitcoincore_rpc::bitcoin::{Block, Txid};
use ord::InscriptionId;
use ordinals::SatPoint;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use usdb_util::{BTCRpcClient, BTCRpcClientRef, USDBScriptHash};

type TransferTrackerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone)]
enum BlockProcessEvent {
    Mint(InscriptionNewItem),
    Transfer(InscriptionTransferItem),
}

struct OrderedBlockProcessEvent {
    tx_position: usize,
    priority: u8,
    event: BlockProcessEvent,
}

pub(crate) trait BlockHintProvider: Send + Sync {
    fn load_block_hint(&self, block_height: u32) -> Result<Option<Arc<Block>>, String>;
}

struct RpcBlockHintProvider {
    btc_client: BTCRpcClientRef,
}

impl RpcBlockHintProvider {
    fn new(btc_client: BTCRpcClientRef) -> Self {
        Self { btc_client }
    }
}

impl BlockHintProvider for RpcBlockHintProvider {
    fn load_block_hint(&self, block_height: u32) -> Result<Option<Arc<Block>>, String> {
        let block = self.btc_client.get_block(block_height)?;
        Ok(Some(Arc::new(block)))
    }
}

pub(crate) trait TransferTrackerApi: Send + Sync {
    fn init<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>>;

    fn calc_create_satpoint<'a>(
        &'a self,
        inscription_id: &'a InscriptionId,
    ) -> TransferTrackerFuture<'a, Result<InscriptionCreateInfo, String>>;

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    ) -> TransferTrackerFuture<'a, Result<(), String>>;

    fn process_block_with_hint<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> TransferTrackerFuture<'a, Result<Vec<InscriptionTransferItem>, String>>;
}

impl TransferTrackerApi for InscriptionTransferTracker {
    fn init<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move { self.init().await })
    }

    fn calc_create_satpoint<'a>(
        &'a self,
        inscription_id: &'a InscriptionId,
    ) -> TransferTrackerFuture<'a, Result<InscriptionCreateInfo, String>> {
        Box::pin(async move { self.calc_create_satpoint(inscription_id).await })
    }

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    ) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.add_new_inscription(inscription_id, owner, satpoint)
                .await
        })
    }

    fn process_block_with_hint<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> TransferTrackerFuture<'a, Result<Vec<InscriptionTransferItem>, String>> {
        Box::pin(async move { self.process_block_with_hint(block_height, block_hint).await })
    }
}

pub(crate) trait IndexStatusApi: Send + Sync {
    fn latest_depend_synced_block_height(&self) -> u32;
    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    );
}

impl IndexStatusApi for StatusManager {
    fn latest_depend_synced_block_height(&self) -> u32 {
        self.latest_depend_synced_block_height()
    }

    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        self.update_index_status(current, total, message);
    }
}

pub struct InscriptionIndexer {
    config: ConfigManagerRef,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    inscription_source: Arc<dyn InscriptionSource>,

    transfer_tracker: Arc<dyn TransferTrackerApi>,
    miner_pass_storage: MinerPassStorageRef,
    balance_monitor: BalanceMonitor,

    pass_energy_manager: PassEnergyManagerRef,
    miner_pass_manager: MinerPassManagerRef,

    status: Arc<dyn IndexStatusApi>,

    // Shutdown signal
    should_stop: Arc<AtomicBool>,
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
            _ => Err(format!(
                "Unsupported inscription source: {} (supported: ord, bitcoind)",
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

    // Get latest block height from depended services
    fn get_latest_block_height(&self) -> u32 {
        self.status.latest_depend_synced_block_height()
    }

    async fn wait_for_new_blocks(&self, last_synced_height: u32) -> u32 {
        loop {
            let msg = format!(
                "Waiting for new blocks... Last synced height: {}",
                last_synced_height
            );
            self.status.update_index_status(None, None, Some(msg));

            let latest_height = self.status.latest_depend_synced_block_height();
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
        let latest_height = self.get_latest_block_height();

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
                let msg = format!(
                    "Balance snapshot consistency check failed before syncing: module=indexer, synced_height={}, genesis_block_height={}, error={}. Please clean data directory and resync from genesis.",
                    current_height, genesis_block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        if current_height >= latest_height {
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

            // Use savepoint to allow partial rollback on failure
            let savepoint_guard = MinePassStorageSavePointGuard::new(&self.miner_pass_storage)?;

            let msg = format!("Syncing block {}", height);
            self.status.update_index_status(None, None, Some(msg));

            // Sync this block
            self.sync_block(height).await?;

            // Update the sync storage to save progress
            let update_synced_height_begin = Instant::now();
            self.miner_pass_storage
                .update_synced_btc_block_height(height)?;
            let update_synced_height_elapsed_ms = update_synced_height_begin.elapsed().as_millis();

            // Commit the savepoint on sync success
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
        self.sync_blocks(block_range).await
    }

    async fn sync_block(&self, height: u32) -> Result<(), String> {
        info!("Processing inscriptions at block height {}", height);
        let sync_block_begin = Instant::now();
        let block_hint = self.block_hint_provider.load_block_hint(height)?;

        // Collect mint events and transfer events first, then apply in tx order.
        let process_inscriptions_begin = Instant::now();
        let new_inscription_items = self
            .collect_block_inscription_mints(height, block_hint.clone())
            .await?;
        let process_inscriptions_elapsed_ms = process_inscriptions_begin.elapsed().as_millis();

        let process_transfers_begin = Instant::now();
        let transfer_items = self
            .collect_block_inscription_transfer_items(height, block_hint.clone())
            .await?;
        let process_transfers_elapsed_ms = process_transfers_begin.elapsed().as_millis();

        let process_events_begin = Instant::now();
        let ordered_events = self.build_ordered_block_events(
            height,
            block_hint,
            new_inscription_items,
            transfer_items,
        )?;
        let (new_inscriptions_count, transfer_count) =
            self.apply_ordered_block_events(ordered_events).await?;
        let process_events_elapsed_ms = process_events_begin.elapsed().as_millis();

        let settle_balance_begin = Instant::now();
        let balance_snapshot = self.balance_monitor.settle_active_balance(height).await?;
        let settle_balance_elapsed_ms = settle_balance_begin.elapsed().as_millis();
        let total_elapsed_ms = sync_block_begin.elapsed().as_millis();

        if new_inscriptions_count == 0 && transfer_count == 0 {
            info!(
                "No unknown inscriptions and transfers found at block height {}",
                height
            );
        }

        info!(
            "Finished block processing: module=indexer, block_height={}, new_inscriptions={}, transfers={}, active_address_count={}, total_active_balance={}, process_inscriptions_elapsed_ms={}, process_transfers_elapsed_ms={}, process_events_elapsed_ms={}, settle_balance_elapsed_ms={}, total_elapsed_ms={}",
            height,
            new_inscriptions_count,
            transfer_count,
            balance_snapshot.active_address_count,
            balance_snapshot.total_balance,
            process_inscriptions_elapsed_ms,
            process_transfers_elapsed_ms,
            process_events_elapsed_ms,
            settle_balance_elapsed_ms,
            total_elapsed_ms
        );

        Ok(())
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

    fn build_ordered_block_events(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        mint_items: Vec<InscriptionNewItem>,
        transfer_items: Vec<InscriptionTransferItem>,
    ) -> Result<Vec<BlockProcessEvent>, String> {
        if mint_items.is_empty() && transfer_items.is_empty() {
            return Ok(Vec::new());
        }

        let mut ordered_events =
            Vec::<OrderedBlockProcessEvent>::with_capacity(mint_items.len() + transfer_items.len());

        // Keep backward-compatible fallback order for tests without a block hint.
        let Some(block_hint) = block_hint else {
            warn!(
                "Missing block hint when ordering block events, fallback to legacy order: module=indexer, block_height={}, mints={}, transfers={}",
                block_height,
                mint_items.len(),
                transfer_items.len()
            );
            let mut legacy_events = Vec::with_capacity(mint_items.len() + transfer_items.len());
            for item in mint_items {
                legacy_events.push(BlockProcessEvent::Mint(item));
            }
            for item in transfer_items {
                legacy_events.push(BlockProcessEvent::Transfer(item));
            }
            return Ok(legacy_events);
        };

        let tx_positions = Self::build_block_tx_position_map(&block_hint);

        for item in mint_items {
            let tx_position = tx_positions
                .get(&item.inscription_id.txid)
                .copied()
                .ok_or_else(|| {
                    let msg = format!(
                        "Mint transaction {} not found in block {} when ordering events for inscription {}",
                        item.inscription_id.txid, block_height, item.inscription_id
                    );
                    error!("{}", msg);
                    msg
                })?;
            ordered_events.push(OrderedBlockProcessEvent {
                tx_position,
                priority: 1,
                event: BlockProcessEvent::Mint(item),
            });
        }

        for item in transfer_items {
            let transfer_txid = *item.txid();
            let tx_position = tx_positions.get(&transfer_txid).copied().ok_or_else(|| {
                let msg = format!(
                    "Transfer transaction {} not found in block {} when ordering events for inscription {}",
                    transfer_txid, block_height, item.inscription_id
                );
                error!("{}", msg);
                msg
            })?;
            ordered_events.push(OrderedBlockProcessEvent {
                tx_position,
                priority: 0,
                event: BlockProcessEvent::Transfer(item),
            });
        }

        ordered_events.sort_by(|a, b| {
            a.tx_position
                .cmp(&b.tx_position)
                .then(a.priority.cmp(&b.priority))
        });

        Ok(ordered_events.into_iter().map(|item| item.event).collect())
    }

    async fn apply_ordered_block_events(
        &self,
        ordered_events: Vec<BlockProcessEvent>,
    ) -> Result<(usize, usize), String> {
        let mut new_inscriptions_count = 0usize;
        let mut transfer_count = 0usize;

        for event in ordered_events {
            match event {
                BlockProcessEvent::Mint(item) => {
                    self.on_new_inscription(&item).await?;
                    new_inscriptions_count += 1;
                }
                BlockProcessEvent::Transfer(item) => {
                    match item.to_address {
                        Some(addr) => {
                            info!(
                                "Inscription {} transferred from {} to {} at block {}",
                                item.inscription_id, item.from_address, addr, item.block_height
                            );

                            self.miner_pass_manager
                                .on_pass_transfer(
                                    &item.inscription_id,
                                    &addr,
                                    &item.satpoint,
                                    item.block_height,
                                )
                                .await?;
                        }
                        None => {
                            info!(
                                "Inscription {} burned from {} at block {}",
                                item.inscription_id, item.from_address, item.block_height
                            );

                            self.miner_pass_manager
                                .on_pass_burned(&item.inscription_id, item.block_height)
                                .await?;
                        }
                    }
                    transfer_count += 1;
                }
            }
        }

        Ok((new_inscriptions_count, transfer_count))
    }

    async fn collect_block_inscription_mints(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> Result<Vec<InscriptionNewItem>, String> {
        let discovered_mints = self
            .inscription_source
            .load_block_mints(block_height, block_hint)
            .await?;
        if discovered_mints.is_empty() {
            info!("No inscriptions found at block height {}", block_height);
            return Ok(Vec::new());
        }

        let mut new_inscription_items = Vec::with_capacity(discovered_mints.len());
        for mint in discovered_mints {
            let create_info = self
                .transfer_tracker
                .calc_create_satpoint(&mint.inscription_id)
                .await?;

            // FXIME: Should not happen? But just in case, we check here
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

        Ok(new_inscription_items)
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
            prev: mint_content.prev_inscription_ids(),
        };
        self.miner_pass_manager.on_mint_pass(&mint_info).await?;

        // Finally, add to transfer tracker for tracking future transfers
        self.transfer_tracker
            .add_new_inscription(
                mint_info.inscription_id.clone(),
                mint_info.mint_owner,
                mint_info.satpoint.clone(),
            )
            .await?;
        Ok(())
    }

    async fn collect_block_inscription_transfer_items(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> Result<Vec<InscriptionTransferItem>, String> {
        let transfer_items = self
            .transfer_tracker
            .process_block_with_hint(block_height, block_hint)
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
