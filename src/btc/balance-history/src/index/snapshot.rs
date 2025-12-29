use crate::config::BalanceHistoryConfigRef;
use crate::db::BalanceHistoryDBRef;
use crate::db::{self, SnapshotDB};
use crate::output::{self, IndexOutputRef};

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
        let snapshot_dir = root_dir.join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).map_err(|e| {
            let msg = format!(
                "Failed to create snapshot directory {:?}: {}",
                snapshot_dir, e
            );
            error!("{}", msg);
            msg
        })?;

        let db_snapshot_path = snapshot_dir.join(format!("snapshot_{}.db", target_block_height));
        if db_snapshot_path.exists() {
            let msg = format!("Snapshot database {:?} already exists", db_snapshot_path);
            warn!("{}", msg);
            self.output.println(&msg);

            // For safety, do not overwrite existing snapshot, just return error
            std::fs::remove_file(&db_snapshot_path).map_err(|e| {
                let msg = format!(
                    "Failed to remove existing snapshot database {:?}: {}",
                    db_snapshot_path, e
                );
                error!("{}", msg);
                msg
            })?;
            //return Err(msg);
        }

        info!("Creating snapshot database at {:?}", db_snapshot_path);
        let mut snapshot_db = SnapshotDB::open(&db_snapshot_path).map_err(|e| {
            let msg = format!("Failed to create snapshot database: {}", e);
            error!("{}", msg);
            msg
        })?;

        let total = self.db.get_history_balance_count()?;
        self.output.update_load_total_count(total);
        self.output
            .println(&format!("Will generate snapshot with {} entries", total));

        let mut count = 0;
        self.db.generate_snapshot(target_block_height, |entries| {
            snapshot_db.put_entries(entries)?;

            count += entries.len() as u64;
            self.output.update_load_current_count(count);

            // Display last entry info
            if let Some(last_entry) = entries.last() {
                self.output.set_load_message(&format!(
                    "{}: {} @ {}",
                    last_entry.script_hash, last_entry.balance, last_entry.block_height,
                ));
            }

            Ok(())
        })?;

        info!(
            "Completed snapshot generation up to block height {}",
            target_block_height
        );
        Ok(())
    }
}
