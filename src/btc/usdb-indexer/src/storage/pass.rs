use crate::index::MinerPassState;
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use ordinals::SatPoint;
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use usdb_util::USDBScriptHash;

// Key for storing the last synced BTC block height
const BTC_SYNCED_BLOCK_HEIGHT_KEY: &str = "btc_synced_block_height";

// Default savepoint name for miner pass operations
const SAVEPOINT_MINER_PASS_OPS: &str = "miner_pass_ops";

pub struct MinerPassInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,

    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: USDBScriptHash, // The owner address who minted the pass

    pub satpoint: SatPoint, // The satpoint of the inscription, maybe changed after transfer

    // The content fields of the pass
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<InscriptionId>,

    // Current owner address of the pass, when the pass is transferred,
    // the owner changes and state changed to Dormant by default
    pub owner: USDBScriptHash,
    pub state: MinerPassState,
}

#[derive(Clone, Debug)]
pub struct ValidMinerPassInfo {
    pub inscription_id: InscriptionId,
    pub owner: USDBScriptHash,
    pub satpoint: SatPoint,
}

#[derive(Clone, Debug)]
pub struct ActiveMinerPassInfo {
    pub inscription_id: InscriptionId,
    pub owner: USDBScriptHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBalanceSnapshot {
    pub block_height: u32,
    pub total_balance: u64,
    pub active_address_count: u32,
}

pub struct MinerPassStorage {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl MinerPassStorage {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::MINER_PASS_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!(
                "Failed to open MinerPassStorage database at {:?}: {}",
                db_path, e
            );
            error!("{}", msg);
            msg
        })?;

        // Init the database
        let storage = MinerPassStorage {
            db_path,
            conn: Mutex::new(conn),
        };
        storage.init_db()?;

        Ok(storage)
    }

    fn init_db(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS state (
                name TEXT PRIMARY KEY,
                value INTEGER
            );

            CREATE TABLE IF NOT EXISTS miner_passes (
                inscription_id TEXT NOT NULL PRIMARY KEY,
                inscription_number INTEGER NOT NULL,

                mint_txid TEXT NOT NULL,
                mint_block_height INTEGER NOT NULL,
                mint_owner TEXT NOT NULL,

                satpoint TEXT NOT NULL,

                eth_main TEXT NOT NULL,
                eth_collab TEXT,
                prev TEXT NOT NULL,

                owner TEXT NOT NULL,
                state TEXT NOT NULL,

                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_miner_pass_owner_state
            ON miner_passes (owner, state);

            CREATE INDEX IF NOT EXISTS idx_miner_pass_eth_main
            ON miner_passes (eth_main);

            CREATE TABLE IF NOT EXISTS active_balance_snapshots (
                block_height INTEGER PRIMARY KEY,
                total_balance INTEGER NOT NULL,
                active_address_count INTEGER NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )
        .map_err(|e| {
            let msg = format!("Failed to initialize MinerPassStorage database: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut stmt = conn
            .prepare(
                "
            SELECT
                owner,
                COUNT(*) AS active_count
            FROM miner_passes
            WHERE state = ?1
            GROUP BY owner
            HAVING COUNT(*) > 1
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare duplicate active owner check statement: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![MinerPassState::Active.as_str()])
            .map_err(|e| {
                let msg = format!("Failed to query duplicate active owners: {}", e);
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to read duplicate active owner row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let owner: String = row.get(0).map_err(|e| {
                let msg = format!(
                    "Failed to get owner field from duplicate active owner row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let active_count: i64 = row.get(1).map_err(|e| {
                let msg = format!(
                    "Failed to get active_count field from duplicate active owner row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

            let msg = format!(
                "Duplicate active miner pass owners detected before enforcing unique constraint: owner={}, active_count={}",
                owner, active_count
            );
            error!("{}", msg);
            return Err(msg);
        }

        conn.execute(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS idx_miner_pass_owner_active_unique
            ON miner_passes (owner)
            WHERE state = 'active';
            ",
            [],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to create unique index for active miner pass owner: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn savepoint_begin(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(&format!("SAVEPOINT {}", SAVEPOINT_MINER_PASS_OPS), [])
            .map_err(|e| {
                let msg = format!(
                    "Failed to begin savepoint {}: {}",
                    SAVEPOINT_MINER_PASS_OPS, e
                );
                error!("{}", msg);
                msg
            })?;

        Ok(())
    }

    pub fn savepoint_commit(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!("RELEASE SAVEPOINT {}", SAVEPOINT_MINER_PASS_OPS),
            [],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to commit savepoint {}: {}",
                SAVEPOINT_MINER_PASS_OPS, e
            );
            error!("{}", msg);
            msg
        })?;
        Ok(())
    }

    pub fn savepoint_rollback(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!("ROLLBACK TO SAVEPOINT {}", SAVEPOINT_MINER_PASS_OPS),
            [],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to rollback savepoint {}: {}",
                SAVEPOINT_MINER_PASS_OPS, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Update btc synced block height
    pub fn update_synced_btc_block_height(&self, height: u32) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "
            INSERT INTO state (name, value)
            VALUES (?1, ?2)
            ON CONFLICT(name) DO UPDATE SET value = excluded.value;
            ",
            rusqlite::params![BTC_SYNCED_BLOCK_HEIGHT_KEY, height as i64],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to update btc_synced_block_height in database: {}",
                e
            );
            log::error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    // Get last synced btc block height
    pub fn get_synced_btc_block_height(&self) -> Result<Option<u32>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare("SELECT value FROM state WHERE name = ?1")
            .map_err(|e| {
                let msg = format!("Failed to prepare statement: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let height: Option<i64> = stmt
            .query_row([BTC_SYNCED_BLOCK_HEIGHT_KEY], |row| {
                row.get::<usize, i64>(0)
            })
            .optional()
            .map_err(|e| {
                let msg = format!("Failed to query btc_synced_block_height: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        Ok(height.map(|h| h as u32))
    }

    pub fn upsert_active_balance_snapshot(
        &self,
        block_height: u32,
        total_balance: u64,
        active_address_count: u32,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "
            INSERT INTO active_balance_snapshots (block_height, total_balance, active_address_count)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(block_height) DO UPDATE SET
                total_balance = excluded.total_balance,
                active_address_count = excluded.active_address_count;
            ",
            rusqlite::params![
                block_height as i64,
                total_balance as i64,
                active_address_count as i64,
            ],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to upsert active balance snapshot at block height {}: {}",
                block_height, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_active_balance_snapshot(
        &self,
        block_height: u32,
    ) -> Result<Option<ActiveBalanceSnapshot>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "
            SELECT
                block_height,
                total_balance,
                active_address_count
            FROM active_balance_snapshots
            WHERE block_height = ?1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get active balance snapshot by block height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query active balance snapshot at block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying active balance snapshot by block height: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let item = Self::row_to_active_balance_snapshot(&row)?;
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }

    pub fn get_latest_active_balance_snapshot(
        &self,
    ) -> Result<Option<ActiveBalanceSnapshot>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "
            SELECT
                block_height,
                total_balance,
                active_address_count
            FROM active_balance_snapshots
            ORDER BY block_height DESC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get latest active balance snapshot: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt.query([]).map_err(|e| {
            let msg = format!("Failed to query latest active balance snapshot: {}", e);
            error!("{}", msg);
            msg
        })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying latest active balance snapshot: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let item = Self::row_to_active_balance_snapshot(&row)?;
            Ok(Some(item))
        } else {
            Ok(None)
        }
    }

    pub fn clear_active_balance_snapshots_from_height(
        &self,
        from_block_height: u32,
    ) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();

        let affected = conn
            .execute(
                "
            DELETE FROM active_balance_snapshots
            WHERE block_height >= ?1;
            ",
                rusqlite::params![from_block_height as i64],
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to clear active balance snapshots from block height {}: {}",
                    from_block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        Ok(affected)
    }

    pub fn add_new_mint_pass(&self, pass_info: &MinerPassInfo) -> Result<(), String> {
        assert!(
            pass_info.state == MinerPassState::Active,
            "Newly minted pass must be in Active state: id {}, state: {:?}",
            pass_info.inscription_id,
            pass_info.state.as_str()
        );
        assert!(
            pass_info.owner == pass_info.mint_owner,
            "Newly minted pass owner must be the mint owner {} != {}",
            pass_info.owner,
            pass_info.mint_owner
        );

        let prev_serialized = pass_info
            .prev
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<String>>()
            .join(",");

        let conn = self.conn.lock().unwrap();

        conn.execute(
            "
            INSERT INTO miner_passes (
                inscription_id,
                inscription_number,

                mint_txid,
                mint_block_height,
                mint_owner,

                satpoint,
                
                eth_main,
                eth_collab,
                prev,

                owner,
                state
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11);
            ",
            rusqlite::params![
                pass_info.inscription_id.to_string(),
                pass_info.inscription_number,
                pass_info.mint_txid.to_string(),
                pass_info.mint_block_height as i64,
                pass_info.mint_owner.to_string(),
                pass_info.satpoint.to_string(),
                pass_info.eth_main,
                pass_info.eth_collab,
                prev_serialized,
                pass_info.owner.to_string(),
                pass_info.state.as_str(),
            ],
        )
        .map_err(|e| {
            let msg = format!("Failed to insert new miner pass into database: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!(
            "Added new miner pass with inscription_id {} to owner {}",
            pass_info.inscription_id, pass_info.owner
        );

        Ok(())
    }

    /// Transfer the ownership of a miner pass to a new owner
    pub fn transfer_owner(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &USDBScriptHash,
        new_satpoint: &SatPoint,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        let affected = conn
            .execute(
                "
            UPDATE miner_passes
            SET owner = ?1, satpoint = ?2
            WHERE inscription_id = ?3;
            ",
                rusqlite::params![
                    new_owner.to_string(),
                    new_satpoint.to_string(),
                    inscription_id.to_string(),
                ],
            )
            .map_err(|e| {
                let msg = format!("Failed to transfer miner pass owner in database: {}", e);
                error!("{}", msg);
                msg
            })?;

        if affected == 0 {
            let msg = format!(
                "No miner pass found with inscription_id {} to transfer ownership to {}",
                inscription_id, new_owner
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Transferred miner pass {} ownership to {}",
            inscription_id, new_owner
        );

        Ok(())
    }

    // Update the satpoint of a miner pass where its inscription_id and current satpoint match
    pub fn update_satpoint(
        &self,
        inscription_id: &InscriptionId,
        prev_satpoint: &SatPoint,
        new_satpoint: &SatPoint,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        let affected = conn
            .execute(
                "
            UPDATE miner_passes
            SET satpoint = ?1
            WHERE inscription_id = ?2 AND satpoint = ?3;
            ",
                rusqlite::params![
                    new_satpoint.to_string(),
                    inscription_id.to_string(),
                    prev_satpoint.to_string(),
                ],
            )
            .map_err(|e| {
                let msg = format!("Failed to update miner pass satpoint in database: {}", e);
                error!("{}", msg);
                msg
            })?;

        if affected == 0 {
            let msg = format!(
                "No miner pass found with inscription_id {} and satpoint {} to update to new satpoint {}",
                inscription_id, prev_satpoint, new_satpoint
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Updated miner pass {} satpoint from {} to {}",
            inscription_id, prev_satpoint, new_satpoint
        );

        Ok(())
    }

    /// Update the state of a miner pass, only if its current state matches prev_state
    pub fn update_state(
        &self,
        inscription_id: &InscriptionId,
        new_state: MinerPassState,
        prev_state: MinerPassState,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        let affected = conn
            .execute(
                "
            UPDATE miner_passes
            SET state = ?1
            WHERE inscription_id = ?2 AND state = ?3;
            ",
                rusqlite::params![
                    new_state.as_str(),
                    inscription_id.to_string(),
                    prev_state.as_str(),
                ],
            )
            .map_err(|e| {
                let msg = format!("Failed to update miner pass state in database: {}", e);
                error!("{}", msg);
                msg
            })?;

        if affected == 0 {
            let msg = format!(
                "No miner pass found with inscription_id {} and state {} to update to new state {}",
                inscription_id,
                prev_state.as_str(),
                new_state.as_str()
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Updated miner pass {} state from {} to {}",
            inscription_id,
            prev_state.as_str(),
            new_state.as_str()
        );

        Ok(())
    }

    fn row_to_pass_item(row: &rusqlite::Row) -> Result<MinerPassInfo, String> {
        let prev_serialized: String = row.get(8).map_err(|e| {
            let msg = format!("Failed to get prev field from miner pass row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let prev_ids = if prev_serialized.is_empty() {
            Vec::new()
        } else {
            prev_serialized
                .split(',')
                .map(|s| s.parse::<InscriptionId>())
                .collect::<Result<Vec<InscriptionId>, _>>()
                .map_err(|e| {
                    let msg = format!(
                        "Failed to parse prev inscription IDs from serialized string: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?
        };

        Ok(MinerPassInfo {
            inscription_id: row
                .get::<_, String>(0)
                .map_err(|e| {
                    let msg = format!(
                        "Failed to get inscription_id field from miner pass row: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse inscription_id from string: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            inscription_number: row.get(1).map_err(|e| {
                let msg = format!(
                    "Failed to get inscription_number field from miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?,

            mint_txid: row
                .get::<_, String>(2)
                .map_err(|e| {
                    let msg = format!("Failed to get mint_txid field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse mint_txid from string: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            mint_block_height: row.get::<_, i64>(3).map_err(|e| {
                let msg = format!(
                    "Failed to get mint_block_height field from miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })? as u32,
            mint_owner: row
                .get::<_, String>(4)
                .map_err(|e| {
                    let msg = format!("Failed to get mint_owner field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse mint_owner from string: {}", e);
                    error!("{}", msg);
                    msg
                })?,

            satpoint: row
                .get::<_, String>(5)
                .map_err(|e| {
                    let msg = format!("Failed to get satpoint field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse satpoint from string: {}", e);
                    error!("{}", msg);
                    msg
                })?,

            eth_main: row.get(6).map_err(|e| {
                let msg = format!("Failed to get eth_main field from miner pass row: {}", e);
                error!("{}", msg);
                msg
            })?,
            eth_collab: row.get(7).map_err(|e| {
                let msg = format!("Failed to get eth_collab field from miner pass row: {}", e);
                error!("{}", msg);
                msg
            })?,

            prev: prev_ids,

            owner: row
                .get::<_, String>(9)
                .map_err(|e| {
                    let msg = format!("Failed to get owner field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse owner from string: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            state: {
                let state_str: String = row.get(10).map_err(|e| {
                    let msg = format!("Failed to get state field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?;
                MinerPassState::from_str(&state_str).map_err(|e| {
                    let msg = format!(
                        "Failed to parse MinerPassState from string {}: {}",
                        state_str, e
                    );
                    error!("{}", msg);
                    msg
                })?
            },
        })
    }

    fn row_to_active_balance_snapshot(
        row: &rusqlite::Row,
    ) -> Result<ActiveBalanceSnapshot, String> {
        let block_height = row.get::<_, i64>(0).map_err(|e| {
            let msg = format!(
                "Failed to get block_height field from active balance snapshot row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;
        let total_balance = row.get::<_, i64>(1).map_err(|e| {
            let msg = format!(
                "Failed to get total_balance field from active balance snapshot row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;
        let active_address_count = row.get::<_, i64>(2).map_err(|e| {
            let msg = format!(
                "Failed to get active_address_count field from active balance snapshot row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        if block_height < 0 || total_balance < 0 || active_address_count < 0 {
            let msg = format!(
                "Invalid negative field in active balance snapshot row: block_height={}, total_balance={}, active_address_count={}",
                block_height, total_balance, active_address_count
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(ActiveBalanceSnapshot {
            block_height: block_height as u32,
            total_balance: total_balance as u64,
            active_address_count: active_address_count as u32,
        })
    }

    pub fn get_pass_by_inscription_id(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<MinerPassInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "
            SELECT
                *
            FROM miner_passes
            WHERE inscription_id = ?1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get miner pass by inscription_id: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query miner pass by inscription_id from database: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying miner pass by inscription_id: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let pass_info = Self::row_to_pass_item(&row)?;
            Ok(Some(pass_info))
        } else {
            Ok(None)
        }
    }

    // Get the last active mint miner pass owned by the given owner address
    // There is one an at most one active mint pass per owner at any time
    pub fn get_last_active_mint_pass_by_owner(
        &self,
        owner: &USDBScriptHash,
    ) -> Result<Option<MinerPassInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "
            SELECT
                *
            FROM miner_passes
            WHERE owner = ?1 AND state = ?2
            ORDER BY mint_block_height DESC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get last active miner pass by owner: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![
                owner.to_string(),
                MinerPassState::Active.as_str()
            ])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query last active miner pass by owner from database: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying last active miner pass by owner: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let pass_info = Self::row_to_pass_item(&row)?;
            Ok(Some(pass_info))
        } else {
            Ok(None)
        }
    }

    // Get all none consumed miner passes by pagination, where state != Consumed
    pub fn get_all_valid_pass_by_page(
        &self,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<ValidMinerPassInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let offset = page * page_size;

        let mut stmt = conn
            .prepare(
                "
            SELECT
                inscription_id,
                satpoint,
                owner
            FROM miner_passes
            WHERE state <> ?1
            ORDER BY mint_block_height DESC
            LIMIT ?2 OFFSET ?3;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get all valid miner passes by page: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![
                MinerPassState::Consumed.as_str(),
                page_size as i64,
                offset as i64
            ])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query all valid miner passes by page from database: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut passes = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying all valid miner passes by page: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let inscription_id = row
                .get::<_, String>(0)
                .map_err(|e| {
                    let msg = format!(
                        "Failed to get inscription_id field from miner pass row: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse inscription_id from string: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            let satpoint = row
                .get::<_, String>(1)
                .map_err(|e| {
                    let msg = format!("Failed to get satpoint field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse satpoint from string: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            let owner = row
                .get::<_, String>(2)
                .map_err(|e| {
                    let msg = format!("Failed to get owner field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse owner from string: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            let pass_info = ValidMinerPassInfo {
                inscription_id,
                satpoint,
                owner,
            };
            passes.push(pass_info);
        }

        Ok(passes)
    }

    pub fn get_all_active_pass_by_page(
        &self,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<ActiveMinerPassInfo>, String> {
        self.get_all_active_pass_by_page_at_height(page, page_size, u32::MAX)
    }

    pub fn get_all_active_pass_by_page_at_height(
        &self,
        page: usize,
        page_size: usize,
        block_height: u32,
    ) -> Result<Vec<ActiveMinerPassInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let offset = page * page_size;

        let mut stmt = conn
            .prepare(
                "
            SELECT
                inscription_id,
                owner
            FROM miner_passes
            WHERE state = ?1 AND mint_block_height <= ?2
            ORDER BY mint_block_height DESC
            LIMIT ?3 OFFSET ?4;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get all active miner passes by page and block height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![
                MinerPassState::Active.as_str(),
                block_height as i64,
                page_size as i64,
                offset as i64
            ])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query all active miner passes by page from database at block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut passes = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying all active miner passes by page: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let inscription_id = row
                .get::<_, String>(0)
                .map_err(|e| {
                    let msg = format!(
                        "Failed to get inscription_id field from miner pass row: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse inscription_id from string: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            let owner = row
                .get::<_, String>(1)
                .map_err(|e| {
                    let msg = format!("Failed to get owner field from miner pass row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse()
                .map_err(|e| {
                    let msg = format!("Failed to parse owner from string: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            let pass_info = ActiveMinerPassInfo {
                inscription_id,
                owner,
            };
            passes.push(pass_info);
        }

        Ok(passes)
    }
}

pub type MinerPassStorageRef = std::sync::Arc<MinerPassStorage>;

pub struct MinePassStorageSavePointGuard<'a> {
    storage: &'a MinerPassStorage,
    committed: bool,
}

impl<'a> MinePassStorageSavePointGuard<'a> {
    pub fn new(storage: &'a MinerPassStorage) -> Result<Self, String> {
        storage.savepoint_begin()?;
        Ok(Self {
            storage,
            committed: false,
        })
    }

    pub fn commit(mut self) -> Result<(), String> {
        assert!(!self.committed, "Savepoint already committed");
        self.storage.savepoint_commit()?;
        self.committed = true;
        Ok(())
    }
}

impl<'a> Drop for MinePassStorageSavePointGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            match self.storage.savepoint_rollback() {
                Ok(_) => {
                    self.storage.savepoint_commit().unwrap_or_else(|e| {
                        error!("Failed to commit after rollback savepoint: {}", e);
                    });
                }
                Err(e) => {
                    error!("Failed to rollback savepoint: {}", e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    fn test_data_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("usdb_pass_storage_{tag}_{nanos}"));
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

    fn satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
        SatPoint {
            outpoint: bitcoincore_rpc::bitcoin::OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout,
            },
            offset,
        }
    }

    fn make_pass(
        ins_tag: u8,
        index: u32,
        owner: USDBScriptHash,
        state: MinerPassState,
        height: u32,
    ) -> MinerPassInfo {
        MinerPassInfo {
            inscription_id: inscription_id(ins_tag, index),
            inscription_number: index as i32 + 1,
            mint_txid: Txid::from_slice(&[ins_tag.wrapping_add(1); 32]).unwrap(),
            mint_block_height: height,
            mint_owner: owner,
            satpoint: satpoint(ins_tag, index, 0),
            eth_main: "0x1111111111111111111111111111111111111111".to_string(),
            eth_collab: Some("0x2222222222222222222222222222222222222222".to_string()),
            prev: vec![inscription_id(ins_tag.wrapping_add(2), 0)],
            owner,
            state,
        }
    }

    #[test]
    fn test_pass_storage_crud_and_state_transition() {
        let dir = test_data_dir("crud");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(1);
        let owner2 = script_hash(2);

        let pass = make_pass(10, 0, owner1, MinerPassState::Active, 100);
        let id = pass.inscription_id;
        storage.add_new_mint_pass(&pass).unwrap();

        let loaded = storage.get_pass_by_inscription_id(&id).unwrap().unwrap();
        assert_eq!(loaded.owner, owner1);
        assert_eq!(loaded.state, MinerPassState::Active);
        assert_eq!(loaded.prev.len(), 1);

        storage
            .update_state(&id, MinerPassState::Dormant, MinerPassState::Active)
            .unwrap();
        let loaded = storage.get_pass_by_inscription_id(&id).unwrap().unwrap();
        assert_eq!(loaded.state, MinerPassState::Dormant);

        let new_satpoint = satpoint(99, 1, 33);
        storage.transfer_owner(&id, &owner2, &new_satpoint).unwrap();
        let loaded = storage.get_pass_by_inscription_id(&id).unwrap().unwrap();
        assert_eq!(loaded.owner, owner2);
        assert_eq!(loaded.satpoint, new_satpoint);

        let newest_satpoint = satpoint(100, 2, 44);
        storage
            .update_satpoint(&id, &new_satpoint, &newest_satpoint)
            .unwrap();
        let loaded = storage.get_pass_by_inscription_id(&id).unwrap().unwrap();
        assert_eq!(loaded.satpoint, newest_satpoint);

        let err = storage.update_state(&id, MinerPassState::Burned, MinerPassState::Active);
        assert!(err.is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_storage_paging_and_active_lookup() {
        let dir = test_data_dir("paging");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(7);
        let owner2 = script_hash(8);
        let owner3 = script_hash(9);

        let p1 = make_pass(21, 0, owner1, MinerPassState::Active, 100);
        let p2 = make_pass(22, 1, owner2, MinerPassState::Active, 200);
        let p3 = make_pass(23, 2, owner3, MinerPassState::Active, 300);
        storage.add_new_mint_pass(&p1).unwrap();
        storage.add_new_mint_pass(&p2).unwrap();
        storage.add_new_mint_pass(&p3).unwrap();

        storage
            .update_state(
                &p1.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
            )
            .unwrap();
        storage
            .update_state(
                &p2.inscription_id,
                MinerPassState::Consumed,
                MinerPassState::Active,
            )
            .unwrap();

        let last_active = storage
            .get_last_active_mint_pass_by_owner(&owner3)
            .unwrap()
            .unwrap();
        assert_eq!(last_active.inscription_id, p3.inscription_id);

        let active = storage.get_all_active_pass_by_page(0, 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].inscription_id, p3.inscription_id);

        let valid = storage.get_all_valid_pass_by_page(0, 10).unwrap();
        let ids: Vec<_> = valid.iter().map(|v| v.inscription_id).collect();
        assert!(ids.contains(&p1.inscription_id));
        assert!(ids.contains(&p3.inscription_id));
        assert!(!ids.contains(&p2.inscription_id));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_storage_enforce_unique_active_owner() {
        let dir = test_data_dir("unique_active_owner");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner = script_hash(21);

        let p1 = make_pass(51, 0, owner, MinerPassState::Active, 100);
        let p2 = make_pass(52, 1, owner, MinerPassState::Active, 200);
        storage.add_new_mint_pass(&p1).unwrap();

        let err = storage.add_new_mint_pass(&p2);
        assert!(err.is_err());

        storage
            .update_state(
                &p1.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
            )
            .unwrap();

        storage.add_new_mint_pass(&p2).unwrap();

        let last_active = storage
            .get_last_active_mint_pass_by_owner(&owner)
            .unwrap()
            .unwrap();
        assert_eq!(last_active.inscription_id, p2.inscription_id);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_storage_active_query_with_block_height_filter() {
        let dir = test_data_dir("active_height");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(11);
        let owner2 = script_hash(12);
        let owner3 = script_hash(13);

        let p1 = make_pass(41, 0, owner1, MinerPassState::Active, 100);
        let p2 = make_pass(42, 1, owner2, MinerPassState::Active, 200);
        let p3 = make_pass(43, 2, owner3, MinerPassState::Active, 300);
        storage.add_new_mint_pass(&p1).unwrap();
        storage.add_new_mint_pass(&p2).unwrap();
        storage.add_new_mint_pass(&p3).unwrap();

        let at_250 = storage
            .get_all_active_pass_by_page_at_height(0, 10, 250)
            .unwrap();
        assert_eq!(at_250.len(), 2);
        assert_eq!(at_250[0].inscription_id, p2.inscription_id);
        assert_eq!(at_250[1].inscription_id, p1.inscription_id);

        let at_100 = storage
            .get_all_active_pass_by_page_at_height(0, 10, 100)
            .unwrap();
        assert_eq!(at_100.len(), 1);
        assert_eq!(at_100[0].inscription_id, p1.inscription_id);

        let at_50 = storage
            .get_all_active_pass_by_page_at_height(0, 10, 50)
            .unwrap();
        assert!(at_50.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_storage_savepoint_guard_rollback_and_commit() {
        let dir = test_data_dir("savepoint");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner = script_hash(31);
        let owner2 = script_hash(32);
        let owner3 = script_hash(33);

        let base_pass = make_pass(31, 0, owner, MinerPassState::Active, 100);
        storage.add_new_mint_pass(&base_pass).unwrap();

        let rollback_pass = make_pass(32, 1, owner2, MinerPassState::Active, 101);
        {
            let _guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage.add_new_mint_pass(&rollback_pass).unwrap();
        }
        let rolled_back = storage
            .get_pass_by_inscription_id(&rollback_pass.inscription_id)
            .unwrap();
        assert!(rolled_back.is_none());

        let commit_pass = make_pass(33, 2, owner3, MinerPassState::Active, 102);
        {
            let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage.add_new_mint_pass(&commit_pass).unwrap();
            guard.commit().unwrap();
        }
        let committed = storage
            .get_pass_by_inscription_id(&commit_pass.inscription_id)
            .unwrap();
        assert!(committed.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_active_balance_snapshot_crud_and_latest() {
        let dir = test_data_dir("snapshot_crud");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_active_balance_snapshot(100, 1_000, 2)
            .unwrap();
        storage
            .upsert_active_balance_snapshot(101, 2_000, 3)
            .unwrap();

        let snap_100 = storage.get_active_balance_snapshot(100).unwrap().unwrap();
        assert_eq!(
            snap_100,
            ActiveBalanceSnapshot {
                block_height: 100,
                total_balance: 1_000,
                active_address_count: 2,
            }
        );

        let latest = storage
            .get_latest_active_balance_snapshot()
            .unwrap()
            .unwrap();
        assert_eq!(latest.block_height, 101);
        assert_eq!(latest.total_balance, 2_000);
        assert_eq!(latest.active_address_count, 3);

        storage
            .upsert_active_balance_snapshot(101, 2_500, 4)
            .unwrap();
        let updated = storage.get_active_balance_snapshot(101).unwrap().unwrap();
        assert_eq!(updated.total_balance, 2_500);
        assert_eq!(updated.active_address_count, 4);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_active_balance_snapshot_clear_from_height() {
        let dir = test_data_dir("snapshot_clear");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage.upsert_active_balance_snapshot(100, 100, 1).unwrap();
        storage.upsert_active_balance_snapshot(101, 200, 2).unwrap();
        storage.upsert_active_balance_snapshot(102, 300, 3).unwrap();

        let removed = storage
            .clear_active_balance_snapshots_from_height(101)
            .unwrap();
        assert_eq!(removed, 2);

        assert!(storage.get_active_balance_snapshot(100).unwrap().is_some());
        assert!(storage.get_active_balance_snapshot(101).unwrap().is_none());
        assert!(storage.get_active_balance_snapshot(102).unwrap().is_none());

        let latest = storage
            .get_latest_active_balance_snapshot()
            .unwrap()
            .unwrap();
        assert_eq!(latest.block_height, 100);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_active_balance_snapshot_savepoint_guard_rollback_and_commit() {
        let dir = test_data_dir("snapshot_savepoint");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_active_balance_snapshot(200, 1_000, 2)
            .unwrap();

        {
            let _guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage
                .upsert_active_balance_snapshot(201, 2_000, 3)
                .unwrap();
        }
        assert!(storage.get_active_balance_snapshot(201).unwrap().is_none());

        {
            let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage
                .upsert_active_balance_snapshot(202, 3_000, 4)
                .unwrap();
            guard.commit().unwrap();
        }
        let snap_202 = storage.get_active_balance_snapshot(202).unwrap();
        assert!(snap_202.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
