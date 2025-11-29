use super::inscription::{InscriptionOperation, InscriptionTransferItem};
use crate::storage::{InscriptionTransferRecordItem, InscriptionTransferStorageRef};
use crate::btc::{BTCClient, BTCClientRef, TxItem, UTXOValueManager, UTXOValueManagerRef};
use crate::config::ConfigManagerRef;
use crate::index::content::InscriptionContentLoader;
use crate::storage::InscriptionStorage;
use crate::util::Util;
use bitcoincore_rpc::bitcoin::Txid;
use bitcoincore_rpc::bitcoin::address::{Address, NetworkUnchecked};
use bitcoincore_rpc::bitcoin::{Amount, OutPoint};
use ord::InscriptionId;
use ordinals::SatPoint;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};



pub struct InscriptionCreateInfo {
    pub satpoint: SatPoint,
    pub value: Amount,
    pub address: Option<Address<NetworkUnchecked>>,
    pub commit_txid: Txid,
}

struct MultiMap {
    map: HashMap<OutPoint, Vec<InscriptionTransferRecordItem>>,
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
    pub fn insert(&mut self, key: OutPoint, value: InscriptionTransferRecordItem) {
        match self.map.get_mut(&key) {
            Some(vec) => {
                for item in vec.iter_mut() {
                    if item.inscription_id == value.inscription_id {
                        if item.block_height < value.block_height {
                            // Update to the latest transfer record
                            *item = value;
                        } else {
                            // Existing record is newer or same, do nothing
                            error!(
                                "Attempted to insert an older or same transfer record for inscription_id {:?}, existing block_height: {}, new block_height: {}. Ignoring.",
                                value.inscription_id, item.block_height, value.block_height
                            );
                        }

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

    pub fn get(&self, key: &OutPoint) -> Option<&Vec<InscriptionTransferRecordItem>> {
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
    storage: InscriptionTransferStorageRef,

    btc_client: BTCClientRef,
    utxo_cache: UTXOValueManagerRef,
}

impl InscriptionTransferTracker {
    pub fn new(
        config: ConfigManagerRef,
        storage: InscriptionTransferStorageRef,
    ) -> Result<Self, String> {
        let btc_client = BTCClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;
        let btc_client = Arc::new(btc_client);

        let utxo_cache = UTXOValueManager::new(btc_client.clone());
        let utxo_cache = Arc::new(utxo_cache);

        let ret = Self {
            config,
            inscriptions: Mutex::new(MultiMap::new()),
            storage,
            btc_client,
            utxo_cache,
        };

        Ok(ret)
    }

    pub async fn init(&self) -> Result<(), String> {
        self.load_all_records().await.map_err(|e| {
            let msg = format!("Failed to load existing transfer records: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!("InscriptionTransferTracker initialized");

        Ok(())
    }

    async fn load_all_records(&self) -> Result<(), String> {
        let records = self
            .storage
            .get_all_inscriptions_with_last_transfer()
            .map_err(|e| {
                let msg = format!("Failed to load existing transfer records: {}", e);
                error!("{}", msg);
                msg
            })?;

        info!("Loaded {} existing transfer records", records.len());

        let mut inscriptions = self.inscriptions.lock().unwrap();
        for record in records {
            if record.to_address.is_none() {
                warn!(
                    "Transfer record for inscription_id {:?} has no to_address (burn as fee), skipping load",
                    record.inscription_id
                );
                continue;
            }

            if Util::is_zero_satpoint(&record.satpoint) {
                warn!(
                    "Transfer record for inscription_id {:?} has zero satpoint, skipping load",
                    record.inscription_id
                );
                continue;
            }

            // One outpoint may have multiple inscriptions transferred to it
            inscriptions.insert(record.satpoint.outpoint.clone(), record);
        }

        info!(
            "Finished loading existing transfer records {}",
            inscriptions.map.len()
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

    pub async fn add_new_inscription(
        &self,
        inscription_id: InscriptionId,
        inscription_number: i32,
        block_height: u64,
        timestamp: u32,
        creator_address: Address<NetworkUnchecked>,
        satpoint: SatPoint,
        value: Amount,
        op: InscriptionOperation,
    ) -> Result<(), String> {
        let record = InscriptionTransferRecordItem {
            inscription_id: inscription_id.clone(),
            inscription_number,
            block_height,
            timestamp,
            satpoint: satpoint.clone(),
            from_address: None,
            to_address: Some(creator_address),
            value,
            index: 0,
            op,
        };
        self.on_inscription_transferred(&record)
            .await
            .map_err(|e| {
                let msg = format!(
                    "Failed to add new inscription transfer record for inscription_id {}, {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    // The inscription content is contained within the input of a reveal transaction,
    // and the inscription is made on the first sat of its input. This sat can then be tracked using the familiar rules of ordinal theory,
    // allowing it to be transferred, bought, sold, lost to fees, and recovered.
    pub async fn calc_create_satpoint(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<InscriptionCreateInfo, String> {
        // First get tx by inscription id
        let tx = self
            .btc_client
            .get_transaction(&inscription_id.txid)
            .await?;

        // FIXME: There maybe multiple inscriptions in one tx input
        let mut index = inscription_id.index as usize;
        if index >= tx.vin.len() {
            let msg = format!(
                "Invalid vout index {} for transaction {}, vin length {}",
                index,
                inscription_id.txid,
                tx.vin.len()
            );
            error!("{}", msg);
            index = tx.vin.len() - 1;
        }

        let vin = &tx.vin[index];

        if vin.txid.is_none() || vin.vout.is_none() {
            // FIXME Maybe coinbase tx?
            let msg = format!(
                "No commit txid found for inscription_id {:?} in transaction {}",
                inscription_id, inscription_id.txid
            );
            warn!("{}", msg);
            return Err(msg);
        }

        let commit_txid = vin.txid.clone().unwrap();
        let satpoint = SatPoint {
            outpoint: OutPoint {
                txid: commit_txid.clone(),
                vout: vin.vout.unwrap(),
            },
            offset: 0,
        };

        let item = TxItem::from_tx(&tx);
        let ret = item.calc_next_satpoint(satpoint, &self.utxo_cache).await?;
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
        };

        Ok(info)
    }

    async fn on_inscription_transferred(
        &self,
        record: &InscriptionTransferRecordItem,
    ) -> Result<(), String> {
        // Store in persistent storage and update db
        self.storage.insert_transfer_record(&record).map_err(|e| {
            let msg = format!(
                "Failed to store transfer record for inscription_id {:?}: {}",
                record.inscription_id, e
            );
            error!("{}", msg);
            msg
        })?;

        // Check if need to track transfer
        if record.to_address.is_some() && record.op.need_track_transfer() {
            info!(
                "Tracking transfer for inscription_id {:?} at outpoint {:?}",
                record.inscription_id, record.satpoint.outpoint
            );

            let mut inscriptions = self.inscriptions.lock().unwrap();
            inscriptions.insert(record.satpoint.outpoint.clone(), record.clone());
        }

        Ok(())
    }

    pub async fn process_block(
        &self,
        block_height: u64,
        inscription_storage: &InscriptionStorage,
    ) -> Result<Vec<InscriptionTransferItem>, String> {
        info!(
            "Processing block {} for inscription transfers",
            block_height,
        );

        let block = self.btc_client.get_block(block_height).await?;

        // Get all inscription ids in this block
        let txs = self.btc_client.get_transactions(&block.tx).await?;
        assert_eq!(
            txs.len(),
            block.tx.len(),
            "Mismatch in number of transactions fetched"
        );

        let mut transfer_items = Vec::new();
        for tx in txs {
            let tx_item = TxItem::from_tx(&tx);

            // Check all input in current tx if match any inscription outpoint
            for vin in &tx_item.vin {
                // Check if current outpoint is included in this tx's input, if exists,
                // then it's a transfer, we should update the transfer record and remove it from monitor list
                let existing_items = {
                    let coll = self.inscriptions.lock().unwrap();
                    coll.get(vin).cloned()
                };

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
                        existing_item.inscription_id, tx.txid, block_height
                    );

                    let ret = tx_item
                        .calc_next_satpoint(existing_item.satpoint, &self.utxo_cache)
                        .await?;
                    if ret.is_none() {
                        let msg = format!(
                            "Failed to calculate next satpoint for inscription_id {:?} in transaction {}",
                            existing_item.inscription_id, tx.txid
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }

                    let sret = ret.unwrap();

                    // Load content from inscription storage
                    let content = inscription_storage
                        .get_inscription_content(&existing_item.inscription_id)?;
                    if content.is_none() {
                        let msg = format!(
                            "Inscription content not found for inscription_id {:?}",
                            existing_item.inscription_id
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }

                    let content = content.unwrap();
                    let ret = InscriptionContentLoader::parse_content_str(
                        &existing_item.inscription_id,
                        &content,
                    )?;

                    // The inscription must be valid USDB inscription
                    if ret.is_none() {
                        let msg = format!(
                            "Inscription content is not valid USDB inscription for inscription_id {:?}",
                            existing_item.inscription_id
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }
                    let ret = ret.unwrap();

                    info!(
                        "Found inscription transfer {} from {} to {}, value {}",
                        existing_item.inscription_id,
                        existing_item.satpoint,
                        sret.satpoint,
                        sret.value,
                    );

                    let record = InscriptionTransferRecordItem {
                        inscription_id: existing_item.inscription_id.clone(),
                        inscription_number: existing_item.inscription_number,
                        block_height,
                        timestamp: block.time as u32,
                        satpoint: sret.satpoint,
                        from_address: existing_item.to_address.clone(),
                        to_address: sret.address.clone(),
                        value: sret.value,
                        index: existing_item.index + 1,
                        op: existing_item.op,
                    };
                    self.on_inscription_transferred(&record).await?;

                    let transfer_item = InscriptionTransferItem {
                        inscription_id: existing_item.inscription_id.clone(),
                        inscription_number: existing_item.inscription_number as u64,
                        block_height,
                        timestamp: block.time as u32,
                        satpoint: sret.satpoint,
                        prev_satpoint: Some(existing_item.satpoint.clone()),
                        from_address: existing_item.to_address.clone().unwrap(),
                        to_address: sret.address.clone(),
                        value: sret.value,
                        content,
                        op: ret.op(),
                        index: existing_item.index + 1,
                    };

                    transfer_items.push(transfer_item);
                }
            }
        }

        Ok(transfer_items)
    }
}
