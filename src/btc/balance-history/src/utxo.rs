use crate::db::BalanceHistoryDBRef;
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};
use moka::sync::Cache;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CacheTxOut {
    pub value: u64,
    pub script_hash: ScriptHash,
}

pub struct UTXOCache {
    cache: Cache<OutPoint, CacheTxOut>, // (block_height, vout_index)
    db: BalanceHistoryDBRef,
}

impl UTXOCache {
    pub fn new(db: BalanceHistoryDBRef) -> Self {
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60)) // 1 hour TTL
            .max_capacity(1024 * 1024 * 10 * 2) // Max 20 million entries
            .build();

        Self { cache, db }
    }

    pub fn put(&self, outpoint: OutPoint, script_hash: ScriptHash, value: u64) -> Result<(), String> {
        self.cache
            .insert(outpoint, CacheTxOut { value, script_hash });

        self.db.put_utxo(&outpoint, &script_hash, value)?;

        Ok(())
    }

    pub fn get(&self, outpoint: &OutPoint) -> Result<Option<CacheTxOut>, String> {
        // First check in-memory cache
        if let Some(cached) = self.cache.get(outpoint) {
            return Ok(Some(cached));
        }

        // Next check persistent storage
        if let Some(entry) = self.db.get_utxo(outpoint)? {
            let cache_tx_out = CacheTxOut {
                value: entry.amount,
                script_hash: entry.script_hash,
            };
            
            Ok(Some(cache_tx_out))
        } else {
            Ok(None)
        }
    }

    pub fn spend(&self, outpoint: &OutPoint) -> Result<Option<CacheTxOut>, String> {
        // First check in-memory cache
        if let Some(cached) = self.cache.get(outpoint) {
            self.cache.invalidate(outpoint);
            return Ok(Some(cached));
        }

        // Next check persistent storage
        if let Some(entry) = self.db.spend_utxo(outpoint)? {
            let cache_tx_out = CacheTxOut {
                value: entry.amount,  
                script_hash: entry.script_hash,
            };
            
            Ok(Some(cache_tx_out))
        } else {
            Ok(None)
        }
    }

    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}

pub type UTXOCacheRef = std::sync::Arc<UTXOCache>;
