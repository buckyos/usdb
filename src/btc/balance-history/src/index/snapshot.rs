use crate::config::BalanceHistoryConfigRef;
use crate::db::{
    BalanceHistoryDBRef, BalanceHistoryEntry, SnapshotCallback, SnapshotDB, SnapshotHash,
    SnapshotMeta,
};
use crate::output::IndexOutputRef;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    output: IndexOutputRef,
}

impl SnapshotGenerator {
    pub fn new(db: Arc<Mutex<SnapshotDB>>, output: IndexOutputRef) -> Self {
        Self {
            db,
            count: Arc::new(AtomicU64::new(0)),
            balance_history_count: Arc::new(AtomicU64::new(0)),
            utxo_count: Arc::new(AtomicU64::new(0)),
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

    pub fn install(&self, data: SnapshotData) -> Result<(), String> {
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
            "Snapshot generated at block height {}, balance history entries: {}, UTXO entries: {}",
            meta.block_height, meta.balance_history_count, meta.utxo_count
        ));

        // Install balance history entries
        self.install_balance_history_snapshot(&snapshot_db, &meta)?;

        // Install UTXO entries
        self.install_utxo_snapshot(&snapshot_db, &meta)?;

        self.db
            .put_btc_block_height(meta.block_height)
            .map_err(|e| {
                let msg = format!("Failed to update BTC block height: {}", e);
                self.output.println(&msg);
                msg
            })?;

        self.output.println(&format!(
            "Completed snapshot installation up to block height {}",
            meta.block_height
        ));
        self.output.finish_load();

        Ok(())
    }

    fn install_balance_history_snapshot(
        &self,
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

            self.db.put_address_history_async(&entries).map_err(|e| {
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

        self.db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output.println("Balance history snapshot installation completed");

        Ok(())
    }

    fn install_utxo_snapshot(
        &self,
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

            self.db.put_utxos(&utxos).map_err(|e| {
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

        self.db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output.println("UTXO snapshot installation completed");

        Ok(())
    }
}
