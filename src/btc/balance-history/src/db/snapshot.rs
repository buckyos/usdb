use super::db::BalanceHistoryEntry;
use bitcoincore_rpc::bitcoin::{ScriptHash, hashes::Hash};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// The version of the snapshot database schema
pub const SNAPSHOT_DB_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub snapshot_height: u64,
    pub generated_at: u64, // UNIX timestamp
    pub version: u32,
}

impl SnapshotMeta {
    pub fn new(snapshot_height: u64) -> Self {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        Self {
            snapshot_height,
            generated_at: since_the_epoch.as_secs(),
            version: SNAPSHOT_DB_VERSION,
        }
    }
}

pub struct SnapshotHash;

impl SnapshotHash {
    pub fn calc_hash(path: &Path) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        use std::fs::File;
        use std::io::{BufReader, Read};

        let file = File::open(path).map_err(|e| {
            let msg = format!("Failed to open snapshot file for hashing: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0; 1024 * 64];

        loop {
            let n = reader.read(&mut buffer).map_err(|e| {
                let msg = format!("Failed to read snapshot file for hashing: {}", e);
                error!("{}", msg);
                msg
            })?;

            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        let hash_result = hasher.finalize();
        Ok(format!("{:x}", hash_result))
    }
}

/// Snapshot Manager
pub struct SnapshotDB {
    path: PathBuf,
    conn: Connection,
}

impl SnapshotDB {
    /// Create or open a snapshot database at the specified path
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| {
            let msg = format!("Failed to open connection: {}", e);
            error!("{}", msg);
            msg
        })?;

        // Initialize schema
        conn.execute_batch(include_str!("schema.sql"))
            .map_err(|e| {
                let msg = format!("Failed to execute schema: {}", e);
                error!("{}", msg);
                msg
            })?;

        conn.execute_batch(
            r#"
                PRAGMA journal_mode = WAL;
                PRAGMA synchronous = NORMAL;
                PRAGMA cache_size = -40000;   -- ≈ 80 MB
            "#,
        )
        .map_err(|e| {
            let msg = format!("Failed to set PRAGMA settings: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            path: path.to_path_buf(),
            conn,
        })
    }

    /// Get the current height of the snapshot (returns None if no snapshot exists)
    pub fn current_height(&self) -> Result<Option<u64>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT snapshot_height FROM meta ORDER BY generated_at DESC LIMIT 1")
            .map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                error!("{}", msg);
                msg
            })?;

        let height = stmt
            .query_row([], |row| row.get::<_, Option<i64>>(0))
            .map_err(|e| {
                let msg = format!("Failed to query row: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(height.map(|h| h as u64))
    }

    /// Update the snapshot metadata
    pub fn update_meta(&self, meta: &SnapshotMeta) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta (snapshot_height, generated_at, version) VALUES (?1, ?2, ?3)",
                (
                    meta.snapshot_height as i64,
                    meta.generated_at as i64,
                    meta.version as i64,
                ),
            )
            .map_err(|e| {
                let msg = format!("Failed to update meta: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn put_entries(&mut self, entries: &[BalanceHistoryEntry]) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| {
            let msg = format!("Failed to start transaction: {}", e);
            error!("{}", msg);
            msg
        })?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO balance_history (script_hash, height, balance, delta) VALUES (?1, ?2, ?3, ?4)"
            ).map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                error!("{}", msg);
                msg
            })?;

            for entry in entries {
                stmt.execute((
                    entry.script_hash.as_ref() as &[u8],
                    entry.block_height as i64,
                    entry.balance as i64,
                    entry.delta as i64,
                ))
                .map_err(|e| {
                    let msg = format!("Failed to insert entry: {}", e);
                    error!("{}", msg);
                    msg
                })?;
            }
        }

        tx.commit().map_err(|e| {
            let msg = format!("Failed to commit transaction: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_entries_count(&self) -> Result<u64, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM balance_history")
            .map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                error!("{}", msg);
                msg
            })?;

        let count: u64 = stmt
            .query_row([], |row| row.get::<_, i64>(0).map(|v| v as u64))
            .map_err(|e| {
                let msg = format!("Failed to query row: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(count)
    }

    pub fn get_entries(
        &self,
        page_index: u64,
        page_size: u32,
    ) -> Result<Vec<BalanceHistoryEntry>, String> {
        let offset = page_index * (page_size as u64);
        let mut stmt = self.conn.prepare(
            "SELECT script_hash, height, balance, delta FROM balance_history ORDER BY id LIMIT ?1 OFFSET ?2"
        ).map_err(|e| {
            let msg = format!("Failed to prepare statement: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut entries_iter = stmt.query([page_size as i64, offset as i64]).map_err(|e| {
            let msg = format!("Failed to query map: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut entries = Vec::with_capacity(page_size as usize);
        while let Some(row) = entries_iter.next().map_err(|e| {
            let msg = format!("Failed to get next row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let entry = BalanceHistoryEntry {
                script_hash: {
                    let blob: Vec<u8> = row.get(0).map_err(|e| {
                        let msg = format!("Failed to get script_hash blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?;

                    ScriptHash::from_slice(&blob).map_err(|e| {
                        let msg = format!("Failed to convert script_hash blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?
                },
                block_height: row.get::<_, i64>(1).map_err(|e| {
                    let msg = format!("Failed to get block height: {}", e);
                    error!("{}", msg);
                    msg
                })? as u32,
                balance: row.get::<_, i64>(2).map_err(|e| {
                    let msg = format!("Failed to get balance: {}", e);
                    error!("{}", msg);
                    msg
                })? as u64,
                delta: row.get::<_, i64>(3).map_err(|e| {
                    let msg = format!("Failed to get delta: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            };

            entries.push(entry);
        }

        Ok(entries)
    }
}

pub type SnapshotDBRef = std::sync::Arc<SnapshotDB>;