use bitcoincore_rpc::bitcoin::{ScriptBuf, ScriptHash};
use rocksdb::{ColumnFamilyDescriptor, DB, Options, WriteBatch};
use rust_rocksdb as rocksdb;
use std::path::{Path, PathBuf};

// Address mapping column family ScriptHash -> ScriptBuf
pub const ADDRESS_CF: &str = "address";

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

        let file = db_dir.join("address_db");
        info!("Opening RocksDB at {}", file.display());

        // Default options
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let mut address_cf_options = Options::default();
        address_cf_options.set_level_compaction_dynamic_level_bytes(true);
        address_cf_options.set_compaction_style(rocksdb::DBCompactionStyle::Level);
        address_cf_options.create_if_missing(true);
        address_cf_options.set_write_buffer_size(256 * 1024 * 1024);           // 256MB
        address_cf_options.set_max_write_buffer_number(8);
        address_cf_options.set_min_write_buffer_number_to_merge(3);
        address_cf_options.set_max_bytes_for_level_base(4 * 1024 * 1024 * 1024); // 4GB
        address_cf_options.set_target_file_size_base(64 * 1024 * 1024);
        address_cf_options.set_compression_type(rocksdb::DBCompressionType::Lz4);

        // Define column families
        let cf_descriptors = vec![ColumnFamilyDescriptor::new(ADDRESS_CF, address_cf_options)];

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

    pub fn put_addresses(&self, list: &Vec<(ScriptHash, ScriptBuf)>) -> Result<(), String> {
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
    
    pub fn get_address(&self, script_hash: &ScriptHash) -> Result<Option<ScriptBuf>, String> {
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
