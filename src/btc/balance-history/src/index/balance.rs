use crate::config::BalanceHistoryConfig;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use usdb_util::USDBScriptHash;
use crate::types::{BalanceHistoryData, BalanceHistoryDataRef};

// Cache item size estimate: USDBScriptHash (32 bytes) + BalanceHistoryData (~20 bytes) ~ 52 bytes
const CACHE_ITEM_SIZE: usize =
    std::mem::size_of::<USDBScriptHash>() + std::mem::size_of::<BalanceHistoryData>();
const CACHE_OVERHEAD_BYTES: usize = 50; // Estimated overhead per entry in lru


pub struct AddressBalanceCache {
    cache: Mutex<LruCache<USDBScriptHash, BalanceHistoryDataRef>>, // script_hash -> balance
}

impl AddressBalanceCache {
    pub fn new(config: &BalanceHistoryConfig) -> Self {
        let max_capacity =
            config.sync.balance_cache_bytes / (CACHE_ITEM_SIZE + CACHE_OVERHEAD_BYTES);
        let max_capacity: usize = 1024 * 1024 * 100; // For testing, limit to 100 million entries
        info!(
            "AddressBalanceCache max capacity: {} entries, total {} bytes",
            max_capacity, config.sync.balance_cache_bytes
        );

        let cache = Mutex::new(LruCache::new(NonZeroUsize::new(max_capacity).unwrap()));

        Self { cache }
    }

    pub fn get_count(&self) -> u64 {
        self.cache.lock().unwrap().len() as u64
    }

    pub fn put(&self, script_hash: USDBScriptHash, data: BalanceHistoryDataRef) {
        self.cache.lock().unwrap().put(script_hash, data);
    }

    pub fn get(
        &self,
        script_hash: USDBScriptHash,
        block_height: u32,
    ) -> Option<BalanceHistoryDataRef> {
        if let Some(cached) = self.cache.lock().unwrap().get(&script_hash) {
            assert!(
                cached.block_height <= block_height,
                "Inconsistent cache state for script_hash: {} {} < {}",
                script_hash,
                cached.block_height,
                block_height
            );

            return Some(cached.clone());
        }

        None
    }

    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
    }
}

pub type AddressBalanceCacheRef = std::sync::Arc<AddressBalanceCache>;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

    #[test]
    fn test_address_balance_cache_size() {
        let count = 1024 * 1024 * 20; // 20 million entries

        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        let available_memory = sys.available_memory();

        let mut cache = LruCache::new(NonZeroUsize::new(count + 1000).unwrap());

        for i in 0..count {
            let script_hash = USDBScriptHash::hash(&i.to_le_bytes());
            let item = BalanceHistoryData {
                block_height: i as u32,
                delta: i as i64,
                balance: i as u64,
            };
            cache.put(script_hash, item);
        }

        sys.refresh_memory();
        let available_memory_after = sys.available_memory();
        let used_memory = available_memory - available_memory_after;
        let item_memory = used_memory / (count as u64);
        println!("Used memory for cache: {} bytes", used_memory);
        println!("Estimated memory per item: {} bytes", item_memory);

        // assert_eq!(cache.entry_count(), count as u64);

        println!("Cache entry count: {}", cache.len());
    }
}
