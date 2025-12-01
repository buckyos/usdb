use rust_rocksdb as rocksdb;

use bitcoincore_rpc::bitcoin::{ScriptBuf, ScriptHash};
use rocksdb::{
    ColumnFamily, ColumnFamilyDescriptor, DB, Direction, Error, IteratorMode, Options, WriteBatch,
};
use std::path::{Path, PathBuf};

// Column family names
pub const BALANCE_HISTORY_CF: &str = "balance_history";
pub const META_CF: &str = "meta";

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

        let mut balance_history_cf_options = Options::default();
        balance_history_cf_options.set_level_compaction_dynamic_level_bytes(true);

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

    fn make_key(script_hash: ScriptHash, block_height: u32) -> Vec<u8> {
        // It is important that the block height is stored in big-endian format
        let height_bytes = block_height.to_be_bytes();

        let mut key = Vec::with_capacity(36);
        key.extend_from_slice(script_hash.as_ref());
        key.extend_from_slice(&height_bytes);
        key
    }

    fn parse_block_height_from_key(key: &[u8]) -> u32 {
        assert!(key.len() == 36, "Invalid balance key length");

        let block_height_bytes = &key[32..36];
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

        self.db.write(&batch).map_err(|e| {
            let msg = format!("Failed to write batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    /// Get the balance entry for a given script_hash at or before the target block height
    pub fn get_balance_entry_at_block_height(
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
                found_key.len() == 36,
                "Invalid balance key length {}",
                found_key.len()
            );

            // Boundary check 1: Ensure the key length is correct and belongs to the same ScriptHash
            // impl AsRef<[u8]> for Hash
            if &found_key[0..32] == script_hash.as_ref() as &[u8] {
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

            assert!(key.len() == 36, "Invalid balance key length {}", key.len());

            // Boundary check 1: Check if the ScriptHash matches
            // If a different ScriptHash is encountered, it means the data for the current address has been fully traversed
            if &key[0..32] != script_hash.as_ref() as &[u8] {
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
}
