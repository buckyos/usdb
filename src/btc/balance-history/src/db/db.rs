use super::helper::get_approx_cf_key_count;
use crate::config::BalanceHistoryConfigRef;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, OutPoint, Txid};
use rocksdb::{
    ColumnFamilyDescriptor, DB, Direction, IteratorMode, Options, ReadOptions, WriteBatch,
    WriteOptions,
};
use rust_rocksdb::{self as rocksdb};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use usdb_util::USDBScriptHash;
use usdb_util::{BalanceHistoryData, OutPointRef, UTXOEntry, UTXOEntryRef, UTXOValue};

// Column family names
pub const BALANCE_HISTORY_CF: &str = "balance_history";
pub const META_CF: &str = "meta";
pub const UTXO_CF: &str = "utxo";
pub const BLOCKS_CF: &str = "blocks";
pub const BLOCK_HEIGHTS_CF: &str = "block_heights";

// Mete key names
pub const META_KEY_BTC_BLOCK_HEIGHT: &str = "btc_block_height";
pub const META_KEY_LAST_BLOCK_FILE_INDEX: &str = "last_block_file_index";

pub const BALANCE_HISTORY_KEY_LEN: usize = USDBScriptHash::LEN + 4; // USDBScriptHash (32 bytes) + block_height (4 bytes)
pub const UTXO_KEY_LEN: usize = Txid::LEN + 4; // OutPoint: txid (32 bytes) + vout (4 bytes)
pub const BLOCKS_KEY_LEN: usize = BlockHash::LEN; // BlockHash (32 bytes)
pub const BLOCKS_VALUE_LEN: usize = std::mem::size_of::<BlockEntry>(); // block_file_index (4 bytes) + block_file_offset (8 bytes) + block_record_index (4 bytes)

#[derive(Debug, Clone)]
pub struct BalanceHistoryEntry {
    pub script_hash: USDBScriptHash,
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub block_file_index: u32,   // which blk file
    pub block_file_offset: u64,  // offset in the blk file
    pub block_record_index: u32, // index in the block record cache
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceHistoryDBMode {
    BestEffort,
    Normal,
}

pub struct BalanceHistoryDB {
    config: BalanceHistoryConfigRef,
    mode: Mutex<BalanceHistoryDBMode>,
    file: PathBuf,
    db: DB,
}

impl BalanceHistoryDB {
    pub fn open(
        config: BalanceHistoryConfigRef,
        mode: BalanceHistoryDBMode,
    ) -> Result<Self, String> {
        let db_dir = config.db_dir();
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
        info!("Opening RocksDB at {}, mode {:?}", file.display(), mode);

        // Default options
        let options = Self::get_options_on_mode(mode);

        // Define column families
        let cf_descriptors = Self::get_cf_descriptors_on_mode(mode);
        let db = DB::open_cf_descriptors(&options, &file, cf_descriptors).map_err(|e| {
            let msg = format!("Failed to open RocksDB at {}: {}", file.display(), e);
            error!("{}", msg);
            msg
        })?;

        Ok(BalanceHistoryDB {
            file,
            db,
            config,
            mode: Mutex::new(mode),
        })
    }

    pub fn switch_mode(&self, mode: BalanceHistoryDBMode) -> Result<(), String> {
        let old_mode;
        {
            let mut guard = self.mode.lock().unwrap();
            old_mode = *guard;
            *guard = mode;
        }

        if old_mode != mode {
            info!(
                "Switched BalanceHistoryDB mode from {:?} to {:?}",
                old_mode, mode
            );
            let new_opts = match mode {
                BalanceHistoryDBMode::BestEffort => vec![
                    ("write_buffer_size".to_string(), "268435456".to_string()), // 256MB
                    ("max_write_buffer_number".to_string(), "8".to_string()),
                    //("memtable_prefix_bloom_ratio".to_string(), "0.1".to_string()),
                ],
                BalanceHistoryDBMode::Normal => vec![
                    ("write_buffer_size".to_string(), "67108864".to_string()), // 64MB
                    ("max_write_buffer_number".to_string(), "2".to_string()),
                    //("memtable_prefix_bloom_ratio".to_string(), "0".to_string()),
                ],
            };
            let cf = self.db.cf_handle(BALANCE_HISTORY_CF).unwrap();
            let new_opts: Vec<(&str, &str)> = new_opts
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            self.db.set_options_cf(cf, &new_opts).map_err(|e| {
                let msg = format!("Failed to set new options for BalanceHistoryCF: {}", e);
                error!("{}", msg);
                msg
            })?;

            let new_opts = match mode {
                BalanceHistoryDBMode::BestEffort => vec![
                    ("write_buffer_size".to_string(), "268435456".to_string()), // 256MB
                    ("max_write_buffer_number".to_string(), "8".to_string()),
                    //(
                    //    "min_write_buffer_number_to_merge".to_string(),
                    //    "3".to_string(),
                    //),
                ],
                BalanceHistoryDBMode::Normal => vec![
                    ("write_buffer_size".to_string(), "67108864".to_string()), // 64MB
                    ("max_write_buffer_number".to_string(), "2".to_string()),
                    //(
                    //    "min_write_buffer_number_to_merge".to_string(),
                    //    "1".to_string(),
                    //),
                ],
            };

            let cf = self.db.cf_handle(UTXO_CF).unwrap();
            let new_opts: Vec<(&str, &str)> = new_opts
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            self.db.set_options_cf(cf, &new_opts).map_err(|e| {
                let msg = format!("Failed to set new options for UTXOCF: {}", e);
                error!("{}", msg);
                msg
            })?;
        }

        Ok(())
    }

    fn get_options_on_mode(mode: BalanceHistoryDBMode) -> Options {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        match mode {
            BalanceHistoryDBMode::BestEffort => {
                options.set_bytes_per_sync(1024 * 1024 * 4); // 4MB
                options.increase_parallelism(num_cpus::get() as i32); // Set parallelism to number of CPU cores
            }
            BalanceHistoryDBMode::Normal => {
                options.set_bytes_per_sync(1024 * 1024 * 1); // 1MB
            }
        }

        options
    }

    fn get_cf_descriptors_on_mode(mode: BalanceHistoryDBMode) -> Vec<ColumnFamilyDescriptor> {
        // Define column families
        vec![
            ColumnFamilyDescriptor::new(
                BALANCE_HISTORY_CF,
                Self::get_balance_history_cf_opts_on_mode(mode),
            ),
            ColumnFamilyDescriptor::new(META_CF, Options::default()),
            ColumnFamilyDescriptor::new(UTXO_CF, Self::get_utxo_cf_opts_on_mode(mode)),
            ColumnFamilyDescriptor::new(BLOCKS_CF, Options::default()),
            ColumnFamilyDescriptor::new(BLOCK_HEIGHTS_CF, Options::default()),
        ]
    }

    fn get_balance_history_cf_opts_on_mode(mode: BalanceHistoryDBMode) -> Options {
        match mode {
            BalanceHistoryDBMode::BestEffort => Self::best_balance_history_cf_opts(),
            BalanceHistoryDBMode::Normal => Self::normal_balance_history_cf_opts(),
        }
    }

    fn best_balance_history_cf_opts() -> Options {
        let mut opts = Self::normal_balance_history_cf_opts();

        opts.set_write_buffer_size(256 * 1024 * 1024); // 256MB
        opts.set_max_write_buffer_number(8);
        opts.set_min_write_buffer_number_to_merge(3);

        let mut block_opts = rocksdb::BlockBasedOptions::default();
        // Enable whole key and prefix bloom filter
        block_opts.set_bloom_filter(10.0, false);
        opts.set_block_based_table_factory(&block_opts);

        // Enable Memtable prefix bloom filter to speed up in-memory data lookups
        opts.set_memtable_prefix_bloom_ratio(0.1);
        opts
    }

    fn normal_balance_history_cf_opts() -> Options {
        let mut balance_history_cf_options = Options::default();
        balance_history_cf_options.set_level_compaction_dynamic_level_bytes(true);
        balance_history_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        balance_history_cf_options.create_if_missing(true);
        balance_history_cf_options.set_max_bytes_for_level_base(2 * 1024 * 1024 * 1024); // 2GB
        balance_history_cf_options.set_target_file_size_base(64 * 1024 * 1024);
        balance_history_cf_options.set_compression_type(rocksdb::DBCompressionType::Lz4);

        balance_history_cf_options.set_prefix_extractor(
            rocksdb::SliceTransform::create_fixed_prefix(USDBScriptHash::LEN),
        );

        balance_history_cf_options
    }

    fn get_utxo_cf_opts_on_mode(mode: BalanceHistoryDBMode) -> Options {
        match mode {
            BalanceHistoryDBMode::BestEffort => Self::best_utxo_cf_opts(),
            BalanceHistoryDBMode::Normal => Self::normal_utxo_cf_opts(),
        }
    }

    fn best_utxo_cf_opts() -> Options {
        let mut opts = Self::normal_utxo_cf_opts();

        opts.set_write_buffer_size(256 * 1024 * 1024); // 256MB
        opts.set_max_write_buffer_number(8);
        opts.set_min_write_buffer_number_to_merge(3);

        opts
    }

    fn normal_utxo_cf_opts() -> Options {
        let mut utxo_cf_options = Options::default();
        utxo_cf_options.set_level_compaction_dynamic_level_bytes(true);
        utxo_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        utxo_cf_options.create_if_missing(true);
        utxo_cf_options.set_max_bytes_for_level_base(4 * 1024 * 1024 * 1024); // 4GB
        utxo_cf_options.set_target_file_size_base(64 * 1024 * 1024);
        utxo_cf_options.set_compression_type(rocksdb::DBCompressionType::Lz4);

        utxo_cf_options
    }

    pub fn open_for_read(
        config: BalanceHistoryConfigRef,
        mode: BalanceHistoryDBMode,
    ) -> Result<Self, String> {
        let db_dir = config.db_dir();
        let file = db_dir.join("balance_history");
        info!("Opening RocksDB in read-only mode at {}", file.display());

        let mut opts = Options::default();
        opts.create_if_missing(false);

        let tmp_dir = std::env::temp_dir().join("usdb_balance_history_secondary");
        if !tmp_dir.exists() {
            std::fs::create_dir_all(&tmp_dir).map_err(|e| {
                let msg = format!(
                    "Could not create secondary RocksDB directory at {}: {}",
                    tmp_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        let cf_descriptors_names = vec![
            BALANCE_HISTORY_CF,
            META_CF,
            UTXO_CF,
            BLOCKS_CF,
            BLOCK_HEIGHTS_CF,
        ];
        let db = DB::open_cf_as_secondary(&opts, &file, &tmp_dir, cf_descriptors_names).map_err(
            |e| {
                let msg = format!("Failed to open RocksDB at {}: {}", file.display(), e);
                error!("{}", msg);
                msg
            },
        )?;

        Ok(BalanceHistoryDB {
            file,
            db,
            config,
            mode: Mutex::new(mode),
        })
    }

    pub fn close(self) {
        drop(self.db);
        info!("Closed RocksDB at {}", self.file.display());
    }

    // Flush secondary DB to catch up with primary in read-only mode
    pub fn flush_with_primary(&self) -> Result<(), String> {
        self.db.try_catch_up_with_primary().map_err(|e| {
            let msg = format!(
                "Failed to flush secondary RocksDB at {}: {}",
                self.file.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        info!("Flushed secondary RocksDB at {}", self.file.display());
        Ok(())
    }

    pub fn flush_all(&self) -> Result<(), String> {
        let mut flush_opts = rocksdb::FlushOptions::default();
        flush_opts.set_wait(true); // Wait until flush is done

        self.db.flush_opt(&flush_opts).map_err(|e| {
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

    fn make_balance_history_key(
        script_hash: &USDBScriptHash,
        block_height: u32,
    ) -> [u8; BALANCE_HISTORY_KEY_LEN] {
        // It is important that the block height is stored in big-endian format
        let mut key = [0u8; BALANCE_HISTORY_KEY_LEN];
        key[..USDBScriptHash::LEN].copy_from_slice(script_hash.as_ref());
        key[USDBScriptHash::LEN..USDBScriptHash::LEN + 4]
            .copy_from_slice(&block_height.to_be_bytes());
        key
    }

    fn parse_block_height_from_key(key: &[u8]) -> u32 {
        assert!(
            key.len() == BALANCE_HISTORY_KEY_LEN,
            "Invalid balance key length {}",
            key.len()
        );
        let block_height_bytes = &key[USDBScriptHash::LEN..USDBScriptHash::LEN + 4];
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

    fn make_utxo_key(outpoint: &OutPoint) -> [u8; UTXO_KEY_LEN] {
        usdb_util::OutPointCodec::encode(outpoint)
    }

    fn parse_utxo_from_value(value: &[u8]) -> UTXOValue {
        UTXOValue::from_slice(value).unwrap()
    }

    fn parse_block_from_value(value: &[u8]) -> BlockEntry {
        assert!(
            value.len() == BLOCKS_VALUE_LEN,
            "Invalid Block value length {}",
            value.len()
        );

        let block_file_index_bytes = &value[0..4];
        let block_file_offset_bytes = &value[4..12];
        let record_index_bytes = &value[12..16];

        let block_file_index = u32::from_be_bytes(block_file_index_bytes.try_into().unwrap());
        let block_file_offset = u64::from_be_bytes(block_file_offset_bytes.try_into().unwrap());
        let block_record_index = u32::from_be_bytes(record_index_bytes.try_into().unwrap());

        BlockEntry {
            block_file_index,
            block_file_offset,
            block_record_index,
        }
    }

    pub fn put_address_history_async(
        &self,
        entries_list: &Vec<BalanceHistoryEntry>,
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in entries_list {
            let key = Self::make_balance_history_key(&entry.script_hash, entry.block_height);

            // Value format: delta (i64) + balance (u64)
            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&entry.delta.to_be_bytes());
            value[8..16].copy_from_slice(&entry.balance.to_be_bytes());

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

    pub fn update_address_history_async(
        &self,
        entries_list: &Vec<BalanceHistoryEntry>,
        block_height: u32,
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in entries_list {
            let key = Self::make_balance_history_key(&entry.script_hash, entry.block_height);

            // Value format: delta (i64) + balance (u64)
            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&entry.delta.to_be_bytes());
            value[8..16].copy_from_slice(&entry.balance.to_be_bytes());

            batch.put_cf(cf, key, value);
        }

        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        let height_bytes = block_height.to_be_bytes();
        batch.put_cf(cf, META_KEY_BTC_BLOCK_HEIGHT, &height_bytes);

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
        script_hash: &USDBScriptHash,
    ) -> Result<BalanceHistoryData, String> {
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

            // Check if the USDBScriptHash matches
            if &found_key[0..USDBScriptHash::LEN] == script_hash.as_ref() as &[u8] {
                let block_height = Self::parse_block_height_from_key(&found_key);
                let (delta, balance) = Self::parse_balance_from_value(&found_val);
                let entry = BalanceHistoryData {
                    block_height,
                    delta,
                    balance,
                };

                return Ok(entry);
            }
        }

        // No records found for this script_hash
        let entry = BalanceHistoryData {
            block_height: 0,
            delta: 0,
            balance: 0,
        };

        Ok(entry)
    }

    /// Get the balance entry for a given script_hash at or before the target block height
    pub fn get_balance_at_block_height(
        &self,
        script_hash: &USDBScriptHash,
        target_height: u32,
    ) -> Result<BalanceHistoryData, String> {
        // Make the search key
        let search_key = Self::make_balance_history_key(script_hash, target_height);

        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut read_opts = ReadOptions::default();
        read_opts.set_prefix_same_as_start(true); // Use prefix seek
        read_opts.set_total_order_seek(false); // Disable total order seek for performance and use prefix seek

        // Create an iterator in reverse mode
        // IteratorMode::From(key, Reverse) positions at:
        // 1. If the key exists, it positions at that key.
        // 2. If the key does not exist, it positions at the first key less than the key (i.e., the previous height).
        let mut iter = self.db.iterator_cf_opt(
            cf,
            read_opts,
            IteratorMode::From(&search_key, Direction::Reverse),
        );

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

            // Boundary check 1: Ensure the key length is correct and belongs to the same USDBScriptHash
            // impl AsRef<[u8]> for Hash
            if &found_key[0..USDBScriptHash::LEN] == script_hash.as_ref() as &[u8] {
                // Found a record for the same address.
                // Since it is Reverse and the starting point is target_height,
                // the found_height here must be <= target_height.
                let block_height = Self::parse_block_height_from_key(&found_key);
                assert!(
                    block_height <= target_height,
                    "Found block height {} greater than target height {} for script_hash {}",
                    block_height,
                    target_height,
                    script_hash,
                );

                let (delta, balance) = Self::parse_balance_from_value(&found_val);
                let entry = BalanceHistoryData {
                    block_height,
                    delta,
                    balance,
                };

                return Ok(entry);
            }
        }

        // If the iterator is empty, or has moved to the previous USDBScriptHash,
        // it means there are no records for this address before the target_height.
        // The default balance is 0. and block_height is 0.
        let entry = BalanceHistoryData {
            block_height: 0,
            delta: 0,
            balance: 0,
        };

        Ok(entry)
    }

    /// Get balance records for a given script_hash within [range_begin, range_end)
    pub fn get_balance_in_range(
        &self,
        script_hash: &USDBScriptHash,
        range_begin: u32,
        range_end: u32,
    ) -> Result<Vec<BalanceHistoryData>, String> {
        assert!(
            range_begin < range_end,
            "Invalid range: {} >= {}",
            range_begin,
            range_end
        );
        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        // Create the start key
        let start_key = Self::make_balance_history_key(script_hash, range_begin);

        let mut read_opts = ReadOptions::default();
        read_opts.set_prefix_same_as_start(true); // Use prefix seek
        read_opts.set_total_order_seek(false); // Disable total order seek for performance and use prefix seek

        // Create an iterator starting from start_key in forward direction
        let iter = self.db.iterator_cf_opt(
            cf,
            read_opts,
            IteratorMode::From(&start_key, Direction::Forward),
        );

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

            // Boundary check 1: Check if the USDBScriptHash matches
            // If a different USDBScriptHash is encountered, it means the data for the current address has been fully traversed
            if &key[0..USDBScriptHash::LEN] != script_hash.as_ref() as &[u8] {
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

            let entry = BalanceHistoryData {
                block_height: height,
                delta,
                balance,
            };

            results.push(entry);
        }

        Ok(results)
    }

    pub fn get_all_balance(
        &self,
        script_hash: &USDBScriptHash,
    ) -> Result<Vec<BalanceHistoryData>, String> {
        self.get_balance_in_range(script_hash, 0, u32::MAX)
    }

    pub fn put_btc_block_height(&self, height: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(true);

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
        script_hash: &USDBScriptHash,
        amount: u64,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut ops = WriteOptions::default();
        ops.set_sync(false);

        // Value format: USDBScriptHash (32 bytes) + amount (u64)
        let value = UTXOValue::encode(script_hash, amount);

        let key = Self::make_utxo_key(outpoint);

        self.db.put_cf_opt(cf, key, value, &ops).map_err(|e| {
            let msg = format!("Failed to put UTXO: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn put_utxos(&self, utxos: &[UTXOEntry]) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = WriteBatch::default();

        for utxo in utxos {
            // Value format: USDBScriptHash (32 bytes) + amount (u64)
            let value = UTXOValue::encode(&utxo.script_hash, utxo.value);
            let key = Self::make_utxo_key(&utxo.outpoint);

            batch.put_cf(cf, key, value);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write UTXO batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn update_utxos_async(
        &self,
        new_utxos: &Vec<(OutPointRef, UTXOEntryRef)>,
        remove_utxos: &Vec<OutPointRef>,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = WriteBatch::default();

        for (outpoint, utxo) in new_utxos {
            // Value format: USDBScriptHash (32 bytes) + amount (u64)
            let value = UTXOValue::encode(&utxo.script_hash, utxo.value);
            let key = Self::make_utxo_key(outpoint);

            batch.put_cf(cf, key, value);
        }

        for outpoint in remove_utxos {
            let key = Self::make_utxo_key(outpoint);
            batch.delete_cf(cf, key);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write UTXO batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Just get the UTXO without removing it
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXOValue>, String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let key = Self::make_utxo_key(outpoint);

        match self.db.get_pinned_cf(cf, key) {
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

    pub fn get_utxos_bulk(
        &self,
        outpoints: &[OutPointRef],
    ) -> Result<Vec<Option<UTXOValue>>, String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let keys: Vec<[u8; UTXO_KEY_LEN]> = outpoints
            .iter()
            .map(|outpoint| Self::make_utxo_key(&outpoint))
            .collect();

        // Convert to iterator of (cf, key)
        let cf_keys = keys.iter().map(|k| (cf, k.as_slice()));

        // Execute batch read
        let results = self.db.multi_get_pinned_cf(cf_keys);

        // Parse results
        let mut entries = Vec::with_capacity(outpoints.len());
        for res in results {
            match res {
                Ok(Some(value)) => entries.push(Some(Self::parse_utxo_from_value(&value))),
                Ok(None) => entries.push(None),
                Err(e) => {
                    let msg = format!("Failed to get UTXO in bulk: {}", e);
                    error!("{}", msg);
                    return Err(msg);
                }
            }
        }

        Ok(entries)
    }

    /*
    // Remove and return the UTXO entry for the given outpoint
    pub fn spend_utxo(&self, outpoint: &OutPoint) -> Result<Option<UTXOValue>, String> {
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
    ) -> Result<Vec<(OutPoint, UTXOValue)>, String> {
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
    */

    // Get approximate count of balance history entries
    pub fn get_history_balance_count(&self) -> Result<u64, String> {
        get_approx_cf_key_count(&self.db, BALANCE_HISTORY_CF)
    }

    pub fn get_utxo_count(&self) -> Result<u64, String> {
        get_approx_cf_key_count(&self.db, UTXO_CF)
    }

    fn generate_balance_history_snapshot_sharded(
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
        seek_key.resize(USDBScriptHash::LEN, 0xFF); // max USDBScriptHash
        let seek_key = Self::make_balance_history_key(
            &USDBScriptHash::from_slice(&seek_key).unwrap(),
            u32::MAX,
        );

        let mut iter = self
            .db
            .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));

        let mut current_script_hash: Option<USDBScriptHash> = None;
        let mut current_founded = false;
        let mut snapshot = Vec::with_capacity(batch_size);
        let mut entries_processed = 0u64;

        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != BALANCE_HISTORY_KEY_LEN {
                continue;
            }

            //info!("Shard {:0x} processing key {:x?}", shard_index, key);
            if key[0] != shard_index {
                // Moved to a new shard
                break;
            }

            entries_processed += 1;

            let script_hash = USDBScriptHash::from_slice(&key[0..USDBScriptHash::LEN]).unwrap();
            let height = u32::from_be_bytes(
                key[USDBScriptHash::LEN..USDBScriptHash::LEN + 4]
                    .try_into()
                    .unwrap(),
            );

            if current_script_hash.as_ref() != Some(&script_hash) {
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
                        cb.on_balance_history_entries(&snapshot, entries_processed)?;
                        snapshot.clear();
                        entries_processed = 0;
                    }
                } else {
                    // Zero balance at this height, do not include in snapshot
                }

                current_founded = true;
            }
        }

        // Flush remaining snapshot entries
        if !snapshot.is_empty() {
            cb.on_balance_history_entries(&snapshot, entries_processed)?;
        }

        Ok(())
    }

    fn generate_utxo_snapshot_sharded(
        &self,
        shard_index: u8,
        batch_size: usize,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String> {
        let cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        let mut seek_key = vec![shard_index];
        seek_key.resize(UTXO_KEY_LEN, 0xFF); // max UTXO key
        let seek_key = seek_key;

        let mut iter = self
            .db
            .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));

        let mut snapshot = Vec::with_capacity(batch_size);
        let mut entries_processed = 0u64;

        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != UTXO_KEY_LEN {
                continue;
            }

            //info!("Shard {:0x} processing key {:x?}", shard_index, key);
            if key[0] != shard_index {
                // Moved to a new shard
                break;
            }

            entries_processed += 1;

            let outpoint = usdb_util::OutPointCodec::decode(&key).unwrap();
            let utxo_value = Self::parse_utxo_from_value(&value);

            let entry = UTXOEntry {
                outpoint,
                script_hash: utxo_value.script_hash,
                value: utxo_value.value,
            };
            snapshot.push(entry);

            if snapshot.len() >= batch_size {
                // Flush snapshot batch
                cb.on_utxo_entries(&snapshot, entries_processed)?;
                snapshot.clear();
                entries_processed = 0;
            }
        }

        // Flush remaining snapshot entries
        if !snapshot.is_empty() {
            cb.on_utxo_entries(&snapshot, entries_processed)?;
        }

        Ok(())
    }

    pub fn generate_balance_history_snapshot_parallel(
        &self,
        target_block_height: u32,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String> {
        use rayon::prelude::*;

        const SHARD_COUNT: u8 = 255;
        const BATCH_SIZE: usize = 1024 * 64;

        (0u8..=SHARD_COUNT)
            .into_par_iter()
            .try_for_each(|shard_index| {
                self.generate_balance_history_snapshot_sharded(
                    target_block_height,
                    shard_index,
                    BATCH_SIZE,
                    cb.clone(),
                )
            })?;

        Ok(())
    }

    pub fn generate_utxo_snapshot_parallel(
        &self,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String> {
        use rayon::prelude::*;

        const SHARD_COUNT: u8 = 255;
        const BATCH_SIZE: usize = 1024 * 64;

        (0u8..=SHARD_COUNT)
            .into_par_iter()
            .try_for_each(|shard_index| {
                self.generate_utxo_snapshot_sharded(shard_index, BATCH_SIZE, cb.clone())
            })?;

        Ok(())
    }

    // Traverse the latest balance entry for each script_hash in descending order
    pub fn traverse_latest<F>(
        &self,
        start_script_hash: Option<USDBScriptHash>,
        batch_size: usize,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(&[BalanceHistoryEntry]) -> Result<(), String>,
    {
        assert!(batch_size > 0, "Batch size must be greater than 0");

        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut iter = match start_script_hash {
            Some(script_hash) => {
                let mut seek_key = Vec::with_capacity(USDBScriptHash::LEN + 4);
                seek_key.extend_from_slice(script_hash.as_ref());
                seek_key.extend_from_slice(&[0xFF; 4]); // max block height

                self.db
                    .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse))
            }
            None => self.db.full_iterator_cf(&cf, IteratorMode::End),
        };

        let mut current_script_hash: Option<USDBScriptHash> = None;
        let mut snapshot = Vec::with_capacity(batch_size);
        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != BALANCE_HISTORY_KEY_LEN {
                continue;
            }

            let script_hash = USDBScriptHash::from_slice(&key[0..USDBScriptHash::LEN]).unwrap();
            let height = u32::from_be_bytes(
                key[USDBScriptHash::LEN..USDBScriptHash::LEN + 4]
                    .try_into()
                    .unwrap(),
            );

            if current_script_hash.is_none() {
                current_script_hash = Some(script_hash);
            } else if current_script_hash.as_ref().unwrap() != &script_hash {
                // Moved to a new script_hash
                current_script_hash = Some(script_hash);
            } else {
                // Already processed this script_hash
                continue;
            }

            let (delta, balance) = Self::parse_balance_from_value(&value);

            let entry = BalanceHistoryEntry {
                script_hash,
                block_height: height,
                delta,
                balance,
            };
            snapshot.push(entry);

            if snapshot.len() >= batch_size {
                // Flush snapshot batch
                callback(&snapshot)?;
                snapshot.clear();
            }
        }

        // Flush remaining snapshot entries
        if !snapshot.is_empty() {
            callback(&snapshot)?;
        }

        Ok(())
    }

    pub fn traverse_at_height<F>(
        &self,
        start_script_hash: Option<USDBScriptHash>,
        target_block_height: u32,
        batch_size: usize,
        mut callback: F,
    ) -> Result<(), String>
    where
        F: FnMut(&[BalanceHistoryEntry]) -> Result<(), String>,
    {
        assert!(batch_size > 0, "Batch size must be greater than 0");

        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut iter = match start_script_hash {
            Some(script_hash) => {
                let mut seek_key = Vec::with_capacity(USDBScriptHash::LEN + 4);
                seek_key.extend_from_slice(script_hash.as_ref());
                seek_key.extend_from_slice(&[0xFF; 4]); // max block height

                self.db
                    .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse))
            }
            None => self.db.full_iterator_cf(&cf, IteratorMode::End),
        };

        let mut current_script_hash: Option<USDBScriptHash> = None;
        let mut current_founded = false;
        let mut snapshot = Vec::with_capacity(batch_size);
        while let Some(Ok((key, value))) = iter.next() {
            if key.len() != BALANCE_HISTORY_KEY_LEN {
                continue;
            }

            let script_hash = USDBScriptHash::from_slice(&key[0..USDBScriptHash::LEN]).unwrap();
            let height = u32::from_be_bytes(
                key[USDBScriptHash::LEN..USDBScriptHash::LEN + 4]
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

    pub fn put_blocks_sync(
        &self,
        last_block_file_index: u32,
        blocks: &Vec<(BlockHash, BlockEntry)>,
        block_heights: &Vec<(u32, BlockHash)>,
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();

        // Put blocks
        let cf = self.db.cf_handle(BLOCKS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCKS_CF);
            error!("{}", msg);
            msg
        })?;
        for (block_hash, block_entry) in blocks {
            // Value format: block_file_index (u32) + block_file_offset (u64) + block_record_index (u32)
            let mut value = [0u8; BLOCKS_VALUE_LEN];
            value[..4].copy_from_slice(&block_entry.block_file_index.to_be_bytes());
            value[4..12].copy_from_slice(&block_entry.block_file_offset.to_be_bytes());
            value[12..16].copy_from_slice(&block_entry.block_record_index.to_be_bytes());

            batch.put_cf(cf, block_hash.as_ref() as &[u8], value);
        }

        // Put block heights
        let cf = self.db.cf_handle(BLOCK_HEIGHTS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_HEIGHTS_CF);
            error!("{}", msg);
            msg
        })?;
        for (block_height, block_hash) in block_heights {
            batch.put_cf(
                cf,
                &block_height.to_be_bytes(),
                block_hash.as_ref() as &[u8],
            );
        }

        // Put META_KEY_LAST_BLOCK_FILE_INDEX in META_CF
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        batch.put_cf(
            cf,
            META_KEY_LAST_BLOCK_FILE_INDEX,
            &last_block_file_index.to_be_bytes(),
        );

        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write Blocks batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_last_block_file_index(&self) -> Result<Option<u32>, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, META_KEY_LAST_BLOCK_FILE_INDEX) {
            Ok(Some(value)) => {
                if value.len() != 4 {
                    let msg = format!(
                        "Invalid last block file index value length: {}",
                        value.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
                let index = u32::from_be_bytes((value.as_ref() as &[u8]).try_into().unwrap());
                Ok(Some(index))
            }
            Ok(None) => {
                // Key does not exist, return index 0
                info!("Last block file index not found in DB, returning None");
                Ok(None)
            }
            Err(e) => {
                let msg = format!("Failed to get last block file index: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub fn get_all_blocks(&self) -> Result<HashMap<BlockHash, BlockEntry>, String> {
        let cf = self.db.cf_handle(BLOCKS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCKS_CF);
            error!("{}", msg);
            msg
        })?;

        let mut results = HashMap::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        for item in iter {
            let (key, value) = item.map_err(|e| {
                let msg = format!("Iterator error: {}", e);
                error!("{}", msg);
                msg
            })?;
            assert!(
                key.len() == BLOCKS_KEY_LEN,
                "Invalid Block key length {}",
                key.len()
            );
            let block_hash = BlockHash::from_slice(&key).unwrap();

            let block_entry = Self::parse_block_from_value(&value);
            results.insert(block_hash, block_entry);
        }

        Ok(results)
    }

    pub fn get_all_block_heights(&self) -> Result<Vec<(u32, BlockHash)>, String> {
        let cf = self.db.cf_handle(BLOCK_HEIGHTS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_HEIGHTS_CF);
            error!("{}", msg);
            msg
        })?;

        let mut results = Vec::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        for item in iter {
            let (key, value) = item.map_err(|e| {
                let msg = format!("Iterator error: {}", e);
                error!("{}", msg);
                msg
            })?;
            assert!(
                key.len() == 4,
                "Invalid BlockHeight key length {}",
                key.len()
            );

            let block_height = u32::from_be_bytes(key.as_ref().try_into().unwrap());
            let block_hash = BlockHash::from_slice(&value).unwrap();
            results.push((block_height, block_hash));
        }

        Ok(results)
    }

    pub fn clear_blocks(&self) -> Result<(), String> {
        // Clear meta last block file index at META_CF first
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        self.db
            .delete_cf(cf, META_KEY_LAST_BLOCK_FILE_INDEX)
            .map_err(|e| {
                let msg = format!("Failed to clear meta last block file index: {}", e);
                error!("{}", msg);
                msg
            })?;

        // Clear BLOCKS_CF and BLOCK_HEIGHTS_CF
        let cf = self.db.cf_handle(BLOCKS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCKS_CF);
            error!("{}", msg);
            msg
        })?;

        self.db.delete_cf(cf, b"").map_err(|e| {
            let msg = format!("Failed to clear Blocks CF: {}", e);
            error!("{}", msg);
            msg
        })?;

        let cf = self.db.cf_handle(BLOCK_HEIGHTS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_HEIGHTS_CF);
            error!("{}", msg);
            msg
        })?;
        self.db.delete_cf(cf, b"").map_err(|e| {
            let msg = format!("Failed to clear Block Heights CF: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!("Cleared all blocks and block heights from DB");
        Ok(())
    }
}

pub type BalanceHistoryDBRef = std::sync::Arc<BalanceHistoryDB>;

pub trait SnapshotCallback: Send + Sync {
    fn on_balance_history_entries(
        &self,
        entries: &[BalanceHistoryEntry],
        entries_processed: u64,
    ) -> Result<(), String>;

    fn on_utxo_entries(&self, entries: &[UTXOEntry], entries_processed: u64) -> Result<(), String>;
}

pub type SnapshotCallbackRef = std::sync::Arc<Box<dyn SnapshotCallback>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use std::sync::Arc;
    use usdb_util::ToUSDBScriptHash;

    #[test]
    fn test_make_and_parse_key() {
        let script = ScriptBuf::from(vec![0u8; 32]);
        let script_hash = script.to_usdb_script_hash();
        let block_height = 123456;

        let key = BalanceHistoryDB::make_balance_history_key(&script_hash, block_height);
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
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;

        let config = std::sync::Arc::new(config);
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let script = ScriptBuf::from(vec![1u8; 32]);
        let script_hash = script.to_usdb_script_hash();

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

        db.update_address_history_async(&entries, 401).unwrap();

        // Get balance at height 50 (before any entries)
        let entry = db.get_balance_at_block_height(&script_hash, 50).unwrap();
        assert_eq!(entry.block_height, 0);
        assert_eq!(entry.delta, 0);
        assert_eq!(entry.balance, 0);

        // Get balance at height 100
        let entry = db.get_balance_at_block_height(&script_hash, 100).unwrap();
        assert_eq!(entry.block_height, 100);
        assert_eq!(entry.delta, 500);
        assert_eq!(entry.balance, 500);

        // Test get_balance_at_block_height
        let entry = db.get_balance_at_block_height(&script_hash, 250).unwrap();
        assert_eq!(entry.block_height, 200);
        assert_eq!(entry.delta, -200);
        assert_eq!(entry.balance, 300);

        // Test get_balance_in_range
        let range_entries = db.get_balance_in_range(&script_hash, 150, 350).unwrap();
        assert_eq!(range_entries.len(), 2);
        assert_eq!(range_entries[0].block_height, 200);
        assert_eq!(range_entries[1].block_height, 300);

        // Test get_balance_in_range with no entries
        let range_entries = db.get_balance_in_range(&script_hash, 500, 600).unwrap();
        assert_eq!(range_entries.len(), 0);

        // Test get_balance_in_range that hits the upper boundary
        let range_entries = db.get_balance_in_range(&script_hash, 350, 401).unwrap();
        assert_eq!(range_entries.len(), 1);
        assert_eq!(range_entries[0].block_height, 400);

        // Test get_balance_in_range that includes all entries
        let range_entries = db.get_balance_in_range(&script_hash, 0, 1000).unwrap();
        assert_eq!(range_entries.len(), 5);
        assert_eq!(range_entries[4].block_height, 401);

        db.close();

        // Test reopen the db
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        let entry = db.get_balance_at_block_height(&script_hash, 250).unwrap();
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
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_utxo_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let outpoint = OutPoint {
            txid: Txid::from_slice(&[2u8; 32]).unwrap(),
            vout: 1,
        };
        let script = ScriptBuf::from(vec![2u8; 32]);
        let script_hash = script.to_usdb_script_hash();
        let amount = 1000u64;

        // Put UTXO
        db.put_utxo(&outpoint, &script_hash, amount).unwrap();

        // Get UTXO
        let utxo_entry = db.get_utxo(&outpoint).unwrap().unwrap();
        assert_eq!(utxo_entry.script_hash, script_hash);
        assert_eq!(utxo_entry.value, amount);

        // Get none existing UTXO
        let missing_outpoint = OutPoint {
            txid: Txid::from_slice(&[3u8; 32]).unwrap(),
            vout: 0,
        };
        let utxo_entry = db.get_utxo(&missing_outpoint).unwrap();
        assert!(utxo_entry.is_none());

        // Consume UTXO
        db.update_utxos_async(&vec![], &vec![Arc::new(outpoint.clone())])
            .unwrap();

        // Try to get UTXO again, should be None
        let utxo_entry = db.get_utxo(&outpoint).unwrap();
        assert!(utxo_entry.is_none());
    }
}
