use bitcoincore_rpc::bitcoin::Txid;
use bitcoincore_rpc::bitcoin::{OutPoint};
use moka::sync::Cache;
use std::time::Duration;
use crate::config::BalanceHistoryConfig;
use usdb_util::USDBScriptHash;

// Cache item size estimate: OutPoint (32 + 4 bytes) + CacheTxOut (8 + 32 bytes) ~ 76 bytes
const CACHE_ITEM_SIZE: usize = std::mem::size_of::<OutPoint>() + std::mem::size_of::<CacheTxOut>();
const MOKA_OVERHEAD_BYTES: usize = 300; // Estimated overhead per entry in moka

#[derive(Debug, Clone)]
pub struct CacheTxOut {
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

pub struct UTXOCache {
    cache: Cache<OutPoint, CacheTxOut>, // (block_height, vout_index)
}

impl UTXOCache {
    pub fn new(config: &BalanceHistoryConfig) -> Self {
        let max_capacity = config.sync.utxo_cache_bytes / (CACHE_ITEM_SIZE + MOKA_OVERHEAD_BYTES);
        info!("UTXOCache max capacity: {} entries, total {} bytes", max_capacity, config.sync.utxo_cache_bytes);
        
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(60 * 60 * 4)) // 4 hours TTL
            .max_capacity(max_capacity as u64) // Max entries based on config
            .initial_capacity(1024 * 1024 * 16)
            .build();

        Self {
            cache,
        }
    }

    pub fn get_count(&self) -> u64 {
        self.cache.entry_count()
    }

    pub fn put(
        &self,
        outpoint: OutPoint,
        script_hash: USDBScriptHash,
        value: u64,
    ) {
        self.cache
            .insert(outpoint, CacheTxOut { value, script_hash });
    }

    pub fn get(&self, outpoint: &OutPoint) -> Option<CacheTxOut> {
        if let Some(cached) = self.cache.get(outpoint) {
            return Some(cached)
        }

        None
    }

    pub fn spend(&self, outpoint: &OutPoint) -> Option<CacheTxOut> {
        if let Some(cached) = self.cache.remove(outpoint) {
            return Some(cached);
        }

        None
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
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use super::*;

    #[test]
    fn test_utxo_cache_size() {
        let count = 1024 * 1024 * 10; // 10 million entries
        let cache = Cache::builder()
                .max_capacity(count * 10)
                .build();
        

        // Append random entries up to count
        let value = CacheTxOut {
            script_hash: USDBScriptHash::from_slice(&[0u8; 32]).unwrap(),
            value: 1000,
        };
        let txid = Txid::from_slice(&[1u8; 32]).unwrap();
        for i in 0..count {
            let outpoint = OutPoint {
                txid: txid.clone(),
                vout: i as u32,
            };
            cache.insert(outpoint, value.clone());
        }

        println!("Cache entry count: {}", cache.entry_count());
        std::thread::sleep(std::time::Duration::from_secs(1000));
    }
}