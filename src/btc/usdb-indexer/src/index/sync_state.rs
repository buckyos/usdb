use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Arc};

pub struct SyncStateStorage {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl SyncStateStorage {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::SYNC_STATE_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!("Failed to open database at {:?}: {}", db_path, e);
            log::error!("{}", msg);
            msg
        })?;

        // Initialize the database schema if necessary
        conn.execute(
            "CREATE TABLE IF NOT EXISTS state (
                    name TEXT PRIMARY KEY,
                    value INTEGER
                )",
            [],
        )
        .map_err(|e| {
            let msg = format!("Failed to create sync_state table: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let storage = Self {
            db_path,
            conn: Mutex::new(conn),
        };

        Ok(storage)
    }

    pub fn get_btc_latest_block_height(&self) -> Result<Option<u32>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare("SELECT value FROM state WHERE name = 'btc_latest_block_height'")
            .map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let height: Option<i64> = stmt
            .query_row([], |row| row.get::<usize, i64>(0))
            .optional()
            .map_err(|e| {
                let msg = format!("Failed to query btc_latest_block_height: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        Ok(height.map(|h| h as u32))
    }

    // Update btc latest block height only block_height = current_block_height + 1 or current_block_height = 0
    pub fn update_btc_latest_block_height(&self, height: u32) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();

        let tx = conn.transaction().map_err(|e| {
            let msg = format!("Failed to start transaction: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        // First get the current height
        let current_height: Option<i64> = tx
            .prepare("SELECT value FROM state WHERE name = 'btc_latest_block_height'")
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).optional())
            .map_err(|e| {
                let msg = format!("Failed to query current btc_latest_block_height: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        if let Some(current) = current_height {
            if height as i64 != current + 1 {
                let msg = format!(
                    "New height {} is not equal to current height {} + 1",
                    height, current
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        // Insert or update the height
        tx.execute(
            "INSERT INTO state (name, value) VALUES ('btc_latest_block_height', ?1)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            [height as i64],
        )
        .map_err(|e| {
            let msg = format!("Failed to update btc_latest_block_height: {}", e);
            error!("{}", msg);
            msg
        })?;

        // Commit the transaction
        tx.commit().map_err(|e| {
            let msg = format!("Failed to commit transaction: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }
}

pub type SyncStateStorageRef = Arc<SyncStateStorage>;