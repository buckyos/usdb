use crate::bench::{BatchBlockBenchMark, BatchBlockBenchMarkRef};
use crate::btc::BTCClientRef;
use crate::cache::{AddressBalanceCacheRef, UTXOCacheRef};
use crate::db::{
    BalanceHistoryDBRef, BalanceHistoryEntry, BlockCommitEntry, BlockStateUpdateBatch,
    BlockUndoBundle, BlockUndoUtxoEntry, ScriptRegistryEntry,
};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash, OutPoint, Txid};
use dashmap::DashMap;
use rayon::slice::ParallelSliceMut;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use usdb_util::{BalanceHistoryData, OutPointRef, UTXOEntryRef, UTXOValue};
use usdb_util::{BtcScriptHash, ToBtcScriptHash};

// EMPTY_COMMIT_HASH is the genesis previous-commit value used when the batch
// starts from block height 0 and there is no earlier committed block.
const EMPTY_COMMIT_HASH: [u8; 32] = [0u8; 32];

const BIP30_DUPLICATE_COINBASES: [(u32, &str, &str); 2] = [
    (
        91_842,
        "00000000000a4d0a398161ffc163c503763b1f4360639393e0e4c8e300e0caec",
        "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599",
    ),
    (
        91_880,
        "00000000000743f190a18c5577a3c2d2a1f610ae9601ac046a38084ccb7cd721",
        "e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468",
    ),
];

type UtxoUpdateSet = (Vec<(OutPointRef, UTXOEntryRef)>, Vec<OutPointRef>);

fn is_bip30_duplicate_coinbase(block_height: u32, block_hash: &BlockHash, txid: &Txid) -> bool {
    BIP30_DUPLICATE_COINBASES
        .iter()
        .any(|(height, expected_block_hash, expected_txid)| {
            block_height == *height
                && block_hash.to_string() == *expected_block_hash
                && txid.to_string() == *expected_txid
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct BlockTxIndex {
    block_height: u32,
    tx_index: u32,
}

struct VOutUtxoInfo {
    item: UTXOEntryRef,
    position: BlockTxIndex,
    spend: bool, // Whether this UTXO is spent in the batch
}

impl VOutUtxoInfo {
    fn created_before(&self, spending_position: BlockTxIndex) -> bool {
        self.position < spending_position
    }
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
    // UTXOs displaced by a duplicate outpoint generation, including the two
    // historical BIP30 coinbase exceptions. They behave like synthetic inputs
    // for balance and undo.
    pub displaced_vout: Vec<PreloadVIn>,
    pub vout: Vec<PreloadVOut>,
}

pub struct PreloadBlock {
    // BTC block height of this preloaded block.
    pub height: u32,
    // BTC block hash paired with the height above. This is committed into the
    // balance-history block commit chain and must stay aligned with height.
    pub block_hash: BlockHash,
    // Preloaded transactions with all vin/vout information needed by the batch processor.
    pub txdata: Vec<PreloadTx>,
}

// BlockBalanceDelta is the canonical per-block logical result that feeds the
// first-version balance-history commit chain.
#[derive(Clone, Debug)]
pub struct BlockBalanceDelta {
    // BTC block height for this logical balance delta set.
    pub block_height: u32,
    // BTC block hash bound to the same logical delta set.
    pub block_hash: BlockHash,
    // Balance entries after this block is applied, sorted by script_hash.
    pub entries: Vec<BalanceHistoryEntry>,
}

struct VInPosition {
    tx_index: usize,
    vin_index: usize,
}

pub struct BatchBlockData {
    block_range: std::ops::Range<u32>,
    blocks: Arc<Mutex<Vec<PreloadBlock>>>,
    // Keep duplicate outpoint generations ordered by creation position so
    // parallel vin loading remains deterministic. On canonical mainnet the
    // only unspent replacements are the two historical BIP30 exceptions.
    vout_utxos: Arc<RwLock<HashMap<OutPointRef, Vec<VOutUtxoInfo>>>>,

    // Keep latest balances for all addresses involved while processing a batch.
    balances: Arc<DashMap<BtcScriptHash, BalanceHistoryData>>,

    // Keep all balance history entries for the batch and flush them to DB once.
    balance_history: Arc<Mutex<Vec<BalanceHistoryEntry>>>,

    // Keep deterministic per-block balance deltas for block commit calculation.
    block_balance_deltas: Arc<Mutex<Vec<BlockBalanceDelta>>>,

    // Keep auxiliary script registry entries discovered while preloading block outputs.
    script_registry: Arc<Mutex<Vec<ScriptRegistryEntry>>>,

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
            block_balance_deltas: Arc::new(Mutex::new(Vec::new())),
            script_registry: Arc::new(Mutex::new(Vec::new())),
            bench_mark: Arc::new(BatchBlockBenchMark::new()),
        }
    }
}

// compute_balance_delta_root hashes the canonical logical balance result of one block.
// The hash only covers balance-history state, not UTXO cache state.
fn compute_balance_delta_root(block: &BlockBalanceDelta) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"balance-history:block-delta-root:v1");
    hasher.update(block.block_height.to_be_bytes());
    hasher.update(block.block_hash.as_ref() as &[u8]);

    for entry in &block.entries {
        hasher.update(entry.script_hash.as_ref() as &[u8]);
        hasher.update(entry.block_height.to_be_bytes());
        hasher.update(entry.delta.to_be_bytes());
        hasher.update(entry.balance.to_be_bytes());
    }

    hasher.finalize().into()
}

// compute_block_commit links one block's logical result to the previous committed block.
fn compute_block_commit(
    block_height: u32,
    block_hash: &BlockHash,
    balance_delta_root: &[u8; 32],
    prev_block_commit: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"balance-history:block-commit:v1");
    hasher.update(block_height.to_be_bytes());
    hasher.update(block_hash.as_ref() as &[u8]);
    hasher.update(balance_delta_root);
    hasher.update(prev_block_commit);
    hasher.finalize().into()
}

// build_block_commits computes a contiguous commit chain for a fully ordered batch.
// The caller must ensure `blocks` are already sorted by block_height.
fn build_block_commits(
    blocks: &[BlockBalanceDelta],
    mut prev_block_commit: [u8; 32],
) -> Vec<BlockCommitEntry> {
    let mut commits = Vec::with_capacity(blocks.len());
    for block in blocks {
        let balance_delta_root = compute_balance_delta_root(block);
        let block_commit = compute_block_commit(
            block.block_height,
            &block.block_hash,
            &balance_delta_root,
            &prev_block_commit,
        );
        commits.push(BlockCommitEntry {
            block_height: block.block_height,
            btc_block_hash: block.block_hash,
            balance_delta_root,
            block_commit,
        });
        prev_block_commit = block_commit;
    }
    commits
}

fn resolve_previous_commit(
    previous_height: Option<u32>,
    previous_commit: Option<&BlockCommitEntry>,
) -> Result<[u8; 32], String> {
    match previous_height {
        None | Some(0) => Ok(previous_commit
            .map(|commit| commit.block_commit)
            .unwrap_or(EMPTY_COMMIT_HASH)),
        Some(height) => previous_commit
            .map(|commit| commit.block_commit)
            .ok_or_else(|| {
                let msg = format!(
                    "Missing previous block commit at height {}. Backfill or resync is required before continuing.",
                    height
                );
                error!("{}", msg);
                msg
            }),
    }
}

// Persist undo only for the hot reorg window close to the current canonical BTC tip.
fn should_persist_undo_for_block(
    block_height: u32,
    latest_btc_height: u32,
    undo_retention_blocks: u32,
) -> bool {
    if undo_retention_blocks == 0 || block_height > latest_btc_height {
        return false;
    }

    let min_undo_height = latest_btc_height.saturating_sub(undo_retention_blocks - 1);
    block_height >= min_undo_height
}

// Return true when at least one block in the batch overlaps the hot undo window.
fn should_persist_any_undo_for_range(
    block_height_range: &std::ops::Range<u32>,
    latest_btc_height: u32,
    undo_retention_blocks: u32,
) -> bool {
    if block_height_range.is_empty() || undo_retention_blocks == 0 {
        return false;
    }

    let min_undo_height = latest_btc_height.saturating_sub(undo_retention_blocks - 1);
    let last_block_height = block_height_range.end - 1;

    block_height_range.start <= latest_btc_height && last_block_height >= min_undo_height
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
        blocks.sort_unstable_by_key(|(height, _)| *height);
        self.validate_loaded_chain(&block_height_range, &blocks)?;

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
        preprocessed_blocks.sort_unstable_by_key(|block| block.height);
        let duplicate_outpoints = Self::index_batch_vouts(&preprocessed_blocks, &data);
        data.bench_mark.preprocess_utxos_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Now preload UTXOs for all blocks
        let begin = std::time::Instant::now();
        let result: Vec<Result<(), String>> = preprocessed_blocks
            .par_iter_mut()
            .map(|preload_block| self.preload_utxos(preload_block, &data))
            .collect();
        for res in result {
            res?;
        }
        Self::resolve_duplicate_outpoint_generations(
            &self.db,
            &mut preprocessed_blocks,
            &data,
            &duplicate_outpoints,
        )?;
        *data.blocks.lock().unwrap() = preprocessed_blocks;

        data.bench_mark.preload_utxos_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

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

    fn validate_loaded_chain(
        &self,
        block_height_range: &std::ops::Range<u32>,
        blocks: &[(u32, Block)],
    ) -> Result<(), String> {
        if blocks.len() != block_height_range.len() {
            return Err(format!(
                "Loaded block count mismatch for range {:?}: expected {}, got {}",
                block_height_range,
                block_height_range.len(),
                blocks.len()
            ));
        }

        let durable_height = self.db.get_btc_block_height()?;
        if block_height_range.start != durable_height.saturating_add(1) {
            return Err(format!(
                "Block batch must extend the durable tip contiguously: durable_height={}, requested_range={:?}",
                durable_height, block_height_range
            ));
        }

        let first = blocks.first().ok_or_else(|| {
            format!(
                "Loaded empty block batch for range {:?}",
                block_height_range
            )
        })?;
        let expected_parent_hash = match self.db.get_block_commit(durable_height)? {
            Some(commit) => commit.btc_block_hash,
            None if durable_height == 0 => self.btc_client.get_block_hash(0)?,
            None => {
                return Err(format!(
                    "Missing durable parent block commit at height {} for batch {:?}",
                    durable_height, block_height_range
                ));
            }
        };
        if first.1.header.prev_blockhash != expected_parent_hash {
            return Err(format!(
                "Block batch parent mismatch at height {}: expected {}, got {}",
                first.0, expected_parent_hash, first.1.header.prev_blockhash
            ));
        }

        for (offset, (height, block)) in blocks.iter().enumerate() {
            let expected_height = block_height_range.start + offset as u32;
            if *height != expected_height {
                return Err(format!(
                    "Non-contiguous loaded block height: expected {}, got {}",
                    expected_height, height
                ));
            }
            if offset > 0 {
                let previous_hash = blocks[offset - 1].1.block_hash();
                if block.header.prev_blockhash != previous_hash {
                    return Err(format!(
                        "Block batch linkage mismatch at height {}: expected parent {}, got {}",
                        height, previous_hash, block.header.prev_blockhash
                    ));
                }
            }
        }

        Ok(())
    }

    fn index_batch_vouts(blocks: &[PreloadBlock], data: &BatchBlockData) -> Vec<OutPointRef> {
        let mut vout_utxo_map = data.vout_utxos.write().unwrap();
        let mut duplicate_outpoints = Vec::new();
        let estimated = blocks
            .iter()
            .flat_map(|block| block.txdata.iter())
            .map(|tx| tx.vout.len())
            .sum::<usize>();
        vout_utxo_map.reserve(estimated);

        for block in blocks {
            for (tx_index, tx) in block.txdata.iter().enumerate() {
                let position = BlockTxIndex {
                    block_height: block.height,
                    tx_index: tx_index as u32,
                };
                for vout in &tx.vout {
                    let generations = vout_utxo_map.entry(vout.outpoint.clone()).or_default();
                    if !generations.is_empty() {
                        duplicate_outpoints.push(vout.outpoint.clone());
                    }
                    generations.push(VOutUtxoInfo {
                        item: vout.cache_tx_out.clone(),
                        position,
                        spend: false,
                    });
                }
            }
        }

        duplicate_outpoints.sort_unstable_by(|left, right| left.as_ref().cmp(right.as_ref()));
        duplicate_outpoints.dedup_by(|left, right| left.as_ref() == right.as_ref());
        duplicate_outpoints
    }

    fn preprocess_block(
        &self,
        block_height: u32,
        block: &Block,
        data: &BatchBlockData,
    ) -> Result<PreloadBlock, String> {
        let mut preload_block = PreloadBlock {
            height: block_height,
            block_hash: block.block_hash(),
            txdata: Vec::with_capacity(block.txdata.len()),
        };
        let mut script_registry_entries = Vec::new();

        // Load all vins' UTXOs into cache
        // Here we do not use rayon because we already used rayon to process blocks in higher level
        preload_block.txdata = block
            .txdata
            .iter()
            .map(|tx| {
                let mut preload_tx = PreloadTx {
                    txid: tx.compute_txid(),
                    vin: Vec::with_capacity(tx.input.len()),
                    displaced_vout: Vec::new(),
                    vout: Vec::with_capacity(tx.output.len()),
                };

                if !tx.is_coinbase() {
                    for vin in &tx.input {
                        let outpoint = &vin.previous_output;

                        // Here we just use None as placeholder, the real UTXO will be loaded in batch later
                        let preload_vin = PreloadVIn {
                            outpoint: Arc::new(*outpoint),
                            cache_tx_out: None,
                            need_flush: true,
                        };
                        preload_tx.vin.push(preload_vin);
                    }
                }

                for (n, vout) in tx.output.iter().enumerate() {
                    // Match Bitcoin Core's UTXO exclusion semantics exactly.
                    if usdb_util::is_core_unspendable(&vout.script_pubkey) {
                        continue;
                    }

                    let outpoint = OutPoint {
                        txid: preload_tx.txid,
                        vout: n as u32,
                    };
                    let script_hash = vout.script_pubkey.to_btc_script_hash();
                    script_registry_entries.push(ScriptRegistryEntry {
                        script_hash,
                        script_pubkey: vout.script_pubkey.clone(),
                    });

                    let cache_tx_out = UTXOValue {
                        value: vout.value.to_sat(),
                        script_hash,
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

        if !script_registry_entries.is_empty() {
            data.script_registry
                .lock()
                .unwrap()
                .extend(script_registry_entries);
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
                let spending_position = BlockTxIndex {
                    block_height: preload_block.height,
                    tx_index: tx_index as u32,
                };
                // First check if the UTXO is already in vout cache (i.e., created in the same batch)
                {
                    let mut vout_utxo_map = data.vout_utxos.write().unwrap();
                    if let Some(generations) = vout_utxo_map.get_mut(&vin.outpoint)
                        && let Some(vout_utxo_info) = generations
                            .iter_mut()
                            .rev()
                            .find(|candidate| candidate.created_before(spending_position))
                    {
                        if vout_utxo_info.spend {
                            return Err(format!(
                                "Double spend of in-batch UTXO {} at block height {}",
                                vin.outpoint, preload_block.height
                            ));
                        }
                        vout_utxo_info.spend = true;

                        vin.cache_tx_out.replace(vout_utxo_info.item.clone());
                        // Same-block spends never existed before the rollback boundary, but
                        // earlier-block spends inside this batch must be restorable if a later
                        // block is rolled back.
                        vin.need_flush =
                            vout_utxo_info.position.block_height < preload_block.height;

                        continue;
                    }
                }

                // Then check if the UTXO is in the global cache.
                // Preload must stay read-only; the real cache spend happens only
                // after the block batch is durably committed to RocksDB.
                if let Some(cache_tx_out) = self.utxo_cache.get(&vin.outpoint) {
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

    fn resolve_duplicate_outpoint_generations(
        db: &BalanceHistoryDBRef,
        blocks: &mut [PreloadBlock],
        data: &BatchBlockData,
        duplicate_outpoints: &[OutPointRef],
    ) -> Result<(), String> {
        let first_height = blocks
            .first()
            .map(|block| block.height)
            .ok_or_else(|| "Cannot resolve duplicate outpoints in an empty batch".to_string())?;
        let last_height = blocks.last().map(|block| block.height).unwrap();
        let contains_bip30_exception = BIP30_DUPLICATE_COINBASES
            .iter()
            .any(|(height, _, _)| *height >= first_height && *height <= last_height);
        if duplicate_outpoints.is_empty() && !contains_bip30_exception {
            return Ok(());
        }

        let mut displacements = Vec::new();
        let mut replaced = HashSet::new();

        // Handle duplicate generations whose original and replacement both
        // live in this batch. Normal inputs have already marked any generation
        // that was spent before the replacement was created.
        {
            let mut vout_utxo_map = data.vout_utxos.write().unwrap();
            for outpoint in duplicate_outpoints {
                let generations = vout_utxo_map.get_mut(outpoint).ok_or_else(|| {
                    format!(
                        "Missing indexed generations for duplicate outpoint {}",
                        outpoint
                    )
                })?;
                for replacement_index in 1..generations.len() {
                    let (earlier, replacement) = generations.split_at_mut(replacement_index);
                    let displaced = &mut earlier[replacement_index - 1];
                    let replacement = &replacement[0];
                    if displaced.spend {
                        continue;
                    }

                    displaced.spend = true;
                    replaced.insert((replacement.position, *outpoint.as_ref()));
                    displacements.push((
                        replacement.position,
                        outpoint.clone(),
                        displaced.item.clone(),
                        displaced.position.block_height < replacement.position.block_height,
                    ));
                }
            }
        }

        // When the original generation was committed by an earlier batch, it
        // is not present in the in-memory generation index. Only the two known
        // historical BIP30 exceptions are allowed to query and replace such a
        // durable UTXO; doing this for every output would be prohibitively
        // expensive during initial sync.
        for (height, _, _) in BIP30_DUPLICATE_COINBASES {
            if height < first_height || height > last_height {
                continue;
            }

            let block_index = (height - first_height) as usize;
            let block = blocks
                .get(block_index)
                .ok_or_else(|| format!("Missing block at BIP30 exception height {}", height))?;
            if block.height != height {
                return Err(format!(
                    "Non-contiguous block batch at BIP30 exception height {}: got {}",
                    height, block.height
                ));
            }
            let Some(tx) = block.txdata.first() else {
                return Err(format!(
                    "Missing coinbase transaction at BIP30 exception height {}",
                    height
                ));
            };
            if !is_bip30_duplicate_coinbase(height, &block.block_hash, &tx.txid) {
                continue;
            }

            let position = BlockTxIndex {
                block_height: height,
                tx_index: 0,
            };
            let mut replacement_count = 0usize;
            for vout in &tx.vout {
                if replaced.contains(&(position, *vout.outpoint.as_ref())) {
                    replacement_count += 1;
                    continue;
                }

                if let Some(displaced) = db.get_utxo(vout.outpoint.as_ref())? {
                    replaced.insert((position, *vout.outpoint.as_ref()));
                    displacements.push((
                        position,
                        vout.outpoint.clone(),
                        Arc::new(displaced),
                        true,
                    ));
                    replacement_count += 1;
                }
            }

            if replacement_count == 0 {
                return Err(format!(
                    "BIP30 duplicate coinbase {} at height {} found no displaced UTXO. Rebuild balance-history from a data model that indexes the original coinbase generation.",
                    tx.txid, block.height
                ));
            }
        }

        displacements.sort_unstable_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.as_ref().cmp(right.1.as_ref()))
        });
        for (position, outpoint, displaced, need_flush) in displacements {
            let block_index = position
                .block_height
                .checked_sub(first_height)
                .ok_or_else(|| {
                    format!(
                        "Invalid BIP30 displacement height {} before batch start {}",
                        position.block_height, first_height
                    )
                })? as usize;
            let tx = blocks
                .get_mut(block_index)
                .and_then(|block| block.txdata.get_mut(position.tx_index as usize))
                .ok_or_else(|| {
                    format!(
                        "Missing transaction for BIP30 displacement at height {}, tx_index {}",
                        position.block_height, position.tx_index
                    )
                })?;
            tx.displaced_vout.push(PreloadVIn {
                outpoint,
                cache_tx_out: Some(displaced),
                need_flush,
            });
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
                let entry = UTXOValue {
                    value: amount.to_sat(),
                    script_hash: script.to_btc_script_hash(),
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
                    .chain(tx.displaced_vout.par_iter())
                    .map(|vin| vin.cache_tx_out.as_ref().unwrap().script_hash);

                // Collect vout addresses
                let vout_hashes = tx.vout.par_iter().map(|vout| vout.cache_tx_out.script_hash);

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
                if let Some(cached) = self.balance_cache.get(&script_hash, target_block_height) {
                    data.balances.insert(script_hash, cached.as_ref().clone());
                    return Ok(());
                }

                // Then load from db
                let balance = self
                    .db
                    .get_balance_at_block_height(&script_hash, target_block_height)?;
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
    latest_btc_height: u32,
    undo_retention_blocks: u32,
}

impl BatchBlockFlusher {
    pub fn new(
        db: BalanceHistoryDBRef,
        utxo_cache: UTXOCacheRef,
        balance_cache: AddressBalanceCacheRef,
        latest_btc_height: u32,
        undo_retention_blocks: u32,
    ) -> Self {
        Self {
            db,
            utxo_cache,
            balance_cache,
            latest_btc_height,
            undo_retention_blocks,
        }
    }

    pub fn flush(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        let (new_utxos, spent_utxos) = self.collect_utxo_updates(data)?;
        let (balance_entries, block_commits, last_block_height) =
            self.collect_balance_updates(data)?;
        let script_registry_entries = self.collect_script_registry_updates(data)?;
        let undo_bundles = self.collect_undo_bundles(data)?;

        let begin = std::time::Instant::now();
        self.db
            .update_block_state_batch_async(BlockStateUpdateBatch {
                new_utxos: &new_utxos,
                remove_utxos: &spent_utxos,
                entries_list: &balance_entries,
                block_height: last_block_height,
                block_commits: &block_commits,
                script_registry_entries: &script_registry_entries,
                undo_bundles: &undo_bundles,
            })?;
        let duration = begin.elapsed();

        data.bench_mark.batch_update_utxo_duration_micros.store(
            duration.as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        data.bench_mark.batch_update_balances_duration_micros.store(
            duration.as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        self.apply_cache_updates(data, &new_utxos, &spent_utxos);

        Ok(())
    }

    fn collect_balance_updates(
        &self,
        data: &BatchBlockDataRef,
    ) -> Result<(Vec<BalanceHistoryEntry>, Vec<BlockCommitEntry>, u32), String> {
        let previous_height = data.block_range.start.checked_sub(1);
        let previous_commit_entry = match previous_height {
            Some(height) => self.db.get_block_commit(height)?,
            None => None,
        };
        let previous_commit =
            resolve_previous_commit(previous_height, previous_commit_entry.as_ref())?;

        let last_block_height = data.block_range.end - 1;
        let all = data.balance_history.lock().unwrap().clone();
        let block_balance_deltas = data.block_balance_deltas.lock().unwrap();
        let block_commits = build_block_commits(&block_balance_deltas, previous_commit);

        data.bench_mark
            .batch_put_balance_counts
            .store(all.len() as u64, std::sync::atomic::Ordering::Relaxed);

        Ok((all, block_commits, last_block_height))
    }

    fn collect_script_registry_updates(
        &self,
        data: &BatchBlockDataRef,
    ) -> Result<Vec<ScriptRegistryEntry>, String> {
        let mut entries = data.script_registry.lock().unwrap().clone();
        entries.par_sort_unstable_by(|left, right| left.script_hash.cmp(&right.script_hash));

        let mut deduped: Vec<ScriptRegistryEntry> = Vec::with_capacity(entries.len());
        for entry in entries {
            if let Some(last) = deduped.last()
                && last.script_hash == entry.script_hash
            {
                if last.script_pubkey.as_bytes() != entry.script_pubkey.as_bytes() {
                    let msg = format!(
                        "Conflicting script registry entries for script_hash {}",
                        entry.script_hash
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                continue;
            }

            deduped.push(entry);
        }

        Ok(deduped)
    }

    fn collect_undo_bundles(
        &self,
        data: &BatchBlockDataRef,
    ) -> Result<Vec<BlockUndoBundle>, String> {
        if !should_persist_any_undo_for_range(
            &data.block_range,
            self.latest_btc_height,
            self.undo_retention_blocks,
        ) {
            return Ok(Vec::new());
        }

        // Undo rollback deletes balance rows by (script_hash, block_height). The canonical
        // per-block balance result in block_balance_deltas should already cover all logical
        // balance mutations, but the undo path intentionally keeps a defensive superset of
        // touched script hashes.
        //
        // The extra cost here is bounded and piggybacks on work we already do for the hot
        // undo window: we are already iterating every vin/vout in order to collect created
        // and spent UTXOs for the same undo bundle, so the marginal overhead is only a few
        // HashSet insertions plus one small merge from block_balance_deltas.
        let touched_by_height: HashMap<u32, Vec<BtcScriptHash>> = {
            let block_balance_deltas = data.block_balance_deltas.lock().unwrap();
            block_balance_deltas
                .iter()
                .map(|block| {
                    (
                        block.block_height,
                        block
                            .entries
                            .iter()
                            .map(|entry| entry.script_hash)
                            .collect(),
                    )
                })
                .collect()
        };

        let blocks = data.blocks.lock().unwrap();
        let mut bundles = Vec::with_capacity(blocks.len());

        for block in blocks.iter() {
            if !should_persist_undo_for_block(
                block.height,
                self.latest_btc_height,
                self.undo_retention_blocks,
            ) {
                continue;
            }

            let mut created_utxos = Vec::new();
            let mut spent_utxos = Vec::new();
            let mut touched_script_hashes = HashSet::new();

            for tx in &block.txdata {
                for vout in &tx.vout {
                    touched_script_hashes.insert(vout.cache_tx_out.script_hash);
                    created_utxos.push(BlockUndoUtxoEntry {
                        outpoint: *vout.outpoint.as_ref(),
                        script_hash: vout.cache_tx_out.script_hash,
                        value: vout.cache_tx_out.value,
                    });
                }

                for vin in tx.vin.iter().chain(tx.displaced_vout.iter()) {
                    if !vin.need_flush {
                        continue;
                    }

                    let spent = vin.cache_tx_out.as_ref().ok_or_else(|| {
                        let msg = format!(
                            "Missing cached spent UTXO when collecting undo bundle: block_height={}, outpoint={}",
                            block.height, vin.outpoint
                        );
                        error!("{}", msg);
                        msg
                    })?;

                    touched_script_hashes.insert(spent.script_hash);

                    spent_utxos.push(BlockUndoUtxoEntry {
                        outpoint: *vin.outpoint.as_ref(),
                        script_hash: spent.script_hash,
                        value: spent.value,
                    });
                }
            }

            // Merge the canonical balance-entry view with the vin/vout-derived view and then
            // sort deterministically before persisting. The HashSet keeps the stored undo index
            // compact even when the same script hash is touched multiple times within one block.
            if let Some(entries) = touched_by_height.get(&block.height) {
                touched_script_hashes.extend(entries.iter().copied());
            }

            let mut touched_script_hashes: Vec<_> = touched_script_hashes.into_iter().collect();
            touched_script_hashes.sort_by_key(|left| left.to_byte_array());

            bundles.push(BlockUndoBundle {
                block_height: block.height,
                btc_block_hash: block.block_hash,
                created_utxos,
                spent_utxos,
                touched_script_hashes,
            });
        }

        Ok(bundles)
    }

    fn collect_utxo_updates(&self, data: &BatchBlockDataRef) -> Result<UtxoUpdateSet, String> {
        // First find all unspent UTXOs to add.
        let mut utxo_list = Vec::new();
        {
            let vout_utxos = data.vout_utxos.read().unwrap();
            utxo_list.reserve(vout_utxos.len());

            for (outpoint, generations) in vout_utxos.iter() {
                let mut unspent = generations.iter().filter(|generation| !generation.spend);
                if let Some(vout_utxo_info) = unspent.next() {
                    if unspent.next().is_some() {
                        return Err(format!(
                            "Multiple unspent generations remain for outpoint {} after BIP30 resolution",
                            outpoint
                        ));
                    }
                    utxo_list.push((outpoint.clone(), vout_utxo_info.item.clone()));
                }
            }
        }

        utxo_list.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
        // Then find all spent UTXOs to remove.
        let mut spent_utxo_list: Vec<_> = {
            use rayon::prelude::*;
            let blocks = data.blocks.lock().unwrap();

            blocks
                .par_iter()
                .flat_map(|block| {
                    block.txdata.par_iter().flat_map(|tx| {
                        tx.vin
                            .par_iter()
                            .chain(tx.displaced_vout.par_iter())
                            .filter(|vin| vin.need_flush)
                            .map(|vin| vin.outpoint.clone())
                    })
                })
                .collect()
        };

        spent_utxo_list.par_sort_unstable();

        data.bench_mark
            .batch_put_utxo_counts
            .store(utxo_list.len() as u64, std::sync::atomic::Ordering::Relaxed);
        data.bench_mark.batch_spent_utxo_counts.store(
            spent_utxo_list.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        Ok((utxo_list, spent_utxo_list))
    }

    fn apply_cache_updates(
        &self,
        data: &BatchBlockDataRef,
        new_utxos: &[(OutPointRef, UTXOEntryRef)],
        spent_utxos: &[OutPointRef],
    ) {
        // A BIP30 replacement removes and recreates the same outpoint in one
        // atomic batch, so cache ordering must mirror the durable DB ordering.
        for outpoint in spent_utxos {
            self.utxo_cache.spend(outpoint.as_ref());
        }

        for (outpoint, utxo) in new_utxos {
            self.utxo_cache.put(outpoint.clone(), utxo.clone());
        }

        for entry in data.balances.iter() {
            self.balance_cache
                .put(entry.key(), Arc::new(entry.value().clone()));
        }

        data.bench_mark.batch_update_balance_cache_counts.store(
            data.balances.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

#[cfg(test)]
mod block_commit_tests {
    use super::*;
    use crate::cache::{AddressBalanceCache, CacheStrategy, UTXOCache};
    use crate::config::BalanceHistoryConfig;
    use crate::db::{BalanceHistoryDB, BalanceHistoryDBMode};
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{ToBtcScriptHash, UTXOValue};

    #[test]
    fn test_bip30_duplicate_coinbase_identity_is_height_bound() {
        let first: Txid = "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599"
            .parse()
            .unwrap();
        let second: Txid = "e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468"
            .parse()
            .unwrap();
        let first_block: BlockHash =
            "00000000000a4d0a398161ffc163c503763b1f4360639393e0e4c8e300e0caec"
                .parse()
                .unwrap();
        let second_block: BlockHash =
            "00000000000743f190a18c5577a3c2d2a1f610ae9601ac046a38084ccb7cd721"
                .parse()
                .unwrap();

        assert!(is_bip30_duplicate_coinbase(91_842, &first_block, &first));
        assert!(is_bip30_duplicate_coinbase(91_880, &second_block, &second));
        assert!(!is_bip30_duplicate_coinbase(91_812, &first_block, &first));
        assert!(!is_bip30_duplicate_coinbase(91_880, &first_block, &second));
    }

    #[test]
    fn test_bip30_duplicate_loads_displaced_generation_from_durable_db() {
        let db = temp_db("bip30_durable_displacement");
        let old_script_hash = make_entry(1, 91_812, 50, 50).script_hash;
        let new_script_hash = make_entry(2, 91_842, 50, 50).script_hash;
        let duplicate_txid: Txid =
            "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599"
                .parse()
                .unwrap();
        let duplicate_block_hash: BlockHash =
            "00000000000a4d0a398161ffc163c503763b1f4360639393e0e4c8e300e0caec"
                .parse()
                .unwrap();
        let outpoint = Arc::new(OutPoint {
            txid: duplicate_txid,
            vout: 0,
        });
        db.put_utxo(outpoint.as_ref(), &old_script_hash, 50)
            .unwrap();

        let mut blocks = vec![PreloadBlock {
            height: 91_842,
            block_hash: duplicate_block_hash,
            txdata: vec![PreloadTx {
                txid: outpoint.txid,
                vin: Vec::new(),
                displaced_vout: Vec::new(),
                vout: vec![PreloadVOut {
                    outpoint: outpoint.clone(),
                    cache_tx_out: Arc::new(UTXOValue {
                        script_hash: new_script_hash,
                        value: 50,
                    }),
                }],
            }],
        }];
        let data = BatchBlockData::new();
        let duplicate_outpoints = BatchBlockPreloader::index_batch_vouts(&blocks, &data);
        assert!(duplicate_outpoints.is_empty());

        BatchBlockPreloader::resolve_duplicate_outpoint_generations(
            &db,
            &mut blocks,
            &data,
            &duplicate_outpoints,
        )
        .unwrap();

        let displaced = &blocks[0].txdata[0].displaced_vout;
        assert_eq!(displaced.len(), 1);
        assert_eq!(*displaced[0].outpoint, *outpoint);
        assert_eq!(
            displaced[0].cache_tx_out.as_ref().unwrap().script_hash,
            old_script_hash
        );
        assert!(displaced[0].need_flush);
    }

    fn temp_db(test_name: &str) -> BalanceHistoryDBRef {
        let mut config = BalanceHistoryConfig::default();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("{}_{}", test_name, nanos));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = Arc::new(config);
        Arc::new(BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap())
    }

    fn test_flusher(db: BalanceHistoryDBRef) -> BatchBlockFlusher {
        let config = Arc::new(BalanceHistoryConfig::default());
        let utxo_cache = Arc::new(UTXOCache::new(config.clone(), CacheStrategy::Normal));
        let balance_cache = Arc::new(AddressBalanceCache::new(config, CacheStrategy::Normal));
        BatchBlockFlusher::new(db, utxo_cache, balance_cache, 1, 0)
    }

    fn test_flusher_with_undo(
        db: BalanceHistoryDBRef,
        latest_btc_height: u32,
        undo_retention_blocks: u32,
    ) -> BatchBlockFlusher {
        let config = Arc::new(BalanceHistoryConfig::default());
        let utxo_cache = Arc::new(UTXOCache::new(config.clone(), CacheStrategy::Normal));
        let balance_cache = Arc::new(AddressBalanceCache::new(config, CacheStrategy::Normal));
        BatchBlockFlusher::new(
            db,
            utxo_cache,
            balance_cache,
            latest_btc_height,
            undo_retention_blocks,
        )
    }

    fn make_entry(seed: u8, block_height: u32, delta: i64, balance: u64) -> BalanceHistoryEntry {
        let script = ScriptBuf::from(vec![seed; 32]);
        BalanceHistoryEntry {
            script_hash: script.to_btc_script_hash(),
            block_height,
            delta,
            balance,
        }
    }

    fn make_script_registry_entry(seed: u8) -> ScriptRegistryEntry {
        let script_pubkey = ScriptBuf::from(vec![seed; 32]);
        ScriptRegistryEntry {
            script_hash: script_pubkey.to_btc_script_hash(),
            script_pubkey,
        }
    }

    #[test]
    fn test_balance_delta_root_is_stable() {
        let block_hash = BlockHash::from_slice(&[7u8; 32]).unwrap();
        let block = BlockBalanceDelta {
            block_height: 10,
            block_hash,
            entries: vec![make_entry(1, 10, 5, 5), make_entry(2, 10, -2, 8)],
        };

        assert_eq!(
            compute_balance_delta_root(&block),
            compute_balance_delta_root(&block)
        );
    }

    #[test]
    fn test_balance_delta_root_changes_when_entry_changes() {
        let block_hash = BlockHash::from_slice(&[7u8; 32]).unwrap();
        let original = BlockBalanceDelta {
            block_height: 10,
            block_hash,
            entries: vec![make_entry(1, 10, 5, 5), make_entry(2, 10, -2, 8)],
        };
        let changed = BlockBalanceDelta {
            block_height: 10,
            block_hash,
            entries: vec![make_entry(1, 10, 6, 6), make_entry(2, 10, -2, 8)],
        };

        assert_ne!(
            compute_balance_delta_root(&original),
            compute_balance_delta_root(&changed)
        );
    }

    #[test]
    fn test_build_block_commits_links_previous_hash() {
        let first = BlockBalanceDelta {
            block_height: 1,
            block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
            entries: vec![make_entry(1, 1, 10, 10)],
        };
        let second = BlockBalanceDelta {
            block_height: 2,
            block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
            entries: vec![make_entry(2, 2, 5, 15)],
        };

        let commits = build_block_commits(&[first.clone(), second.clone()], EMPTY_COMMIT_HASH);
        assert_eq!(commits.len(), 2);

        let first_root = compute_balance_delta_root(&first);
        let expected_first =
            compute_block_commit(1, &first.block_hash, &first_root, &EMPTY_COMMIT_HASH);
        assert_eq!(commits[0].block_commit, expected_first);

        let second_root = compute_balance_delta_root(&second);
        let expected_second = compute_block_commit(
            2,
            &second.block_hash,
            &second_root,
            &commits[0].block_commit,
        );
        assert_eq!(commits[1].block_commit, expected_second);
    }

    #[test]
    fn test_build_block_commits_depends_on_previous_commit() {
        let block = BlockBalanceDelta {
            block_height: 11,
            block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            entries: vec![make_entry(4, 11, 3, 33)],
        };

        let left = build_block_commits(std::slice::from_ref(&block), [1u8; 32]);
        let right = build_block_commits(&[block], [2u8; 32]);
        assert_ne!(left[0].block_commit, right[0].block_commit);
    }

    #[test]
    fn test_resolve_previous_commit_uses_empty_hash_for_genesis_boundary() {
        assert_eq!(
            resolve_previous_commit(None, None).unwrap(),
            EMPTY_COMMIT_HASH
        );
        assert_eq!(
            resolve_previous_commit(Some(0), None).unwrap(),
            EMPTY_COMMIT_HASH
        );
    }

    #[test]
    fn test_resolve_previous_commit_requires_existing_non_genesis_commit() {
        let error = resolve_previous_commit(Some(7), None).unwrap_err();
        assert!(error.contains("Missing previous block commit at height 7"));
    }

    #[test]
    fn test_collect_balance_updates_accepts_missing_height_zero_commit() {
        let db = temp_db("balance_history_genesis_prev_commit_boundary");
        let flusher = test_flusher(db);
        let mut data = BatchBlockData::new();

        let block = BlockBalanceDelta {
            block_height: 1,
            block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
            entries: vec![make_entry(1, 1, 10, 10)],
        };

        data.block_range = 1..2;
        *data.balance_history.lock().unwrap() = block.entries.clone();
        *data.block_balance_deltas.lock().unwrap() = vec![block.clone()];
        let data = Arc::new(data);

        let (balance_entries, block_commits, last_block_height) =
            flusher.collect_balance_updates(&data).unwrap();

        assert_eq!(last_block_height, 1);
        assert_eq!(balance_entries.len(), 1);
        assert_eq!(balance_entries[0].script_hash, block.entries[0].script_hash);
        assert_eq!(
            balance_entries[0].block_height,
            block.entries[0].block_height
        );
        assert_eq!(balance_entries[0].delta, block.entries[0].delta);
        assert_eq!(balance_entries[0].balance, block.entries[0].balance);
        assert_eq!(
            block_commits,
            build_block_commits(&[block], EMPTY_COMMIT_HASH)
        );
    }

    #[test]
    fn test_collect_script_registry_updates_deduplicates_same_script() {
        let db = temp_db("balance_history_script_registry_dedup");
        let flusher = test_flusher(db);
        let data = Arc::new(BatchBlockData::new());
        let entry = make_script_registry_entry(42);

        *data.script_registry.lock().unwrap() = vec![entry.clone(), entry.clone()];

        let entries = flusher.collect_script_registry_updates(&data).unwrap();
        assert_eq!(entries, vec![entry]);
    }

    #[test]
    fn test_collect_script_registry_updates_rejects_conflicting_script() {
        let db = temp_db("balance_history_script_registry_conflict");
        let flusher = test_flusher(db);
        let data = Arc::new(BatchBlockData::new());
        let entry = make_script_registry_entry(42);
        let conflicting = ScriptRegistryEntry {
            script_hash: entry.script_hash,
            script_pubkey: ScriptBuf::from(vec![43u8; 32]),
        };

        *data.script_registry.lock().unwrap() = vec![entry, conflicting];

        let error = flusher.collect_script_registry_updates(&data).unwrap_err();
        assert!(error.contains("Conflicting script registry entries"));
    }

    #[test]
    fn test_flusher_persists_script_registry_entries() {
        let db = temp_db("balance_history_script_registry_flush");
        let flusher = test_flusher(db.clone());
        let mut data = BatchBlockData::new();
        let entry = make_script_registry_entry(44);

        data.block_range = 1..2;
        *data.block_balance_deltas.lock().unwrap() = vec![BlockBalanceDelta {
            block_height: 1,
            block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
            entries: Vec::new(),
        }];
        *data.script_registry.lock().unwrap() = vec![entry.clone()];
        let data = Arc::new(data);

        flusher.flush(&data).unwrap();

        assert_eq!(
            db.get_script_registry_entry(&entry.script_hash).unwrap(),
            Some(entry.script_pubkey)
        );
    }

    #[test]
    fn test_collect_undo_bundles_keeps_vout_touches_without_balance_entries() {
        let db = temp_db("balance_history_undo_tracks_vout_script_hashes");
        let flusher = test_flusher_with_undo(db, 10, 16);
        let mut data = BatchBlockData::new();

        let script_hash = make_entry(9, 10, 0, 0).script_hash;
        let outpoint = Arc::new(OutPoint {
            txid: Txid::from_slice(&[7u8; 32]).unwrap(),
            vout: 0,
        });
        let utxo = Arc::new(UTXOValue {
            script_hash,
            value: 125_000_000,
        });

        data.block_range = 10..11;
        *data.blocks.lock().unwrap() = vec![PreloadBlock {
            height: 10,
            block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
            txdata: vec![PreloadTx {
                txid: Txid::from_slice(&[11u8; 32]).unwrap(),
                vin: Vec::new(),
                displaced_vout: Vec::new(),
                vout: vec![PreloadVOut {
                    outpoint,
                    cache_tx_out: utxo,
                }],
            }],
        }];
        *data.block_balance_deltas.lock().unwrap() = Vec::new();
        let data = Arc::new(data);

        let bundles = flusher.collect_undo_bundles(&data).unwrap();

        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].block_height, 10);
        assert_eq!(bundles[0].touched_script_hashes, vec![script_hash]);
    }

    #[test]
    fn test_collect_undo_bundles_deduplicates_and_sorts_touch_set() {
        let db = temp_db("balance_history_undo_dedup_and_sort_touch_set");
        let flusher = test_flusher_with_undo(db, 20, 32);
        let mut data = BatchBlockData::new();

        let script_a = make_entry(1, 20, 0, 0).script_hash;
        let script_b = make_entry(2, 20, 0, 0).script_hash;
        let script_c = make_entry(3, 20, 0, 0).script_hash;

        data.block_range = 20..21;
        *data.blocks.lock().unwrap() = vec![PreloadBlock {
            height: 20,
            block_hash: BlockHash::from_slice(&[20u8; 32]).unwrap(),
            txdata: vec![PreloadTx {
                txid: Txid::from_slice(&[21u8; 32]).unwrap(),
                vin: vec![
                    PreloadVIn {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[22u8; 32]).unwrap(),
                            vout: 0,
                        }),
                        cache_tx_out: Some(Arc::new(UTXOValue {
                            script_hash: script_c,
                            value: 30,
                        })),
                        need_flush: true,
                    },
                    PreloadVIn {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[23u8; 32]).unwrap(),
                            vout: 1,
                        }),
                        cache_tx_out: Some(Arc::new(UTXOValue {
                            script_hash: script_a,
                            value: 40,
                        })),
                        need_flush: true,
                    },
                ],
                displaced_vout: Vec::new(),
                vout: vec![
                    PreloadVOut {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[24u8; 32]).unwrap(),
                            vout: 0,
                        }),
                        cache_tx_out: Arc::new(UTXOValue {
                            script_hash: script_b,
                            value: 50,
                        }),
                    },
                    PreloadVOut {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[25u8; 32]).unwrap(),
                            vout: 1,
                        }),
                        cache_tx_out: Arc::new(UTXOValue {
                            script_hash: script_a,
                            value: 60,
                        }),
                    },
                ],
            }],
        }];
        *data.block_balance_deltas.lock().unwrap() = vec![BlockBalanceDelta {
            block_height: 20,
            block_hash: BlockHash::from_slice(&[20u8; 32]).unwrap(),
            entries: vec![
                BalanceHistoryEntry {
                    script_hash: script_c,
                    block_height: 20,
                    delta: -30,
                    balance: 70,
                },
                BalanceHistoryEntry {
                    script_hash: script_b,
                    block_height: 20,
                    delta: 50,
                    balance: 50,
                },
                BalanceHistoryEntry {
                    script_hash: script_a,
                    block_height: 20,
                    delta: -40,
                    balance: 60,
                },
            ],
        }];

        let data = Arc::new(data);
        let bundles = flusher.collect_undo_bundles(&data).unwrap();
        let mut expected = vec![script_a, script_b, script_c];
        expected.sort_by_key(|left| left.to_byte_array());

        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].block_height, 20);
        assert_eq!(bundles[0].created_utxos.len(), 2);
        assert_eq!(bundles[0].spent_utxos.len(), 2);
        assert_eq!(bundles[0].touched_script_hashes.len(), 3);
        assert_eq!(bundles[0].touched_script_hashes, expected);
    }

    #[test]
    fn test_collect_undo_bundles_match_processed_balance_deltas() {
        let db = temp_db("balance_history_undo_matches_processed_block_deltas");
        let flusher = test_flusher_with_undo(db, 30, 32);
        let processor = BatchBlockBalanceProcessor::new();
        let mut data = BatchBlockData::new();

        let script_a = make_entry(1, 29, 0, 0).script_hash;
        let script_b = make_entry(2, 29, 0, 0).script_hash;
        let script_c = make_entry(3, 29, 0, 0).script_hash;

        data.block_range = 30..31;
        data.balances.insert(
            script_a,
            BalanceHistoryData {
                block_height: 29,
                delta: 0,
                balance: 100,
            },
        );
        data.balances.insert(
            script_b,
            BalanceHistoryData {
                block_height: 29,
                delta: 0,
                balance: 0,
            },
        );
        data.balances.insert(
            script_c,
            BalanceHistoryData {
                block_height: 29,
                delta: 0,
                balance: 70,
            },
        );

        *data.blocks.lock().unwrap() = vec![PreloadBlock {
            height: 30,
            block_hash: BlockHash::from_slice(&[30u8; 32]).unwrap(),
            txdata: vec![PreloadTx {
                txid: Txid::from_slice(&[31u8; 32]).unwrap(),
                vin: vec![
                    PreloadVIn {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[32u8; 32]).unwrap(),
                            vout: 0,
                        }),
                        cache_tx_out: Some(Arc::new(UTXOValue {
                            script_hash: script_a,
                            value: 40,
                        })),
                        need_flush: true,
                    },
                    PreloadVIn {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[33u8; 32]).unwrap(),
                            vout: 1,
                        }),
                        cache_tx_out: Some(Arc::new(UTXOValue {
                            script_hash: script_c,
                            value: 30,
                        })),
                        need_flush: true,
                    },
                ],
                displaced_vout: Vec::new(),
                vout: vec![
                    PreloadVOut {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[34u8; 32]).unwrap(),
                            vout: 0,
                        }),
                        cache_tx_out: Arc::new(UTXOValue {
                            script_hash: script_a,
                            value: 60,
                        }),
                    },
                    PreloadVOut {
                        outpoint: Arc::new(OutPoint {
                            txid: Txid::from_slice(&[35u8; 32]).unwrap(),
                            vout: 1,
                        }),
                        cache_tx_out: Arc::new(UTXOValue {
                            script_hash: script_b,
                            value: 50,
                        }),
                    },
                ],
            }],
        }];

        let data = Arc::new(data);
        processor.process(&data).unwrap();

        let mut expected_entries = [
            BalanceHistoryEntry {
                script_hash: script_a,
                block_height: 30,
                delta: 20,
                balance: 120,
            },
            BalanceHistoryEntry {
                script_hash: script_b,
                block_height: 30,
                delta: 50,
                balance: 50,
            },
            BalanceHistoryEntry {
                script_hash: script_c,
                block_height: 30,
                delta: -30,
                balance: 40,
            },
        ];
        expected_entries.sort_by(|left, right| left.script_hash.cmp(&right.script_hash));

        let block_balance_deltas = data.block_balance_deltas.lock().unwrap();
        assert_eq!(block_balance_deltas.len(), 1);
        let entries = &block_balance_deltas[0].entries;
        assert_eq!(entries.len(), 3);
        for (actual, expected) in entries.iter().zip(expected_entries.iter()) {
            assert_eq!(actual.script_hash, expected.script_hash);
            assert_eq!(actual.block_height, expected.block_height);
            assert_eq!(actual.delta, expected.delta);
            assert_eq!(actual.balance, expected.balance);
        }
        drop(block_balance_deltas);

        let bundles = flusher.collect_undo_bundles(&data).unwrap();
        let mut expected_touched = vec![script_a, script_b, script_c];
        expected_touched.sort_by_key(|left| left.to_byte_array());
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].block_height, 30);
        assert_eq!(bundles[0].created_utxos.len(), 2);
        assert_eq!(bundles[0].spent_utxos.len(), 2);
        assert_eq!(bundles[0].touched_script_hashes, expected_touched);
    }

    #[test]
    fn test_should_persist_undo_only_inside_hot_window() {
        assert!(!should_persist_undo_for_block(100, 150, 0));
        assert!(!should_persist_undo_for_block(100, 150, 32));
        assert!(should_persist_undo_for_block(119, 150, 32));
        assert!(should_persist_undo_for_block(150, 150, 32));
    }

    #[test]
    fn test_should_persist_any_undo_for_range_only_when_batch_hits_hot_window() {
        assert!(!should_persist_any_undo_for_range(&(80..100), 150, 32));
        assert!(!should_persist_any_undo_for_range(&(151..160), 150, 32));
        assert!(should_persist_any_undo_for_range(&(100..121), 150, 32));
        assert!(should_persist_any_undo_for_range(&(140..151), 150, 32));
    }
}

// Use to keep the balance history result for a block
type BlockHistoryResult = HashMap<BtcScriptHash, BalanceHistoryEntry>;

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
        let mut block_history_results: Vec<
            Result<HashMap<BtcScriptHash, BalanceHistoryData>, String>,
        > = blocks
            .par_iter()
            .map(|block| {
                // Traverse all transactions to calculate balance delta
                let mut block_history: HashMap<BtcScriptHash, BalanceHistoryData> =
                    HashMap::with_capacity(block.txdata.len() * 16);
                for tx in block.txdata.iter() {
                    // Process vin (decrease balance)
                    for vin in tx.vin.iter().chain(tx.displaced_vout.iter()) {
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
            })
            .collect();

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

        let mut block_balance_deltas = data.block_balance_deltas.lock().unwrap();
        assert!(
            block_balance_deltas.is_empty(),
            "Block balance delta vector is not empty before flushing"
        );
        block_balance_deltas.reserve(blocks.len());

        for (block, ret) in blocks.iter().zip(block_history_results.into_iter()) {
            let block_history = ret?;
            let mut entries = Vec::with_capacity(block_history.len());
            for (script_hash, data) in block_history.into_iter() {
                let entry = BalanceHistoryEntry {
                    script_hash,
                    block_height: data.block_height,
                    delta: data.delta,
                    balance: data.balance,
                };
                entries.push(entry);
            }

            entries.par_sort_by(|a, b| a.script_hash.cmp(&b.script_hash));
            all.extend(entries.iter().cloned());
            block_balance_deltas.push(BlockBalanceDelta {
                block_height: block.height,
                block_hash: block.block_hash,
                entries,
            });
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

    pub fn process_blocks(
        &self,
        block_height_range: std::ops::Range<u32>,
        latest_btc_height: u32,
        undo_retention_blocks: u32,
    ) -> Result<(), String> {
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

        // Re-read the canonical hash after all expensive preprocessing. If the
        // BTC branch changed while this batch was being built, reject it before
        // any RocksDB or shared-cache mutation occurs.
        self.validate_canonical_batch_end(&data)?;

        data.bench_mark.process_balances_duration_micros.store(
            begin.elapsed().as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Flush all data to db and caches
        let flusher = BatchBlockFlusher::new(
            self.db.clone(),
            self.utxo_cache.clone(),
            self.balance_cache.clone(),
            latest_btc_height,
            undo_retention_blocks,
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

    fn validate_canonical_batch_end(&self, data: &BatchBlockDataRef) -> Result<(), String> {
        let blocks = data.blocks.lock().unwrap();
        let last = blocks
            .last()
            .ok_or_else(|| "Cannot validate canonical end of an empty block batch".to_string())?;
        let canonical_hash = self.btc_client.get_block_hash(last.height)?;
        if canonical_hash != last.block_hash {
            let msg = format!(
                "BTC canonical batch end changed before commit at height {}: loaded {}, canonical {}",
                last.height, last.block_hash, canonical_hash
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(())
    }
}
