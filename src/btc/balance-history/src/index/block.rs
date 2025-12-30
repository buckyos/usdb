use super::utxo::{CacheTxOut, UTXOCacheRef};
use crate::btc::BTCClientRef;
use crate::db::BalanceHistoryDBRef;
use bitcoincore_rpc::bitcoin::{Block, OutPoint, Txid};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use usdb_util::ToUSDBScriptHash;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BlockTxIndex {
    block_height: u64,
    tx_index: u32,
}

struct VOutUtxoInfo {
    item: CacheTxOut,
    spend: bool, // Whether this UTXO is spent in the batch
}

pub struct PreloadTx {
    pub txid: Txid,
    pub vin: Vec<(OutPoint, Option<CacheTxOut>)>,
    pub vout: Vec<(OutPoint, CacheTxOut)>,
}

pub struct PreloadBlock {
    pub height: u64,
    pub txdata: Vec<PreloadTx>,
}

struct VInPosition {
    tx_index: usize,
    vin_index: usize,
}

pub struct BatchBlockData {
    blocks: Arc<Mutex<Vec<PreloadBlock>>>,
    vout_uxtos: Arc<Mutex<HashMap<OutPoint, VOutUtxoInfo>>>,
}

impl BatchBlockData {
    pub fn new() -> Self {
        Self {
            blocks: Arc::new(Mutex::new(Vec::new())),
            vout_uxtos: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub type BatchBlockDataRef = Arc<BatchBlockData>;

pub struct BatchBlockPreloader {
    btc_client: BTCClientRef,
    db: BalanceHistoryDBRef,
    utxo_cache: UTXOCacheRef,
}

impl BatchBlockPreloader {
    pub fn new(
        btc_client: BTCClientRef,
        db: BalanceHistoryDBRef,
        utxo_cache: UTXOCacheRef,
    ) -> Self {
        Self {
            btc_client,
            db,
            utxo_cache,
        }
    }

    pub fn preload(
        &self,
        block_height_range: std::ops::Range<u64>,
    ) -> Result<BatchBlockDataRef, String> {
        let mut blocks = Vec::new();
        for height in block_height_range.clone() {
            let block = self.btc_client.get_block_by_height(height)?;

            blocks.push((height, block));
        }

        let data = BatchBlockData::new();
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

        Ok(data)
    }

    fn preprocess_block(
        &self,
        block_height: u64,
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
                    preload_tx.vin.push((outpoint.clone(), None));
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

                preload_tx.vout.push((outpoint, cache_tx_out));
            }

            preload_block.txdata.push(preload_tx);
        }

        // Append all vout UTXOs to UTXO cache
        let mut vout_utxo_map = data.vout_uxtos.lock().unwrap();
        for tx in &preload_block.txdata {
            for (outpoint, cache_tx_out) in &tx.vout {
                vout_utxo_map.insert(
                    outpoint.clone(),
                    VOutUtxoInfo {
                        item: cache_tx_out.clone(),
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
            for (vin_index, (outpoint, value)) in tx.vin.iter_mut().enumerate() {
                // First check if the UTXO is already in vout cache (i.e., created in the same batch)
                let mut vout_utxo_map = data.vout_uxtos.lock().unwrap();
                if let Some(vout_utxo_info) = vout_utxo_map.get_mut(outpoint) {
                    assert!(
                        !vout_utxo_info.spend,
                        "Double spend of UTXO in the same batch: {}",
                        outpoint
                    );
                    vout_utxo_info.spend = true;

                    value.replace(vout_utxo_info.item.clone());
                    continue;
                }

                // Then check if the UTXO is in utxo cache
                if let Some(cache_tx_out) = self.utxo_cache.get(outpoint)? {
                    value.replace(cache_tx_out);
                    continue;
                }

                // Append to load list for batch loading
                let pos = VInPosition {
                    tx_index,
                    vin_index,
                };

                outpoints_to_load.push(outpoint.clone());
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
                .1
                .replace(utxo);
        }

        Ok(())
    }

    fn fetch_utxos(&self, outpoints: &[OutPoint]) -> Result<Vec<CacheTxOut>, String> {
        // First try to get from db
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
}
