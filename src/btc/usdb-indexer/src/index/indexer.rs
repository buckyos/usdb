use super::content::{InscriptionContentLoader, USDBInscription};
use super::energy::{PassEnergyManager, PassEnergyManagerRef};
use super::inscription::BlockInscriptionsCollector;
use super::inscription::InscriptionNewItem;
use super::pass::{MinerPassManager, MinerPassManagerRef, PassMintInscriptionInfo};
use super::transfer::InscriptionTransferTracker;
use crate::btc::{OrdClient, OrdClientRef};
use crate::config::ConfigManagerRef;
use crate::status::StatusManagerRef;
use crate::storage::{MinerPassStorage, MinerPassStorageRef, MinePassStorageSavePointGuard};
use ord::api::Inscription;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use usdb_util::{BTCRpcClient, BTCRpcClientRef};

pub struct InscriptionIndexer {
    config: ConfigManagerRef,
    btc_client: BTCRpcClientRef,
    ord_client: OrdClientRef,

    transfer_tracker: InscriptionTransferTracker,
    miner_pass_storage: MinerPassStorageRef,

    pass_energy_manager: PassEnergyManagerRef,
    miner_pass_manager: MinerPassManagerRef,

    status: StatusManagerRef,

    // Shutdown signal
    should_stop: Arc<AtomicBool>,
}

impl InscriptionIndexer {
    pub fn new(config: ConfigManagerRef, status: StatusManagerRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;

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

        let ret = Self {
            config,
            btc_client: Arc::new(btc_client),
            ord_client: Arc::new(ord_client),

            transfer_tracker,

            pass_energy_manager,
            miner_pass_manager,
            miner_pass_storage,
            status,

            should_stop: Arc::new(AtomicBool::new(false)),
        };

        Ok(ret)
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

        let current_height = self.miner_pass_storage.get_synced_btc_block_height()?.unwrap_or(0);
        if current_height >= latest_height {
            info!(
                "No new blocks to sync. Current height: {}, Latest height: {}",
                current_height, latest_height
            );

            return Ok(current_height);
        }

        let next_height = current_height + 1;
        let block_range = next_height..=latest_height;
        let ret = self.sync_blocks(block_range.clone()).await;
        if let Err(e) = ret {
            let msg = format!(
                "Failed to sync inscriptions from block range {:?}: {}",
                block_range, e
            );
            error!("{}", msg);
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

            // Use savepoint to allow partial rollback on failure
            let savepoint_guard = MinePassStorageSavePointGuard::new(&self.miner_pass_storage)?;

            // Sync this block
            self.sync_block(height).await?;

            // Update the sync storage to save progress
            self.miner_pass_storage.update_synced_btc_block_height(height)?;

            // Commit the savepoint on sync success
            savepoint_guard.commit()?;

            current_height = height;
        }

        Ok(current_height)
    }

    async fn sync_block(&self, height: u32) -> Result<(), String> {
        info!("Processing inscriptions at block height {}", height);

        let mut collector = BlockInscriptionsCollector::new(height);

        // First process block inscriptions
        self.process_block_inscriptions(height, &mut collector)
            .await?;

        // Then process inscription transfers
        self.process_block_inscription_transfer(height).await?;

        // Check if there is anything to process
        if collector.is_empty() {
            info!(
                "No unknown inscriptions and transfers found at block height {}",
                height
            );
            return Ok(());
        }

        info!(
            "Finished processing inscriptions at block height {}: {} new inscriptions, {} transfers",
            height,
            collector.new_inscriptions().len(),
            collector.transfer_inscriptions().len()
        );

        Ok(())
    }

    async fn process_block_inscriptions(
        &self,
        block_height: u32,
        collector: &mut BlockInscriptionsCollector,
    ) -> Result<(), String> {
        let inscription_ids = self
            .ord_client
            .get_inscription_by_block(block_height)
            .await?;
        if inscription_ids.is_empty() {
            info!("No inscriptions found at block height {}", block_height);
            return Ok(());
        }

        let begin_tick = std::time::Instant::now();
        let inscriptions = self.ord_client.get_inscriptions(&inscription_ids).await?;

        debug!(
            "Fetched {} inscriptions at block {} in {:?}",
            block_height,
            inscriptions.len(),
            begin_tick.elapsed()
        );

        assert_eq!(
            inscriptions.len(),
            inscription_ids.len(),
            "Number of inscriptions fetched should match number of inscription IDs"
        );

        let usdb_inscriptions = self
            .load_inscriptions_content(&inscriptions)
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to load inscriptions content at block {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        for (i, item) in usdb_inscriptions.into_iter().enumerate() {
            if item.is_none() {
                debug!(
                    "Inscription {} at block {} is None, skipping",
                    inscription_ids[i], block_height
                );
                continue;
            }

            let (inscription, content, usdb_inscription) = item.unwrap();
            if inscription.number < 0 {
                warn!(
                    "Inscription {} at block {} has negative number {}, skipping",
                    inscription.id, block_height, inscription.number
                );
                continue;
            }

            let create_info = self
                .transfer_tracker
                .calc_create_satpoint(&inscription.id)
                .await?;

            // FXIME: Should not happen? But just in case, we check here
            if create_info.address.is_none() {
                let msg = format!(
                    "Inscription {} at block {} has no creator address",
                    inscription.id, block_height
                );
                error!("{}", msg);
                return Err(msg);
            }

            let inscription_new_item = InscriptionNewItem {
                inscription_id: inscription.id.clone(),
                inscription_number: inscription.number,
                block_height,
                timestamp: inscription.timestamp as u32,
                address: create_info.address.unwrap(), // The creator address
                satpoint: inscription.satpoint,
                value: create_info.value,

                op: usdb_inscription.op(),
                content: usdb_inscription,
                content_string: content,

                commit_txid: create_info.commit_txid,
            };

            // Process the new inscription
            self.on_new_inscription(&inscription_new_item).await?;

            // Add to collector for further processing
            collector.add_new_inscription(inscription_new_item);
        }

        Ok(())
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

    async fn load_inscriptions_content(
        &self,
        inscriptions: &Vec<Inscription>,
    ) -> Result<Vec<Option<(Inscription, String, USDBInscription)>>, String> {
        const BATCH_SIZE: usize = 64;
        let mut contents = Vec::with_capacity(inscriptions.len());

        for chunk in inscriptions.chunks(BATCH_SIZE) {
            let mut handles = Vec::with_capacity(chunk.len());

            for inscription in chunk {
                let ord_client = self.ord_client.clone();
                let inscription = inscription.clone();
                let config = self.config.clone();

                let handle = tokio::spawn(async move {
                    InscriptionContentLoader::load_content(
                        &ord_client,
                        &inscription.id,
                        inscription.content_type.as_deref(),
                        &config,
                    )
                    .await
                    .map(|opt| opt.map(|(content, usdb)| (inscription, content, usdb)))
                });

                handles.push(handle);
            }

            for handle in handles {
                let content = handle.await.map_err(|e| {
                    let msg = format!("Failed to join task for loading inscription content: {}", e);
                    error!("{}", msg);
                    msg
                })??;

                contents.push(content);
            }
        }

        Ok(contents)
    }

    async fn process_block_inscription_transfer(&self, block_height: u32) -> Result<(), String> {
        let transfer_items = self.transfer_tracker.process_block(block_height).await?;
        if transfer_items.is_empty() {
            info!(
                "No inscription transfers found at block height {}",
                block_height
            );
            return Ok(());
        }

        for item in transfer_items {
            match item.to_address {
                Some(addr) => {
                    info!(
                        "Inscription {} transferred from {} to {} at block {}",
                        item.inscription_id, item.from_address, addr, block_height
                    );

                    self.miner_pass_manager
                        .on_pass_transfer(&item.inscription_id, &addr, &item.satpoint, block_height)
                        .await?;
                }
                None => {
                    info!(
                        "Inscription {} burned from {} at block {}",
                        item.inscription_id, item.from_address, block_height
                    );

                    self.miner_pass_manager
                        .on_pass_burned(&item.inscription_id, block_height)
                        .await?;
                }
            }
        }

        Ok(())
    }
}
