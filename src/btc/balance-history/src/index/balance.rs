use crate::config::BalanceHistoryConfig;
use moka::sync::Cache;
use std::{collections::HashMap, time::Duration};
use usdb_util::USDBScriptHash;

// Cache item size estimate: USDBScriptHash (32 bytes) + AddressBalanceItem (~20 bytes) ~ 52 bytes
const CACHE_ITEM_SIZE: usize = 52;

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
        let max_capacity = config.sync.balance_cache_bytes / CACHE_ITEM_SIZE;

        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24)) // 1 day TTL
            .max_capacity(max_capacity as u64) // Max entries based on config
            .build();

        Self { cache }
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
