use super::content::{InscriptionContentLoader, USDBInscription};
use super::inscription::BlockInscriptionsCollector;
use super::inscription::{InscriptionNewItem, InscriptionOperation};
use super::sync_state::{SyncStateStorage, SyncStateStorageRef};
use super::transfer::InscriptionTransferTracker;
use crate::btc::{OrdClient, OrdClientRef};
use crate::config::ConfigManagerRef;
use crate::storage::{InscriptionsManager, InscriptionsManagerRef};
use ord::api::Inscription;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use usdb_util::{BTCRpcClient, BTCRpcClientRef};

pub struct InscriptionIndexer {
    config: ConfigManagerRef,
    btc_client: BTCRpcClientRef,
    ord_client: OrdClientRef,

    current_block_height: AtomicU32,
    state: SyncStateStorageRef,

    inscriptions_manager: InscriptionsManagerRef,
    transfer_tracker: InscriptionTransferTracker,
}

impl InscriptionIndexer {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;

        // Init state storage
        let state = SyncStateStorage::new(&config.data_dir())?;

        let inscriptions_manager = Arc::new(InscriptionsManager::new(config.clone())?);
        let transfer_tracker = InscriptionTransferTracker::new(
            config.clone(),
            inscriptions_manager.transfer_storage().clone(),
        )?;

        let ret = Self {
            config,
            btc_client: Arc::new(btc_client),
            ord_client: Arc::new(ord_client),

            state: Arc::new(state),
            current_block_height: AtomicU32::new(0),

            inscriptions_manager,
            transfer_tracker,
        };

        Ok(ret)
    }

    pub fn current_block_height(&self) -> u32 {
        self.current_block_height.load(Ordering::SeqCst)
    }

    pub async fn init(&self) -> Result<(), String> {
        self.transfer_tracker.init().await?;

        info!("Inscription transfer tracker initialized");

        Ok(())
    }

    async fn run(&self) -> Result<(), String> {
        loop {
            if self.current_block_height() == 0 {
                match self.state.get_btc_latest_block_height() {
                    Ok(height) => {
                        // Should skip the last processed block, so we add 1 here
                        let height = height.unwrap_or(0) + 1;

                        let now = if height > self.config.config().usdb.genesis_block_height {
                            height
                        } else {
                            self.config.config().usdb.genesis_block_height
                        };

                        self.current_block_height.store(now, Ordering::SeqCst);

                        info!("Inscription indexer starting from block height {}", now);
                    }
                    Err(e) => {
                        error!("Failed to get latest block height: {}", e);

                        // Sleep and retry
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }

                assert!(
                    self.current_block_height() > 0,
                    "Current block height should be greater than 0"
                );

                match self.sync_once().await {
                    Ok(_) => {
                        // Successfully synced once, and sleep for a while before next sync
                        tokio::time::sleep(std::time::Duration::from_secs(10)).await
                    }
                    Err(e) => {
                        error!("Failed to sync inscriptions: {}", e);

                        // Sleep and retry
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
        }
    }

    // Get latest block height from BTC and ord, use the smaller one!
    async fn get_latest_block_height(&self) -> Result<u32, String> {
        let height = self.btc_client.get_latest_block_height()?;

        let ord_height = self.ord_client.get_latest_block_height().await?;

        // Check the difference between two heights, if the difference is too large, log a warning
        if height > ord_height + 10 {
            warn!(
                "BTC latest block height {} is significantly ahead of Ord latest block height {}",
                height, ord_height
            );
        } else if ord_height > height + 10 {
            warn!(
                "Ord latest block height {} is significantly ahead of BTC latest block height {}",
                ord_height, height
            );
        }

        if height < ord_height {
            Ok(height)
        } else {
            Ok(ord_height)
        }
    }

    async fn sync_once(&self) -> Result<(), String> {
        let height = self.get_latest_block_height().await?;

        if self.current_block_height() > height {
            if self.current_block_height() > height + 1 {
                warn!(
                    "Current sync block height {} is ahead of latest block height {}",
                    self.current_block_height(),
                    height
                );
            }

            return Ok(());
        }

        let sync_begin = self.current_block_height();
        let sync_end = height;
        if let Err(e) = self.sync_blocks(sync_begin, sync_end).await {
            let msg = format!(
                "Failed to sync inscriptions from block [{}, {}]: {}",
                sync_begin, sync_end, e
            );
            error!("{}", msg);
            return Err(msg);
        }

        assert_eq!(
            self.current_block_height(),
            height + 1,
            "After syncing, current block height should be latest block height + 1"
        );

        Ok(())
    }

    // Sync blocks from begin to end, inclusive: [begin, end]
    async fn sync_blocks(&self, begin: u32, end: u32) -> Result<(), String> {
        assert!(
            begin <= end,
            "Begin block height should be less than or equal to end"
        );

        for height in begin..=end {
            info!("Syncing inscriptions at block height {}", height);

            // Sync this block
            self.sync_block(height).await?;

            // Update the sync storage
            self.state.update_btc_latest_block_height(height)?;

            // Update current block height
            self.current_block_height
                .store(height + 1, Ordering::SeqCst);
        }

        Ok(())
    }

    async fn sync_block(&self, height: u32) -> Result<(), String> {
        info!("Processing inscriptions at block height {}", height);

        let mut collector = BlockInscriptionsCollector::new(height);

        // First process block inscriptions
        self.process_block_inscriptions(height, &mut collector)
            .await?;

        // Then process inscription transfers
        self.scan_block_inscription_transfer(height, &mut collector)
            .await?;

        // Check if there is anything to process
        if collector.is_empty() {
            info!(
                "No unknown inscriptions and transfers found at block height {}",
                height
            );
            return Ok(());
        }

        if collector.transfer_inscriptions().len() > 0 {
            // Only remove inscriptions that is transfer op, inscribe data op should always be there!
            let iter = collector
                .transfer_inscriptions()
                .iter()
                .filter(|item| item.op == InscriptionOperation::Transfer);
            let list = iter.collect::<Vec<_>>();

            if list.len() > 0 {
                info!(
                    "Removing {} transferred inscriptions at block height {}",
                    list.len(),
                    height
                );

                self.transfer_tracker
                    .remove_inscriptions_on_block_complete(&list)
                    .await?;
            }
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
        // First add to storage to persist
        self.inscriptions_manager
            .inscription_storage()
            .add_new_inscription(
                &item.inscription_id,
                item.inscription_number,
                item.block_height,
                item.timestamp,
                item.satpoint,
                item.commit_txid,
                item.value,
                &item.content_string,
                item.content.op(),
                &item.address,
            )?;

        // Then record inscription transfer and track it
        self.transfer_tracker
            .add_new_inscription(
                item.inscription_id.clone(),
                item.inscription_number,
                item.block_height,
                item.timestamp,
                item.address.clone(),
                item.satpoint,
                item.value,
                item.content.op(),
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

    async fn scan_block_inscription_transfer(
        &self,
        block_height: u32,
        collector: &mut BlockInscriptionsCollector,
    ) -> Result<(), String> {
        let transfer_items = self
            .transfer_tracker
            .process_block(
                block_height,
                &self.inscriptions_manager.inscription_storage(),
            )
            .await?;

        for item in &transfer_items {
            self.inscriptions_manager
                .inscription_storage()
                .transfer_owner(&item.inscription_id, &item.to_address, block_height)?;
        }

        // Add transfer items to collector for further processing
        collector.add_transfer_inscriptions(transfer_items);

        Ok(())
    }
}
