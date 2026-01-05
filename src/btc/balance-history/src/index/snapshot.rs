use crate::config::BalanceHistoryConfigRef;
use crate::db::{
    BalanceHistoryDBRef, BalanceHistoryEntry, SnapshotCallback, SnapshotDB, SnapshotHash,
    SnapshotMeta,
};
use crate::output::IndexOutputRef;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

    pub fn run(&self, target_block_height: u32) -> Result<(), String> {
        info!(
            "Starting snapshot generation up to block height {}",
            target_block_height
        );

        self.output.start_load(0);

        let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);

        self.output.println(&format!(
            "Creating snapshot database at {} height {}",
            root_dir.display(),
            target_block_height
        ));
        let snapshot_db = SnapshotDB::open_by_height(&root_dir, target_block_height, true)
            .map_err(|e| {
                let msg = format!("Failed to create snapshot database: {}", e);
                error!("{}", msg);
                msg
            })?;

        let snapshot_meta = SnapshotMeta::new(target_block_height);
        snapshot_db.update_meta(&snapshot_meta)?;

        let total = self.db.get_history_balance_count()?;
        self.output.update_load_total_count(total);
        self.output
            .println(&format!("Will generate snapshot with {} entries at block height {}", total, target_block_height));

        let generator = SnapshotGenerator {
            db: Arc::new(Mutex::new(snapshot_db)),
            count: Arc::new(AtomicU64::new(0)),
            output: self.output.clone(),
        };
        let cb = Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>);
        self.db
            .generate_snapshot_parallel(target_block_height, cb)?;

        let total_count = generator.db.lock().unwrap().get_entries_count()?;

        let msg = format!(
            "Completed snapshot generation up to block height {}, total entries: {}",
            target_block_height, total_count
        );
        self.output.println(&msg);

        Ok(())
    }
}

#[derive(Clone)]
struct SnapshotGenerator {
    db: Arc<Mutex<SnapshotDB>>,
    count: Arc<AtomicU64>,
    output: IndexOutputRef,
}

impl SnapshotCallback for SnapshotGenerator {
    fn on_snapshot_entries(&self, entries: &[BalanceHistoryEntry], entries_processed: u64) -> Result<(), String> {
        self.db.lock().unwrap().put_entries(entries)?;

        // Use entries_processed to update count
        let count =
            self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
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
            self.output.println("No snapshot file hash provided, skipping verification");
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

        let total = snapshot_db.get_entries_count()?;
        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        // Load balance by batch
        let page_size = 1024 * 256; // 256k entries per batch
        let mut page_index = 0;
        loop {
            let entries = snapshot_db
                .get_entries(page_index, page_size)
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

            let count = (page_index as u64) * (page_size as u64) + (entries.len() as u64);
            self.output.update_load_current_count(count);
            page_index += 1;

            if entries.len() < page_size as usize {
                break;
            }
        }
        self.db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

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
}
