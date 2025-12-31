use crate::config::BalanceHistoryConfig;
use moka::sync::Cache;
use std::time::Duration;
use usdb_util::USDBScriptHash;

// Cache item size estimate: USDBScriptHash (32 bytes) + AddressBalanceItem (~20 bytes) ~ 52 bytes
const CACHE_ITEM_SIZE: usize = std::mem::size_of::<USDBScriptHash>() + std::mem::size_of::<AddressBalanceItem>();
const MOKA_OVERHEAD_BYTES: usize = 300; // Estimated overhead per entry in moka

#[derive(Debug, Clone)]
pub struct AddressBalanceItem {
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub struct AddressBalanceCache {
    cache: Cache<USDBScriptHash, AddressBalanceItem>, // script_hash -> balance
}

impl AddressBalanceCache {
    pub fn new(config: &BalanceHistoryConfig) -> Self {
        let max_capacity = config.sync.balance_cache_bytes / (CACHE_ITEM_SIZE + MOKA_OVERHEAD_BYTES);

        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 4)) // 4 hours TTL
            .max_capacity(max_capacity as u64) // Max entries based on config
            .initial_capacity(1024 * 1024 * 10)
            .build();

        Self { cache }
    }

    pub fn get_count(&self) -> u64 {
        self.cache.entry_count()
    }

    pub fn put(&self, script_hash: USDBScriptHash, entry: AddressBalanceItem) {
        let item = AddressBalanceItem {
            block_height: entry.block_height,
            delta: entry.delta,
            balance: entry.balance,
        };
        self.cache.insert(script_hash, item);
    }

    pub fn get(
        &self,
        script_hash: USDBScriptHash,
        block_height: u32,
    ) -> Option<AddressBalanceItem> {
        if let Some(cached) = self.cache.get(&script_hash) {
            assert!(
                cached.block_height <= block_height,
                "Inconsistent cache state for script_hash: {} {} < {}",
                script_hash,
                cached.block_height,
                block_height
            );

            return Some(cached);
        }

        None
    }

    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}

pub type AddressBalanceCacheRef = std::sync::Arc<AddressBalanceCache>;


#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

    #[test]
    fn test_address_balance_cache_size() {
        let count = 1024 * 1024 * 10; // 10 million entries
        let cache = Cache::builder()
                .max_capacity(count * 10)
                .build();

        for i in 0..count {
            let script_hash = USDBScriptHash::hash(&i.to_le_bytes());
            let item = AddressBalanceItem {
                block_height: i as u32,
                delta: i as i64,
                balance: i as u64,
            };
            cache.insert(script_hash, item);
        }

        // assert_eq!(cache.entry_count(), count as u64);

        println!("Cache entry count: {}", cache.entry_count());
        std::thread::sleep(std::time::Duration::from_secs(1000));
    }
}