use rust_rocksdb as rocksdb;

use bitcoincore_rpc::bitcoin::{ScriptBuf, ScriptHash};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use rocksdb::{
    ColumnFamily, ColumnFamilyDescriptor, DB, Direction, Error, IteratorMode, Options, WriteBatch, WriteOptions,
};
use std::path::{Path, PathBuf};

// Column family names
pub const BALANCE_HISTORY_CF: &str = "balance_history";
pub const META_CF: &str = "meta";

// Mete key names
pub const META_KEY_BTC_BLOCK_HEIGHT: &str = "btc_block_height";

pub const BALANCE_HISTORY_KEY_LEN: usize = ScriptHash::LEN + 4; // ScriptHash (20 bytes) + block_height (4 bytes)
pub struct BalanceHistoryEntry {
    pub script_hash: ScriptHash,
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub struct BalanceHistoryDB {
    file: PathBuf,
    db: DB,
}

impl BalanceHistoryDB {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let db_dir = data_dir.join("db");
        if !db_dir.exists() {
            std::fs::create_dir_all(&db_dir).map_err(|e| {
                let msg = format!(
                    "Could not create database directory at {}: {}",
                    db_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        let file = db_dir.join("balance_history");
        info!("Opening RocksDB at {}", file.display());

        // Default options
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let mut balance_history_cf_options = Options::default();
        balance_history_cf_options.set_level_compaction_dynamic_level_bytes(true);
        balance_history_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        balance_history_cf_options.create_if_missing(true);

        // Define column families
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(BALANCE_HISTORY_CF, balance_history_cf_options),
            ColumnFamilyDescriptor::new(META_CF, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&options, &file, cf_descriptors).map_err(|e| {
            let msg = format!("Failed to open RocksDB at {}: {}", file.display(), e);
            error!("{}", msg);
            msg
        })?;

        Ok(BalanceHistoryDB { file, db })
    }

    pub fn close(self) {
        drop(self.db);
        info!("Closed RocksDB at {}", self.file.display());
    }

    pub fn flush(&self) -> Result<(), String> {
        self.db.flush().map_err(|e| {
            let msg = format!("Failed to flush RocksDB at {}: {}", self.file.display(), e);
            error!("{}", msg);
            msg
        })
    }

    fn make_key(script_hash: ScriptHash, block_height: u32) -> Vec<u8> {
        // It is important that the block height is stored in big-endian format
        let height_bytes = block_height.to_be_bytes();

        let mut key = Vec::with_capacity(BALANCE_HISTORY_KEY_LEN);
        key.extend_from_slice(script_hash.as_ref());
        key.extend_from_slice(&height_bytes);
        key
    }

    fn parse_block_height_from_key(key: &[u8]) -> u32 {
        assert!(key.len() == BALANCE_HISTORY_KEY_LEN, "Invalid balance key length {}", key.len());
        let block_height_bytes = &key[ScriptHash::LEN..ScriptHash::LEN + 4];
        let block_height = u32::from_be_bytes(block_height_bytes.try_into().unwrap());

        block_height
    }

    // Parse balance and delta from value bytes
    fn parse_balance_from_value(value: &[u8]) -> (i64, u64) {
        assert!(value.len() == 16, "Invalid balance value length");

        // Value format: delta (i64) + balance (u64) in big-endian
        let delta_bytes = &value[0..8];
        let balance_bytes = &value[8..16];
        let delta = i64::from_be_bytes(delta_bytes.try_into().unwrap());
        let balance = u64::from_be_bytes(balance_bytes.try_into().unwrap());
        (delta, balance)
    }

    pub fn put_address_history(&self, entries: &[BalanceHistoryEntry]) -> Result<(), String> {
        let mut batch = WriteBatch::default();
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in entries {
            let key = Self::make_key(entry.script_hash, entry.block_height);

            // Value format: delta (i64) + balance (u64)
            let mut value = Vec::with_capacity(16);
            value.extend_from_slice(&entry.delta.to_be_bytes());
            value.extend_from_slice(&entry.balance.to_be_bytes());

            batch.put_cf(cf, key, value);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Get the latest balance entry for a given script_hash
    pub fn get_latest_balance(
        &self,
        script_hash: ScriptHash,
    ) -> Result<BalanceHistoryEntry, String> {
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        // Create an iterator in reverse mode starting from the maximum possible key for the script_hash
        let max_key = Self::make_key(script_hash, u32::MAX);
        let mut iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(&max_key, Direction::Reverse));

        if let Some(item) = iter.next() {
            let (found_key, found_val) = item.map_err(|e| {
                let msg = format!("Iterator error: {}", e);
                error!("{}", msg);
                msg
            })?;

            assert!(
                found_key.len() == BALANCE_HISTORY_KEY_LEN,
                "Invalid balance key length {}",
                found_key.len()
            );

            // Check if the ScriptHash matches
            if &found_key[0..ScriptHash::LEN] == script_hash.as_ref() as &[u8] {
                let block_height = Self::parse_block_height_from_key(&found_key);
                let (delta, balance) = Self::parse_balance_from_value(&found_val);
                let entry = BalanceHistoryEntry {
                    script_hash,
                    block_height,
                    delta,
                    balance,
                };

                return Ok(entry);
            }
        }

        // No records found for this script_hash
        let entry = BalanceHistoryEntry {
            script_hash,
            block_height: 0,
            delta: 0,
            balance: 0,
        };

        Ok(entry)
    }
    
    /// Get the balance entry for a given script_hash at or before the target block height
    pub fn get_balance_at_block_height(
        &self,
        script_hash: ScriptHash,
        target_height: u32,
    ) -> Result<BalanceHistoryEntry, String> {
        // Make the search key
        let search_key = Self::make_key(script_hash, target_height);

        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        // Create an iterator in reverse mode
        // IteratorMode::From(key, Reverse) positions at:
        // 1. If the key exists, it positions at that key.
        // 2. If the key does not exist, it positions at the first key less than the key (i.e., the previous height).
        let mut iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(&search_key, Direction::Reverse));

        if let Some(item) = iter.next() {
            let (found_key, found_val) = item.map_err(|e| {
                let msg = format!("Iterator error: {}", e);
                error!("{}", msg);
                msg
            })?;

            assert!(
                found_key.len() == BALANCE_HISTORY_KEY_LEN,
                "Invalid balance key length {}",
                found_key.len()
            );

            // Boundary check 1: Ensure the key length is correct and belongs to the same ScriptHash
            // impl AsRef<[u8]> for Hash
            if &found_key[0..ScriptHash::LEN] == script_hash.as_ref() as &[u8] {
                // Found a record for the same address.
                // Since it is Reverse and the starting point is target_height,
                // the found_height here must be <= target_height.
                let block_height = Self::parse_block_height_from_key(&found_key);
                let (delta, balance) = Self::parse_balance_from_value(&found_val);
                let entry = BalanceHistoryEntry {
                    script_hash,
                    block_height,
                    delta,
                    balance,
                };

                return Ok(entry);
            }
        }

        // If the iterator is empty, or has moved to the previous ScriptHash,
        // it means there are no records for this address before the target_height.
        // The default balance is 0. and block_height is 0.
        let entry = BalanceHistoryEntry {
            script_hash,
            block_height: 0,
            delta: 0,
            balance: 0,
        };

        Ok(entry)
    }

    /// Get balance records for a given script_hash within [range_begin, range_end)
    pub fn get_balance_in_range(
        &self,
        script_hash: ScriptHash,
        range_begin: u32,
        range_end: u32,
    ) -> Result<Vec<BalanceHistoryEntry>, String> {
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        // Create the start key
        let start_key = Self::make_key(script_hash, range_begin);

        // Create an iterator starting from start_key in forward direction
        let iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(&start_key, Direction::Forward));

        let mut results = Vec::new();
        for item in iter {
            let (key, value) = item.map_err(|e| {
                let msg = format!("Iterator error: {}", e);
                error!("{}", msg);
                msg
            })?;

            assert!(key.len() == BALANCE_HISTORY_KEY_LEN, "Invalid balance key length {}", key.len());

            // Boundary check 1: Check if the ScriptHash matches
            // If a different ScriptHash is encountered, it means the data for the current address has been fully traversed
            if &key[0..ScriptHash::LEN] != script_hash.as_ref() as &[u8] {
                break;
            }

            // Parse height
            let height = Self::parse_block_height_from_key(&key);

            // Boundary check 2: Check if height exceeds range (range_end is exclusive)
            if height >= range_end {
                break;
            }

            // Parse Value
            let (delta, balance) = Self::parse_balance_from_value(&value);

            let entry = BalanceHistoryEntry {
                script_hash,
                block_height: height,
                delta,
                balance,
            };

            results.push(entry);
        }

        Ok(results)
    }

    pub fn put_btc_block_height(&self, height: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(false);

        let height_bytes = height.to_be_bytes();
        self.db
            .put_cf_opt(cf, META_KEY_BTC_BLOCK_HEIGHT, &height_bytes, &ops)
            .map_err(|e| {
                let msg = format!("Failed to put BTC block height: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn get_btc_block_height(&self) -> Result<u32, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, META_KEY_BTC_BLOCK_HEIGHT) {
            Ok(Some(value)) => {
                if value.len() != 4 {
                    let msg = format!(
                        "Invalid BTC block height value length: {}",
                        value.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
                let height = u32::from_be_bytes((value.as_ref() as &[u8]).try_into().unwrap());
                Ok(height)
            }
            Ok(None) => {
                // Key does not exist, return height 0
                info!("BTC block height not found in DB, returning 0");
                Ok(0)
            }
            Err(e) => {
                let msg = format!("Failed to get BTC block height: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }
}

pub type BalanceHistoryDBRef = std::sync::Arc<BalanceHistoryDB>;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

    #[test]
    fn test_make_and_parse_key() {
        let script = ScriptBuf::from(vec![0u8; 32]);
        let script_hash = script.script_hash();
        let block_height = 123456;

        let key = BalanceHistoryDB::make_key(script_hash, block_height);
        let parsed_height = BalanceHistoryDB::parse_block_height_from_key(&key);

        assert_eq!(block_height, parsed_height);
    }

    #[test]
    fn test_parse_balance_from_value() {
        let delta: i64 = -500;
        let balance: u64 = 1500;

        let mut value = Vec::with_capacity(16);
        value.extend_from_slice(&delta.to_be_bytes());
        value.extend_from_slice(&balance.to_be_bytes());

        let (parsed_delta, parsed_balance) = BalanceHistoryDB::parse_balance_from_value(&value);

        assert_eq!(delta, parsed_delta);
        assert_eq!(balance, parsed_balance);
    }

    #[test]
    fn test_balance_history_db_put_and_get() {
        let temp_dir = std::env::temp_dir().join("balance_history_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db = BalanceHistoryDB::new(&temp_dir).unwrap();

        let script = ScriptBuf::from(vec![1u8; 32]);
        let script_hash = script.script_hash();

        let entries = vec![
            BalanceHistoryEntry {
                script_hash,
                block_height: 100,
                delta: 500,
                balance: 500,
            },
            BalanceHistoryEntry {
                script_hash,
                block_height: 200,
                delta: -200,
                balance: 300,
            },
            BalanceHistoryEntry {
                script_hash,
                block_height: 300,
                delta: 700,
                balance: 1000,
            },
            BalanceHistoryEntry {
                script_hash,
                block_height: 400,
                delta: -100,
                balance: 900,
            },
            BalanceHistoryEntry {
                script_hash,
                block_height: 401,
                delta: -100,
                balance: 800,
            },
        ];

        db.put_address_history(&entries).unwrap();

        // Get balance at height 50 (before any entries)
        let entry = db
            .get_balance_at_block_height(script_hash, 50)
            .unwrap();
        assert_eq!(entry.block_height, 0);
        assert_eq!(entry.delta, 0);
        assert_eq!(entry.balance, 0);

        // Get balance at height 100
        let entry = db
            .get_balance_at_block_height(script_hash, 100)
            .unwrap();
        assert_eq!(entry.block_height, 100);
        assert_eq!(entry.delta, 500);
        assert_eq!(entry.balance, 500);

        // Test get_balance_at_block_height
        let entry = db
            .get_balance_at_block_height(script_hash, 250)
            .unwrap();
        assert_eq!(entry.block_height, 200);
        assert_eq!(entry.delta, -200);
        assert_eq!(entry.balance, 300);

        // Test get_balance_in_range
        let range_entries = db
            .get_balance_in_range(script_hash, 150, 350)
            .unwrap();
        assert_eq!(range_entries.len(), 2);
        assert_eq!(range_entries[0].block_height, 200);
        assert_eq!(range_entries[1].block_height, 300);

        // Test get_balance_in_range with no entries
        let range_entries = db
            .get_balance_in_range(script_hash, 500, 600)
            .unwrap();
        assert_eq!(range_entries.len(), 0);

        // Test get_balance_in_range that hits the upper boundary
        let range_entries = db
            .get_balance_in_range(script_hash, 350, 401)
            .unwrap();
        assert_eq!(range_entries.len(), 1);
        assert_eq!(range_entries[0].block_height, 400);

        // Test get_balance_in_range that includes all entries
        let range_entries = db
            .get_balance_in_range(script_hash, 0, 1000)
            .unwrap();
        assert_eq!(range_entries.len(), 5);
        assert_eq!(range_entries[4].block_height, 401);

        db.close();

        // Test reopen the db
        let db = BalanceHistoryDB::new(&temp_dir).unwrap();
        let entry = db
            .get_balance_at_block_height(script_hash, 250)
            .unwrap();
        assert_eq!(entry.block_height, 200);
        assert_eq!(entry.delta, -200);
        assert_eq!(entry.balance, 300);

        // Test get BTC block height when not set
        let height = db.get_btc_block_height().unwrap();
        assert_eq!(height, 0);

        // Test put and get BTC block height
        db.put_btc_block_height(123456).unwrap();
        let height = db.get_btc_block_height().unwrap();
        assert_eq!(height, 123456);
    }
}
