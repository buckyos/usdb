use moka::sync::Cache;
use std::time::Duration;
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};


#[derive(Debug, Clone)]
pub struct CacheTxOut {
    pub value: u64,
    pub script_hash: ScriptHash,
}

pub struct UTXOCache {
    cache: Cache<OutPoint, CacheTxOut>, // (block_height, vout_index)
}

impl UTXOCache {
    pub fn new() -> Self {
        let cache = Cache::builder()
            .time_to_live(Duration::from_hours(24 * 1)) // 1 hour TTL
            .max_capacity(1024 * 1024 * 10 * 2) // Max 20 million entries
            .build();

        Self { cache }
    }

    pub fn insert(&self, outpoint: OutPoint, script_hash: ScriptHash, value: u64) {
        self.cache.insert(outpoint, CacheTxOut { value, script_hash });
    }

    pub fn get(&self, outpoint: &OutPoint) -> Option<CacheTxOut> {
        self.cache.get(outpoint)
    }

    pub fn get_and_remove(&self, outpoint: &OutPoint) -> Option<CacheTxOut> {
        self.cache.remove(outpoint)
    }

    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}

pub type UTXOCacheRef = std::sync::Arc<UTXOCache>;