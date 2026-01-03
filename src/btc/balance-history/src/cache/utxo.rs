use super::CacheStrategy;
use crate::config::BalanceHistoryConfig;
use crate::config::BalanceHistoryConfigRef;
use bitcoincore_rpc::bitcoin::OutPoint;
use bitcoincore_rpc::bitcoin::Txid;
use lru::LruCache;
use std::sync::Mutex;
use usdb_util::{OutPointRef, UTXOEntry, UTXOEntryRef};

// Cache item size estimate: OutPoint (32 + 4 bytes) + UTXOEntry (8 + 32 bytes) ~ 76 bytes
const CACHE_ITEM_SIZE: usize = std::mem::size_of::<OutPoint>() + std::mem::size_of::<UTXOEntry>();
const CACHE_OVERHEAD_BYTES: usize = 50; // Estimated overhead per entry in lru
const NORMAL_CACHE_MAX_ENTRIES: usize = 1024 * 16; // 16K entries for normal strategy

pub struct UTXOCache {
    cache: Mutex<LruCache<OutPointRef, UTXOEntryRef>>,
    strategy: Mutex<CacheStrategy>,
    config: BalanceHistoryConfigRef,
}

impl UTXOCache {
    pub fn new(config: BalanceHistoryConfigRef, strategy: CacheStrategy) -> Self {
        let max_capacity = Self::cap_by_strategy(strategy, &config);

        // let max_capacity: usize = 1024 * 1024 * 20; // For testing, limit to 80 million entries
        info!(
            "UTXOCache max capacity: {} entries, config max {} bytes, strategy: {:?}",
            max_capacity, config.sync.utxo_max_cache_bytes, strategy
        );

        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(max_capacity).unwrap(),
        ));

        Self {
            cache,
            strategy: Mutex::new(strategy),
            config,
        }
    }

    fn cap_by_strategy(strategy: CacheStrategy, config: &BalanceHistoryConfig) -> usize {
        match strategy {
            CacheStrategy::BestEffort => {
                config.sync.utxo_max_cache_bytes / (CACHE_ITEM_SIZE + CACHE_OVERHEAD_BYTES)
            }
            CacheStrategy::Normal => NORMAL_CACHE_MAX_ENTRIES,
        }
    }

    pub fn get_count(&self) -> u64 {
        self.cache.lock().unwrap().len() as u64
    }

    pub fn put(&self, outpoint: OutPointRef, utxo: UTXOEntryRef) {
        self.cache.lock().unwrap().put(outpoint, utxo);
    }

    pub fn get(&self, outpoint: &OutPoint) -> Option<UTXOEntryRef> {
        if let Some(cached) = self.cache.lock().unwrap().get(outpoint) {
            return Some(cached.clone());
        }

        None
    }

    pub fn spend(&self, outpoint: &OutPoint) -> Option<UTXOEntryRef> {
        if let Some(cached) = self.cache.lock().unwrap().pop(outpoint) {
            return Some(cached);
        }

        None
    }

    pub fn shrink(&self, target_count: usize) {
        let mut cache = self.cache.lock().unwrap();

        info!(
            "Shrinking UTXOCache to target count: {} -> {}",
            cache.len(),
            target_count
        );
        cache.resize(std::num::NonZeroUsize::new(target_count).unwrap());
    }

    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        info!("Clearing UTXOCache, current count: {}", cache.len());
        cache.clear();
    }

    pub fn update_strategy(&self, strategy: CacheStrategy) {
        let mut strategy_lock = self.strategy.lock().unwrap();
        if *strategy_lock != strategy {
            info!(
                "Updating UTXOCache strategy: {:?} -> {:?}",
                *strategy_lock, strategy
            );
            *strategy_lock = strategy;

            // Recreate cache with new strategy
            let max_capacity = Self::cap_by_strategy(strategy, &self.config);

            // Move existing entries to new cache
            let mut cache = self.cache.lock().unwrap();
            cache.resize(std::num::NonZeroUsize::new(max_capacity).unwrap());
        }
    }

    /*
    The two coinbase transactions both exist in two blocks.
    This problem was solved in BIP30 so this cannot happen again.
    So we quickly skip these two known bad coinbase transactions here.

    d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599 91812 91842
    e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468 91722 91880
     */
    pub fn check_black_list_coinbase_tx(&self, block_height: u64, txid: &Txid) -> bool {
        if block_height == 91812
            && txid.to_string()
                == "d5d27987d2a3dfc724e359870c6644b40e497bdc0589a033220fe15429d88599"
        {
            return true;
        }

        if block_height == 91722
            && txid.to_string()
                == "e3bf3d07d4b0375638d5f1db5255fe07ba2c4cb067cd81b84ee974b6585fb468"
        {
            return true;
        }

        false
    }
}

pub type UTXOCacheRef = std::sync::Arc<UTXOCache>;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use usdb_util::USDBScriptHash;

    #[test]
    fn test_utxo_cache_size() {
        let count = 1024 * 1024 * 20; // 20 million entries
        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        let available_memory = sys.available_memory();

        let mut cache = LruCache::new(std::num::NonZeroUsize::new(count + 1000).unwrap());

        // Append random entries up to count
        let value = UTXOEntry {
            script_hash: USDBScriptHash::from_slice(&[0u8; 32]).unwrap(),
            value: 1000,
        };
        let txid = Txid::from_slice(&[1u8; 32]).unwrap();
        for i in 0..count {
            let outpoint = OutPoint {
                txid: txid.clone(),
                vout: i as u32,
            };
            cache.put(outpoint, value.clone());
        }

        sys.refresh_memory();
        let available_memory_after = sys.available_memory();
        let used_memory = available_memory - available_memory_after;
        let item_memory = used_memory / (count as u64);
        println!("Used memory for cache: {} bytes", used_memory);
        println!("Estimated memory per item: {} bytes", item_memory);

        println!("Cache entry count: {}", cache.len());
    }
}
