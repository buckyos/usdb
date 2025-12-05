use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};
use moka::sync::Cache;
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Clone)]
pub struct AddressBalanceItem {
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub struct AddressBalanceCache {
    cache: Cache<ScriptHash, AddressBalanceItem>, // script_hash -> balance
    db: BalanceHistoryDBRef,
}

impl AddressBalanceCache {
    pub fn new(db: BalanceHistoryDBRef) -> Self {
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60)) // 1 hour TTL
            .max_capacity(1024 * 1024 * 10 * 1) // Max 10 million entries
            .build();

        Self { cache, db }
    }

    pub fn put(&self, script_hash: ScriptHash, entry: AddressBalanceItem) {
        let item  = AddressBalanceItem {
            block_height: entry.block_height,
            delta: entry.delta,
            balance: entry.balance,
        };
        self.cache.insert(script_hash, item);
    }

    pub fn get(&self, script_hash: ScriptHash) -> Result<AddressBalanceItem, String> {
        if let Some(cached) = self.cache.get(&script_hash) {
            return Ok(cached);
        }

        // Check persistent storage
        let entry = self.db.get_latest_balance(script_hash)?;
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
    address_sync_cache: HashMap<ScriptHash, AddressBalanceItem>, 
}

impl AddressBalanceSyncCache {
    pub fn new(address_balance_cache: AddressBalanceCacheRef) -> Self {
        let address_sync_cache = HashMap::new();

        Self {
            address_balance_cache,
            address_sync_cache,
        }
    }

    pub fn on_block_synced(&mut self, entries: &Vec<BalanceHistoryEntry>) {
        for entry in entries {
            let item = AddressBalanceItem {
                block_height: entry.block_height,
                delta: entry.delta,
                balance: entry.balance,
            };

            self.address_balance_cache.put(
                entry.script_hash,
                item.clone(),
            );

            self.address_sync_cache.insert(entry.script_hash, item);
        }
    }

    pub fn get(&self, script_hash: ScriptHash) -> Result<AddressBalanceItem, String> {
        if let Some(cached) = self.address_sync_cache.get(&script_hash) {
            return Ok(cached.clone());
        }

        // Load from lru cache and db if necessary
        let entry = self.address_balance_cache.get(script_hash)?;
        Ok(entry)
    }
}