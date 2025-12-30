use bitcoincore_rpc::bitcoin::{ScriptBuf, ScriptHash};
use rocksdb::{ColumnFamilyDescriptor, DB, Options, WriteBatch};
use rust_rocksdb as rocksdb;
use std::path::{Path, PathBuf};
use usdb_util::USDBScriptHash;

// Address mapping column family USDBScriptHash -> ScriptBuf
pub const ADDRESS_CF: &str = "address";

// File index column family BlockFileIndex -> boolean
pub const FILE_CF: &str = "file";

// To store metadata like last indexed block height
pub const META_CF: &str = "meta";

pub const META_FILE_INDEXED: &str = "file_indexed";
pub const META_LAST_INDEXED_BLOCK_HEIGHT: &str = "last_indexed_block_height";

pub struct AddressDB {
    file: PathBuf,
    db: DB,
}

impl AddressDB {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
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

        let file = db_dir.join("address");
        info!("Opening RocksDB at {}", file.display());

        // Default options
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let mut address_cf_options = Options::default();
        address_cf_options.set_level_compaction_dynamic_level_bytes(true);
        address_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        address_cf_options.create_if_missing(true);
        address_cf_options.set_write_buffer_size(256 * 1024 * 1024); // 256MB
        address_cf_options.set_max_write_buffer_number(8);
        address_cf_options.set_min_write_buffer_number_to_merge(3);
        address_cf_options.set_max_bytes_for_level_base(4 * 1024 * 1024 * 1024); // 4GB
        address_cf_options.set_target_file_size_base(64 * 1024 * 1024);
        address_cf_options.set_compression_type(rocksdb::DBCompressionType::Lz4);


        // Define column families
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(ADDRESS_CF, address_cf_options),
            ColumnFamilyDescriptor::new(FILE_CF, Options::default()),
            ColumnFamilyDescriptor::new(META_CF, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&options, &file, cf_descriptors).map_err(|e| {
            let msg = format!("Failed to open RocksDB at {}: {}", file.display(), e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self { file, db })
    }

    pub fn get_db_dir(data_dir: &Path) -> PathBuf {
        let db_dir = data_dir.join("db");
        db_dir
    }

    pub fn set_file_indexed(&self, file_index: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(FILE_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", FILE_CF);
            error!("{}", msg);
            msg
        })?;

        let mut write_opts = rocksdb::WriteOptions::default();
        write_opts.set_sync(true);

        self.db
            .put_cf(
                cf,
                &file_index.to_le_bytes() as &[u8],
                &1u8.to_le_bytes() as &[u8],
            )
            .map_err(|e| {
                let msg = format!("Failed to mark file {} as indexed: {}", file_index, e);
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn is_file_indexed(&self, file_index: u32) -> Result<bool, String> {
        let cf = self.db.cf_handle(FILE_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", FILE_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, &file_index.to_le_bytes() as &[u8]) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => {
                let msg = format!("Failed to check if file {} is indexed: {}", file_index, e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub fn set_all_file_indexed(&self) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut write_opts = rocksdb::WriteOptions::default();
        write_opts.set_sync(true);

        self.db
            .put_cf(
                cf,
                META_FILE_INDEXED.as_bytes(),
                &1u8.to_le_bytes() as &[u8],
            )
            .map_err(|e| {
                let msg = format!("Failed to mark all files as indexed: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn is_all_file_indexed(&self) -> Result<bool, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, META_FILE_INDEXED.as_bytes()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => {
                let msg = format!("Failed to check if all files are indexed: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub fn set_indexed_block_height(&self, height: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut write_opts = rocksdb::WriteOptions::default();
        write_opts.set_sync(true);

        self.db
            .put_cf(
                cf,
                META_LAST_INDEXED_BLOCK_HEIGHT.as_bytes(),
                &height.to_le_bytes() as &[u8],
            )
            .map_err(|e| {
                let msg = format!("Failed to set last indexed block height {}: {}", height, e);
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn get_indexed_block_height(&self) -> Result<Option<u32>, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, META_LAST_INDEXED_BLOCK_HEIGHT.as_bytes()) {
            Ok(Some(value)) => {
                if value.len() != 4 {
                    let msg = format!(
                        "Invalid BTC block height value length: {}",
                        value.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
                
                let height = u32::from_le_bytes((value.as_ref() as &[u8]).try_into().unwrap());
                Ok(Some(height))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!("Failed to get last indexed block height: {}", e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub fn put_addresses(&self, list: &Vec<(USDBScriptHash, ScriptBuf)>) -> Result<(), String> {
        let cf = self.db.cf_handle(ADDRESS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", ADDRESS_CF);
            error!("{}", msg);
            msg
        })?;

        let mut write_opts = rocksdb::WriteOptions::default();
        write_opts.set_sync(false);

        let mut batch = WriteBatch::default();

        for (script_hash, script) in list {
            batch.put_cf(cf, script_hash.as_ref() as &[u8], script.as_bytes());
        }

        self.db.write_opt(&batch, &write_opts).map_err(|e| {
            let msg = format!("Failed to write batch of addresses: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn flush(&self) -> Result<(), String> {
        self.db.flush().map_err(|e| {
            let msg = format!("Failed to flush RocksDB: {}", e);
            error!("{}", msg);
            msg
        })
    }

    pub fn get_address(&self, script_hash: &USDBScriptHash) -> Result<Option<ScriptBuf>, String> {
        let cf = self.db.cf_handle(ADDRESS_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", ADDRESS_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, script_hash.as_ref() as &[u8]) {
            Ok(Some(value)) => {
                let script = ScriptBuf::from(value);
                Ok(Some(script))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                let msg = format!(
                    "Failed to get address for script hash {}: {}",
                    script_hash, e
                );
                error!("{}", msg);
                Err(msg)
            }
        }
    }
}

pub type AddressDBRef = std::sync::Arc<AddressDB>;
