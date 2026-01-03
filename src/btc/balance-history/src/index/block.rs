use crate::bench::{BatchBlockBenchMark, BatchBlockBenchMarkRef};
use crate::btc::BTCClientRef;
use crate::cache::{AddressBalanceCacheRef, UTXOCacheRef};
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use bitcoincore_rpc::bitcoin::{Block, OutPoint, Txid};
use dashmap::DashMap;
use rayon::slice::ParallelSliceMut;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use usdb_util::{BalanceHistoryData, OutPointRef, UTXOEntry, UTXOEntryRef};
use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BlockTxIndex {
    block_height: u32,
    tx_index: u32,
}

struct VOutUtxoInfo {
    item: UTXOEntryRef,
    spend: bool, // Whether this UTXO is spent in the batch
}

pub struct PreloadVIn {
    pub outpoint: OutPointRef,
    pub cache_tx_out: Option<UTXOEntryRef>,
    pub need_flush: bool,
}

pub struct PreloadVOut {
    pub outpoint: OutPointRef,
    pub cache_tx_out: UTXOEntryRef,
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
    vout_utxos: Arc<RwLock<HashMap<OutPointRef, VOutUtxoInfo>>>,

    // Keep let latest balances for all addresses involved from block to block t+n
    balances: Arc<DashMap<USDBScriptHash, BalanceHistoryData>>,

    // Use to keep all balance history entries for the batch, will be flushed to db at once
    balance_history: Arc<Mutex<Vec<BalanceHistoryEntry>>>,

    bench_mark: BatchBlockBenchMarkRef,
}

impl BatchBlockData {
    pub fn new() -> Self {
        Self {
            block_range: 0..0,
            blocks: Arc::new(Mutex::new(Vec::new())),
            vout_utxos: Arc::new(RwLock::new(HashMap::new())),
            balances: Arc::new(DashMap::new()),
            balance_history: Arc::new(Mutex::new(Vec::new())),
            bench_mark: Arc::new(BatchBlockBenchMark::new()),
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
        use rayon::prelude::*;

        assert!(
            block_height_range.start < block_height_range.end,
            "Invalid block height range {:?}",
            block_height_range
        );

        let mut data = BatchBlockData::new();
        data.block_range = block_height_range.clone();
        let data = Arc::new(data);

        let begin = std::time::Instant::now();
        let mut blocks = Vec::with_capacity(block_height_range.len());
        let ret: Vec<Result<(u32, Block), String>> = block_height_range
            .clone()
            .into_par_iter()
            .map(|height| {
                self.btc_client
                    .get_block_by_height(height)
                    .map(|block| (height, block))
            })
            .collect();

        for res in ret {
            let (height, block) = res?;
            blocks.push((height, block));
        }

        data.bench_mark.load_blocks_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Preprocess all blocks in parallel and got all vin and vout UTXOs
        let begin = std::time::Instant::now();
        let result: Vec<Result<PreloadBlock, String>> = blocks
            .into_par_iter()
            .map(|(block_height, block)| {
                let preload_block = self.preprocess_block(block_height, &block, &data)?;

                Ok(preload_block)
            })
            .collect();

        let mut preprocessed_blocks = Vec::with_capacity(result.len());
        for res in result {
            preprocessed_blocks.push(res?);
        }
        data.bench_mark.preprocess_utxos_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Now preload UTXOs for all blocks
        let begin = std::time::Instant::now();
        let result: Vec<Result<(), String>> = preprocessed_blocks
            .into_par_iter()
            .map(|mut preload_block| {
                self.preload_utxos(&mut preload_block, &data)?;

                data.blocks.lock().unwrap().push(preload_block);

                Ok(())
            })
            .collect();
        for res in result {
            res?;
        }

        data.bench_mark.preload_utxos_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Sort the blocks by height
        {
            let mut blocks = data.blocks.lock().unwrap();
            blocks.par_sort_unstable_by(|a, b| a.height.cmp(&b.height));
        }

        // Load balances at the starting block height - 1
        if block_height_range.start > 0 {
            let begin = std::time::Instant::now();
            let target_block_height = block_height_range.start - 1;
            self.preload_balances(target_block_height, &data)?;

            data.bench_mark.preload_balances_duration_micros.store(
                begin.elapsed().as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
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
        // Here we do not use rayon because we already used rayon to process blocks in higher level
        preload_block.txdata = block
            .txdata
            .iter()
            .map(|tx| {
                let mut preload_tx = PreloadTx {
                    txid: tx.compute_txid(),
                    vin: Vec::with_capacity(tx.input.len()),
                    vout: Vec::with_capacity(tx.output.len()),
                };

                if !tx.is_coinbase() {
                    for vin in &tx.input {
                        let outpoint = &vin.previous_output;

                        // Here we just use None as placeholder, the real UTXO will be loaded in batch later
                        let preload_vin = PreloadVIn {
                            outpoint: Arc::new(outpoint.clone()),
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

                    let cache_tx_out = UTXOEntry {
                        value: vout.value.to_sat(),
                        script_hash: vout.script_pubkey.to_usdb_script_hash(),
                    };

                    let preload_vout = PreloadVOut {
                        outpoint: Arc::new(outpoint),
                        cache_tx_out: Arc::new(cache_tx_out),
                    };
                    preload_tx.vout.push(preload_vout);
                }

                preload_tx
            })
            .collect();

        // Append all vout UTXOs to UTXO cache
        let mut vout_utxo_map = data.vout_utxos.write().unwrap();
        let estimated = preload_block
            .txdata
            .iter()
            .map(|tx| tx.vout.len())
            .sum::<usize>();
        vout_utxo_map.reserve(estimated);

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

    fn preload_utxos(
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
                {
                    let mut vout_utxo_map = data.vout_utxos.write().unwrap();
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
        let begin = std::time::Instant::now();
        data.bench_mark
            .preload_utxos_from_none_memory_counts
            .fetch_add(
                outpoints_to_load.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        let loaded_utxos = self.fetch_utxos(&outpoints_to_load)?;
        data.bench_mark
            .preload_utxos_from_none_memory_duration_micros
            .store(
                begin.elapsed().as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );

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

    fn fetch_utxos(&self, outpoints: &[OutPointRef]) -> Result<Vec<UTXOEntryRef>, String> {
        // First try to get from db by bulk
        let all = self.db.get_utxos_bulk(outpoints)?;

        // Then load from rpc for missing ones
        let mut result = Vec::with_capacity(outpoints.len());
        for (i, item) in all.into_iter().enumerate() {
            if let Some(utxo) = item {
                result.push(Arc::new(utxo));
            } else {
                // Load from rpc
                let (script, amount) = self.btc_client.get_utxo(&outpoints[i])?;
                let entry = UTXOEntry {
                    value: amount.to_sat(),
                    script_hash: script.to_usdb_script_hash(),
                };
                result.push(Arc::new(entry));
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
        let blocks = data.blocks.lock().unwrap();
        let addresses: HashSet<_> = blocks
            .par_iter()
            .flat_map(|block| block.txdata.par_iter())
            .flat_map(|tx| {
                // Collect vin addresses
                let vin_hashes = tx
                    .vin
                    .par_iter()
                    .map(|vin| vin.cache_tx_out.as_ref().unwrap().script_hash.clone());

                // Collect vout addresses
                let vout_hashes = tx
                    .vout
                    .par_iter()
                    .map(|vout| vout.cache_tx_out.script_hash.clone());

                vin_hashes.chain(vout_hashes)
            })
            .collect();

        let mut sorted_addresses: Vec<_> = addresses.into_iter().collect();
        sorted_addresses.par_sort_unstable();

        data.bench_mark.preload_balances_counts.store(
            sorted_addresses.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Batch load balances
        sorted_addresses
            .into_par_iter()
            .map(|script_hash| {
                // First load from global balance cache
                if let Some(cached) = self
                    .balance_cache
                    .get(&script_hash, target_block_height as u32)
                {
                    data.balances.insert(script_hash, cached.as_ref().clone());
                    return Ok(());
                }

                // Then load from db
                let balance = self
                    .db
                    .get_balance_at_block_height(&script_hash, target_block_height as u32)?;
                data.bench_mark
                    .preload_balances_from_db_counts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                data.balances.insert(script_hash, balance);
                Ok(())
            })
            .find_any(|ret: &Result<(), String>| ret.is_err())
            .map_or_else(|| Ok(()), |e| e.clone())?;

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

    pub fn flush(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        self.flush_utxos(data)?;
        self.flush_balances(data)?;

        Ok(())
    }

    fn flush_balances(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        // Update block balance caches to global balance caches
        {
            for entry in data.balances.iter() {
                self.balance_cache
                    .put(entry.key(), Arc::new(entry.value().clone()));
            }

            data.bench_mark.batch_update_balance_cache_counts.store(
                data.balances.len() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        // Update balance to db in batch
        let begin = std::time::Instant::now();

        let last_block_height = data.block_range.end - 1;
        let all = data.balance_history.lock().unwrap();
        self.db
            .update_address_history_sync(&all, last_block_height as u32)?;

        // Update bench mark info
        let duration = begin.elapsed();
        data.bench_mark.batch_update_balances_duration_micros.store(
            duration.as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        data.bench_mark
            .batch_put_balance_counts
            .store(all.len() as u64, std::sync::atomic::Ordering::Relaxed);

        Ok(())
    }

    fn flush_utxos(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        // Flush UTXO cache
        // First found unspent UTXOs to add to cache and db
        let mut utxo_list = Vec::new();
        {
            let vout_utxos = data.vout_utxos.read().unwrap();
            utxo_list.reserve(vout_utxos.len());

            for (outpoint, vout_utxo_info) in vout_utxos.iter() {
                if vout_utxo_info.spend {
                    continue;
                }

                utxo_list.push((outpoint.clone(), vout_utxo_info.item.clone()));

                self.utxo_cache
                    .put(outpoint.clone(), vout_utxo_info.item.clone());
            }
        }

        utxo_list.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));

        // Then found all spent UTXOs to remove from db
        let mut spent_utxo_list: Vec<_> = {
            use rayon::prelude::*;
            let blocks = data.blocks.lock().unwrap();

            blocks
                .par_iter()
                .flat_map(|block| {
                    block.txdata.par_iter().flat_map(|tx| {
                        tx.vin
                            .par_iter()
                            .filter(|vin| vin.need_flush)
                            .map(|vin| vin.outpoint.clone())
                    })
                })
                .collect()
        };

        spent_utxo_list.par_sort_unstable();

        // Update UTXOs in db finally
        let begin = std::time::Instant::now();

        self.db.update_utxos_async(&utxo_list, &spent_utxo_list)?;

        let duration = begin.elapsed();
        data.bench_mark.batch_update_utxo_duration_micros.store(
            duration.as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Update bench mark info
        data.bench_mark
            .batch_put_utxo_counts
            .store(utxo_list.len() as u64, std::sync::atomic::Ordering::Relaxed);
        data.bench_mark.batch_spent_utxo_counts.store(
            spent_utxo_list.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

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
        let mut block_history_count = 0;

        // First calc delta in parallel
        use rayon::prelude::*;
        let mut block_history_results: Vec<Result<HashMap<USDBScriptHash, BalanceHistoryData>, String>> = blocks.par_iter().map(|block| {

            // Traverse all transactions to calculate balance delta
            let mut block_history: HashMap<USDBScriptHash, BalanceHistoryData> = HashMap::with_capacity(block.txdata.len() * 16);
            for tx in block.txdata.iter() {
                // Process vin (decrease balance)
                for vin in tx.vin.iter() {
                    let vout = vin.cache_tx_out.as_ref().unwrap();

                    match block_history.entry(vout.script_hash) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // Create new entry
                            let new_balance = BalanceHistoryData {
                                block_height: block.height,
                                delta: -(vout.value as i64),
                                balance: 0, // Just set balance to 0, we will update it below
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
                    match block_history.entry(vout.cache_tx_out.script_hash) {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // Create new entry
                            let new_balance = BalanceHistoryData {
                                block_height: block.height,
                                delta: vout.cache_tx_out.value as i64,
                                balance: 0, // Just set balance to 0, we will update it below
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

            Ok(block_history)
        }).collect();


        // Then update balances based on deltas serialized
        for ret in block_history_results.iter_mut() {
            let block_history = ret.as_mut().map_err(|e| e.to_string())?;
            for (&script_hash, history_entry) in block_history.iter_mut() {
                // First load current balance entry to get the last balance
                let mut balance_entry = data.balances.get_mut(&script_hash).ok_or_else(|| {
                    let msg = format!(
                        "Balance not found for address {} at block height {}",
                        script_hash, history_entry.block_height
                    );
                    error!("{}", msg);
                    msg
                })?;

                // Ensure balance will not go negative and calculate new balance
                let balance = balance_entry.balance as i64;
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
                balance_entry.delta = history_entry.delta;
                balance_entry.balance = history_entry.balance;
                balance_entry.block_height = history_entry.block_height;
            }

            block_history_count += block_history.len();
        }

        // Convert to vector and sort
        info!(
            "Processed {} balance history entries for block range {:?}",
            block_history_count, data.block_range,
        );

        let mut all = data.balance_history.lock().unwrap();
        assert!(
            all.is_empty(),
            "Balance history vector is not empty before flushing"
        );
        all.reserve(block_history_count);

        for ret in block_history_results.into_iter() {
            let block_history = ret?;
            for (script_hash, data) in block_history.into_iter() {
                let entry = BalanceHistoryEntry {
                    script_hash,
                    block_height: data.block_height,
                    delta: data.delta,
                    balance: data.balance,
                };
                all.push(entry);
            }
        }

        all.par_sort_by(|a, b| {
            if a.script_hash != b.script_hash {
                return a.script_hash.cmp(&b.script_hash);
            }

            a.block_height.cmp(&b.block_height)
        });

        Ok(())
    }
}

#[derive(Clone)]
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

    pub fn process_blocks(&self, block_height_range: std::ops::Range<u32>) -> Result<(), String> {
        let preloader = BatchBlockPreloader::new(
            self.btc_client.clone(),
            self.db.clone(),
            self.utxo_cache.clone(),
            self.balance_cache.clone(),
        );
        let data = preloader.preload(block_height_range.clone())?;

        let begin = std::time::Instant::now();
        let processor = BatchBlockBalanceProcessor::new();
        processor.process(&data)?;

        data.bench_mark.process_balances_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Flush all data to db and caches
        let flusher = BatchBlockFlusher::new(
            self.db.clone(),
            self.utxo_cache.clone(),
            self.balance_cache.clone(),
        );
        flusher.flush(&data)?;

        data.bench_mark.balance_cache_counts.store(
            self.balance_cache.get_count(),
            std::sync::atomic::Ordering::Relaxed,
        );
        data.bench_mark.utxo_cache_counts.store(
            self.utxo_cache.get_count(),
            std::sync::atomic::Ordering::Relaxed,
        );

        data.bench_mark.log();

        Ok(())
    }
}
