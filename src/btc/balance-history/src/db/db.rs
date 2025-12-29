use super::helper::get_approx_cf_key_count;
use crate::config::BalanceHistoryConfigRef;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptHash, Txid};
use rocksdb::{
    ColumnFamilyDescriptor, DB, Direction, IteratorMode, Options, WriteBatch, WriteOptions,
};
use rust_rocksdb::{self as rocksdb};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// Column family names
pub const BALANCE_HISTORY_CF: &str = "balance_history";
pub const META_CF: &str = "meta";
pub const UTXO_CF: &str = "utxo";
// pub const BLOCKS_CF: &str = "blocks";

// Mete key names
pub const META_KEY_BTC_BLOCK_HEIGHT: &str = "btc_block_height";

pub const BALANCE_HISTORY_KEY_LEN: usize = ScriptHash::LEN + 4; // ScriptHash (20 bytes) + block_height (4 bytes)
pub const UTXO_KEY_LEN: usize = Txid::LEN + 4; // OutPoint: txid (32 bytes) + vout (4 bytes)
// pub const BLOCKS_KEY_LEN: usize = BlockHash::LEN; // BlockHash (32 bytes) + block_height (4 bytes)

pub struct BalanceHistoryEntry {
    pub script_hash: ScriptHash,
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub struct UTXOEntry {
    pub script_hash: ScriptHash,
    pub amount: u64,
}

#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub block_file_index: u32,     // which blk file
    pub block_file_offset: u64,    // offset in the blk file
    pub block_record_index: usize, // index in the block record cache
}

pub struct BalanceHistoryDB {
    config: BalanceHistoryConfigRef,
    file: PathBuf,
    db: DB,
}

impl BalanceHistoryDB {
    pub fn new(data_dir: &Path, config: BalanceHistoryConfigRef) -> Result<Self, String> {
        let db_dir = Self::get_db_dir(data_dir);
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

        let mut utxo_cf_options = Options::default();
        utxo_cf_options.set_level_compaction_dynamic_level_bytes(true);
        utxo_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        utxo_cf_options.create_if_missing(true);

        // Define column families
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(BALANCE_HISTORY_CF, balance_history_cf_options),
            ColumnFamilyDescriptor::new(META_CF, Options::default()),
            ColumnFamilyDescriptor::new(UTXO_CF, utxo_cf_options),
        ];

        let db = DB::open_cf_descriptors(&options, &file, cf_descriptors).map_err(|e| {
            let msg = format!("Failed to open RocksDB at {}: {}", file.display(), e);
            error!("{}", msg);
            msg
        })?;

        Ok(BalanceHistoryDB { file, db, config })
    }

    pub fn close(self) {
        drop(self.db);
        info!("Closed RocksDB at {}", self.file.display());
    }

    pub fn get_db_dir(data_dir: &Path) -> PathBuf {
        let db_dir = data_dir.join("db");
        db_dir
    }

    pub fn flush_all(&self) -> Result<(), String> {
        self.db.flush().map_err(|e| {
            let msg = format!("Failed to flush RocksDB at {}: {}", self.file.display(), e);
            error!("{}", msg);
            msg
        })
    }

    pub fn flush_balance_history(&self) -> Result<(), String> {
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        self.db.flush_cf(cf).map_err(|e| {
            let msg = format!(
                "Failed to flush column family {}: {}",
                BALANCE_HISTORY_CF, e
            );
            error!("{}", msg);
            msg
        })
    }

    fn make_balance_history_key(script_hash: ScriptHash, block_height: u32) -> Vec<u8> {
        // It is important that the block height is stored in big-endian format
        let height_bytes = block_height.to_be_bytes();

        let mut key = Vec::with_capacity(BALANCE_HISTORY_KEY_LEN);
        key.extend_from_slice(script_hash.as_ref());
        key.extend_from_slice(&height_bytes);
        key
    }

    fn parse_block_height_from_key(key: &[u8]) -> u32 {
        assert!(
            key.len() == BALANCE_HISTORY_KEY_LEN,
            "Invalid balance key length {}",
            key.len()
        );
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

    fn make_utxo_key(outpoint: &bitcoincore_rpc::bitcoin::OutPoint) -> Vec<u8> {
        let mut key = Vec::with_capacity(UTXO_KEY_LEN);
        key.extend_from_slice(outpoint.txid.as_ref());
        key.extend_from_slice(&outpoint.vout.to_be_bytes());
        key
    }

    fn parse_utxo_from_value(value: &[u8]) -> UTXOEntry {
        assert!(
            value.len() == ScriptHash::LEN + 8,
            "Invalid UTXO value length"
        );

        let script_hash_bytes = &value[0..ScriptHash::LEN];
        let amount_bytes = &value[ScriptHash::LEN..ScriptHash::LEN + 8];

        let script_hash = ScriptHash::from_slice(script_hash_bytes).unwrap();
        let amount = u64::from_be_bytes(amount_bytes.try_into().unwrap());

        UTXOEntry {
            script_hash,
            amount,
        }
    }

    fn parse_block_from_value(record_index: usize, value: &[u8]) -> BlockEntry {
        assert!(value.len() == 12, "Invalid Block value length");

        let block_file_index_bytes = &value[0..4];
        let block_file_offset_bytes = &value[4..12];

        let block_file_index = u32::from_be_bytes(block_file_index_bytes.try_into().unwrap());
        let block_file_offset = u64::from_be_bytes(block_file_offset_bytes.try_into().unwrap());

        BlockEntry {
            block_file_index,
            block_file_offset,
            block_record_index: record_index,
        }
    }

    pub fn put_address_history_sync(
        &self,
        entries_list: &[Vec<BalanceHistoryEntry>],
        block_height: u32,
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entries in entries_list {
            for entry in entries {
                let key = Self::make_balance_history_key(entry.script_hash, entry.block_height);

                // Value format: delta (i64) + balance (u64)
                let mut value = Vec::with_capacity(16);
                value.extend_from_slice(&entry.delta.to_be_bytes());
                value.extend_from_slice(&entry.balance.to_be_bytes());

                batch.put_cf(cf, key, value);
            }
        }

        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        let height_bytes = block_height.to_be_bytes();
        batch.put_cf(cf, META_KEY_BTC_BLOCK_HEIGHT, &height_bytes);

        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);
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
        let max_key = Self::make_balance_history_key(script_hash, u32::MAX);
        let mut iter = self
            .db
            .iterator_cf(cf, IteratorMode::From(&max_key, Direction::Reverse));

        if let Some(item) = iter.next() {
            let (found_key, found_val) = item.map_err(|e| {
                let msg = format!("Iterator error: {} {}", script_hash, e);
                error!("{}", msg);
                msg
            })?;

            assert!(
                found_key.len() == BALANCE_HISTORY_KEY_LEN,
                "Invalid balance key length {} {}",
                script_hash,
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
        let search_key = Self::make_balance_history_key(script_hash, target_height);

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
        let start_key = Self::make_balance_history_key(script_hash, range_begin);

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

            assert!(
                key.len() == BALANCE_HISTORY_KEY_LEN,
                "Invalid balance key length {}",
                key.len()
            );

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
                    let msg = format!("Invalid BTC block height value length: {}", value.len());
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

    pub fn put_utxo(
        &self,
        outpoint: &OutPoint,
        script_hash: &ScriptHash,
        amount: u64,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(false);

        // Value format: ScriptHash (20 bytes) + amount (u64)
        let mut value = Vec::with_capacity(ScriptHash::LEN + 8);
        value.extend_from_slice(script_hash.as_ref() as &[u8]);
        value.extend_from_slice(&amount.to_be_bytes());

        let key = Self::make_utxo_key(outpoint);

        self.db.put_cf_opt(cf, key, value, &ops).map_err(|e| {
            let msg = format!("Failed to put UTXO: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn update_utxos_sync(
        &self,
        new_utxos: &HashMap<OutPoint, (ScriptHash, u64)>,
        remove_utxos: &HashSet<OutPoint>,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = WriteBatch::default();

        for (outpoint, (script_hash, amount)) in new_utxos {
            // Value format: ScriptHash (20 bytes) + amount (u64)
            let mut value = Vec::with_capacity(ScriptHash::LEN + 8);
            value.extend_from_slice(script_hash.as_ref() as &[u8]);
            value.extend_from_slice(&amount.to_be_bytes());

            let key = Self::make_utxo_key(outpoint);

            batch.put_cf(cf, key, value);
        }

        for outpoint in remove_utxos {
            let key = Self::make_utxo_key(outpoint);
            batch.delete_cf(cf, key);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write UTXO batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Just get the UTXO without removing it
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXOEntry>, String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let key = Self::make_utxo_key(outpoint);

        match self.db.get_cf(cf, key) {
            Ok(Some(value)) => {
                let utxo_entry = Self::parse_utxo_from_value(&value);
                Ok(Some(utxo_entry))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!("Failed to get UTXO: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    // Remove and return the UTXO entry for the given outpoint
    pub fn spend_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXOEntry>, String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(false);

        let key = Self::make_utxo_key(outpoint);

        match self.db.get_cf(cf, &key) {
            Ok(Some(value)) => {
                let utxo_entry = Self::parse_utxo_from_value(&value);

                // Remove the UTXO
                self.db.delete_cf_opt(cf, &key, &ops).map_err(|e| {
                    let msg = format!("Failed to delete UTXO: {}", e);
                    error!("{}", msg);
                    msg
                })?;

                Ok(Some(utxo_entry))
            }
            Ok(None) => {
                let msg = format!("UTXO not found for outpoint: {}", outpoint);
                warn!("{}", msg);
                Ok(None)
            }
            Err(e) => {
                let msg = format!("Failed to get UTXO: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub fn spend_utxos(
        &self,
        outpoints: &Vec<OutPoint>,
    ) -> Result<Vec<(OutPoint, UTXOEntry)>, String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(false);

        let mut spent_utxos = Vec::new();

        for outpoint in outpoints {
            let key = Self::make_utxo_key(outpoint);

            match self.db.get_cf(cf, &key) {
                Ok(Some(value)) => {
                    let utxo_entry = Self::parse_utxo_from_value(&value);

                    // Remove the UTXO
                    self.db.delete_cf_opt(cf, &key, &ops).map_err(|e| {
                        let msg = format!("Failed to delete UTXO: {}", e);
                        error!("{}", msg);
                        msg
                    })?;

                    spent_utxos.push((outpoint.clone(), utxo_entry));
                }
                Ok(None) => {
                    let msg = format!("UTXO not found for outpoint: {}", outpoint);
                    warn!("{}", msg);
                }
                Err(e) => {
                    let msg = format!("Failed to get UTXO: {}", e);
                    error!("{}", msg);
                    return Err(msg);
                }
            }
        }

        Ok(spent_utxos)
    }

    pub fn get_history_balance_count(&self) -> Result<u64, String> {
        get_approx_cf_key_count(&self.db, BALANCE_HISTORY_CF)
    }

    fn generate_snapshot_sharded(
        &self,
        target_block_height: u32,
        shard_index: u8,
        batch_size: usize,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut seek_key = vec![shard_index];
        seek_key.resize(ScriptHash::LEN, 0xFF); // max ScriptHash
        seek_key.extend_from_slice(&[0xFF; 4]); // max block height

        let mut iter = self
            .db
            .iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));

        let mut current_script_hash: Option<ScriptHash> = None;
        let mut current_founded = false;
        let mut snapshot = Vec::with_capacity(batch_size);
        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != BALANCE_HISTORY_KEY_LEN {
                continue;
            }

            if key[0] != shard_index {
                // Moved to a new shard
                break;
            }

            let script_hash = ScriptHash::from_slice(&key[0..ScriptHash::LEN]).unwrap();
            let height = u32::from_be_bytes(
                key[ScriptHash::LEN..ScriptHash::LEN + 4]
                    .try_into()
                    .unwrap(),
            );

            if current_script_hash.is_none() {
                current_script_hash = Some(script_hash);
            } else if current_script_hash.as_ref().unwrap() != &script_hash {
                // Moved to a new script_hash
                current_script_hash = Some(script_hash);
                current_founded = false;
            }

            if !current_founded && height <= target_block_height {
                let (delta, balance) = Self::parse_balance_from_value(&value);
                if balance > 0 {
                    let entry = BalanceHistoryEntry {
                        script_hash,
                        block_height: height,
                        delta,
                        balance,
                    };
                    snapshot.push(entry);

                    if snapshot.len() >= batch_size {
                        // Flush snapshot batch
                        cb.on_snapshot_entries(&snapshot)?;
                        snapshot.clear();
                    }
                } else {
                    // Zero balance at this height, do not include in snapshot
                }

                current_founded = true;
            }
        }

        // Flush remaining snapshot entries
        if !snapshot.is_empty() {
            cb.on_snapshot_entries(&snapshot)?;
        }

        Ok(())
    }

    pub fn generate_snapshot_parallel(
        &self,
        target_block_height: u32,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String>
    {
        use rayon::prelude::*;

        const SHARD_COUNT: u8 = 255;
        const BATCH_SIZE: usize = 1024 * 64;

        (0u8..=SHARD_COUNT).into_par_iter().try_for_each(|shard_index| {
            self.generate_snapshot_sharded(
                target_block_height,
                shard_index,
                BATCH_SIZE,
                cb.clone(),
            )
        })?;

        Ok(())
    }
    
    pub fn generate_snapshot<F>(
        &self,
        target_block_height: u32,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(&[BalanceHistoryEntry]) -> Result<(), String>,
    {
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        const BATCH_SIZE: usize = 1024 * 64;
        let mut iter = self.db.iterator_cf(&cf, IteratorMode::End);

        let mut current_script_hash: Option<ScriptHash> = None;
        let mut current_founded = false;
        let mut snapshot = Vec::with_capacity(BATCH_SIZE);
        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != BALANCE_HISTORY_KEY_LEN {
                continue;
            }

            let script_hash = ScriptHash::from_slice(&key[0..ScriptHash::LEN]).unwrap();
            let height = u32::from_be_bytes(
                key[ScriptHash::LEN..ScriptHash::LEN + 4]
                    .try_into()
                    .unwrap(),
            );

            if current_script_hash.is_none() {
                current_script_hash = Some(script_hash);
            } else if current_script_hash.as_ref().unwrap() != &script_hash {
                // Moved to a new script_hash
                current_script_hash = Some(script_hash);
                current_founded = false;
            }

            if !current_founded && height <= target_block_height {
                let (delta, balance) = Self::parse_balance_from_value(&value);
                if balance > 0 {
                    let entry = BalanceHistoryEntry {
                        script_hash,
                        block_height: height,
                        delta,
                        balance,
                    };
                    snapshot.push(entry);

                    if snapshot.len() >= BATCH_SIZE {
                        // Flush snapshot batch
                        callback(&snapshot)?;
                        snapshot.clear();
                    }
                } else {
                    // Zero balance at this height, do not include in snapshot
                }

                current_founded = true;
            }
        }
        // Flush remaining snapshot entries
        if !snapshot.is_empty() {
            callback(&snapshot)?;
        }

        Ok(())
    }
    /*
    pub fn put_blocks(
        &self,
        blocks: &Vec<(BlockHash, BlockEntry)>,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(BLOCKS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCKS_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = WriteBatch::default();

        for (block_hash, block_entry) in blocks {
            // Value format: block_file_index (u32) + block_file_offset (u64)
            let mut value = Vec::with_capacity(12);
            value.extend_from_slice(&block_entry.block_file_index.to_be_bytes());
            value.extend_from_slice(&block_entry.block_file_offset.to_be_bytes());

            batch.put_cf(cf, block_hash.as_ref() as &[u8], value);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write Blocks batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_block(&self, block_hash: &BlockHash) -> Result<Option<BlockEntry>, String> {
        let cf = self.db.cf_handle(BLOCKS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCKS_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, block_hash.as_ref() as &[u8]) {
            Ok(Some(value)) => {
                let block_entry = Self::parse_block_from_value(&value);
                Ok(Some(block_entry))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!("Failed to get Block: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }
    */
}

pub type BalanceHistoryDBRef = std::sync::Arc<BalanceHistoryDB>;

pub trait SnapshotCallback: Send + Sync {
    fn on_snapshot_entries(&self, entries: &[BalanceHistoryEntry]) -> Result<(), String>;
}

pub type SnapshotCallbackRef = std::sync::Arc<Box<dyn SnapshotCallback>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

    #[test]
    fn test_make_and_parse_key() {
        let script = ScriptBuf::from(vec![0u8; 32]);
        let script_hash = script.script_hash();
        let block_height = 123456;

        let key = BalanceHistoryDB::make_balance_history_key(script_hash, block_height);
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
        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);

        let temp_dir = std::env::temp_dir().join("balance_history_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db = BalanceHistoryDB::new(&temp_dir, config.clone()).unwrap();

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
        let entry = db.get_balance_at_block_height(script_hash, 50).unwrap();
        assert_eq!(entry.block_height, 0);
        assert_eq!(entry.delta, 0);
        assert_eq!(entry.balance, 0);

        // Get balance at height 100
        let entry = db.get_balance_at_block_height(script_hash, 100).unwrap();
        assert_eq!(entry.block_height, 100);
        assert_eq!(entry.delta, 500);
        assert_eq!(entry.balance, 500);

        // Test get_balance_at_block_height
        let entry = db.get_balance_at_block_height(script_hash, 250).unwrap();
        assert_eq!(entry.block_height, 200);
        assert_eq!(entry.delta, -200);
        assert_eq!(entry.balance, 300);

        // Test get_balance_in_range
        let range_entries = db.get_balance_in_range(script_hash, 150, 350).unwrap();
        assert_eq!(range_entries.len(), 2);
        assert_eq!(range_entries[0].block_height, 200);
        assert_eq!(range_entries[1].block_height, 300);

        // Test get_balance_in_range with no entries
        let range_entries = db.get_balance_in_range(script_hash, 500, 600).unwrap();
        assert_eq!(range_entries.len(), 0);

        // Test get_balance_in_range that hits the upper boundary
        let range_entries = db.get_balance_in_range(script_hash, 350, 401).unwrap();
        assert_eq!(range_entries.len(), 1);
        assert_eq!(range_entries[0].block_height, 400);

        // Test get_balance_in_range that includes all entries
        let range_entries = db.get_balance_in_range(script_hash, 0, 1000).unwrap();
        assert_eq!(range_entries.len(), 5);
        assert_eq!(range_entries[4].block_height, 401);

        db.close();

        // Test reopen the db
        let db = BalanceHistoryDB::new(&temp_dir, config.clone()).unwrap();
        let entry = db.get_balance_at_block_height(script_hash, 250).unwrap();
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

    #[test]
    fn test_utxo_put_get_consume() {
        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);
        let temp_dir = std::env::temp_dir().join("balance_history_utxo_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let db = BalanceHistoryDB::new(&temp_dir, config.clone()).unwrap();

        let outpoint = OutPoint {
            txid: Txid::from_slice(&[2u8; 32]).unwrap(),
            vout: 1,
        };
        let script = ScriptBuf::from(vec![2u8; 32]);
        let script_hash = script.script_hash();
        let amount = 1000u64;

        // Put UTXO
        db.put_utxo(&outpoint, &script_hash, amount).unwrap();

        // Get UTXO
        let utxo_entry = db.get_utxo(&outpoint).unwrap().unwrap();
        assert_eq!(utxo_entry.script_hash, script_hash);
        assert_eq!(utxo_entry.amount, amount);

        // Get none existing UTXO
        let missing_outpoint = OutPoint {
            txid: Txid::from_slice(&[3u8; 32]).unwrap(),
            vout: 0,
        };
        let utxo_entry = db.get_utxo(&missing_outpoint).unwrap();
        assert!(utxo_entry.is_none());

        // Consume UTXO
        let consumed_entry = db.spend_utxo(&outpoint).unwrap().unwrap();
        assert_eq!(consumed_entry.script_hash, script_hash);
        assert_eq!(consumed_entry.amount, amount);

        // Try to get UTXO again, should be None
        let utxo_entry = db.get_utxo(&outpoint).unwrap();
        assert!(utxo_entry.is_none());
    }
}
