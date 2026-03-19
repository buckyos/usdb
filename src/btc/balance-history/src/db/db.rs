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
// BLOCK_COMMITS_CF stores the first-version logical commit chain metadata for balance-history.
pub const BLOCK_COMMITS_CF: &str = "block_commits";
pub const BLOCK_UNDO_META_CF: &str = "block_undo_meta";
pub const BLOCK_UNDO_CREATED_UTXOS_CF: &str = "block_undo_created_utxos";
pub const BLOCK_UNDO_SPENT_UTXOS_CF: &str = "block_undo_spent_utxos";
pub const BLOCK_UNDO_BALANCE_INDEX_CF: &str = "block_undo_balance_index";

// Mete key names
pub const META_KEY_BTC_BLOCK_HEIGHT: &str = "btc_block_height";
pub const META_KEY_LAST_BLOCK_FILE_INDEX: &str = "last_block_file_index";
pub const META_KEY_ROLLBACK_IN_PROGRESS: &str = "rollback_in_progress";
pub const META_KEY_ROLLBACK_TARGET_HEIGHT: &str = "rollback_target_height";
pub const META_KEY_ROLLBACK_NEXT_HEIGHT: &str = "rollback_next_height";
pub const META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT: &str = "rollback_supported_from_height";
pub const META_KEY_UNDO_RETAINED_FROM_HEIGHT: &str = "undo_retained_from_height";

pub const BALANCE_HISTORY_KEY_LEN: usize = USDBScriptHash::LEN + 4; // USDBScriptHash (32 bytes) + block_height (4 bytes)
pub const UTXO_KEY_LEN: usize = Txid::LEN + 4; // OutPoint: txid (32 bytes) + vout (4 bytes)
pub const BLOCKS_KEY_LEN: usize = BlockHash::LEN; // BlockHash (32 bytes)
pub const BLOCKS_VALUE_LEN: usize = std::mem::size_of::<BlockEntry>(); // block_file_index (4 bytes) + block_file_offset (8 bytes) + block_record_index (4 bytes)
// Value layout in BLOCK_COMMITS_CF: block hash + balance delta root + block commit.
pub const BLOCK_COMMIT_VALUE_LEN: usize = BlockHash::LEN + 32 + 32;
pub const BLOCK_UNDO_META_VALUE_LEN: usize = 2 + BlockHash::LEN + 4 + 4 + 4;
pub const BLOCK_UNDO_UTXO_KEY_LEN: usize = 4 + 4;
pub const BLOCK_UNDO_UTXO_VALUE_LEN: usize = UTXO_KEY_LEN + USDBScriptHash::LEN + 8;
pub const BLOCK_UNDO_BALANCE_INDEX_KEY_LEN: usize = 4 + USDBScriptHash::LEN;

#[derive(Debug, Clone)]
pub struct BalanceHistoryEntry {
    // Address script hash.
    pub script_hash: USDBScriptHash,
    // Block height where this record was written.
    pub block_height: u32,
    // Balance delta applied at this height.
    pub delta: i64,
    // Final balance after the delta is applied.
    pub balance: u64,
}

#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub block_file_index: u32,   // which blk file
    pub block_file_offset: u64,  // offset in the blk file
    pub block_record_index: u32, // index in the block record cache
}

// BlockCommitEntry is the persisted per-block commit metadata exposed to downstream services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockCommitEntry {
    // BTC block height that this commit corresponds to.
    pub block_height: u32,
    // BTC block hash paired with the height above.
    pub btc_block_hash: BlockHash,
    // Hash of the canonical per-block balance delta set.
    pub balance_delta_root: [u8; 32],
    // Rolling block commit linked to the previous committed block.
    pub block_commit: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUndoMetaEntry {
    // Block height that this undo bundle can revert.
    pub block_height: u32,
    // On-disk undo encoding version for forward compatibility.
    pub format_version: u16,
    // Canonical BTC block hash paired with the block height above.
    pub btc_block_hash: BlockHash,
    // Number of UTXOs created by the block and deleted during rollback.
    pub created_utxo_count: u32,
    // Number of spent UTXOs restored during rollback.
    pub spent_utxo_count: u32,
    // Number of script hashes whose exact-height balance rows are removed during rollback.
    pub balance_entry_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUndoUtxoEntry {
    // Outpoint to delete or restore while reverting a block.
    pub outpoint: OutPoint,
    // Script hash owning the UTXO at the reverted height.
    pub script_hash: USDBScriptHash,
    // UTXO value used to reconstruct spent outputs.
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUndoBundle {
    // Block height that this bundle reverts.
    pub block_height: u32,
    // BTC block hash used to validate undo/canonical chain alignment.
    pub btc_block_hash: BlockHash,
    // Outputs created by the block and removed on rollback.
    pub created_utxos: Vec<BlockUndoUtxoEntry>,
    // Outputs spent by the block and restored on rollback.
    pub spent_utxos: Vec<BlockUndoUtxoEntry>,
    // Script hashes whose exact block-height balance rows must be deleted.
    pub touched_script_hashes: Vec<USDBScriptHash>,
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

    pub fn get_mode(&self) -> BalanceHistoryDBMode {
        let guard = self.mode.lock().unwrap();
        *guard
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
                let msg = format!("Failed to set new options for UTXO CF: {}", e);
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
            ColumnFamilyDescriptor::new(BLOCK_COMMITS_CF, Options::default()),
            ColumnFamilyDescriptor::new(BLOCK_UNDO_META_CF, Options::default()),
            ColumnFamilyDescriptor::new(
                BLOCK_UNDO_CREATED_UTXOS_CF,
                Self::get_block_undo_height_cf_opts(),
            ),
            ColumnFamilyDescriptor::new(
                BLOCK_UNDO_SPENT_UTXOS_CF,
                Self::get_block_undo_height_cf_opts(),
            ),
            ColumnFamilyDescriptor::new(
                BLOCK_UNDO_BALANCE_INDEX_CF,
                Self::get_block_undo_height_cf_opts(),
            ),
        ]
    }

    fn get_block_undo_height_cf_opts() -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.set_prefix_extractor(rocksdb::SliceTransform::create_fixed_prefix(4));
        opts
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
            BLOCK_COMMITS_CF,
            BLOCK_UNDO_META_CF,
            BLOCK_UNDO_CREATED_UTXOS_CF,
            BLOCK_UNDO_SPENT_UTXOS_CF,
            BLOCK_UNDO_BALANCE_INDEX_CF,
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

    fn make_block_undo_meta_key(block_height: u32) -> [u8; 4] {
        block_height.to_be_bytes()
    }

    fn serialize_block_undo_meta_value(
        entry: &BlockUndoMetaEntry,
    ) -> [u8; BLOCK_UNDO_META_VALUE_LEN] {
        let mut value = [0u8; BLOCK_UNDO_META_VALUE_LEN];
        value[..2].copy_from_slice(&entry.format_version.to_be_bytes());
        value[2..2 + BlockHash::LEN].copy_from_slice(entry.btc_block_hash.as_ref());
        let mut offset = 2 + BlockHash::LEN;
        value[offset..offset + 4].copy_from_slice(&entry.created_utxo_count.to_be_bytes());
        offset += 4;
        value[offset..offset + 4].copy_from_slice(&entry.spent_utxo_count.to_be_bytes());
        offset += 4;
        value[offset..offset + 4].copy_from_slice(&entry.balance_entry_count.to_be_bytes());
        value
    }

    fn parse_block_undo_meta_value(
        block_height: u32,
        value: &[u8],
    ) -> Result<BlockUndoMetaEntry, String> {
        if value.len() != BLOCK_UNDO_META_VALUE_LEN {
            let msg = format!(
                "Invalid block undo meta value length for height {}: expected {}, got {}",
                block_height,
                BLOCK_UNDO_META_VALUE_LEN,
                value.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        let format_version = u16::from_be_bytes(value[..2].try_into().unwrap());
        let btc_block_hash = BlockHash::from_slice(&value[2..2 + BlockHash::LEN]).map_err(|e| {
            let msg = format!(
                "Invalid BTC block hash in block undo meta at height {}: {}",
                block_height, e
            );
            error!("{}", msg);
            msg
        })?;
        let mut offset = 2 + BlockHash::LEN;
        let created_utxo_count = u32::from_be_bytes(value[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let spent_utxo_count = u32::from_be_bytes(value[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let balance_entry_count = u32::from_be_bytes(value[offset..offset + 4].try_into().unwrap());

        Ok(BlockUndoMetaEntry {
            block_height,
            format_version,
            btc_block_hash,
            created_utxo_count,
            spent_utxo_count,
            balance_entry_count,
        })
    }

    fn make_block_undo_utxo_key(block_height: u32, seq: u32) -> [u8; BLOCK_UNDO_UTXO_KEY_LEN] {
        let mut key = [0u8; BLOCK_UNDO_UTXO_KEY_LEN];
        key[..4].copy_from_slice(&block_height.to_be_bytes());
        key[4..].copy_from_slice(&seq.to_be_bytes());
        key
    }

    fn serialize_block_undo_utxo_value(
        entry: &BlockUndoUtxoEntry,
    ) -> [u8; BLOCK_UNDO_UTXO_VALUE_LEN] {
        let mut value = [0u8; BLOCK_UNDO_UTXO_VALUE_LEN];
        value[..UTXO_KEY_LEN].copy_from_slice(&Self::make_utxo_key(&entry.outpoint));
        let mut offset = UTXO_KEY_LEN;
        value[offset..offset + USDBScriptHash::LEN].copy_from_slice(entry.script_hash.as_ref());
        offset += USDBScriptHash::LEN;
        value[offset..offset + 8].copy_from_slice(&entry.value.to_be_bytes());
        value
    }

    fn parse_block_undo_utxo_value(value: &[u8]) -> Result<BlockUndoUtxoEntry, String> {
        if value.len() != BLOCK_UNDO_UTXO_VALUE_LEN {
            let msg = format!(
                "Invalid block undo utxo value length: expected {}, got {}",
                BLOCK_UNDO_UTXO_VALUE_LEN,
                value.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        let outpoint = usdb_util::OutPointCodec::decode(&value[..UTXO_KEY_LEN]).map_err(|e| {
            let msg = format!("Failed to decode block undo outpoint: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut script_hash_bytes = [0u8; USDBScriptHash::LEN];
        let mut offset = UTXO_KEY_LEN;
        script_hash_bytes.copy_from_slice(&value[offset..offset + USDBScriptHash::LEN]);
        offset += USDBScriptHash::LEN;
        let value_sats = u64::from_be_bytes(value[offset..offset + 8].try_into().unwrap());

        Ok(BlockUndoUtxoEntry {
            outpoint,
            script_hash: USDBScriptHash::from_byte_array(script_hash_bytes),
            value: value_sats,
        })
    }

    fn make_block_undo_balance_index_key(
        block_height: u32,
        script_hash: &USDBScriptHash,
    ) -> [u8; BLOCK_UNDO_BALANCE_INDEX_KEY_LEN] {
        let mut key = [0u8; BLOCK_UNDO_BALANCE_INDEX_KEY_LEN];
        key[..4].copy_from_slice(&block_height.to_be_bytes());
        key[4..].copy_from_slice(script_hash.as_ref());
        key
    }

    fn block_height_prefix_matches(key: &[u8], block_height: u32) -> bool {
        key.len() >= 4 && key[..4] == block_height.to_be_bytes()
    }

    fn parse_u32_be_key(key: &[u8]) -> Result<u32, String> {
        if key.len() != 4 {
            let msg = format!("Invalid 4-byte key length: expected 4, got {}", key.len());
            error!("{}", msg);
            return Err(msg);
        }

        Ok(u32::from_be_bytes(key.try_into().unwrap()))
    }

    // Block commit keys are indexed directly by big-endian block height.
    fn make_block_commit_key(block_height: u32) -> [u8; 4] {
        block_height.to_be_bytes()
    }

    // Serialize one BlockCommitEntry into the stable on-disk value layout used by BLOCK_COMMITS_CF.
    fn serialize_block_commit_value(entry: &BlockCommitEntry) -> [u8; BLOCK_COMMIT_VALUE_LEN] {
        let mut value = [0u8; BLOCK_COMMIT_VALUE_LEN];
        value[..BlockHash::LEN].copy_from_slice(entry.btc_block_hash.as_ref());
        value[BlockHash::LEN..BlockHash::LEN + 32].copy_from_slice(&entry.balance_delta_root);
        value[BlockHash::LEN + 32..].copy_from_slice(&entry.block_commit);
        value
    }

    // Parse a BLOCK_COMMITS_CF value back into the structured metadata returned by the DB API.
    fn parse_block_commit_value(
        block_height: u32,
        value: &[u8],
    ) -> Result<BlockCommitEntry, String> {
        if value.len() != BLOCK_COMMIT_VALUE_LEN {
            let msg = format!(
                "Invalid block commit value length: expected {}, got {}",
                BLOCK_COMMIT_VALUE_LEN,
                value.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        let btc_block_hash = BlockHash::from_slice(&value[..BlockHash::LEN]).map_err(|e| {
            let msg = format!("Failed to parse block hash from block commit: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut balance_delta_root = [0u8; 32];
        balance_delta_root.copy_from_slice(&value[BlockHash::LEN..BlockHash::LEN + 32]);

        let mut block_commit = [0u8; 32];
        block_commit.copy_from_slice(&value[BlockHash::LEN + 32..]);

        Ok(BlockCommitEntry {
            block_height,
            btc_block_hash,
            balance_delta_root,
            block_commit,
        })
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

    // Only for testing - put address history entries together with the block height metadata in one batch write.
    fn update_address_history_async(
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

    // Persist one batch of balance history entries together with the matching block commit metadata.
    // This keeps logical state and commit chain metadata on the same write boundary.
    pub fn update_address_history_with_block_commits_async(
        &self,
        entries_list: &Vec<BalanceHistoryEntry>,
        block_height: u32,
        block_commits: &[BlockCommitEntry],
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();

        let balance_cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in entries_list {
            let key = Self::make_balance_history_key(&entry.script_hash, entry.block_height);

            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&entry.delta.to_be_bytes());
            value[8..16].copy_from_slice(&entry.balance.to_be_bytes());

            batch.put_cf(balance_cf, key, value);
        }

        let block_commit_cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in block_commits {
            let key = Self::make_block_commit_key(entry.block_height);
            let value = Self::serialize_block_commit_value(entry);
            batch.put_cf(block_commit_cf, key, value);
        }

        let meta_cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        let height_bytes = block_height.to_be_bytes();
        batch.put_cf(meta_cf, META_KEY_BTC_BLOCK_HEIGHT, &height_bytes);

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Persist one block batch atomically across UTXO state, balance history,
    // block commits and the stable BTC height marker.
    pub fn update_block_state_async(
        &self,
        new_utxos: &[(OutPointRef, UTXOEntryRef)],
        remove_utxos: &[OutPointRef],
        entries_list: &[BalanceHistoryEntry],
        block_height: u32,
        block_commits: &[BlockCommitEntry],
    ) -> Result<(), String> {
        self.update_block_state_with_undo_async(
            new_utxos,
            remove_utxos,
            entries_list,
            block_height,
            block_commits,
            &[],
        )
    }

    pub fn update_block_state_with_undo_async(
        &self,
        new_utxos: &[(OutPointRef, UTXOEntryRef)],
        remove_utxos: &[OutPointRef],
        entries_list: &[BalanceHistoryEntry],
        block_height: u32,
        block_commits: &[BlockCommitEntry],
        undo_bundles: &[BlockUndoBundle],
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();

        let utxo_cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;

        for (outpoint, utxo) in new_utxos {
            let value = UTXOValue::encode(&utxo.script_hash, utxo.value);
            let key = Self::make_utxo_key(outpoint);
            batch.put_cf(utxo_cf, key, value);
        }

        for outpoint in remove_utxos {
            let key = Self::make_utxo_key(outpoint);
            batch.delete_cf(utxo_cf, key);
        }

        let balance_cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in entries_list {
            let key = Self::make_balance_history_key(&entry.script_hash, entry.block_height);

            let mut value = [0u8; 16];
            value[..8].copy_from_slice(&entry.delta.to_be_bytes());
            value[8..16].copy_from_slice(&entry.balance.to_be_bytes());

            batch.put_cf(balance_cf, key, value);
        }

        let block_commit_cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in block_commits {
            let key = Self::make_block_commit_key(entry.block_height);
            let value = Self::serialize_block_commit_value(entry);
            batch.put_cf(block_commit_cf, key, value);
        }

        let meta_cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        let height_bytes = block_height.to_be_bytes();
        batch.put_cf(meta_cf, META_KEY_BTC_BLOCK_HEIGHT, &height_bytes);

        self.append_block_undo_bundles_to_batch(&mut batch, undo_bundles)?;

        if !undo_bundles.is_empty() {
            let first_undo_height = undo_bundles
                .iter()
                .map(|bundle| bundle.block_height)
                .min()
                .unwrap();

            if self.get_rollback_supported_from_height()?.is_none() {
                batch.put_cf(
                    meta_cf,
                    META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT,
                    first_undo_height.to_be_bytes(),
                );
            }

            if self.get_undo_retained_from_height()?.is_none() {
                batch.put_cf(
                    meta_cf,
                    META_KEY_UNDO_RETAINED_FROM_HEIGHT,
                    first_undo_height.to_be_bytes(),
                );
            }
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write atomic block-state batch to DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn append_block_undo_bundles_to_batch(
        &self,
        batch: &mut WriteBatch,
        undo_bundles: &[BlockUndoBundle],
    ) -> Result<(), String> {
        if undo_bundles.is_empty() {
            return Ok(());
        }

        let meta_cf = self.db.cf_handle(BLOCK_UNDO_META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_UNDO_META_CF);
            error!("{}", msg);
            msg
        })?;
        let created_cf = self
            .db
            .cf_handle(BLOCK_UNDO_CREATED_UTXOS_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_CREATED_UTXOS_CF);
                error!("{}", msg);
                msg
            })?;
        let spent_cf = self
            .db
            .cf_handle(BLOCK_UNDO_SPENT_UTXOS_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_SPENT_UTXOS_CF);
                error!("{}", msg);
                msg
            })?;
        let balance_cf = self
            .db
            .cf_handle(BLOCK_UNDO_BALANCE_INDEX_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_BALANCE_INDEX_CF);
                error!("{}", msg);
                msg
            })?;

        for bundle in undo_bundles {
            let meta = BlockUndoMetaEntry {
                block_height: bundle.block_height,
                format_version: 1,
                btc_block_hash: bundle.btc_block_hash,
                created_utxo_count: bundle.created_utxos.len() as u32,
                spent_utxo_count: bundle.spent_utxos.len() as u32,
                balance_entry_count: bundle.touched_script_hashes.len() as u32,
            };

            batch.put_cf(
                meta_cf,
                Self::make_block_undo_meta_key(bundle.block_height),
                Self::serialize_block_undo_meta_value(&meta),
            );

            for (seq, entry) in bundle.created_utxos.iter().enumerate() {
                batch.put_cf(
                    created_cf,
                    Self::make_block_undo_utxo_key(bundle.block_height, seq as u32),
                    Self::serialize_block_undo_utxo_value(entry),
                );
            }

            for (seq, entry) in bundle.spent_utxos.iter().enumerate() {
                batch.put_cf(
                    spent_cf,
                    Self::make_block_undo_utxo_key(bundle.block_height, seq as u32),
                    Self::serialize_block_undo_utxo_value(entry),
                );
            }

            for script_hash in &bundle.touched_script_hashes {
                batch.put_cf(
                    balance_cf,
                    Self::make_block_undo_balance_index_key(bundle.block_height, script_hash),
                    [1u8; 1],
                );
            }
        }

        Ok(())
    }

    pub fn get_block_undo_meta(
        &self,
        block_height: u32,
    ) -> Result<Option<BlockUndoMetaEntry>, String> {
        let cf = self.db.cf_handle(BLOCK_UNDO_META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_UNDO_META_CF);
            error!("{}", msg);
            msg
        })?;

        match self
            .db
            .get_cf(cf, Self::make_block_undo_meta_key(block_height))
        {
            Ok(Some(value)) => Ok(Some(Self::parse_block_undo_meta_value(
                block_height,
                &value,
            )?)),
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!(
                    "Failed to get block undo meta at height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    fn get_block_undo_utxos_from_cf(
        &self,
        cf_name: &str,
        block_height: u32,
    ) -> Result<Vec<BlockUndoUtxoEntry>, String> {
        let cf = self.db.cf_handle(cf_name).ok_or_else(|| {
            let msg = format!("Column family {} not found", cf_name);
            error!("{}", msg);
            msg
        })?;
        let start_key = Self::make_block_undo_utxo_key(block_height, 0);
        let mut read_opts = ReadOptions::default();
        read_opts.set_prefix_same_as_start(true);
        read_opts.set_total_order_seek(false);
        let iter = self.db.iterator_cf_opt(
            cf,
            read_opts,
            IteratorMode::From(&start_key, Direction::Forward),
        );

        let mut entries = Vec::new();
        for item in iter {
            let (key, value) = item.map_err(|e| {
                let msg = format!(
                    "Iterator error when reading block undo utxos from {} at height {}: {}",
                    cf_name, block_height, e
                );
                error!("{}", msg);
                msg
            })?;
            if !Self::block_height_prefix_matches(&key, block_height) {
                break;
            }
            entries.push(Self::parse_block_undo_utxo_value(&value)?);
        }
        Ok(entries)
    }

    pub fn get_block_undo_created_utxos(
        &self,
        block_height: u32,
    ) -> Result<Vec<BlockUndoUtxoEntry>, String> {
        self.get_block_undo_utxos_from_cf(BLOCK_UNDO_CREATED_UTXOS_CF, block_height)
    }

    pub fn get_block_undo_spent_utxos(
        &self,
        block_height: u32,
    ) -> Result<Vec<BlockUndoUtxoEntry>, String> {
        self.get_block_undo_utxos_from_cf(BLOCK_UNDO_SPENT_UTXOS_CF, block_height)
    }

    pub fn get_block_undo_touched_script_hashes(
        &self,
        block_height: u32,
    ) -> Result<Vec<USDBScriptHash>, String> {
        let cf = self
            .db
            .cf_handle(BLOCK_UNDO_BALANCE_INDEX_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_BALANCE_INDEX_CF);
                error!("{}", msg);
                msg
            })?;
        let start_key = Self::make_block_undo_balance_index_key(
            block_height,
            &USDBScriptHash::from_byte_array([0u8; USDBScriptHash::LEN]),
        );
        let mut read_opts = ReadOptions::default();
        read_opts.set_prefix_same_as_start(true);
        read_opts.set_total_order_seek(false);
        let iter = self.db.iterator_cf_opt(
            cf,
            read_opts,
            IteratorMode::From(&start_key, Direction::Forward),
        );

        let mut script_hashes = Vec::new();
        for item in iter {
            let (key, _value) = item.map_err(|e| {
                let msg = format!(
                    "Iterator error when reading block undo balance index at height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;
            if !Self::block_height_prefix_matches(&key, block_height) {
                break;
            }
            let mut bytes = [0u8; USDBScriptHash::LEN];
            bytes.copy_from_slice(&key[4..4 + USDBScriptHash::LEN]);
            script_hashes.push(USDBScriptHash::from_byte_array(bytes));
        }

        Ok(script_hashes)
    }

    pub fn get_block_undo_bundle(
        &self,
        block_height: u32,
    ) -> Result<Option<BlockUndoBundle>, String> {
        let meta = match self.get_block_undo_meta(block_height)? {
            Some(meta) => meta,
            None => return Ok(None),
        };

        Ok(Some(BlockUndoBundle {
            block_height,
            btc_block_hash: meta.btc_block_hash,
            created_utxos: self.get_block_undo_created_utxos(block_height)?,
            spent_utxos: self.get_block_undo_spent_utxos(block_height)?,
            touched_script_hashes: self.get_block_undo_touched_script_hashes(block_height)?,
        }))
    }

    fn append_delete_block_undo_bundle_to_batch(
        &self,
        batch: &mut WriteBatch,
        block_height: u32,
    ) -> Result<(), String> {
        let meta_cf = self.db.cf_handle(BLOCK_UNDO_META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_UNDO_META_CF);
            error!("{}", msg);
            msg
        })?;
        let created_cf = self
            .db
            .cf_handle(BLOCK_UNDO_CREATED_UTXOS_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_CREATED_UTXOS_CF);
                error!("{}", msg);
                msg
            })?;
        let spent_cf = self
            .db
            .cf_handle(BLOCK_UNDO_SPENT_UTXOS_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_SPENT_UTXOS_CF);
                error!("{}", msg);
                msg
            })?;
        let balance_cf = self
            .db
            .cf_handle(BLOCK_UNDO_BALANCE_INDEX_CF)
            .ok_or_else(|| {
                let msg = format!("Column family {} not found", BLOCK_UNDO_BALANCE_INDEX_CF);
                error!("{}", msg);
                msg
            })?;

        batch.delete_cf(meta_cf, Self::make_block_undo_meta_key(block_height));

        let created = self.get_block_undo_created_utxos(block_height)?;
        for (seq, _entry) in created.iter().enumerate() {
            batch.delete_cf(
                created_cf,
                Self::make_block_undo_utxo_key(block_height, seq as u32),
            );
        }

        let spent = self.get_block_undo_spent_utxos(block_height)?;
        for (seq, _entry) in spent.iter().enumerate() {
            batch.delete_cf(
                spent_cf,
                Self::make_block_undo_utxo_key(block_height, seq as u32),
            );
        }

        let touched = self.get_block_undo_touched_script_hashes(block_height)?;
        for script_hash in touched {
            batch.delete_cf(
                balance_cf,
                Self::make_block_undo_balance_index_key(block_height, &script_hash),
            );
        }

        Ok(())
    }

    pub fn rollback_one_block(&self, block_height: u32) -> Result<(), String> {
        let current_height = self.get_btc_block_height()?;
        if current_height != block_height {
            let msg = format!(
                "Rollback currently only supports the latest committed height: current_height={}, rollback_height={}",
                current_height, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let bundle = self.get_block_undo_bundle(block_height)?.ok_or_else(|| {
            let msg = format!("Missing block undo bundle at height {}", block_height);
            error!("{}", msg);
            msg
        })?;

        let utxo_cf = self.db.cf_handle(UTXO_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", UTXO_CF);
            error!("{}", msg);
            msg
        })?;
        let balance_cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;
        let block_commit_cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;
        let meta_cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = WriteBatch::default();

        for created in &bundle.created_utxos {
            batch.delete_cf(utxo_cf, Self::make_utxo_key(&created.outpoint));
        }

        for spent in &bundle.spent_utxos {
            batch.put_cf(
                utxo_cf,
                Self::make_utxo_key(&spent.outpoint),
                UTXOValue::encode(&spent.script_hash, spent.value),
            );
        }

        for script_hash in &bundle.touched_script_hashes {
            batch.delete_cf(
                balance_cf,
                Self::make_balance_history_key(script_hash, block_height),
            );
        }

        batch.delete_cf(block_commit_cf, Self::make_block_commit_key(block_height));

        self.append_delete_block_undo_bundle_to_batch(&mut batch, block_height)?;

        let previous_height = block_height.saturating_sub(1);
        batch.put_cf(
            meta_cf,
            META_KEY_BTC_BLOCK_HEIGHT,
            previous_height.to_be_bytes(),
        );

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to rollback block height {}: {}", block_height, e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Roll back committed forward state down to the inclusive target height.
    pub fn rollback_to_block_height(&self, target_height: u32) -> Result<(), String> {
        let current_height = self.get_btc_block_height()?;
        if target_height > current_height {
            let msg = format!(
                "Invalid rollback target height {} above current height {}",
                target_height, current_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        if target_height == current_height {
            return Ok(());
        }

        self.put_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS, 1)?;
        self.put_u32_meta(META_KEY_ROLLBACK_TARGET_HEIGHT, target_height)?;
        self.put_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT, current_height)?;

        let result = (|| {
            let mut next_height = current_height;
            while next_height > target_height {
                self.rollback_one_block(next_height)?;
                next_height -= 1;
                self.put_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT, next_height)?;
            }
            Ok::<(), String>(())
        })();

        match result {
            Ok(()) => {
                self.clear_rollback_state()?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    // Resume an interrupted rollback using persisted rollback meta state.
    pub fn resume_rollback_if_needed(&self) -> Result<bool, String> {
        if self.get_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS)? != Some(1) {
            return Ok(false);
        }

        let target_height = self
            .get_u32_meta(META_KEY_ROLLBACK_TARGET_HEIGHT)?
            .ok_or_else(|| {
                let msg = "Missing rollback_target_height while rollback_in_progress=1".to_string();
                error!("{}", msg);
                msg
            })?;
        let mut next_height = self
            .get_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT)?
            .ok_or_else(|| {
                let msg = "Missing rollback_next_height while rollback_in_progress=1".to_string();
                error!("{}", msg);
                msg
            })?;
        let current_height = self.get_btc_block_height()?;

        if next_height != current_height {
            let msg = format!(
                "Rollback resume state mismatch: rollback_next_height={}, current_height={}",
                next_height, current_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        if target_height > next_height {
            let msg = format!(
                "Invalid rollback resume state: target_height {} > next_height {}",
                target_height, next_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        while next_height > target_height {
            self.rollback_one_block(next_height)?;
            next_height -= 1;
            self.put_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT, next_height)?;
        }

        self.clear_rollback_state()?;
        Ok(true)
    }

    pub fn prune_undo_before_height(&self, min_retained_height: u32) -> Result<usize, String> {
        if self.get_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS)? == Some(1) {
            warn!(
                "Skipping undo prune while rollback is in progress: min_retained_height={}",
                min_retained_height
            );
            return Ok(0);
        }

        let rollback_supported_from_height = match self.get_rollback_supported_from_height()? {
            Some(height) => height,
            None => return Ok(0),
        };

        let effective_min_retained_height = min_retained_height.max(rollback_supported_from_height);

        if let Some(current_retained_height) = self.get_undo_retained_from_height()? {
            if current_retained_height >= effective_min_retained_height {
                return Ok(0);
            }
        }

        let meta_cf = self.db.cf_handle(BLOCK_UNDO_META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_UNDO_META_CF);
            error!("{}", msg);
            msg
        })?;

        let iter = self.db.iterator_cf(
            meta_cf,
            IteratorMode::From(&0u32.to_be_bytes(), Direction::Forward),
        );

        let mut heights_to_prune = Vec::new();
        for item in iter {
            let (key, _value) = item.map_err(|e| {
                let msg = format!(
                    "Iterator error when scanning block undo meta for pruning: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

            let block_height = Self::parse_u32_be_key(&key)?;
            if block_height >= effective_min_retained_height {
                break;
            }
            heights_to_prune.push(block_height);
        }

        if heights_to_prune.is_empty() {
            self.put_undo_retained_from_height(effective_min_retained_height)?;
            return Ok(0);
        }

        let mut batch = WriteBatch::default();
        for block_height in &heights_to_prune {
            self.append_delete_block_undo_bundle_to_batch(&mut batch, *block_height)?;
        }

        let meta_cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        batch.put_cf(
            meta_cf,
            META_KEY_UNDO_RETAINED_FROM_HEIGHT,
            effective_min_retained_height.to_be_bytes(),
        );

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!(
                "Failed to prune undo journal before height {}: {}",
                effective_min_retained_height, e
            );
            error!("{}", msg);
            msg
        })?;

        info!(
            "Pruned undo journal entries: removed_blocks={}, undo_retained_from_height={}",
            heights_to_prune.len(),
            effective_min_retained_height
        );

        Ok(heights_to_prune.len())
    }

    pub fn put_undo_retained_from_height(&self, height: u32) -> Result<(), String> {
        self.put_u32_meta(META_KEY_UNDO_RETAINED_FROM_HEIGHT, height)
    }

    pub fn get_undo_retained_from_height(&self) -> Result<Option<u32>, String> {
        self.get_u32_meta(META_KEY_UNDO_RETAINED_FROM_HEIGHT)
    }

    pub fn put_rollback_supported_from_height(&self, height: u32) -> Result<(), String> {
        self.put_u32_meta(META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT, height)
    }

    pub fn get_rollback_supported_from_height(&self) -> Result<Option<u32>, String> {
        self.get_u32_meta(META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT)
    }

    fn put_u32_meta(&self, key: &str, value: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        let mut ops = WriteOptions::default();
        ops.set_sync(false);
        self.db
            .put_cf_opt(cf, key, value.to_be_bytes(), &ops)
            .map_err(|e| {
                let msg = format!("Failed to write meta key {}: {}", key, e);
                error!("{}", msg);
                msg
            })
    }

    fn delete_meta_key(&self, key: &str) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        self.db.delete_cf(cf, key).map_err(|e| {
            let msg = format!("Failed to delete meta key {}: {}", key, e);
            error!("{}", msg);
            msg
        })
    }

    fn clear_rollback_state(&self) -> Result<(), String> {
        self.delete_meta_key(META_KEY_ROLLBACK_IN_PROGRESS)?;
        self.delete_meta_key(META_KEY_ROLLBACK_TARGET_HEIGHT)?;
        self.delete_meta_key(META_KEY_ROLLBACK_NEXT_HEIGHT)?;
        Ok(())
    }

    fn get_u32_meta(&self, key: &str) -> Result<Option<u32>, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;
        match self.db.get_cf(cf, key) {
            Ok(Some(value)) => {
                if value.len() != 4 {
                    let msg = format!(
                        "Invalid meta value length for key {}: expected 4, got {}",
                        key,
                        value.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
                Ok(Some(u32::from_be_bytes(
                    value.as_slice().try_into().unwrap(),
                )))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!("Failed to read meta key {}: {}", key, e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    // Read the committed block metadata for one exact block height.
    pub fn get_block_commit(&self, block_height: u32) -> Result<Option<BlockCommitEntry>, String> {
        let cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;

        let key = Self::make_block_commit_key(block_height);
        match self.db.get_cf(cf, key).map_err(|e| {
            let msg = format!(
                "Failed to get block commit for height {}: {}",
                block_height, e
            );
            error!("{}", msg);
            msg
        })? {
            Some(value) => Ok(Some(Self::parse_block_commit_value(block_height, &value)?)),
            None => Ok(None),
        }
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

    /// Get the balance delta for a given script_hash at the target block height if it exists, otherwise return None
    pub fn get_balance_delta_at_block_height(
        &self,
        script_hash: &USDBScriptHash,
        target_height: u32,
    ) -> Result<Option<BalanceHistoryData>, String> {
        // Make the target key
        let target_key = Self::make_balance_history_key(script_hash, target_height);

        let cf = self.db.cf_handle(BALANCE_HISTORY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BALANCE_HISTORY_CF);
            error!("{}", msg);
            msg
        })?;

        // Just do a direct get without iterator since we only want to check if there is a record for the exact block height
        match self.db.get_cf(cf, target_key) {
            Ok(Some(value)) => {
                let (delta, balance) = Self::parse_balance_from_value(&value);
                let entry = BalanceHistoryData {
                    block_height: target_height,
                    delta,
                    balance,
                };
                Ok(Some(entry))
            }
            Ok(None) => Ok(None), // No record found for this height, return None
            Err(e) => {
                let msg = format!("Failed to get balance delta: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
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

    pub fn get_block_commit_count(&self) -> Result<u64, String> {
        get_approx_cf_key_count(&self.db, BLOCK_COMMITS_CF)
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

        info!(
            "Balance history shard {:0x} processed complete",
            shard_index
        );

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

        info!("UTXO shard {:0x} processed complete", shard_index);

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

        info!("Balance history snapshot generation complete");

        Ok(())
    }

    pub fn generate_utxo_snapshot_parallel(&self, cb: SnapshotCallbackRef) -> Result<(), String> {
        use rayon::prelude::*;

        const SHARD_COUNT: u8 = 255;
        const BATCH_SIZE: usize = 1024 * 64;

        (0u8..=SHARD_COUNT)
            .into_par_iter()
            .try_for_each(|shard_index| {
                self.generate_utxo_snapshot_sharded(shard_index, BATCH_SIZE, cb.clone())
            })?;

        info!("UTXO snapshot generation complete");

        Ok(())
    }

    pub fn generate_block_commit_snapshot(
        &self,
        target_block_height: u32,
        cb: SnapshotCallbackRef,
    ) -> Result<(), String> {
        const BATCH_SIZE: usize = 1024 * 64;

        let cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;

        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        let mut snapshot = Vec::with_capacity(BATCH_SIZE);
        let mut entries_processed = 0u64;

        for item in iter {
            let (key, value) = item.map_err(|e| {
                let msg = format!(
                    "Iterator error while generating block commit snapshot: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

            if key.len() != 4 {
                continue;
            }

            let block_height = u32::from_be_bytes(key.as_ref().try_into().unwrap());
            if block_height > target_block_height {
                break;
            }

            snapshot.push(Self::parse_block_commit_value(block_height, &value)?);
            entries_processed += 1;

            if snapshot.len() >= BATCH_SIZE {
                cb.on_block_commit_entries(&snapshot, entries_processed)?;
                snapshot.clear();
                entries_processed = 0;
            }
        }

        if !snapshot.is_empty() {
            cb.on_block_commit_entries(&snapshot, entries_processed)?;
        }

        info!(
            "Block commit snapshot generation complete: target_block_height={}",
            target_block_height
        );

        Ok(())
    }

    pub fn put_block_commits_async(
        &self,
        block_commits: &[BlockCommitEntry],
    ) -> Result<(), String> {
        let mut batch = WriteBatch::default();

        let block_commit_cf = self.db.cf_handle(BLOCK_COMMITS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", BLOCK_COMMITS_CF);
            error!("{}", msg);
            msg
        })?;

        for entry in block_commits {
            let key = Self::make_block_commit_key(entry.block_height);
            let value = Self::serialize_block_commit_value(entry);
            batch.put_cf(block_commit_cf, key, value);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(false);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to write block commits batch to DB: {}", e);
            error!("{}", msg);
            msg
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

    fn clear_column_family(&self, cf_name: &str) -> Result<usize, String> {
        let cf = self.db.cf_handle(cf_name).ok_or_else(|| {
            let msg = format!("Column family {} not found", cf_name);
            error!("{}", msg);
            msg
        })?;

        let mut keys = Vec::new();
        let iter = self.db.iterator_cf(&cf, IteratorMode::Start);
        for item in iter {
            let (key, _) = item.map_err(|e| {
                let msg = format!("Iterator error while clearing {}: {}", cf_name, e);
                error!("{}", msg);
                msg
            })?;
            keys.push(key.to_vec());
        }

        if keys.is_empty() {
            info!("Column family already empty: {}", cf_name);
            return Ok(0);
        }

        let mut batch = WriteBatch::default();
        for key in &keys {
            batch.delete_cf(cf, key);
        }

        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);
        self.db.write_opt(&batch, &write_options).map_err(|e| {
            let msg = format!("Failed to clear column family {}: {}", cf_name, e);
            error!("{}", msg);
            msg
        })?;

        info!(
            "Cleared column family {}: removed_keys={}",
            cf_name,
            keys.len()
        );
        Ok(keys.len())
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

        let removed_blocks = self.clear_column_family(BLOCKS_CF)?;
        let removed_heights = self.clear_column_family(BLOCK_HEIGHTS_CF)?;
        let removed_commits = self.clear_column_family(BLOCK_COMMITS_CF)?;

        if self.get_last_block_file_index()?.is_some() {
            let msg = "Block index clear verification failed: last_block_file_index still exists"
                .to_string();
            error!("{}", msg);
            return Err(msg);
        }

        if !self.get_all_blocks()?.is_empty() {
            let msg = "Block index clear verification failed: blocks CF is not empty".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        if !self.get_all_block_heights()?.is_empty() {
            let msg =
                "Block index clear verification failed: block_heights CF is not empty".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Cleared all persisted block index state: blocks_removed={}, heights_removed={}, commits_removed={}",
            removed_blocks, removed_heights, removed_commits
        );
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

    fn on_block_commit_entries(
        &self,
        entries: &[BlockCommitEntry],
        entries_processed: u64,
    ) -> Result<(), String>;
}

pub type SnapshotCallbackRef = std::sync::Arc<Box<dyn SnapshotCallback>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use std::sync::{Arc, Mutex};
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

        // update_address_history_async writes the stable BTC block height together with history data
        let height = db.get_btc_block_height().unwrap();
        assert_eq!(height, 401);

        // Test put and get BTC block height
        db.put_btc_block_height(123456).unwrap();
        let height = db.get_btc_block_height().unwrap();
        assert_eq!(height, 123456);
    }

    #[test]
    fn test_block_commit_round_trip() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_block_commit_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;

        let config = std::sync::Arc::new(config);
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let block_hash = BlockHash::from_slice(&[3u8; 32]).unwrap();
        let commit = BlockCommitEntry {
            block_height: 42,
            btc_block_hash: block_hash,
            balance_delta_root: [4u8; 32],
            block_commit: [5u8; 32],
        };

        db.update_address_history_with_block_commits_async(&Vec::new(), 42, &[commit.clone()])
            .unwrap();

        let loaded = db.get_block_commit(42).unwrap().unwrap();
        assert_eq!(loaded, commit);
        assert_eq!(db.get_btc_block_height().unwrap(), 42);
    }

    #[test]
    fn test_get_block_commit_returns_none_when_missing() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_block_commit_missing_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;

        let config = std::sync::Arc::new(config);
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        assert!(db.get_block_commit(99).unwrap().is_none());
    }

    #[test]
    fn test_block_commit_multiple_round_trip() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_block_commit_multi_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;

        let config = std::sync::Arc::new(config);
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let first = BlockCommitEntry {
            block_height: 41,
            btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
            balance_delta_root: [2u8; 32],
            block_commit: [3u8; 32],
        };
        let second = BlockCommitEntry {
            block_height: 42,
            btc_block_hash: BlockHash::from_slice(&[4u8; 32]).unwrap(),
            balance_delta_root: [5u8; 32],
            block_commit: [6u8; 32],
        };

        db.update_address_history_with_block_commits_async(
            &Vec::new(),
            42,
            &[first.clone(), second.clone()],
        )
        .unwrap();

        assert_eq!(db.get_block_commit(41).unwrap().unwrap(), first);
        assert_eq!(db.get_block_commit(42).unwrap().unwrap(), second);
        assert_eq!(db.get_btc_block_height().unwrap(), 42);
    }

    #[test]
    fn test_generate_block_commit_snapshot_respects_target_height() {
        #[derive(Clone)]
        struct BlockCommitCollector {
            entries: Arc<Mutex<Vec<BlockCommitEntry>>>,
        }

        impl SnapshotCallback for BlockCommitCollector {
            fn on_balance_history_entries(
                &self,
                _entries: &[BalanceHistoryEntry],
                _entries_processed: u64,
            ) -> Result<(), String> {
                Ok(())
            }

            fn on_utxo_entries(
                &self,
                _entries: &[UTXOEntry],
                _entries_processed: u64,
            ) -> Result<(), String> {
                Ok(())
            }

            fn on_block_commit_entries(
                &self,
                entries: &[BlockCommitEntry],
                _entries_processed: u64,
            ) -> Result<(), String> {
                self.entries.lock().unwrap().extend_from_slice(entries);
                Ok(())
            }
        }

        let mut config = BalanceHistoryConfig::default();
        let temp_dir = std::env::temp_dir().join("balance_history_generate_block_commit_snapshot");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;

        let config = std::sync::Arc::new(config);
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let entries = vec![
            BlockCommitEntry {
                block_height: 10,
                btc_block_hash: BlockHash::from_slice(&[1u8; 32]).unwrap(),
                balance_delta_root: [2u8; 32],
                block_commit: [3u8; 32],
            },
            BlockCommitEntry {
                block_height: 11,
                btc_block_hash: BlockHash::from_slice(&[4u8; 32]).unwrap(),
                balance_delta_root: [5u8; 32],
                block_commit: [6u8; 32],
            },
            BlockCommitEntry {
                block_height: 12,
                btc_block_hash: BlockHash::from_slice(&[7u8; 32]).unwrap(),
                balance_delta_root: [8u8; 32],
                block_commit: [9u8; 32],
            },
        ];
        db.put_block_commits_async(&entries).unwrap();

        let collected = Arc::new(Mutex::new(Vec::new()));
        let cb: SnapshotCallbackRef = Arc::new(Box::new(BlockCommitCollector {
            entries: collected.clone(),
        }));

        db.generate_block_commit_snapshot(11, cb).unwrap();

        let loaded = collected.lock().unwrap().clone();
        assert_eq!(loaded, entries[..2].to_vec());
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

    #[test]
    fn test_clear_blocks_removes_all_block_state() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_clear_blocks_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let block_hash_1 = BlockHash::from_slice(&[7u8; 32]).unwrap();
        let block_hash_2 = BlockHash::from_slice(&[8u8; 32]).unwrap();
        let blocks = vec![
            (
                block_hash_1,
                BlockEntry {
                    block_file_index: 1,
                    block_file_offset: 10,
                    block_record_index: 0,
                },
            ),
            (
                block_hash_2,
                BlockEntry {
                    block_file_index: 2,
                    block_file_offset: 20,
                    block_record_index: 1,
                },
            ),
        ];
        let block_heights = vec![(0, block_hash_1), (1, block_hash_2)];
        db.put_blocks_sync(2, &blocks, &block_heights).unwrap();

        let commit = BlockCommitEntry {
            block_height: 1,
            btc_block_hash: block_hash_2,
            balance_delta_root: [1u8; 32],
            block_commit: [2u8; 32],
        };
        db.update_address_history_with_block_commits_async(&Vec::new(), 1, &[commit])
            .unwrap();

        assert_eq!(db.get_last_block_file_index().unwrap(), Some(2));
        assert_eq!(db.get_all_blocks().unwrap().len(), 2);
        assert_eq!(db.get_all_block_heights().unwrap().len(), 2);
        assert!(db.get_block_commit(1).unwrap().is_some());

        db.clear_blocks().unwrap();

        assert_eq!(db.get_last_block_file_index().unwrap(), None);
        assert!(db.get_all_blocks().unwrap().is_empty());
        assert!(db.get_all_block_heights().unwrap().is_empty());
        assert!(db.get_block_commit(1).unwrap().is_none());
    }

    #[test]
    fn test_update_block_state_async_writes_all_state_atomically() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_atomic_block_state_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let spent_outpoint = OutPoint {
            txid: Txid::from_slice(&[9u8; 32]).unwrap(),
            vout: 1,
        };
        let spent_script = ScriptBuf::from(vec![9u8; 32]);
        let spent_script_hash = spent_script.to_usdb_script_hash();
        db.put_utxo(&spent_outpoint, &spent_script_hash, 900)
            .unwrap();

        let new_outpoint = OutPoint {
            txid: Txid::from_slice(&[8u8; 32]).unwrap(),
            vout: 2,
        };
        let new_script = ScriptBuf::from(vec![8u8; 32]);
        let new_script_hash = new_script.to_usdb_script_hash();
        let new_utxo = Arc::new(UTXOValue {
            script_hash: new_script_hash,
            value: 1800,
        });

        let balance_entry = BalanceHistoryEntry {
            script_hash: new_script_hash,
            block_height: 12,
            delta: 1800,
            balance: 1800,
        };

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[7u8; 32]).unwrap(),
            balance_delta_root: [6u8; 32],
            block_commit: [5u8; 32],
        };

        db.update_block_state_async(
            &[(Arc::new(new_outpoint.clone()), new_utxo.clone())],
            &[Arc::new(spent_outpoint.clone())],
            &[balance_entry.clone()],
            12,
            &[commit.clone()],
        )
        .unwrap();

        assert!(db.get_utxo(&spent_outpoint).unwrap().is_none());

        let loaded_utxo = db.get_utxo(&new_outpoint).unwrap().unwrap();
        assert_eq!(loaded_utxo.script_hash, new_script_hash);
        assert_eq!(loaded_utxo.value, 1800);

        let loaded_balance = db
            .get_balance_delta_at_block_height(&new_script_hash, 12)
            .unwrap()
            .unwrap();
        assert_eq!(loaded_balance.block_height, 12);
        assert_eq!(loaded_balance.delta, 1800);
        assert_eq!(loaded_balance.balance, 1800);

        assert_eq!(db.get_block_commit(12).unwrap().unwrap(), commit);
        assert_eq!(db.get_btc_block_height().unwrap(), 12);
    }

    #[test]
    fn test_block_undo_bundle_round_trip() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_block_undo_round_trip_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let created = BlockUndoUtxoEntry {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[1u8; 32]).unwrap(),
                vout: 2,
            },
            script_hash: ScriptBuf::from(vec![1u8; 32]).to_usdb_script_hash(),
            value: 100,
        };
        let spent = BlockUndoUtxoEntry {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[2u8; 32]).unwrap(),
                vout: 3,
            },
            script_hash: ScriptBuf::from(vec![2u8; 32]).to_usdb_script_hash(),
            value: 200,
        };
        let touched_script_hash = ScriptBuf::from(vec![3u8; 32]).to_usdb_script_hash();
        let bundle = BlockUndoBundle {
            block_height: 88,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            created_utxos: vec![created.clone()],
            spent_utxos: vec![spent.clone()],
            touched_script_hashes: vec![touched_script_hash],
        };

        db.update_block_state_with_undo_async(&[], &[], &[], 88, &[], &[bundle.clone()])
            .unwrap();

        let loaded_meta = db.get_block_undo_meta(88).unwrap().unwrap();
        assert_eq!(loaded_meta.block_height, 88);
        assert_eq!(loaded_meta.btc_block_hash, bundle.btc_block_hash);
        assert_eq!(loaded_meta.created_utxo_count, 1);
        assert_eq!(loaded_meta.spent_utxo_count, 1);
        assert_eq!(loaded_meta.balance_entry_count, 1);

        let loaded_bundle = db.get_block_undo_bundle(88).unwrap().unwrap();
        assert_eq!(loaded_bundle, bundle);
    }

    #[test]
    fn test_undo_meta_height_round_trip() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_undo_meta_height_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        assert_eq!(db.get_undo_retained_from_height().unwrap(), None);
        assert_eq!(db.get_rollback_supported_from_height().unwrap(), None);

        db.put_undo_retained_from_height(120).unwrap();
        db.put_rollback_supported_from_height(80).unwrap();

        assert_eq!(db.get_undo_retained_from_height().unwrap(), Some(120));
        assert_eq!(db.get_rollback_supported_from_height().unwrap(), Some(80));
    }

    #[test]
    fn test_rollback_one_block_reverts_forward_state_and_clears_undo() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_rollback_one_block_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let existing_outpoint = OutPoint {
            txid: Txid::from_slice(&[4u8; 32]).unwrap(),
            vout: 1,
        };
        let existing_script_hash = ScriptBuf::from(vec![4u8; 32]).to_usdb_script_hash();
        db.put_utxo(&existing_outpoint, &existing_script_hash, 400)
            .unwrap();

        let previous_balance = BalanceHistoryEntry {
            script_hash: existing_script_hash,
            block_height: 11,
            delta: 400,
            balance: 400,
        };
        let previous_balances = vec![previous_balance];
        db.update_address_history_with_block_commits_async(&previous_balances, 11, &[])
            .unwrap();

        let new_outpoint = OutPoint {
            txid: Txid::from_slice(&[5u8; 32]).unwrap(),
            vout: 2,
        };
        let new_script_hash = ScriptBuf::from(vec![5u8; 32]).to_usdb_script_hash();
        let new_utxo = Arc::new(UTXOValue {
            script_hash: new_script_hash,
            value: 900,
        });
        let new_balance = BalanceHistoryEntry {
            script_hash: new_script_hash,
            block_height: 12,
            delta: 900,
            balance: 900,
        };
        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[6u8; 32]).unwrap(),
            balance_delta_root: [7u8; 32],
            block_commit: [8u8; 32],
        };
        let undo_bundle = BlockUndoBundle {
            block_height: 12,
            btc_block_hash: commit.btc_block_hash,
            created_utxos: vec![BlockUndoUtxoEntry {
                outpoint: new_outpoint.clone(),
                script_hash: new_script_hash,
                value: 900,
            }],
            spent_utxos: vec![BlockUndoUtxoEntry {
                outpoint: existing_outpoint.clone(),
                script_hash: existing_script_hash,
                value: 400,
            }],
            touched_script_hashes: vec![new_script_hash],
        };

        db.update_block_state_with_undo_async(
            &[(Arc::new(new_outpoint.clone()), new_utxo)],
            &[Arc::new(existing_outpoint.clone())],
            &[new_balance],
            12,
            &[commit.clone()],
            &[undo_bundle],
        )
        .unwrap();

        assert!(db.get_utxo(&existing_outpoint).unwrap().is_none());
        assert!(db.get_utxo(&new_outpoint).unwrap().is_some());
        assert!(db.get_block_commit(12).unwrap().is_some());
        assert!(db.get_block_undo_bundle(12).unwrap().is_some());
        assert_eq!(db.get_btc_block_height().unwrap(), 12);

        db.rollback_one_block(12).unwrap();

        let restored = db.get_utxo(&existing_outpoint).unwrap().unwrap();
        assert_eq!(restored.script_hash, existing_script_hash);
        assert_eq!(restored.value, 400);
        assert!(db.get_utxo(&new_outpoint).unwrap().is_none());
        assert!(
            db.get_balance_delta_at_block_height(&new_script_hash, 12)
                .unwrap()
                .is_none()
        );
        assert!(db.get_block_commit(12).unwrap().is_none());
        assert!(db.get_block_undo_bundle(12).unwrap().is_none());
        assert_eq!(db.get_btc_block_height().unwrap(), 11);
    }

    #[test]
    fn test_rollback_to_block_height_reverts_multiple_blocks_and_clears_meta_state() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_rollback_to_height_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let script_11 = ScriptBuf::from(vec![1u8; 32]).to_usdb_script_hash();
        let entry_11 = BalanceHistoryEntry {
            script_hash: script_11,
            block_height: 11,
            delta: 10,
            balance: 10,
        };
        let entries_11 = vec![entry_11];
        db.update_address_history_with_block_commits_async(&entries_11, 11, &[])
            .unwrap();

        let outpoint_12 = OutPoint {
            txid: Txid::from_slice(&[9u8; 32]).unwrap(),
            vout: 0,
        };
        let script_12 = ScriptBuf::from(vec![2u8; 32]).to_usdb_script_hash();
        let utxo_12 = Arc::new(UTXOValue {
            script_hash: script_12,
            value: 20,
        });
        let entry_12 = BalanceHistoryEntry {
            script_hash: script_12,
            block_height: 12,
            delta: 20,
            balance: 20,
        };
        let commit_12 = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[2u8; 32]).unwrap(),
            balance_delta_root: [2u8; 32],
            block_commit: [2u8; 32],
        };
        let undo_12 = BlockUndoBundle {
            block_height: 12,
            btc_block_hash: commit_12.btc_block_hash,
            created_utxos: vec![BlockUndoUtxoEntry {
                outpoint: outpoint_12.clone(),
                script_hash: script_12,
                value: 20,
            }],
            spent_utxos: Vec::new(),
            touched_script_hashes: vec![script_12],
        };
        db.update_block_state_with_undo_async(
            &[(Arc::new(outpoint_12.clone()), utxo_12)],
            &[],
            &[entry_12],
            12,
            &[commit_12],
            &[undo_12],
        )
        .unwrap();

        let outpoint_13 = OutPoint {
            txid: Txid::from_slice(&[8u8; 32]).unwrap(),
            vout: 1,
        };
        let script_13 = ScriptBuf::from(vec![3u8; 32]).to_usdb_script_hash();
        let utxo_13 = Arc::new(UTXOValue {
            script_hash: script_13,
            value: 30,
        });
        let entry_13 = BalanceHistoryEntry {
            script_hash: script_13,
            block_height: 13,
            delta: 30,
            balance: 30,
        };
        let commit_13 = BlockCommitEntry {
            block_height: 13,
            btc_block_hash: BlockHash::from_slice(&[3u8; 32]).unwrap(),
            balance_delta_root: [3u8; 32],
            block_commit: [3u8; 32],
        };
        let undo_13 = BlockUndoBundle {
            block_height: 13,
            btc_block_hash: commit_13.btc_block_hash,
            created_utxos: vec![BlockUndoUtxoEntry {
                outpoint: outpoint_13.clone(),
                script_hash: script_13,
                value: 30,
            }],
            spent_utxos: Vec::new(),
            touched_script_hashes: vec![script_13],
        };
        db.update_block_state_with_undo_async(
            &[(Arc::new(outpoint_13.clone()), utxo_13)],
            &[],
            &[entry_13],
            13,
            &[commit_13],
            &[undo_13],
        )
        .unwrap();

        db.rollback_to_block_height(11).unwrap();

        assert_eq!(db.get_btc_block_height().unwrap(), 11);
        assert!(db.get_utxo(&outpoint_12).unwrap().is_none());
        assert!(db.get_utxo(&outpoint_13).unwrap().is_none());
        assert!(db.get_block_commit(12).unwrap().is_none());
        assert!(db.get_block_commit(13).unwrap().is_none());
        assert!(db.get_block_undo_bundle(12).unwrap().is_none());
        assert!(db.get_block_undo_bundle(13).unwrap().is_none());
        assert!(
            db.get_balance_delta_at_block_height(&script_12, 12)
                .unwrap()
                .is_none()
        );
        assert!(
            db.get_balance_delta_at_block_height(&script_13, 13)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS).unwrap(),
            None
        );
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_TARGET_HEIGHT).unwrap(),
            None
        );
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT).unwrap(),
            None
        );
    }

    #[test]
    fn test_prune_undo_before_height_removes_only_older_undo_entries() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_prune_undo_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        for height in 10..=12u32 {
            let script_hash = ScriptBuf::from(vec![height as u8; 32]).to_usdb_script_hash();
            let outpoint = OutPoint {
                txid: Txid::from_slice(&[height as u8; 32]).unwrap(),
                vout: height,
            };
            let utxo = Arc::new(UTXOValue {
                script_hash,
                value: height as u64,
            });
            let entry = BalanceHistoryEntry {
                script_hash,
                block_height: height,
                delta: height as i64,
                balance: height as u64,
            };
            let commit = BlockCommitEntry {
                block_height: height,
                btc_block_hash: BlockHash::from_slice(&[height as u8; 32]).unwrap(),
                balance_delta_root: [height as u8; 32],
                block_commit: [height as u8; 32],
            };
            let undo = BlockUndoBundle {
                block_height: height,
                btc_block_hash: commit.btc_block_hash,
                created_utxos: vec![BlockUndoUtxoEntry {
                    outpoint: outpoint.clone(),
                    script_hash,
                    value: height as u64,
                }],
                spent_utxos: Vec::new(),
                touched_script_hashes: vec![script_hash],
            };

            db.update_block_state_with_undo_async(
                &[(Arc::new(outpoint), utxo)],
                &[],
                &[entry],
                height,
                &[commit],
                &[undo],
            )
            .unwrap();
        }

        let removed = db.prune_undo_before_height(12).unwrap();
        assert_eq!(removed, 2);
        assert!(db.get_block_undo_bundle(10).unwrap().is_none());
        assert!(db.get_block_undo_bundle(11).unwrap().is_none());
        assert!(db.get_block_undo_bundle(12).unwrap().is_some());
        assert_eq!(db.get_undo_retained_from_height().unwrap(), Some(12));
    }

    #[test]
    fn test_resume_rollback_if_needed_continues_from_meta_state() {
        let mut config = BalanceHistoryConfig::default();

        let temp_dir = std::env::temp_dir().join("balance_history_resume_rollback_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = std::sync::Arc::new(config);

        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();

        let base_script = ScriptBuf::from(vec![1u8; 32]).to_usdb_script_hash();
        let base_entry = BalanceHistoryEntry {
            script_hash: base_script,
            block_height: 11,
            delta: 11,
            balance: 11,
        };
        let base_entries = vec![base_entry];
        db.update_address_history_with_block_commits_async(&base_entries, 11, &[])
            .unwrap();

        for height in 12..=13u32 {
            let script_hash = ScriptBuf::from(vec![height as u8; 32]).to_usdb_script_hash();
            let outpoint = OutPoint {
                txid: Txid::from_slice(&[height as u8; 32]).unwrap(),
                vout: height,
            };
            let utxo = Arc::new(UTXOValue {
                script_hash,
                value: height as u64,
            });
            let entry = BalanceHistoryEntry {
                script_hash,
                block_height: height,
                delta: height as i64,
                balance: height as u64,
            };
            let commit = BlockCommitEntry {
                block_height: height,
                btc_block_hash: BlockHash::from_slice(&[height as u8; 32]).unwrap(),
                balance_delta_root: [height as u8; 32],
                block_commit: [height as u8; 32],
            };
            let undo = BlockUndoBundle {
                block_height: height,
                btc_block_hash: commit.btc_block_hash,
                created_utxos: vec![BlockUndoUtxoEntry {
                    outpoint: outpoint.clone(),
                    script_hash,
                    value: height as u64,
                }],
                spent_utxos: Vec::new(),
                touched_script_hashes: vec![script_hash],
            };

            db.update_block_state_with_undo_async(
                &[(Arc::new(outpoint.clone()), utxo)],
                &[],
                &[entry],
                height,
                &[commit],
                &[undo],
            )
            .unwrap();
        }

        db.rollback_one_block(13).unwrap();
        db.put_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS, 1).unwrap();
        db.put_u32_meta(META_KEY_ROLLBACK_TARGET_HEIGHT, 11)
            .unwrap();
        db.put_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT, 12).unwrap();

        let resumed = db.resume_rollback_if_needed().unwrap();
        assert!(resumed);
        assert_eq!(db.get_btc_block_height().unwrap(), 11);
        assert!(db.get_block_undo_bundle(12).unwrap().is_none());
        assert!(db.get_block_undo_bundle(13).unwrap().is_none());
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_IN_PROGRESS).unwrap(),
            None
        );
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_TARGET_HEIGHT).unwrap(),
            None
        );
        assert_eq!(
            db.get_u32_meta(META_KEY_ROLLBACK_NEXT_HEIGHT).unwrap(),
            None
        );
    }
}
