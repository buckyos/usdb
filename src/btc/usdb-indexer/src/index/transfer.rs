use crate::btc::{TxItem, UTXOValueManager, UTXOValueManagerRef};
use crate::config::ConfigManagerRef;
use crate::inscription::InscriptionTransferItem;
use crate::storage::MinerPassStorageRef;
use crate::storage::ValidMinerPassInfo;
use bitcoincore_rpc::bitcoin::Block;
use bitcoincore_rpc::bitcoin::Txid;
use bitcoincore_rpc::bitcoin::{Amount, OutPoint};
use ord::{InscriptionId, ParsedEnvelope};
use ordinals::SatPoint;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use usdb_util::{BTCRpcClient, BTCRpcClientRef, USDBScriptHash};

pub struct InscriptionCreateInfo {
    pub satpoint: SatPoint,
    pub value: Amount,
    pub address: Option<USDBScriptHash>,
    pub commit_txid: Txid,
    pub commit_outpoint: OutPoint,
}

#[derive(Clone)]
pub struct TransferTrackSeed {
    pub inscription_id: InscriptionId,
    pub owner: USDBScriptHash,
    pub satpoint: SatPoint,
}

#[derive(Clone)]
struct MultiMap {
    map: HashMap<OutPoint, Vec<ValidMinerPassInfo>>,
}

impl MultiMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    // Insert a value for a given key, if value already exists for the key, do nothing
    pub fn insert(&mut self, key: OutPoint, value: ValidMinerPassInfo) {
        match self.map.get_mut(&key) {
            Some(vec) => {
                for item in vec.iter_mut() {
                    if item.inscription_id == value.inscription_id {
                        *item = value;

                        return;
                    }
                }

                vec.push(value);
            }
            None => {
                self.map.insert(key, vec![value]);
            }
        }
    }

    pub fn get(&self, key: &OutPoint) -> Option<&Vec<ValidMinerPassInfo>> {
        self.map.get(key)
    }

    pub fn has_key(&self, key: &OutPoint) -> bool {
        self.map.contains_key(key)
    }

    pub fn delete(&mut self, key: &OutPoint) {
        self.map.remove(key);
    }

    pub fn delete_value(&mut self, key: &OutPoint, inscription_id: &InscriptionId) {
        if let Some(vec) = self.map.get_mut(key) {
            vec.retain(|item| item.inscription_id != *inscription_id);
            if vec.is_empty() {
                self.map.remove(key);
            }
        }
    }
}

pub struct InscriptionTransferTracker {
    config: ConfigManagerRef,
    inscriptions: Mutex<MultiMap>,
    staged_blocks: Mutex<HashMap<u32, MultiMap>>,
    miner_pass_storage: MinerPassStorageRef,

    btc_client: BTCRpcClientRef,
    utxo_manager: UTXOValueManagerRef,
}

impl InscriptionTransferTracker {
    pub fn new(
        config: ConfigManagerRef,
        miner_pass_storage: MinerPassStorageRef,
    ) -> Result<Self, String> {
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;
        let btc_client = Arc::new(btc_client);

        let utxo_manager = UTXOValueManager::new(btc_client.clone());
        let utxo_manager = Arc::new(utxo_manager);

        let ret = Self {
            config,
            inscriptions: Mutex::new(MultiMap::new()),
            staged_blocks: Mutex::new(HashMap::new()),
            miner_pass_storage,
            btc_client,
            utxo_manager,
        };

        Ok(ret)
    }

    pub async fn init(&self) -> Result<(), String> {
        self.load_all_passes().await.map_err(|e| {
            let msg = format!("Failed to load existing transfer records: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!("InscriptionTransferTracker initialized");

        Ok(())
    }

    async fn load_all_passes(&self) -> Result<(), String> {
        let mut page_index = 0;
        let page_size = 1024;
        loop {
            let passes = self
                .miner_pass_storage
                .get_all_valid_pass_by_page(page_index, page_size)
                .map_err(|e| {
                    let msg = format!("Failed to load miner passes: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            if passes.is_empty() {
                break;
            }

            info!("Loaded {} miner passes", passes.len());

            let count = passes.len();
            let mut inscriptions = self.inscriptions.lock().unwrap();
            for pass in passes {
                inscriptions.insert(pass.satpoint.outpoint.clone(), pass);
            }

            if count < page_size {
                break;
            }

            page_index += 1;
        }

        info!(
            "Finished loading existing transfer records {}",
            self.inscriptions.lock().unwrap().len()
        );

        Ok(())
    }

    pub async fn remove_inscriptions_on_block_complete(
        &self,
        inscription_transfer_items: &[&InscriptionTransferItem],
    ) -> Result<(), String> {
        let mut inscriptions = self.inscriptions.lock().unwrap();

        for item in inscription_transfer_items {
            inscriptions.delete_value(&item.satpoint.outpoint, &item.inscription_id);

            info!(
                "Removed inscription transfer record for inscription_id {:?} on outpoint {:?}",
                item.inscription_id, item.satpoint.outpoint
            );
        }

        Ok(())
    }

    // Add a new inscription to track transfers
    pub async fn add_new_inscription(
        &self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    ) -> Result<(), String> {
        let mut inscriptions = self.inscriptions.lock().unwrap();
        inscriptions.insert(
            satpoint.outpoint.clone(),
            ValidMinerPassInfo {
                inscription_id,
                owner,
                satpoint,
            },
        );

        Ok(())
    }

    // The inscription content is contained within the input of a reveal transaction,
    // and the inscription is made on the first sat of its input. This sat can then be tracked using the familiar rules of ordinal theory,
    // allowing it to be transferred, bought, sold, lost to fees, and recovered.
    pub async fn calc_create_satpoint(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<InscriptionCreateInfo, String> {
        // First get reveal tx by inscription id
        let tx = self.btc_client.get_transaction(&inscription_id.txid)?;
        let envelopes = ParsedEnvelope::from_transaction(&tx);
        let index = inscription_id.index as usize;
        if index >= envelopes.len() {
            let msg = format!(
                "Invalid inscription index {} for transaction {}, envelope length {}",
                index,
                inscription_id.txid,
                envelopes.len()
            );
            error!("{}", msg);
            return Err(msg);
        }
        let envelope = &envelopes[index];
        // Use envelope.input instead of inscription index to map to the actual reveal vin.
        // Multiple inscriptions may exist on one input, so index->vin is not always valid.
        let input_index = envelope.input as usize;
        if input_index >= tx.input.len() {
            let msg = format!(
                "Invalid envelope input index {} for inscription {} in transaction {}, vin length {}",
                input_index,
                inscription_id,
                inscription_id.txid,
                tx.input.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        let vin = &tx.input[input_index];
        let commit_outpoint = vin.previous_output;

        let commit_txid = commit_outpoint.txid;
        let satpoint = SatPoint {
            outpoint: OutPoint {
                txid: commit_txid.clone(),
                vout: commit_outpoint.vout,
            },
            offset: 0,
        };

        let item = TxItem::from_tx(tx);
        let ret = item
            .calc_output_satpoint(satpoint, &self.utxo_manager)
            .await?;
        if ret.is_none() {
            let msg = format!(
                "No satpoint found for inscription_id {:?} in transaction {}",
                inscription_id, inscription_id.txid
            );
            error!("{}", msg);
            return Err(msg);
        }

        let ret = ret.unwrap();
        let info = InscriptionCreateInfo {
            satpoint: ret.satpoint,
            value: ret.value,
            address: ret.address,
            commit_txid,
            commit_outpoint,
        };

        Ok(info)
    }

    pub async fn process_block(
        &self,
        block_height: u32,
    ) -> Result<Vec<InscriptionTransferItem>, String> {
        self.process_block_with_hint(block_height, None, Vec::new())
            .await
    }

    pub async fn process_block_with_hint(
        &self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        extra_tracked_inscriptions: Vec<TransferTrackSeed>,
    ) -> Result<Vec<InscriptionTransferItem>, String> {
        info!(
            "Processing block {} for inscription transfers",
            block_height,
        );

        {
            let staged_blocks = self.staged_blocks.lock().unwrap();
            if !staged_blocks.is_empty() {
                // We process one block-at-a-time; leftover staged state indicates previous flow broke.
                let msg = format!(
                    "Uncommitted staged transfer state exists before processing block {}: staged_block_count={}",
                    block_height,
                    staged_blocks.len()
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        let block = match block_hint {
            Some(block) => block,
            None => Arc::new(self.btc_client.get_block(block_height)?),
        };

        let mut working_inscriptions = {
            let coll = self.inscriptions.lock().unwrap();
            coll.clone()
        };
        // Track tentative transfer result in a working copy; commit/rollback controls visibility.
        for seed in extra_tracked_inscriptions {
            working_inscriptions.insert(
                seed.satpoint.outpoint.clone(),
                ValidMinerPassInfo {
                    inscription_id: seed.inscription_id,
                    owner: seed.owner,
                    satpoint: seed.satpoint,
                },
            );
        }

        let mut transfer_items = Vec::new();
        for tx in &block.txdata {
            let tx_item = TxItem::from_tx(tx.clone());

            // Check all input in current tx if match any inscription outpoint
            for vin in &tx_item.tx.input {
                // Check if current outpoint is included in this tx's input, if exists,
                // then it's a transfer, we should update the transfer record and remove it from monitor list
                let existing_items = working_inscriptions.get(&vin.previous_output).cloned();

                if existing_items.is_none() {
                    continue;
                }
                let existing_items = existing_items.unwrap();
                if existing_items.is_empty() {
                    continue;
                }

                for existing_item in existing_items {
                    info!(
                        "Found transfer for inscription_id {} in transaction {} block {}",
                        existing_item.inscription_id, tx_item.txid, block_height
                    );

                    let ret = tx_item
                        .calc_output_satpoint(existing_item.satpoint, &self.utxo_manager)
                        .await?;
                    if ret.is_none() {
                        let msg = format!(
                            "Failed to calculate output satpoint for inscription_id {} in transaction {}",
                            existing_item.inscription_id, tx_item.txid
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }

                    let ret = ret.unwrap();
                    match ret.address {
                        Some(new_owner) => {
                            if new_owner == existing_item.owner {
                                info!(
                                    "Inscription {} transferred back to the same owner {}, satpoint {} -> {}",
                                    existing_item.inscription_id,
                                    new_owner,
                                    existing_item.satpoint,
                                    ret.satpoint
                                );
                            } else {
                                info!(
                                    "Inscription {} transferred from {} to {}, satpoint {} -> {}",
                                    existing_item.inscription_id,
                                    existing_item.owner,
                                    new_owner,
                                    existing_item.satpoint,
                                    ret.satpoint
                                );
                            }

                            // Update tracked inscription info in the working copy.
                            working_inscriptions.delete_value(
                                &existing_item.satpoint.outpoint,
                                &existing_item.inscription_id,
                            );
                            working_inscriptions.insert(
                                ret.satpoint.outpoint.clone(),
                                ValidMinerPassInfo {
                                    inscription_id: existing_item.inscription_id.clone(),
                                    owner: new_owner,
                                    satpoint: ret.satpoint,
                                },
                            );
                        }
                        None => {
                            info!(
                                "Inscription {} lost to fees at satpoint {}",
                                existing_item.inscription_id, ret.satpoint
                            );

                            // Remove from tracked inscription info in the working copy.
                            working_inscriptions.delete_value(
                                &existing_item.satpoint.outpoint,
                                &existing_item.inscription_id,
                            );
                        }
                    }

                    let transfer_item = InscriptionTransferItem {
                        inscription_id: existing_item.inscription_id.clone(),
                        block_height,
                        prev_satpoint: existing_item.satpoint.clone(),
                        satpoint: ret.satpoint,
                        from_address: existing_item.owner.clone(),
                        to_address: ret.address,
                    };

                    transfer_items.push(transfer_item);
                }
            }
        }

        let mut staged_blocks = self.staged_blocks.lock().unwrap();
        if let Some(prev) = staged_blocks.insert(block_height, working_inscriptions) {
            let msg = format!(
                "Staged transfer state for block {} already exists when processing the block, this should not happen, previous staged state will be overwritten, prev_staged_block_height={}",
                block_height,
                prev.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(transfer_items)
    }

    pub fn commit_staged_block(&self, block_height: u32) -> Result<(), String> {
        let staged_map = {
            let mut staged_blocks = self.staged_blocks.lock().unwrap();
            staged_blocks.remove(&block_height).ok_or_else(|| {
                let msg = format!(
                    "No staged transfer state found for commit at block {}",
                    block_height
                );
                error!("{}", msg);
                msg
            })?
        };

        let mut inscriptions = self.inscriptions.lock().unwrap();
        // Atomic swap publishes the staged snapshot only after upper-layer block processing succeeds.
        *inscriptions = staged_map;

        info!("Committed staged transfer state at block {}", block_height);
        Ok(())
    }

    pub fn rollback_staged_block(&self, block_height: u32) -> Result<(), String> {
        let mut staged_blocks = self.staged_blocks.lock().unwrap();
        // Rollback only discards staged copy; committed in-memory tracking state remains unchanged.
        if staged_blocks.remove(&block_height).is_none() {
            let msg = format!(
                "No staged transfer state found for rollback at block {}",
                block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Rolled back staged transfer state at block {}",
            block_height
        );
        Ok(())
    }
}
