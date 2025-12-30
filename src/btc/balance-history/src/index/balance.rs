use crate::config::BalanceHistoryConfig;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
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
    db: BalanceHistoryDBRef,
}

impl AddressBalanceCache {
    pub fn new(db: BalanceHistoryDBRef, config: &BalanceHistoryConfig) -> Self {
        let max_capacity = config.sync.balance_cache_bytes / CACHE_ITEM_SIZE;

        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 24)) // 1 day TTL
            .max_capacity(max_capacity as u64) // Max entries based on config
            .build();

        Self { cache, db }
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
    ) -> Result<AddressBalanceItem, String> {
        if let Some(cached) = self.cache.get(&script_hash) {
            assert!(
                cached.block_height < block_height,
                "Inconsistent cache state for script_hash: {} {} < {}",
                script_hash,
                cached.block_height,
                block_height
            );

            return Ok(cached);
        }

        // Check persistent storage
        let entry = self
            .db
            .get_balance_at_block_height(script_hash, block_height)?;
        let item = AddressBalanceItem {
            block_height: entry.block_height,
            delta: entry.delta,
            balance: entry.balance,
        };

        Ok(item)
    }

    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}

pub type AddressBalanceCacheRef = std::sync::Arc<AddressBalanceCache>;

pub struct AddressBalanceSyncCache {
    address_balance_cache: AddressBalanceCacheRef,
    address_sync_cache: HashMap<USDBScriptHash, AddressBalanceItem>,
}

impl AddressBalanceSyncCache {
    pub fn new(address_balance_cache: AddressBalanceCacheRef) -> Self {
        let address_sync_cache = HashMap::new();

        Self {
            address_balance_cache,
            address_sync_cache,
        }
    }

    // Update sync cache on new block synced (should not update address_balance_cache yet)
    pub fn on_block_synced(&mut self, entries: &Vec<BalanceHistoryEntry>) {
        for entry in entries {
            let item = AddressBalanceItem {
                block_height: entry.block_height,
                delta: entry.delta,
                balance: entry.balance,
            };

            self.address_sync_cache.insert(entry.script_hash, item);
        }
    }

    pub fn flush_sync_cache(&mut self) {
        for (script_hash, item) in &self.address_sync_cache {
            self.address_balance_cache.put(*script_hash, item.clone());
        }
        self.address_sync_cache.clear();
    }

    pub fn get(
        &self,
        script_hash: USDBScriptHash,
        block_height: u32,
    ) -> Result<AddressBalanceItem, String> {
        if let Some(cached) = self.address_sync_cache.get(&script_hash) {
            assert!(
                cached.block_height < block_height,
                "Inconsistent sync cache state for script_hash: {} {} < {}",
                script_hash,
                cached.block_height,
                block_height
            );
            return Ok(cached.clone());
        }

        // Load from lru cache and db if necessary
        let entry = self.address_balance_cache.get(script_hash, block_height)?;
        Ok(entry)
    }
}
