use super::balance::{AddressBalanceCacheRef, AddressBalanceItem};
use super::utxo::{CacheTxOut, UTXOCacheRef};
use crate::btc::BTCClientRef;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use bitcoincore_rpc::bitcoin::{Block, OutPoint, Txid};
use rayon::slice::ParallelSliceMut;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BlockTxIndex {
    block_height: u32,
    tx_index: u32,
}

struct VOutUtxoInfo {
    item: CacheTxOut,
    spend: bool, // Whether this UTXO is spent in the batch
}

pub struct PreloadVIn {
    pub outpoint: OutPoint,
    pub cache_tx_out: Option<CacheTxOut>,
    pub need_flush: bool,
}

pub struct PreloadVOut {
    pub outpoint: OutPoint,
    pub cache_tx_out: CacheTxOut,
}

pub struct PreloadTx {
    pub txid: Txid,
    pub vin: Vec<PreloadVIn>,
    pub vout: Vec<PreloadVOut>,
}

pub struct PreloadBlock {
    pub height: u32,
    pub txdata: Vec<PreloadTx>,
}

struct VInPosition {
    tx_index: usize,
    vin_index: usize,
}

pub struct BatchBlockData {
    block_range: std::ops::Range<u32>,
    blocks: Arc<Mutex<Vec<PreloadBlock>>>,
    vout_utxos: Arc<Mutex<HashMap<OutPoint, VOutUtxoInfo>>>,
    balances: Arc<Mutex<HashMap<USDBScriptHash, BalanceHistoryEntry>>>,
    balance_history: Arc<Mutex<Vec<BalanceHistoryEntry>>>,
}

impl BatchBlockData {
    pub fn new() -> Self {
        Self {
            block_range: 0..0,
            blocks: Arc::new(Mutex::new(Vec::new())),
            vout_utxos: Arc::new(Mutex::new(HashMap::new())),
            balances: Arc::new(Mutex::new(HashMap::new())),
            balance_history: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

pub type BatchBlockDataRef = Arc<BatchBlockData>;

pub struct BatchBlockPreloader {
    btc_client: BTCClientRef,
    db: BalanceHistoryDBRef,
    utxo_cache: UTXOCacheRef,
    balance_cache: AddressBalanceCacheRef,
}

impl BatchBlockPreloader {
    pub fn new(
        btc_client: BTCClientRef,
        db: BalanceHistoryDBRef,
        utxo_cache: UTXOCacheRef,
        balance_cache: AddressBalanceCacheRef,
    ) -> Self {
        Self {
            btc_client,
            db,
            utxo_cache,
            balance_cache,
        }
    }

    pub fn preload(
        &self,
        block_height_range: std::ops::Range<u32>,
    ) -> Result<BatchBlockDataRef, String> {
        assert!(
            block_height_range.start < block_height_range.end,
            "Invalid block height range {:?}",
            block_height_range
        );

        let mut blocks = Vec::new();
        for height in block_height_range.clone() {
            let block = self.btc_client.get_block_by_height(height)?;

            blocks.push((height, block));
        }

        let mut data = BatchBlockData::new();
        data.block_range = block_height_range.clone();
        let data = Arc::new(data);

        use rayon::prelude::*;
        let result: Vec<Result<(), String>> = blocks
            .into_par_iter()
            .map(|(block_height, block)| {
                let mut preload_block = self.preprocess_block(block_height, &block, &data)?;

                self.preload_block(&mut preload_block, &data)?;

                data.blocks.lock().unwrap().push(preload_block);

                Ok(())
            })
            .collect();

        for res in result {
            res?;
        }

        // Sort the blocks by height
        {
            let mut blocks = data.blocks.lock().unwrap();
            blocks.par_sort_unstable_by(|a, b| a.height.cmp(&b.height));
        }

        // Load balances at the starting block height - 1
        if block_height_range.start > 0 {
            let target_block_height = block_height_range.start - 1;
            self.preload_balances(target_block_height, &data)?;
        }

        Ok(data)
    }

    fn preprocess_block(
        &self,
        block_height: u32,
        block: &Block,
        data: &BatchBlockData,
    ) -> Result<PreloadBlock, String> {
        let mut preload_block = PreloadBlock {
            height: block_height,
            txdata: Vec::with_capacity(block.txdata.len()),
        };

        // Load all vins' UTXOs into cache
        for tx in &block.txdata {
            let mut preload_tx = PreloadTx {
                txid: tx.compute_txid(),
                vin: Vec::with_capacity(tx.input.len()),
                vout: Vec::with_capacity(tx.output.len()),
            };

            if !tx.is_coinbase() {
                for vin in &tx.input {
                    let outpoint = &vin.previous_output;

                    // Here we juse use None as placeholder, the real UTXO will be loaded in batch later
                    let preload_vin = PreloadVIn {
                        outpoint: outpoint.clone(),
                        cache_tx_out: None,
                        need_flush: true,
                    };
                    preload_tx.vin.push(preload_vin);
                }
            }

            for (n, vout) in tx.output.iter().enumerate() {
                // Skip outputs that cannot be spent
                if vout.script_pubkey.is_op_return() {
                    continue;
                }

                let outpoint = OutPoint {
                    txid: preload_tx.txid,
                    vout: n as u32,
                };

                let cache_tx_out = CacheTxOut {
                    value: vout.value.to_sat(),
                    script_hash: vout.script_pubkey.to_usdb_script_hash(),
                };

                let preload_vout = PreloadVOut {
                    outpoint,
                    cache_tx_out,
                };
                preload_tx.vout.push(preload_vout);
            }

            preload_block.txdata.push(preload_tx);
        }

        // Append all vout UTXOs to UTXO cache
        let mut vout_utxo_map = data.vout_utxos.lock().unwrap();
        for tx in &preload_block.txdata {
            for vout in &tx.vout {
                vout_utxo_map.insert(
                    vout.outpoint.clone(),
                    VOutUtxoInfo {
                        item: vout.cache_tx_out.clone(),
                        spend: false,
                    },
                );
            }
        }

        Ok(preload_block)
    }

    fn preload_block(
        &self,
        preload_block: &mut PreloadBlock,
        data: &BatchBlockData,
    ) -> Result<(), String> {
        // Collect all UTXOs to load
        let mut outpoints_to_load = Vec::new();
        let mut outpoints_pos = Vec::new();

        for (tx_index, tx) in &mut preload_block.txdata.iter_mut().enumerate() {
            for (vin_index, vin) in tx.vin.iter_mut().enumerate() {
                // First check if the UTXO is already in vout cache (i.e., created in the same batch)
                let mut vout_utxo_map = data.vout_utxos.lock().unwrap();
                if let Some(vout_utxo_info) = vout_utxo_map.get_mut(&vin.outpoint) {
                    assert!(
                        !vout_utxo_info.spend,
                        "Double spend of UTXO in the same batch: {}",
                        vin.outpoint
                    );
                    vout_utxo_info.spend = true;

                    vin.cache_tx_out.replace(vout_utxo_info.item.clone());
                    vin.need_flush = false; // No need to flush UTXO created in the same batch

                    continue;
                }

                // Then check if the UTXO is in utxo cache then use it(need spend)
                if let Some(cache_tx_out) = self.utxo_cache.spend(&vin.outpoint) {
                    vin.cache_tx_out.replace(cache_tx_out);
                    continue;
                }

                // Append to load list for batch loading
                let pos = VInPosition {
                    tx_index,
                    vin_index,
                };

                outpoints_to_load.push(vin.outpoint.clone());
                outpoints_pos.push(pos);
            }
        }

        if outpoints_to_load.is_empty() {
            return Ok(());
        }

        // Batch load UTXOs
        let loaded_utxos = self.fetch_utxos(&outpoints_to_load)?;

        assert!(
            loaded_utxos.len() == outpoints_to_load.len(),
            "Loaded UTXO count mismatch: expected {}, got {}",
            outpoints_to_load.len(),
            loaded_utxos.len()
        );

        // Fill in loaded UTXOs
        for (pos, utxo) in outpoints_pos.into_iter().zip(loaded_utxos.into_iter()) {
            preload_block.txdata[pos.tx_index].vin[pos.vin_index]
                .cache_tx_out
                .replace(utxo);
        }

        Ok(())
    }

    fn fetch_utxos(&self, outpoints: &[OutPoint]) -> Result<Vec<CacheTxOut>, String> {
        // First try to get from db by bulk
        let all = self.db.get_utxos_bulk(outpoints)?;

        // Then load from rpc for missing ones
        let mut result = Vec::with_capacity(outpoints.len());
        for (i, item) in all.into_iter().enumerate() {
            if let Some(utxo) = item {
                result.push(CacheTxOut {
                    value: utxo.amount,
                    script_hash: utxo.script_hash,
                });
            } else {
                // Load from rpc
                let (script, amount) = self.btc_client.get_utxo(&outpoints[i])?;
                result.push(CacheTxOut {
                    value: amount.to_sat(),
                    script_hash: script.to_usdb_script_hash(),
                });
            }
        }

        Ok(result)
    }

    // Preload balances for all addresses involved up to target_block_height (<= target_block_height)
    fn preload_balances(
        &self,
        target_block_height: u32,
        data: &BatchBlockData,
    ) -> Result<(), String> {
        use rayon::prelude::*;

        // Collect all addresses involved
        let mut addresses = HashSet::new();
        {
            let blocks = data.blocks.lock().unwrap();
            for block in blocks.iter() {
                for tx in block.txdata.iter() {
                    // Collect vin addresses
                    for vin in tx.vin.iter() {
                        let vout = vin.cache_tx_out.as_ref().unwrap();
                        // addresses.get_or_insert_with(&vout.script_hash, vout.script_hash.to_owned);
                        addresses.insert(vout.script_hash.clone());
                    }

                    for vout in tx.vout.iter() {
                        addresses.insert(vout.cache_tx_out.script_hash.clone());
                    }
                }
            }
        }

        let mut sorted_addresses: Vec<_> = addresses.into_iter().collect();
        sorted_addresses.par_sort_unstable();

        // Batch load balances
        let result: Vec<Result<BalanceHistoryEntry, String>> = sorted_addresses
            .into_par_iter()
            .map(|script_hash| {
                // First load from balance cache
                if let Some(cached) = self
                    .balance_cache
                    .get(script_hash, target_block_height as u32)
                {
                    let entry = BalanceHistoryEntry {
                        script_hash,
                        block_height: cached.block_height,
                        delta: cached.delta,
                        balance: cached.balance,
                    };

                    return Ok(entry);
                }

                // Then load from db
                let balance = self
                    .db
                    .get_balance_at_block_height(script_hash, target_block_height as u32)?;

                Ok(balance)
            })
            .collect();

        let mut balances_map = data.balances.lock().unwrap();
        for res in result {
            let balance = res?;
            balances_map.insert(balance.script_hash.clone(), balance);
        }

        Ok(())
    }
}

pub struct BatchBlockFlusher {
    db: BalanceHistoryDBRef,
    utxo_cache: UTXOCacheRef,
    balance_cache: AddressBalanceCacheRef,
}

impl BatchBlockFlusher {
    pub fn new(
        db: BalanceHistoryDBRef,
        utxo_cache: UTXOCacheRef,
        balance_cache: AddressBalanceCacheRef,
    ) -> Self {
        Self {
            db,
            utxo_cache,
            balance_cache,
        }
    }

    pub async fn flush(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        self.flush_utxos(data)?;
        self.flush_balances(data)?;

        Ok(())
    }

    fn flush_balances(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        // Update balance cache
        {
            let balances = data.balances.lock().unwrap();
            for (script_hash, entry) in balances.iter() {
                self.balance_cache.put(
                    *script_hash,
                    AddressBalanceItem {
                        block_height: entry.block_height,
                        delta: entry.delta,
                        balance: entry.balance,
                    },
                );
            }
        }

        // Update balance to db in batch
        let last_block_height = data.block_range.end - 1;
        let all = data.balance_history.lock().unwrap();
        self.db
            .update_address_history_sync(&all, last_block_height as u32)?;

        Ok(())
    }

    fn flush_utxos(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        // Flush UTXO cache
        // First found unspent UTXOs to add to cache and db
        let mut utxo_list = Vec::new();
        {
            let vout_utxos = data.vout_utxos.lock().unwrap();
            utxo_list.reserve(vout_utxos.len());

            for (outpoint, vout_utxo_info) in vout_utxos.iter() {
                if vout_utxo_info.spend {
                    continue;
                }

                utxo_list.push((
                    outpoint.clone(),
                    (
                        vout_utxo_info.item.script_hash.clone(),
                        vout_utxo_info.item.value,
                    ),
                ));
                self.utxo_cache.put(
                    outpoint.clone(),
                    vout_utxo_info.item.script_hash.clone(),
                    vout_utxo_info.item.value,
                );
            }
        }

        utxo_list.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));

        // Then found all spent UTXOs to remove from db
        let mut spent_utxo_list = Vec::new();
        {
            // Traverse all vin to find spent ones
            let blocks = data.blocks.lock().unwrap();
            let mut total = 0;
            for block in blocks.iter() {
                for tx in block.txdata.iter() {
                    total += tx.vin.len();
                }
            }

            spent_utxo_list.reserve(total);

            for block in blocks.iter() {
                for tx in block.txdata.iter() {
                    for vin in tx.vin.iter() {
                        if vin.need_flush {
                            spent_utxo_list.push(vin.outpoint.clone());
                        }
                    }
                }
            }
        }

        spent_utxo_list.par_sort_unstable();

        // Update UTXOs in db finally
        self.db.update_utxos_async(&utxo_list, &spent_utxo_list)?;

        Ok(())
    }
}

// Use to keep the balance history result for a block
type BlockHistoryResult = HashMap<USDBScriptHash, BalanceHistoryEntry>;

pub struct BatchBlockBalanceProcessor {}

impl BatchBlockBalanceProcessor {
    pub fn new() -> Self {
        Self {}
    }

    pub fn process(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        
        // For each block in the batch, process balances
        let blocks = data.blocks.lock().unwrap();
        let mut block_history_results = Vec::with_capacity(blocks.len());
        let mut block_history_count = 0;

        for (block, height) in blocks.iter().zip(data.block_range.clone()) {
            assert!(
                block.height == height,
                "Block height mismatch: expected {}, got {}",
                height,
                block.height
            );

            // First traverse all transactions to calculate balance delta
            let mut block_history = HashMap::new();
            let mut balances = data.balances.lock().unwrap();
            for tx in block.txdata.iter() {
                // Process vin (decrease balance)
                for vin in tx.vin.iter() {
                    let vout = vin.cache_tx_out.as_ref().unwrap();

                    match block_history.entry(&vout.script_hash) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // Create new entry
                            let current_balance = balances.get(&vout.script_hash).ok_or_else(|| {
                                let msg = format!(
                                    "Balance not found for address {} at block height {}",
                                    vout.script_hash, block.height
                                );
                                error!("{}", msg);
                                msg
                            })?;

                            assert!(
                                current_balance.block_height < block.height,
                                "Balance block height {} is greater or equal to current block height {} for script_hash {}",
                                current_balance.block_height,
                                block.height,
                                vout.script_hash
                            );
                            assert!(
                                current_balance.balance >= vout.value,
                                "Insufficient balance for script_hash {}: {} < {}",
                                vout.script_hash,
                                current_balance.balance,
                                vout.value
                            );

                            let new_balance = BalanceHistoryEntry {
                                script_hash: vout.script_hash.clone(),
                                block_height: block.height,
                                delta: -(vout.value as i64),
                                balance: current_balance.balance,   // Just copy current balance, we will update it below
                            };

                            e.insert(new_balance);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Update existing entry's delta
                            let entry = e.get_mut();

                            entry.delta -= vout.value as i64;
                        }
                    }
                }

                // Process vout (increase balance)
                for vout in tx.vout.iter() {
                    match block_history.entry(&vout.cache_tx_out.script_hash) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // Create new entry
                            let current_balance = balances.get(&vout.cache_tx_out.script_hash).ok_or_else(|| {
                                let msg = format!(
                                    "Balance not found for address {} at block height {}",
                                    vout.cache_tx_out.script_hash, block.height
                                );
                                error!("{}", msg);
                                msg
                            })?;

                            assert!(
                                current_balance.block_height < block.height,
                                "Balance block height {} is greater or equal to current block height {} for script_hash {}",
                                current_balance.block_height,
                                block.height,
                                vout.cache_tx_out.script_hash
                            );

                            let new_balance = BalanceHistoryEntry {
                                script_hash: vout.cache_tx_out.script_hash.clone(),
                                block_height: block.height,
                                delta: vout.cache_tx_out.value as i64,
                                balance: current_balance.balance,   // Just copy current balance, we will update it below
                            };

                            e.insert(new_balance);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Update existing entry's delta
                            let entry = e.get_mut();

                            entry.delta += vout.cache_tx_out.value as i64;
                        }
                    }
                }
            }

            // Then update balances based on deltas
            for (&script_hash, history_entry) in block_history.iter_mut() {
                // First ensure balance will not go negative and calculate new balance
                let balance = history_entry.balance as i64;
                assert!(
                    balance + history_entry.delta >= 0,
                    "Insufficient balance for script_hash {} at block height {}: {} + {} < 0",
                    script_hash,
                    history_entry.block_height,
                    balance,
                    history_entry.delta
                );
                history_entry.balance = (balance + history_entry.delta) as u64;

                // Update the main balance map for current batch processing
                let balance_entry = balances.get_mut(script_hash).ok_or_else(|| {
                    let msg = format!(
                        "Balance not found for address {} at block height {}",
                        script_hash, history_entry.block_height
                    );
                    error!("{}", msg);
                    msg
                })?;

                balance_entry.delta = history_entry.delta;
                balance_entry.balance = history_entry.balance;
                balance_entry.block_height = history_entry.block_height;
            }

            block_history_count += block_history.len();
            block_history_results.push(block_history);
        }

        // Convert to vector and sort
        info!(
            "Processed {} balance history entries for block range {:?}",
            block_history_count,
            data.block_range,
        );

        let mut all = data.balance_history.lock().unwrap();
        assert!(all.is_empty(), "Balance history vector is not empty before flushing");
        all.reserve(block_history_count);
        
        for block_history in block_history_results.into_iter() {
            for (_, entry) in block_history.into_iter() {
                all.push(entry);
            }
        }

        use rayon::prelude::*;
        all.par_sort_by(|a, b| {
            if a.script_hash != b.script_hash {
                return a.script_hash.cmp(&b.script_hash);
            }

            a.block_height.cmp(&b.block_height)
        });

        Ok(())
    }
}

pub struct BatchBlockProcessor {
    btc_client: BTCClientRef,
    db: BalanceHistoryDBRef,
    utxo_cache: UTXOCacheRef,
    balance_cache: AddressBalanceCacheRef,
}

impl BatchBlockProcessor {
    pub fn new(
        btc_client: BTCClientRef,
        db: BalanceHistoryDBRef,
        utxo_cache: UTXOCacheRef,
        balance_cache: AddressBalanceCacheRef,
    ) -> Self {
        Self {
            btc_client,
            db,
            utxo_cache,
            balance_cache,
        }
    }

    pub async fn process_batch(
        &self,
        block_height_range: std::ops::Range<u32>,
    ) -> Result<(), String> {
        let preloader = BatchBlockPreloader::new(
            self.btc_client.clone(),
            self.db.clone(),
            self.utxo_cache.clone(),
            self.balance_cache.clone(),
        );
        let data = preloader.preload(block_height_range.clone())?;

        let processor = BatchBlockBalanceProcessor::new();
        processor.process(&data)?;

        let flusher = BatchBlockFlusher::new(
            self.db.clone(),
            self.utxo_cache.clone(),
            self.balance_cache.clone(),
        );
        flusher.flush(&data).await?;

        Ok(())
    }
}
