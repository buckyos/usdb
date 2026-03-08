use crate::index::MinerPassState;
use bitcoincore_rpc::bitcoin::{Txid, hashes::Hash};
use ord::InscriptionId;
use rocksdb::{ColumnFamilyDescriptor, DB, Options};
use rust_rocksdb::{self as rocksdb};
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
const META_CF: &str = "meta";

// Highest block height that has fully finalized energy writes.
const META_KEY_SYNCED_BLOCK_HEIGHT: &[u8] = b"synced_block_height";
// Block height currently in-progress for energy writes (crash-recovery marker).
const META_KEY_PENDING_BLOCK_HEIGHT: &[u8] = b"pending_block_height";
// Cached maximum block height currently present in pass_energy CF.
const META_KEY_MAX_RECORD_BLOCK_HEIGHT: &[u8] = b"max_record_block_height";

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
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(PASS_ENERGY_CF, Options::default()),
            ColumnFamilyDescriptor::new(META_CF, Options::default()),
        ];

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
        bytes[32..36].copy_from_slice(&inscription_id.index.to_be_bytes());
        bytes[36..40].copy_from_slice(&block_height.to_be_bytes());
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
        let energy_cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;
        let meta_cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
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

        // Persist record and update max block height marker in one write batch.
        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(energy_cf, key_bytes, value_bytes);
        let current_max = self.get_meta_u32(META_KEY_MAX_RECORD_BLOCK_HEIGHT)?;
        if match current_max {
            Some(h) => record.block_height > h,
            None => true,
        } {
            batch.put_cf(
                meta_cf,
                META_KEY_MAX_RECORD_BLOCK_HEIGHT,
                record.block_height.to_be_bytes(),
            );
        }

        self.db.write(&batch).map_err(|e| {
            let msg = format!(
                "Failed to insert pass energy record with metadata update: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn get_meta_u32(&self, key: &[u8]) -> Result<Option<u32>, String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        match self.db.get_cf(cf, key).map_err(|e| {
            let msg = format!("Failed to read energy meta key {:?}: {}", key, e);
            error!("{}", msg);
            msg
        })? {
            Some(bytes) => {
                if bytes.len() != 4 {
                    let msg = format!(
                        "Invalid energy meta value length for key {:?}: expected 4, got {}",
                        key,
                        bytes.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn set_meta_u32(&self, key: &[u8], value: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        self.db.put_cf(cf, key, value.to_be_bytes()).map_err(|e| {
            let msg = format!(
                "Failed to persist energy meta key {:?} with value {}: {}",
                key, value, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn delete_meta_key(&self, key: &[u8]) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        self.db.delete_cf(cf, key).map_err(|e| {
            let msg = format!("Failed to delete energy meta key {:?}: {}", key, e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_synced_block_height(&self) -> Result<Option<u32>, String> {
        self.get_meta_u32(META_KEY_SYNCED_BLOCK_HEIGHT)
    }

    pub fn set_synced_block_height(&self, block_height: u32) -> Result<(), String> {
        self.set_meta_u32(META_KEY_SYNCED_BLOCK_HEIGHT, block_height)
    }

    pub fn get_pending_block_height(&self) -> Result<Option<u32>, String> {
        self.get_meta_u32(META_KEY_PENDING_BLOCK_HEIGHT)
    }

    pub fn set_pending_block_height(&self, block_height: u32) -> Result<(), String> {
        self.set_meta_u32(META_KEY_PENDING_BLOCK_HEIGHT, block_height)
    }

    pub fn clear_pending_block_height(&self) -> Result<(), String> {
        self.delete_meta_key(META_KEY_PENDING_BLOCK_HEIGHT)
    }

    pub fn finalize_block_sync(&self, block_height: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(META_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", META_CF);
            error!("{}", msg);
            msg
        })?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(cf, META_KEY_SYNCED_BLOCK_HEIGHT, block_height.to_be_bytes());
        batch.delete_cf(cf, META_KEY_PENDING_BLOCK_HEIGHT);

        self.db.write(&batch).map_err(|e| {
            let msg = format!(
                "Failed to finalize energy block sync metadata at height {}: {}",
                block_height, e
            );
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
                break;
            }
        }

        Ok(last_record)
    }

    pub fn get_pass_energy_records_by_page_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
        to_block_height: u32,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<PassEnergyRecord>, String> {
        if from_block_height > to_block_height {
            let msg = format!(
                "Invalid energy height range: from_block_height {} > to_block_height {}",
                from_block_height, to_block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let offset = page.checked_mul(page_size).ok_or_else(|| {
            let msg = format!(
                "Pagination overflow when querying pass energy range: page={}, page_size={}",
                page, page_size
            );
            error!("{}", msg);
            msg
        })?;

        let start_key = Self::make_energy_key(inscription_id, from_block_height)?;
        let mut iter = self.db.iterator_cf(
            cf,
            rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );

        let mut skipped = 0usize;
        let mut records = Vec::new();

        while let Some(item) = iter.next() {
            let (key_bytes, value_bytes) = item.map_err(|e| {
                let msg = format!("Failed to iterate pass energy records by range: {}", e);
                error!("{}", msg);
                msg
            })?;

            let key = Self::parse_energy_key(&key_bytes)?;

            // Records are ordered by (inscription_id, block_height). Once inscription changes,
            // all following records are out of this query scope.
            if key.inscription_id != *inscription_id {
                break;
            }

            if key.block_height > to_block_height {
                break;
            }

            if skipped < offset {
                skipped += 1;
                continue;
            }

            let (value, _): (PassEnergyValue, _) =
                bincode::serde::decode_from_slice(&value_bytes, bincode::config::standard())
                    .map_err(|e| {
                        let msg = format!("Failed to deserialize PassEnergyValue: {}", e);
                        error!("{}", msg);
                        msg
                    })?;

            records.push(PassEnergyRecord {
                inscription_id: key.inscription_id.clone(),
                block_height: key.block_height,
                state: value.state,
                active_block_height: value.active_block_height,
                owner_address: value.owner_address,
                owner_balance: value.owner_balance,
                owner_delta: value.owner_delta,
                energy: value.energy,
            });

            if records.len() >= page_size {
                break;
            }
        }

        Ok(records)
    }

    pub fn count_pass_energy_records_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
        to_block_height: u32,
    ) -> Result<u64, String> {
        if from_block_height > to_block_height {
            let msg = format!(
                "Invalid energy height range: from_block_height {} > to_block_height {}",
                from_block_height, to_block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let start_key = Self::make_energy_key(inscription_id, from_block_height)?;
        let mut iter = self.db.iterator_cf(
            cf,
            rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );

        let mut count: u64 = 0;
        while let Some(item) = iter.next() {
            let (key_bytes, _value_bytes) = item.map_err(|e| {
                let msg = format!(
                    "Failed to iterate pass energy records by range count: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

            let key = Self::parse_energy_key(&key_bytes)?;
            if key.inscription_id != *inscription_id {
                break;
            }
            if key.block_height > to_block_height {
                break;
            }

            count = count.checked_add(1).ok_or_else(|| {
                let msg = format!(
                    "Pass energy record count overflow: inscription_id={}, from_block_height={}, to_block_height={}",
                    inscription_id, from_block_height, to_block_height
                );
                error!("{}", msg);
                msg
            })?;
        }

        Ok(count)
    }

    // Clear all pass energy records from given block height (inclusive)
    pub fn clear_records_from_height(&self, from_block_height: u32) -> Result<(), String> {
        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut iter = self.db.iterator_cf(cf, rocksdb::IteratorMode::Start);

        let mut keys_to_delete: Vec<Vec<u8>> = Vec::new();
        let mut max_kept_height: Option<u32> = None;

        while let Some(item) = iter.next() {
            let (key_bytes, _value_bytes) = item.map_err(|e| {
                let msg = format!("Failed to iterate pass energy records: {}", e);
                error!("{}", msg);
                msg
            })?;

            let key = Self::parse_energy_key(&key_bytes)?;

            if key.block_height >= from_block_height {
                keys_to_delete.push(key_bytes.to_vec());
            } else {
                max_kept_height = Some(match max_kept_height {
                    Some(current) => current.max(key.block_height),
                    None => key.block_height,
                });
            }
        }

        let mut delete_batch = rocksdb::WriteBatch::default();
        for key in keys_to_delete {
            delete_batch.delete_cf(cf, key);
        }

        self.db.write(&delete_batch).map_err(|e| {
            let msg = format!("Failed to delete pass energy records: {}", e);
            error!("{}", msg);
            msg
        })?;

        // The iterator above has already seen all records, so we can update max marker
        // directly without an extra full-CF scan.
        match max_kept_height {
            Some(height) => self.set_meta_u32(META_KEY_MAX_RECORD_BLOCK_HEIGHT, height)?,
            None => self.delete_meta_key(META_KEY_MAX_RECORD_BLOCK_HEIGHT)?,
        }

        Ok(())
    }

    pub fn get_max_record_block_height(&self) -> Result<Option<u32>, String> {
        // Fast path: read cached max height from metadata.
        if let Some(height) = self.get_meta_u32(META_KEY_MAX_RECORD_BLOCK_HEIGHT)? {
            return Ok(Some(height));
        }

        // Backfill path for older DB versions without metadata.
        let scanned = self.scan_max_record_block_height()?;
        if let Some(height) = scanned {
            self.set_meta_u32(META_KEY_MAX_RECORD_BLOCK_HEIGHT, height)?;
        }
        Ok(scanned)
    }

    fn scan_max_record_block_height(&self) -> Result<Option<u32>, String> {
        let cf = self.db.cf_handle(PASS_ENERGY_CF).ok_or_else(|| {
            let msg = format!("Column family {} not found", PASS_ENERGY_CF);
            error!("{}", msg);
            msg
        })?;

        let mut iter = self
            .db
            .iterator_cf(cf, rocksdb::IteratorMode::Start)
            .peekable();
        let mut max_height: Option<u32> = None;

        while let Some(item) = iter.next() {
            let (key_bytes, _value_bytes) = item.map_err(|e| {
                let msg = format!("Failed to iterate pass energy records: {}", e);
                error!("{}", msg);
                msg
            })?;

            let key = Self::parse_energy_key(&key_bytes)?;
            max_height = Some(match max_height {
                Some(current) => current.max(key.block_height),
                None => key.block_height,
            });
        }

        Ok(max_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    fn test_data_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("usdb_pass_energy_storage_{tag}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script_hash(tag: u8) -> USDBScriptHash {
        let script = ScriptBuf::from(vec![tag; 32]);
        script.to_usdb_script_hash()
    }

    fn inscription_id(tag: u8, index: u32) -> InscriptionId {
        InscriptionId {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            index,
        }
    }

    fn make_record(
        ins_tag: u8,
        index: u32,
        block_height: u32,
        owner_tag: u8,
        energy: u64,
    ) -> PassEnergyRecord {
        PassEnergyRecord {
            inscription_id: inscription_id(ins_tag, index),
            block_height,
            state: MinerPassState::Active,
            active_block_height: block_height,
            owner_address: script_hash(owner_tag),
            owner_balance: 10_000 + block_height as u64,
            owner_delta: block_height as i64,
            energy,
        }
    }

    #[test]
    fn test_pass_energy_insert_and_get_roundtrip() {
        let dir = test_data_dir("roundtrip");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        let record = make_record(10, 0, 120, 1, 777);
        storage.insert_pass_energy_record(&record).unwrap();

        let loaded = storage
            .get_pass_energy_record(&record.inscription_id, record.block_height)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, MinerPassState::Active);
        assert_eq!(loaded.active_block_height, 120);
        assert_eq!(loaded.owner_address, script_hash(1));
        assert_eq!(loaded.owner_balance, 10_120);
        assert_eq!(loaded.owner_delta, 120);
        assert_eq!(loaded.energy, 777);

        let missing = storage
            .get_pass_energy_record(&record.inscription_id, record.block_height + 1)
            .unwrap();
        assert!(missing.is_none());

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_last_pass_energy_record_returns_latest_before_height() {
        let dir = test_data_dir("last_record");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        let record_100 = make_record(20, 0, 100, 2, 1000);
        let record_120 = make_record(20, 0, 120, 2, 1200);
        let record_140 = make_record(20, 0, 140, 2, 1400);
        storage.insert_pass_energy_record(&record_100).unwrap();
        storage.insert_pass_energy_record(&record_120).unwrap();
        storage.insert_pass_energy_record(&record_140).unwrap();

        let last = storage
            .find_last_pass_energy_record(&record_100.inscription_id, 130)
            .unwrap()
            .unwrap();
        assert_eq!(last.block_height, 120);
        assert_eq!(last.energy, 1200);

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_clear_records_from_height_inclusive() {
        let dir = test_data_dir("clear_height");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        let keep_150 = make_record(30, 0, 150, 3, 1500);
        let clear_160 = make_record(30, 0, 160, 3, 1600);
        let clear_200 = make_record(30, 0, 200, 3, 2000);
        let other_clear_170 = make_record(31, 1, 170, 4, 1700);
        storage.insert_pass_energy_record(&keep_150).unwrap();
        storage.insert_pass_energy_record(&clear_160).unwrap();
        storage.insert_pass_energy_record(&clear_200).unwrap();
        storage.insert_pass_energy_record(&other_clear_170).unwrap();

        storage.clear_records_from_height(160).unwrap();

        let kept = storage
            .get_pass_energy_record(&keep_150.inscription_id, keep_150.block_height)
            .unwrap();
        assert!(kept.is_some());

        let removed_160 = storage
            .get_pass_energy_record(&clear_160.inscription_id, clear_160.block_height)
            .unwrap();
        assert!(removed_160.is_none());

        let removed_200 = storage
            .get_pass_energy_record(&clear_200.inscription_id, clear_200.block_height)
            .unwrap();
        assert!(removed_200.is_none());

        let removed_other = storage
            .get_pass_energy_record(
                &other_clear_170.inscription_id,
                other_clear_170.block_height,
            )
            .unwrap();
        assert!(removed_other.is_none());

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_block_sync_meta_pending_and_finalize_roundtrip() {
        let dir = test_data_dir("meta_roundtrip");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        assert_eq!(storage.get_pending_block_height().unwrap(), None);
        assert_eq!(storage.get_synced_block_height().unwrap(), None);

        storage.set_pending_block_height(123).unwrap();
        assert_eq!(storage.get_pending_block_height().unwrap(), Some(123));

        storage.finalize_block_sync(123).unwrap();
        assert_eq!(storage.get_pending_block_height().unwrap(), None);
        assert_eq!(storage.get_synced_block_height().unwrap(), Some(123));

        storage.set_pending_block_height(124).unwrap();
        assert_eq!(storage.get_pending_block_height().unwrap(), Some(124));
        storage.clear_pending_block_height().unwrap();
        assert_eq!(storage.get_pending_block_height().unwrap(), None);

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_get_max_record_block_height() {
        let dir = test_data_dir("max_height");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        assert_eq!(storage.get_max_record_block_height().unwrap(), None);

        storage
            .insert_pass_energy_record(&make_record(40, 0, 120, 5, 1))
            .unwrap();
        storage
            .insert_pass_energy_record(&make_record(41, 1, 300, 6, 2))
            .unwrap();
        storage
            .insert_pass_energy_record(&make_record(42, 2, 250, 7, 3))
            .unwrap();

        assert_eq!(storage.get_max_record_block_height().unwrap(), Some(300));

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_records_by_page_in_height_range() {
        let dir = test_data_dir("range_page");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        let target = inscription_id(50, 0);
        let other = inscription_id(51, 1);
        for height in [100, 110, 120, 130] {
            storage
                .insert_pass_energy_record(&PassEnergyRecord {
                    inscription_id: target.clone(),
                    block_height: height,
                    state: MinerPassState::Active,
                    active_block_height: height,
                    owner_address: script_hash(8),
                    owner_balance: 1_000 + height as u64,
                    owner_delta: 1,
                    energy: height as u64,
                })
                .unwrap();
        }
        storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: other,
                block_height: 115,
                state: MinerPassState::Active,
                active_block_height: 115,
                owner_address: script_hash(9),
                owner_balance: 2_000,
                owner_delta: 2,
                energy: 115,
            })
            .unwrap();

        let page0 = storage
            .get_pass_energy_records_by_page_in_height_range(&target, 100, 130, 0, 2)
            .unwrap();
        assert_eq!(page0.len(), 2);
        assert_eq!(page0[0].block_height, 100);
        assert_eq!(page0[1].block_height, 110);

        let page1 = storage
            .get_pass_energy_records_by_page_in_height_range(&target, 100, 130, 1, 2)
            .unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].block_height, 120);
        assert_eq!(page1[1].block_height, 130);

        let page2 = storage
            .get_pass_energy_records_by_page_in_height_range(&target, 100, 130, 2, 2)
            .unwrap();
        assert!(page2.is_empty());

        let err = storage
            .get_pass_energy_records_by_page_in_height_range(&target, 130, 100, 0, 10)
            .unwrap_err();
        assert!(err.contains("Invalid energy height range"));

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_count_pass_energy_records_in_height_range() {
        let dir = test_data_dir("range_count");
        let storage = PassEnergyStorage::new(&dir).unwrap();

        let target = inscription_id(60, 0);
        for height in [100, 110, 120, 130] {
            storage
                .insert_pass_energy_record(&PassEnergyRecord {
                    inscription_id: target.clone(),
                    block_height: height,
                    state: MinerPassState::Active,
                    active_block_height: height,
                    owner_address: script_hash(7),
                    owner_balance: 1_000 + height as u64,
                    owner_delta: 1,
                    energy: height as u64,
                })
                .unwrap();
        }

        let another = inscription_id(61, 0);
        storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: another,
                block_height: 115,
                state: MinerPassState::Active,
                active_block_height: 115,
                owner_address: script_hash(8),
                owner_balance: 2_000,
                owner_delta: 2,
                energy: 115,
            })
            .unwrap();

        let count = storage
            .count_pass_energy_records_in_height_range(&target, 100, 125)
            .unwrap();
        assert_eq!(count, 3);

        let empty = storage
            .count_pass_energy_records_in_height_range(&target, 131, 140)
            .unwrap();
        assert_eq!(empty, 0);

        let err = storage
            .count_pass_energy_records_in_height_range(&target, 130, 100)
            .unwrap_err();
        assert!(err.contains("Invalid energy height range"));

        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
