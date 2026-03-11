use crate::config::BalanceHistoryConfigRef;
use crate::db::{
    BalanceHistoryDB, BalanceHistoryDBMode, BalanceHistoryDBRef, BalanceHistoryEntry,
    BlockCommitEntry, SnapshotCallback, SnapshotDB, SnapshotHash, SnapshotMeta,
};
use crate::output::IndexOutputRef;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use usdb_util::{USDBScriptHash, UTXOEntry};

pub struct SnapshotIndexer {
    config: BalanceHistoryConfigRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
}

impl SnapshotIndexer {
    pub fn new(
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Self {
        Self { config, db, output }
    }

    pub fn run(&self, target_block_height: u32, with_utxo: bool) -> Result<(), String> {
        info!(
            "Starting snapshot generation up to block height {}, with_utxo={}",
            target_block_height, with_utxo
        );

        // Check that target block height is not greater than last synced BTC block height
        let last_synced_height = self.db.get_btc_block_height()?;
        if target_block_height > last_synced_height {
            let msg = format!(
                "Target block height {} is greater than last synced BTC block height {}",
                target_block_height, last_synced_height
            );
            self.output.eprintln(&msg);
            return Err(msg);
        }

        // If target block height is less than last synced height, some UTXO data may be missing, so we show a warning
        if with_utxo && target_block_height != last_synced_height {
            let msg = format!(
                "Target block height {} is less than last synced BTC block height {}. UTXO data may be incomplete.",
                target_block_height, last_synced_height
            );
            self.output.eprintln(&msg);
        }

        self.output.start_load(0);

        let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);

        self.output.println(&format!(
            "Creating snapshot database at {} height {}, with_utxo={}",
            root_dir.display(),
            target_block_height,
            with_utxo
        ));
        let snapshot_db = SnapshotDB::open_by_height(&root_dir, target_block_height, true)
            .map_err(|e| {
                let msg = format!("Failed to create snapshot database: {}", e);
                self.output.eprintln(&msg);
                msg
            })?;

        let snapshot_db = Arc::new(Mutex::new(snapshot_db));
        let mut snapshot_meta = SnapshotMeta::new(target_block_height);

        // First generate balance history snapshot
        {
            let total = self.db.get_history_balance_count()?;
            self.output.update_load_total_count(total);
            self.output.println(&format!(
                "Will generate balance history snapshot with {} entries at block height {}",
                total, target_block_height
            ));

            let generator = SnapshotGenerator::new(snapshot_db.clone(), self.output.clone());

            let cb = Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>);
            self.db
                .generate_balance_history_snapshot_parallel(target_block_height, cb)?;

            let total_count = generator.balance_history_count.load(Ordering::SeqCst);
            snapshot_meta.balance_history_count = total_count;

            let msg = format!(
                "Completed balance history snapshot generation up to block height {}, total entries: {}",
                target_block_height, total_count
            );
            self.output.println(&msg);
        }

        // Then generate UTXO snapshot if needed
        if with_utxo {
            let total = self.db.get_utxo_count()?;
            self.output.update_load_total_count(total);
            self.output.println(&format!(
                "Will generate UTXO snapshot with {} entries at block height {}",
                total, target_block_height
            ));

            let generator = SnapshotGenerator::new(snapshot_db.clone(), self.output.clone());
            let cb = Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>);
            self.db.generate_utxo_snapshot_parallel(cb)?;

            let total_count = generator.utxo_count.load(Ordering::SeqCst);
            snapshot_meta.utxo_count = total_count;

            let msg = format!(
                "Completed UTXO snapshot generation up to block height {}, total UTXOs: {}",
                target_block_height, total_count
            );

            self.output.println(&msg);
        }

        {
            let estimated_total = self
                .db
                .get_block_commit_count()?
                .min(u64::from(target_block_height) + 1);
            self.output.update_load_total_count(estimated_total);
            self.output.println(&format!(
                "Will generate block commit snapshot up to block height {}",
                target_block_height
            ));

            let generator = SnapshotGenerator::new(snapshot_db.clone(), self.output.clone());
            let cb = Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>);
            self.db.generate_block_commit_snapshot(target_block_height, cb)?;

            let total_count = generator.block_commit_count.load(Ordering::SeqCst);
            snapshot_meta.block_commit_count = total_count;

            let msg = format!(
                "Completed block commit snapshot generation up to block height {}, total commits: {}",
                target_block_height, total_count
            );
            self.output.println(&msg);
        }

        let db_path = snapshot_db.lock().unwrap().path().to_owned();
        self.output.println(&format!(
            "Snapshot database created at {}",
            db_path.display()
        ));

        // Finally, update snapshot meta with counts
        snapshot_db.lock().unwrap().update_meta(&snapshot_meta)?;

        Ok(())
    }
}

#[derive(Clone)]
struct SnapshotGenerator {
    db: Arc<Mutex<SnapshotDB>>,
    count: Arc<AtomicU64>,
    balance_history_count: Arc<AtomicU64>,
    utxo_count: Arc<AtomicU64>,
    block_commit_count: Arc<AtomicU64>,
    output: IndexOutputRef,
}

impl SnapshotGenerator {
    pub fn new(db: Arc<Mutex<SnapshotDB>>, output: IndexOutputRef) -> Self {
        Self {
            db,
            count: Arc::new(AtomicU64::new(0)),
            balance_history_count: Arc::new(AtomicU64::new(0)),
            utxo_count: Arc::new(AtomicU64::new(0)),
            block_commit_count: Arc::new(AtomicU64::new(0)),
            output,
        }
    }
}

impl SnapshotCallback for SnapshotGenerator {
    fn on_balance_history_entries(
        &self,
        entries: &[BalanceHistoryEntry],
        entries_processed: u64,
    ) -> Result<(), String> {
        self.db
            .lock()
            .unwrap()
            .put_balance_history_entries(entries)?;

        // Update the counts
        self.balance_history_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);

        // Use entries_processed to update count
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);

        // Display last entry info
        if let Some(last_entry) = entries.last() {
            self.output.set_load_message(&format!(
                "{}: {} sat @ {}",
                last_entry.script_hash, last_entry.balance, last_entry.block_height,
            ));
        }

        Ok(())
    }

    fn on_utxo_entries(&self, entries: &[UTXOEntry], entries_processed: u64) -> Result<(), String> {
        self.db.lock().unwrap().put_utxo_entries(entries)?;

        // Use entries_processed to update count
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);

        // Update the counts
        self.utxo_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);

        // Display last entry info
        if let Some(last_entry) = entries.last() {
            self.output
                .set_load_message(&format!("{}", last_entry.outpoint,));
        }

        Ok(())
    }

    fn on_block_commit_entries(
        &self,
        entries: &[BlockCommitEntry],
        entries_processed: u64,
    ) -> Result<(), String> {
        self.db.lock().unwrap().put_block_commit_entries(entries)?;

        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);

        self.block_commit_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);

        if let Some(last_entry) = entries.last() {
            self.output.set_load_message(&format!(
                "block_commit@{} {:x}",
                last_entry.block_height, last_entry.btc_block_hash
            ));
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotData {
    pub file: PathBuf,

    // If specified, verify the snapshot file hash before installation
    pub hash: Option<String>,
}

pub struct SnapshotInstaller {
    config: BalanceHistoryConfigRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
}

impl SnapshotInstaller {
    pub fn new(
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Self {
        Self { config, db, output }
    }

    pub fn install(self, data: SnapshotData) -> Result<(), String> {
        info!("Starting snapshot installation from {:?}", data,);

        self.output.start_load(0);

        // First check hash is correct
        if !data.file.exists() {
            let msg = format!("Snapshot file {:?} does not exist", data.file);
            error!("{}", msg);
            return Err(msg);
        }

        if let Some(hash) = data.hash {
            self.output.println("Verifying snapshot file hash...");
            let file_hash = SnapshotHash::calc_hash(&data.file)?;
            if file_hash.to_ascii_lowercase() != hash.to_ascii_lowercase() {
                let msg = format!(
                    "Snapshot file hash mismatch: expected {}, got {}",
                    hash, file_hash
                );
                error!("{}", msg);
                return Err(msg);
            }
        } else {
            self.output
                .println("No snapshot file hash provided, skipping verification");
        }

        let snapshot_db = SnapshotDB::open(&data.file).map_err(|e| {
            let msg = format!("Failed to open snapshot database: {}", e);
            error!("{}", msg);
            msg
        })?;

        let meta = snapshot_db.get_meta().map_err(|e| {
            let msg = format!("Failed to read snapshot metadata: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!("Snapshot metadata: {:?}", meta);
        self.output.println(&format!(
            "Snapshot generated at block height {}, balance history entries: {}, UTXO entries: {}, block commits: {}",
            meta.block_height, meta.balance_history_count, meta.utxo_count, meta.block_commit_count
        ));

        let staging_root = self.prepare_staging_root()?;
        let staging_config = self.make_staging_config(staging_root.clone());
        let staging_db = BalanceHistoryDB::open(staging_config, BalanceHistoryDBMode::BestEffort)
            .map_err(|e| {
                let msg = format!("Failed to initialize staging database: {}", e);
                self.output.println(&msg);
                msg
            })?;

        // Install into staging DB first, then atomically switch the live DB directory.
        self.install_balance_history_snapshot(&staging_db, &snapshot_db, &meta)?;
        self.install_utxo_snapshot(&staging_db, &snapshot_db, &meta)?;
        self.install_block_commit_snapshot(&staging_db, &snapshot_db, &meta)?;

        staging_db
            .put_btc_block_height(meta.block_height)
            .map_err(|e| {
                let msg = format!("Failed to update BTC block height: {}", e);
                self.output.println(&msg);
                msg
            })?;
        staging_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush staging database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        let output = self.output.clone();
        self.swap_staging_db_into_place(staging_db, staging_root)?;

        output.println(&format!(
            "Completed snapshot installation up to block height {}",
            meta.block_height
        ));
        output.finish_load();

        Ok(())
    }

    fn install_balance_history_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &SnapshotDB,
        meta: &SnapshotMeta,
    ) -> Result<(), String> {
        let total = meta.balance_history_count;
        if total == 0 {
            self.output
                .println("No balance history entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing balance history snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        // Load balance by batch
        let page_size = 1024 * 256; // 256k entries per batch
        let mut last_script_hash: Option<USDBScriptHash> = None;
        let mut installed_total = 0u64;
        loop {
            let entries = snapshot_db
                .get_balance_history_entries(page_size, last_script_hash.as_ref())
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot entries: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_address_history_async(&entries).map_err(|e| {
                let msg = format!("Failed to write snapshot entries to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += entries.len() as u64;

            if let Some(last_entry) = entries.last() {
                last_script_hash = Some(last_entry.script_hash.clone());
            }

            self.output.update_load_current_count(installed_total);

            if entries.len() < page_size as usize {
                break;
            }
        }

        assert!(
            installed_total == total,
            "Installed total {} does not match expected total {}",
            installed_total,
            total
        );

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output
            .println("Balance history snapshot installation completed");

        Ok(())
    }

    fn install_utxo_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &SnapshotDB,
        meta: &SnapshotMeta,
    ) -> Result<(), String> {
        let total = meta.utxo_count;
        if total == 0 {
            self.output
                .println("No UTXO entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing UTXO snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        // Load UTXO by batch
        let page_size = 1024 * 256; // 256k entries per batch
        let mut last_outpoint = None;
        let mut installed_total = 0u64;
        loop {
            let utxos = snapshot_db
                .get_utxo_entries(page_size, last_outpoint.as_ref())
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot UTXOs: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_utxos(&utxos).map_err(|e| {
                let msg = format!("Failed to write snapshot UTXOs to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += utxos.len() as u64;

            if let Some(last_utxo) = utxos.last() {
                last_outpoint = Some(last_utxo.outpoint);
            }

            self.output.update_load_current_count(installed_total);

            if utxos.len() < page_size as usize {
                break;
            }
        }

        assert!(
            installed_total == total,
            "Installed UTXOs total {} does not match expected total {}",
            installed_total,
            total
        );

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output.println("UTXO snapshot installation completed");

        Ok(())
    }

    fn install_block_commit_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &SnapshotDB,
        meta: &SnapshotMeta,
    ) -> Result<(), String> {
        let total = meta.block_commit_count;
        if total == 0 {
            self.output
                .println("No block commit entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing block commit snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        let page_size = 1024 * 256;
        let mut last_block_height = None;
        let mut installed_total = 0u64;
        loop {
            let entries = snapshot_db
                .get_block_commit_entries(page_size, last_block_height)
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot block commits: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_block_commits_async(&entries).map_err(|e| {
                let msg = format!("Failed to write snapshot block commits to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += entries.len() as u64;

            if let Some(last_entry) = entries.last() {
                last_block_height = Some(last_entry.block_height);
            }

            self.output.update_load_current_count(installed_total);

            if entries.len() < page_size as usize {
                break;
            }
        }

        assert!(
            installed_total == total,
            "Installed block commits total {} does not match expected total {}",
            installed_total,
            total
        );

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output
            .println("Block commit snapshot installation completed");

        Ok(())
    }

    fn prepare_staging_root(&self) -> Result<PathBuf, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let staging_root = self
            .config
            .root_dir
            .join(format!("snapshot_install_staging_{}", nanos));
        if staging_root.exists() {
            std::fs::remove_dir_all(&staging_root).map_err(|e| {
                let msg = format!(
                    "Failed to clear existing staging root {}: {}",
                    staging_root.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        std::fs::create_dir_all(&staging_root).map_err(|e| {
            let msg = format!(
                "Failed to create staging root {}: {}",
                staging_root.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(staging_root)
    }

    fn make_staging_config(&self, staging_root: PathBuf) -> BalanceHistoryConfigRef {
        let mut cfg = self.config.as_ref().clone();
        cfg.root_dir = staging_root;
        Arc::new(cfg)
    }

    fn swap_staging_db_into_place(
        self,
        staging_db: BalanceHistoryDB,
        staging_root: PathBuf,
    ) -> Result<(), String> {
        let live_db_dir = self.config.db_dir();
        let staged_db_dir = staging_root.join("db");
        let backup_db_dir = self.config.root_dir.join(format!(
            "db_backup_snapshot_install_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));

        if !staged_db_dir.exists() {
            let msg = format!(
                "Staged DB directory does not exist: {}",
                staged_db_dir.display()
            );
            error!("{}", msg);
            return Err(msg);
        }

        staging_db.close();

        let live_db = Arc::try_unwrap(self.db).map_err(|_| {
            let msg = "Failed to acquire exclusive ownership of live DB before snapshot swap"
                .to_string();
            error!("{}", msg);
            msg
        })?;
        live_db.close();

        if live_db_dir.exists() {
            std::fs::rename(&live_db_dir, &backup_db_dir).map_err(|e| {
                let msg = format!(
                    "Failed to move live DB directory {} to backup {}: {}",
                    live_db_dir.display(),
                    backup_db_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        if let Err(e) = std::fs::rename(&staged_db_dir, &live_db_dir) {
            if backup_db_dir.exists() {
                let _ = std::fs::rename(&backup_db_dir, &live_db_dir);
            }
            let msg = format!(
                "Failed to promote staged DB {} to live {}: {}",
                staged_db_dir.display(),
                live_db_dir.display(),
                e
            );
            error!("{}", msg);
            return Err(msg);
        }

        if backup_db_dir.exists() {
            info!(
                "Preserved previous live DB backup after snapshot install: {}",
                backup_db_dir.display()
            );
            self.output.println(&format!(
                "Previous live DB preserved at {}",
                backup_db_dir.display()
            ));
        }

        if staging_root.exists() {
            std::fs::remove_dir_all(&staging_root).map_err(|e| {
                let msg = format!(
                    "Failed to remove staging root {}: {}",
                    staging_root.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use crate::db::{BalanceHistoryDBMode, BlockCommitEntry};
    use crate::output::IndexOutput;
    use crate::status::SyncStatusManager;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid};
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("balance_history_snapshot_{}_{}", tag, nanos));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn test_install_replaces_live_db_with_staged_snapshot() {
        let root_dir = temp_root("install_replace");
        let mut config = BalanceHistoryConfig::default();
        config.root_dir = root_dir.clone();
        let config = Arc::new(config);

        let old_script = ScriptBuf::from(vec![1u8; 32]);
        let old_script_hash = old_script.to_usdb_script_hash();
        let old_outpoint = OutPoint {
            txid: Txid::from_slice(&[2u8; 32]).unwrap(),
            vout: 0,
        };

        let live_db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        let old_commit = BlockCommitEntry {
            block_height: 3,
            btc_block_hash: BlockHash::from_slice(&[7u8; 32]).unwrap(),
            balance_delta_root: [8u8; 32],
            block_commit: [9u8; 32],
        };
        live_db
            .put_address_history_async(&vec![BalanceHistoryEntry {
                script_hash: old_script_hash,
                block_height: 3,
                delta: 50,
                balance: 50,
            }])
            .unwrap();
        live_db.put_utxo(&old_outpoint, &old_script_hash, 50).unwrap();
        live_db.put_block_commits_async(&[old_commit]).unwrap();
        live_db.put_btc_block_height(3).unwrap();
        let live_db = Arc::new(live_db);

        let snapshot_path = root_dir.join("install_source_snapshot.db");
        let new_script = ScriptBuf::from(vec![9u8; 32]);
        let new_script_hash = new_script.to_usdb_script_hash();
        let new_outpoint = OutPoint {
            txid: Txid::from_slice(&[4u8; 32]).unwrap(),
            vout: 1,
        };
        let new_commit = BlockCommitEntry {
            block_height: 10,
            btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
            balance_delta_root: [11u8; 32],
            block_commit: [12u8; 32],
        };

        {
            let mut snapshot_db = SnapshotDB::open(&snapshot_path).unwrap();
            snapshot_db
                .put_balance_history_entries(&[BalanceHistoryEntry {
                    script_hash: new_script_hash,
                    block_height: 10,
                    delta: 75,
                    balance: 75,
                }])
                .unwrap();
            snapshot_db
                .put_utxo_entries(&[UTXOEntry {
                    outpoint: new_outpoint,
                    script_hash: new_script_hash,
                    value: 75,
                }])
                .unwrap();
            snapshot_db
                .put_block_commit_entries(std::slice::from_ref(&new_commit))
                .unwrap();

            let mut meta = SnapshotMeta::new(10);
            meta.balance_history_count = 1;
            meta.utxo_count = 1;
            meta.block_commit_count = 1;
            snapshot_db.update_meta(&meta).unwrap();
        }

        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let installer = SnapshotInstaller::new(config.clone(), live_db, output);
        installer
            .install(SnapshotData {
                file: snapshot_path,
                hash: None,
            })
            .unwrap();

        let reopened_db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 10);

        let old_balance = reopened_db
            .get_balance_delta_at_block_height(&old_script_hash, 3)
            .unwrap();
        assert!(old_balance.is_none(), "old live DB balance entry should be replaced by snapshot");
        assert!(reopened_db.get_utxo(&old_outpoint).unwrap().is_none());
        assert!(reopened_db.get_block_commit(3).unwrap().is_none());

        let new_balance = reopened_db
            .get_balance_delta_at_block_height(&new_script_hash, 10)
            .unwrap()
            .unwrap();
        assert_eq!(new_balance.balance, 75);
        assert_eq!(new_balance.delta, 75);

        let new_utxo = reopened_db.get_utxo(&new_outpoint).unwrap().unwrap();
        assert_eq!(new_utxo.script_hash, new_script_hash);
        assert_eq!(new_utxo.value, 75);

        let installed_commit = reopened_db.get_block_commit(10).unwrap().unwrap();
        assert_eq!(installed_commit, new_commit);

        let staging_dirs: Vec<_> = std::fs::read_dir(&root_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("snapshot_install_staging_"))
            .collect();
        assert!(staging_dirs.is_empty(), "temporary staging directories should be cleaned up");

        let backup_dirs: Vec<_> = std::fs::read_dir(&root_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("db_backup_snapshot_install_"))
            .collect();
        assert_eq!(backup_dirs.len(), 1, "previous live DB backup should be preserved by default");
    }
}
