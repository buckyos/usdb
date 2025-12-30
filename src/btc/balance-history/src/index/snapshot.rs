use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{BalanceHistoryDBRef, BalanceHistoryEntry, SnapshotCallback, SnapshotDB, SnapshotMeta};
use crate::output::IndexOutputRef;

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
        
        info!("Creating snapshot database at {} height {}", root_dir.display(), target_block_height);
        self.output.println(&format!(
            "Creating snapshot database at {} height {}",
            root_dir.display(),
            target_block_height
        ));
        let snapshot_db = SnapshotDB::open_by_height(&root_dir, target_block_height, true).map_err(|e| {
            let msg = format!("Failed to create snapshot database: {}", e);
            error!("{}", msg);
            msg
        })?;

        let snapshot_meta = SnapshotMeta::new(target_block_height as u64);
        snapshot_db.update_meta(&snapshot_meta)?;

        let total = self.db.get_history_balance_count()?;
        self.output.update_load_total_count(total);
        self.output
            .println(&format!("Will generate snapshot with {} entries", total));

        let generator = SnapshotGenerator {
            db: Arc::new(Mutex::new(snapshot_db)),
            count: Arc::new(AtomicU64::new(0)),
            output: self.output.clone(),
        };
        let cb = Arc::new(Box::new(generator) as Box<dyn SnapshotCallback>);
        self.db
            .generate_snapshot_parallel(target_block_height, cb)?;

        info!(
            "Completed snapshot generation up to block height {}",
            target_block_height
        );
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
    fn on_snapshot_entries(&self, entries: &[BalanceHistoryEntry]) -> Result<(), String> {
        self.db.lock().unwrap().put_entries(entries)?;

        let count = self.count.fetch_add(entries.len() as u64, Ordering::SeqCst) + entries.len() as u64;
        self.output.update_load_current_count(count);

        // Display last entry info
        if let Some(last_entry) = entries.last() {
            self.output.set_load_message(&format!(
                "{}: {} @ {}",
                last_entry.script_hash, last_entry.balance, last_entry.block_height,
            ));
        }

        Ok(())
    }
}
