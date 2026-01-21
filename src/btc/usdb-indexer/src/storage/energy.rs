use crate::index::MinerPassState;
use bitcoincore_rpc::bitcoin::{Txid, hashes::Hash};
use ord::InscriptionId;
use rocksdb::{ColumnFamilyDescriptor, DB, Options};
use rust_rocksdb as rocksdb;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use usdb_util::USDBScriptHash;

#[derive(Clone, Debug)]
pub struct PassEnergyKey {
    pub inscription_id: InscriptionId,
    pub block_height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PassEnergyValue {
    pub state: MinerPassState,
    pub active_block_height: u32, // The block height when the pass mint or balance decreased(for active passes)
    pub owner_address: USDBScriptHash,
    pub owner_balance: u64, // In Satoshi at block height
    pub owner_delta: i64,   // In Satoshi at block height
    pub energy: u64,        // Energy balance associated with the pass at block height
}

#[derive(Clone, Debug)]
pub struct PassEnergyRecord {
    pub inscription_id: InscriptionId,
    pub block_height: u32,

    pub state: MinerPassState,
    pub active_block_height: u32, // The block height when the pass mint or balance decreased(for active passes)
    pub owner_address: USDBScriptHash,
    pub owner_balance: u64, // in Satoshi at block height
    pub owner_delta: i64,   // in Satoshi at block height
    pub energy: u64,        // Energy balance associated with the pass at block height
}

// Column family name for pass energy records
const PASS_ENERGY_CF: &str = "pass_energy";

pub struct PassEnergyStorage {
    file: PathBuf,
    db: DB,
}

impl PassEnergyStorage {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::PASS_ENERGY_DB_DIR);

        if db_path.exists() {
            std::fs::create_dir_all(&db_path).map_err(|e| {
                let msg = format!(
                    "Could not create pass energy db directory at {}: {}",
                    db_path.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        // Default options
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        // Define column families
        let cf_descriptors = vec![ColumnFamilyDescriptor::new(
            PASS_ENERGY_CF,
            Options::default(),
        )];

        let db = DB::open_cf_descriptors(&options, &db_path, cf_descriptors).map_err(|e| {
            let msg = format!("Failed to open RocksDB at {}: {}", db_path.display(), e);
            error!("{}", msg);
            msg
        })?;

        info!("Opened Pass Energy RocksDB at {}", db_path.display());

        let ret = Self { file: db_path, db };
        Ok(ret)
    }

    // Just concatenate inscription id and block height as key Txid:LEN + 4 + 4
    fn make_energy_key(
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Vec<u8>, String> {
        let mut bytes = [0u8; Txid::LEN + 4 + 4];
        bytes[..32].copy_from_slice(inscription_id.txid.as_byte_array());
        bytes[32..].copy_from_slice(&inscription_id.index.to_be_bytes());
        bytes[36..].copy_from_slice(&block_height.to_be_bytes());
        Ok(bytes.to_vec())
    }

    fn parse_energy_key(key_bytes: &[u8]) -> Result<PassEnergyKey, String> {
        if key_bytes.len() != Txid::LEN + 4 + 4 {
            let msg = format!(
                "Invalid PassEnergyKey length: {}, expected {}",
                key_bytes.len(),
                Txid::LEN + 4 + 4
            );
            error!("{}", msg);
            return Err(msg);
        }

        let txid = Txid::from_slice(&key_bytes[..32]).map_err(|e| {
            let msg = format!("Failed to parse Txid from PassEnergyKey: {}", e);
            error!("{}", msg);
            msg
        })?;
        let index = u32::from_be_bytes(key_bytes[32..36].try_into().map_err(|e| {
            let msg = format!("Failed to parse index from PassEnergyKey: {}", e);
            error!("{}", msg);
            msg
        })?);
        let block_height = u32::from_be_bytes(key_bytes[36..40].try_into().map_err(|e| {
            let msg = format!("Failed to parse block height from PassEnergyKey: {}", e);
            error!("{}", msg);
            msg
        })?);

        Ok(PassEnergyKey {
            inscription_id: InscriptionId { txid, index },
            block_height,
        })
    }

    pub fn insert_pass_energy_record(&self, record: &PassEnergyRecord) -> Result<(), String> {
        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let key_bytes = Self::make_energy_key(&record.inscription_id, record.block_height)?;
        let value = PassEnergyValue {
            state: record.state.clone(),
            active_block_height: record.active_block_height,
            owner_address: record.owner_address,
            owner_balance: record.owner_balance,
            owner_delta: record.owner_delta,
            energy: record.energy,
        };

        let value_bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
            .map_err(|e| {
                let msg = format!("Failed to serialize PassEnergyValue: {}", e);
                error!("{}", msg);
                msg
            })?;

        self.db.put_cf(cf, key_bytes, value_bytes).map_err(|e| {
            let msg = format!("Failed to insert pass energy record: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Get pass energy record for given inscription id at specific block height
    pub fn get_pass_energy_record(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyValue>, String> {
        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let key_bytes = Self::make_energy_key(inscription_id, block_height)?;

        match self.db.get_cf(cf, key_bytes).map_err(|e| {
            let msg = format!("Failed to get pass energy record: {}", e);
            error!("{}", msg);
            msg
        })? {
            Some(value_bytes) => {
                let (value, _) =
                    bincode::serde::decode_from_slice(&value_bytes, bincode::config::standard())
                        .map_err(|e| {
                            let msg = format!("Failed to deserialize PassEnergyValue: {}", e);
                            error!("{}", msg);
                            msg
                        })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    // Find the last pass energy record for given inscription id before or at from_block_height
    pub fn find_last_pass_energy_record(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
    ) -> Result<Option<PassEnergyRecord>, String> {
        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let max_key = Self::make_energy_key(inscription_id, from_block_height)?;
        let mut iter = self.db.iterator_cf(
            cf,
            rocksdb::IteratorMode::From(&max_key, rocksdb::Direction::Reverse),
        );

        let mut last_record: Option<PassEnergyRecord> = None;

        while let Some(item) = iter.next() {
            let (key_bytes, value_bytes) = item.map_err(|e| {
                let msg = format!("Failed to iterate pass energy records: {}", e);
                error!("{}", msg);
                msg
            })?;

            let key = Self::parse_energy_key(&key_bytes)?;

            if key.inscription_id == *inscription_id && key.block_height <= from_block_height {
                let (value, _): (PassEnergyValue, _) =
                    bincode::serde::decode_from_slice(&value_bytes, bincode::config::standard())
                        .map_err(|e| {
                            let msg = format!("Failed to deserialize PassEnergyValue: {}", e);
                            error!("{}", msg);
                            msg
                        })?;

                last_record = Some(PassEnergyRecord {
                    inscription_id: key.inscription_id.clone(),
                    block_height: key.block_height,
                    state: value.state,
                    active_block_height: value.active_block_height,
                    owner_address: value.owner_address,
                    owner_balance: value.owner_balance,
                    owner_delta: value.owner_delta,
                    energy: value.energy,
                });
            }
        }

        Ok(last_record)
    }
}
