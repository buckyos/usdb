use crate::index::MinerPassState;
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use ord::templates::inscription;
use ordinals::SatPoint;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use usdb_util::USDBScriptHash;

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
            ",
        )
        .map_err(|e| {
            let msg = format!("Failed to initialize MinerPassStorage database: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
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
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10);
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
}

pub type MinerPassStorageRef = std::sync::Arc<MinerPassStorage>;
