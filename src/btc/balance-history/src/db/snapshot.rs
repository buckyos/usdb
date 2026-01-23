use super::db::BalanceHistoryEntry;
use bitcoincore_rpc::bitcoin::OutPoint;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use usdb_util::{OutPointCodec, USDBScriptHash, UTXOEntry};

// The version of the snapshot database schema
pub const SNAPSHOT_DB_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct SnapshotMeta {
    pub block_height: u32,
    pub balance_history_count: u64,
    pub utxo_count: u64,
    pub generated_at: u64, // UNIX timestamp
    pub version: u32,
}

impl SnapshotMeta {
    pub fn new(block_height: u32) -> Self {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        Self {
            block_height,
            balance_history_count: 0,
            utxo_count: 0,
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

    pub fn open_by_height(
        root_dir: &Path,
        block_height: u32,
        create_new: bool,
    ) -> Result<Self, String> {
        let snapshot_dir = root_dir.join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).map_err(|e| {
            let msg = format!(
                "Failed to create snapshot directory {:?}: {}",
                snapshot_dir, e
            );
            error!("{}", msg);
            msg
        })?;

        let db_path = snapshot_dir.join(format!("snapshot_{}.db", block_height));
        if create_new {
            if db_path.exists() {
                let msg = format!("Snapshot database {:?} already exists", db_path);
                warn!("{}", msg);

                // For safety, rename existing snapshot to old file
                let old_db_path = snapshot_dir.join(format!(
                    "snapshot_{}_{}.db",
                    block_height,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                ));
                std::fs::rename(&db_path, &old_db_path).map_err(|e| {
                    let msg = format!(
                        "Failed to rename existing snapshot database {:?} to {:?}: {}",
                        db_path, old_db_path, e
                    );
                    error!("{}", msg);
                    msg
                })?;
            }
        } else {
            if !db_path.exists() {
                let msg = format!("Snapshot database {:?} does not exist", db_path);
                warn!("{}", msg);
                return Err(msg);
            }
        }

        Self::open(&db_path)
    }

    /// Get the current block height of the snapshot (returns None if no snapshot exists)
    pub fn current_block_height(&self) -> Result<Option<u64>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT block_height FROM meta ORDER BY generated_at DESC LIMIT 1")
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
                "INSERT INTO meta (block_height, balance_history_count, utxo_count, generated_at, version) VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    meta.block_height as i64,
                    meta.balance_history_count as i64,
                    meta.utxo_count as i64,
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

    pub fn get_meta(&self) -> Result<SnapshotMeta, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT block_height, balance_history_count, utxo_count, generated_at, version FROM meta ORDER BY generated_at DESC LIMIT 1",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                error!("{}", msg);
                msg
            })?;

        let meta = stmt
            .query_row([], |row| {
                Ok(SnapshotMeta {
                    block_height: row.get::<_, i64>(0).map(|v| v as u32)?,
                    balance_history_count: row.get::<_, i64>(1).map(|v| v as u64)?,
                    utxo_count: row.get::<_, i64>(2).map(|v| v as u64)?,
                    generated_at: row.get::<_, i64>(3).map(|v| v as u64)?,
                    version: row.get::<_, i64>(4).map(|v| v as u32)?,
                })
            })
            .map_err(|e| {
                let msg = format!("Failed to query row: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(meta)
    }

    pub fn put_balance_history_entries(
        &mut self,
        entries: &[BalanceHistoryEntry],
    ) -> Result<(), String> {
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

    pub fn put_utxo_entries(&mut self, entries: &[UTXOEntry]) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| {
            let msg = format!("Failed to start transaction: {}", e);
            error!("{}", msg);
            msg
        })?;

        {
            let mut stmt = tx
                .prepare("INSERT INTO utxos (outpoint, script_hash, value) VALUES (?1, ?2, ?3)")
                .map_err(|e| {
                    let msg = format!("Failed to prepare statement: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            for entry in entries {
                stmt.execute((
                    entry.outpoint_vec(),
                    entry.script_hash.as_ref() as &[u8],
                    entry.value as i64,
                ))
                .map_err(|e| {
                    let msg = format!("Failed to insert UTXO entry: {}", e);
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

    pub fn stat_balance_history_entries_count(&self) -> Result<u64, String> {
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

    pub fn get_balance_history_entry(
        &self,
        script_hash: &USDBScriptHash,
    ) -> Result<Option<BalanceHistoryEntry>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT script_hash, height, balance, delta FROM balance_history WHERE script_hash = ?1"
        ).map_err(|e| {
            let msg = format!("Failed to prepare statement: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut rows = stmt.query([script_hash.as_ref() as &[u8]]).map_err(|e| {
            let msg = format!("Failed to query map: {}", e);
            error!("{}", msg);
            msg
        })?;

        if let Some(row) = rows.next().map_err(|e| {
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

                    USDBScriptHash::from_slice(&blob).map_err(|e| {
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

            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    pub fn get_balance_history_entries_by_page(
        &self,
        page_offset: u32,
        page_size: u32,
    ) -> Result<Vec<BalanceHistoryEntry>, String> {
        let sql = "
            SELECT script_hash, height, balance, delta 
            FROM balance_history 
            ORDER BY script_hash ASC 
            LIMIT ?1 OFFSET ?2
        ";

        let mut stmt = self.conn.prepare(sql).map_err(|e| {
            let msg = format!("Failed to prepare statement: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut entries_iter = stmt
            .query(rusqlite::params![
                page_size as i64,
                (page_offset as i64) * (page_size as i64),
            ])
            .map_err(|e| {
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

                    USDBScriptHash::from_slice(&blob).map_err(|e| {
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

    pub fn get_balance_history_entries(
        &self,
        page_size: u32,
        last_script_hash: Option<&USDBScriptHash>,
    ) -> Result<Vec<BalanceHistoryEntry>, String> {
        let sql = match last_script_hash {
            Some(_) => {
                "
                SELECT script_hash, height, balance, delta 
                FROM balance_history 
                WHERE script_hash > ?1 
                ORDER BY script_hash ASC 
                LIMIT ?2
                "
            }
            None => {
                "
                SELECT script_hash, height, balance, delta 
                FROM balance_history 
                ORDER BY script_hash ASC 
                LIMIT ?1
                "
            }
        };

        let params = match last_script_hash {
            Some(last_script_hash) => {
                rusqlite::params![last_script_hash.as_ref() as &[u8], page_size as i64,]
            }
            None => rusqlite::params![page_size as i64],
        };

        let mut stmt = self.conn.prepare(sql).map_err(|e| {
            let msg = format!("Failed to prepare statement: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut entries_iter = stmt.query(params).map_err(|e| {
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

                    USDBScriptHash::from_slice(&blob).map_err(|e| {
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

    pub fn stat_utxo_entries_count(&self) -> Result<u64, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM utxos")
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

    pub fn get_utxo_entries(
        &self,
        page_size: u32,
        last_outpoint: Option<&OutPoint>,
    ) -> Result<Vec<UTXOEntry>, String> {
        let sql = match last_outpoint {
            Some(_) => {
                "
                SELECT outpoint, script_hash, value 
                FROM utxos 
                WHERE outpoint > ?1 
                ORDER BY outpoint ASC 
                LIMIT ?2
                "
            }
            None => {
                "
                SELECT outpoint, script_hash, value 
                FROM utxos 
                ORDER BY outpoint ASC 
                LIMIT ?1
                "
            }
        };

        let params = match last_outpoint {
            Some(last_outpoint) => {
                rusqlite::params![OutPointCodec::encode(last_outpoint), page_size as i64,]
            }
            None => rusqlite::params![page_size as i64],
        };

        let mut stmt = self.conn.prepare(sql).map_err(|e| {
            let msg = format!("Failed to prepare statement: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut entries_iter = stmt.query(params).map_err(|e| {
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
            let entry = UTXOEntry {
                outpoint: {
                    let blob: Vec<u8> = row.get(0).map_err(|e| {
                        let msg = format!("Failed to get outpoint blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?;

                    OutPointCodec::decode(&blob).map_err(|e| {
                        let msg = format!("Failed to convert outpoint blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?
                },
                script_hash: {
                    let blob: Vec<u8> = row.get(1).map_err(|e| {
                        let msg = format!("Failed to get script_hash blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?;

                    USDBScriptHash::from_slice(&blob).map_err(|e| {
                        let msg = format!("Failed to convert script_hash blob: {}", e);
                        error!("{}", msg);
                        msg
                    })?
                },
                value: row.get::<_, i64>(2).map_err(|e| {
                    let msg = format!("Failed to get value: {}", e);
                    error!("{}", msg);
                    msg
                })? as u64,
            };
            entries.push(entry);
        }

        Ok(entries)
    }
}

pub type SnapshotDBRef = std::sync::Arc<SnapshotDB>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;

    #[test]
    fn test_snapshot_db_creation() {
        let dir = std::env::temp_dir().join("usdb").join("test_snapshot");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("snapshot_test.db");
        if db_path.exists() {
            std::fs::remove_file(&db_path).unwrap();
        }

        let snapshot_db = SnapshotDB::open(&db_path).unwrap();
        let meta = SnapshotMeta::new(100);
        snapshot_db.update_meta(&meta).unwrap();

        let retrieved_meta = snapshot_db.get_meta().unwrap();
        assert_eq!(retrieved_meta.block_height, 100);
        assert_eq!(retrieved_meta.version, SNAPSHOT_DB_VERSION);
    }

    #[test]
    fn test_load() {
        let config = BalanceHistoryConfig::default();
        let target_block_height = 900_000;
        // let file_name = format!("snapshot_{}.db", target_block_height);
        // let dir = config.snapshot_dir().join(file_name);
        let snapshot_db = SnapshotDB::open_by_height(&config.root_dir, target_block_height, false)
            .expect("Failed to load snapshot DB");

        let script_hash = usdb_util::parse_script_hash(
            "1ab30e67c2f1cdfe77c5e47bc458e3f12ab6acc95778f5f26db5396d6647cd89",
        )
        .unwrap();

        let entry = snapshot_db
            .get_balance_history_entry(&script_hash)
            .expect("Entry not found");
        println!(
            "Entry at height {}: balance={}, delta={}",
            entry.block_height, entry.balance, entry.delta
        );
    }
}
