use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry};
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash};
use moka::sync::Cache;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CacheTxOut {
    pub value: u64,
    pub script_hash: ScriptHash,
}

pub struct UTXOCache {
    cache: Cache<OutPoint, CacheTxOut>, // (block_height, vout_index)
    db: BalanceHistoryDBRef,
    write_cache: Mutex<HashMap<OutPoint, (ScriptHash, u64)>>, // To avoid duplicate writes
}

impl UTXOCache {
    pub fn new(db: BalanceHistoryDBRef) -> Self {
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60)) // 1 hour TTL
            .max_capacity(1024 * 1024 * 10 * 2) // Max 20 million entries
            .build();
        let write_cache = Mutex::new(HashMap::with_capacity(1024));

        Self {
            cache,
            db,
            write_cache,
        }
    }

    pub fn put(
        &self,
        outpoint: OutPoint,
        script_hash: ScriptHash,
        value: u64,
    ) -> Result<(), String> {
        self.cache
            .insert(outpoint, CacheTxOut { value, script_hash });

        // First add to write cache, will flush later use flush_write_cache method
        let mut write_cache = self.write_cache.lock().unwrap();
        let ret = write_cache.insert(outpoint, (script_hash, value));
        assert!(
            ret.is_none(),
            "Duplicate UTXO put in write cache: {}",
            outpoint
        );

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
        // First remove from write cache if exists
        {
            let mut write_cache = self.write_cache.lock().unwrap();
            write_cache.remove(outpoint);
        }

        // Then check in-memory cache
        if let Some(cached) = self.cache.get(outpoint) {
            self.cache.invalidate(outpoint);
            return Ok(Some(cached));
        }

        // At last check persistent storage
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

    // Flush write cache to DB
    pub fn flush_write_cache(&self) -> Result<(), String> {
        let mut write_cache = self.write_cache.lock().unwrap();
        if write_cache.is_empty() {
            return Ok(());
        }

        let utxos = write_cache
            .iter()
            .map(|(outpoint, (script_hash, amount))| (outpoint.clone(), *script_hash, *amount))
            .collect::<Vec<_>>();
        self.db.put_utxos(&utxos)?;
        write_cache.clear();

        Ok(())
    }
}

pub type UTXOCacheRef = std::sync::Arc<UTXOCache>;
