use crate::index::InscriptionOperation;
use bitcoincore_rpc::bitcoin::Amount;
use bitcoincore_rpc::bitcoin::address::{Address, NetworkUnchecked};
use bitcoincore_rpc::bitcoin::{Network, Txid};
use ord::InscriptionId;
use ordinals::SatPoint;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct InscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,

    pub genesis_block_height: u64,
    pub genesis_timestamp: u32,
    pub genesis_satpoint: SatPoint,
    pub commit_txid: Txid,
    pub value: Amount,

    pub content: String,
    pub op: InscriptionOperation,

    pub creator: Address<NetworkUnchecked>,
    pub owner: Address<NetworkUnchecked>,

    pub last_block_height: u64, // Last block height that this inscription transferred to new owner

    pub transfer_count: u64,
}

pub struct InscriptionStorage {
    db_path: PathBuf,
    network: Network,
    conn: Mutex<Connection>,
}

impl InscriptionStorage {
    pub fn new(data_dir: &Path, network: Network) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::INSCRIPTIONS_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!("Failed to open database at {:?}: {}", db_path, e);
            log::error!("{}", msg);
            msg
        })?;

        // Initialize the database schema if necessary
        conn.execute(
            "CREATE TABLE IF NOT EXISTS inscriptions (
                inscription_id TEXT PRIMARY KEY,
                inscription_number INTEGER,
                
                genesis_block_height INTEGER,
                genesis_timestamp INTEGER,
                genesis_satpoint TEXT,
                commit_txid TEXT,
                value INTEGER,

                content TEXT,
                op TEXT,

                creator TEXT,
                owner TEXT,

                last_block_height INTEGER,  /* Last block height that this inscription transferred to new owner */

                transfer_count INTEGER
                )",
            [],
        )
        .map_err(|e| {
            let msg = format!("Failed to create inscriptions table: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        // Create indices
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_block_height ON inscriptions (genesis_block_height)",
            [],
        )
        .map_err(|e| {
            let msg = format!("Failed to create index on genesis_block_height: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let storage = Self {
            db_path,
            network,
            conn: Mutex::new(conn),
        };

        Ok(storage)
    }

    pub fn add_new_inscription(
        &self,
        inscription_id: &InscriptionId,
        inscription_number: i32,

        block_height: u64,
        timestamp: u32,
        satpoint: SatPoint,
        commit_txid: Txid,
        value: Amount,

        content: &str,
        op: InscriptionOperation,

        creator: &Address<NetworkUnchecked>,
    ) -> Result<(), String> {
        let creator = creator
            .clone()
            .require_network(self.network)
            .map_err(|e| {
                let msg = format!("Invalid creator address network: {}", e);
                log::error!("{}", msg);
                msg
            })?
            .to_string();

        let conn = self.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO inscriptions (
                inscription_id,
                inscription_number,
                
                genesis_block_height,
                genesis_timestamp,
                genesis_satpoint,
                commit_txid,
                value,

                content,
                op,

                creator,
                owner,

                last_block_height,
                transfer_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                inscription_id.to_string(),
                inscription_number,
                block_height as i64,
                timestamp as i64,
                satpoint.to_string(),
                commit_txid.to_string(),
                value.to_sat() as i64,
                content,
                op.as_str(),
                &creator,
                &creator,
                block_height as i64,
                0i64
            ],
        )
        .map_err(|e| {
            let msg = format!("Failed to insert new inscription {}: {}", inscription_id, e);
            log::error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn transfer_owner(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &Option<Address<NetworkUnchecked>>,
        block_height: u64,
    ) -> Result<(), String> {
        let new_owner = match new_owner {
            Some(addr) => Some(
                addr.clone()
                    .require_network(self.network)
                    .map_err(|e| {
                        let msg = format!("Invalid new owner address network: {}", e);
                        log::error!("{}", msg);
                        msg
                    })?
                    .to_string(),
            ),
            None => {
                // The inscription is burned as fee
                warn!(
                    "Inscription {} is being burned as fee at block height {}",
                    inscription_id, block_height
                );
                None
            }
        };

        let conn = self.conn.lock().unwrap();

        let tx_result = conn.execute(
            "UPDATE inscriptions
             SET owner = ?1,
                 last_block_height = ?2,
                 transfer_count = transfer_count + 1
             WHERE inscription_id = ?3 AND last_block_height <= ?2",
            rusqlite::params![&new_owner, block_height as i64, inscription_id.to_string()],
        );

        match tx_result {
            Ok(rows_updated) => {
                if rows_updated == 0 {
                    let msg = format!(
                        "Failed to transfer inscription {} to {:?} at block height {}: no rows updated (possibly due to out-of-order transfer)",
                        inscription_id, new_owner, block_height
                    );
                    warn!("{}", msg);

                    // TODO might be better to return an error here?
                } else {
                    info!(
                        "Transferred ownership of inscription {} to {:?} at block height {}",
                        inscription_id, new_owner, block_height
                    );
                }

                Ok(())
            }
            Err(e) => {
                let msg = format!(
                    "Failed to update owner for inscription {}: {}",
                    inscription_id, e
                );
                log::error!("{}", msg);
                Err(msg)
            }
        }
    }

    fn row_to_info(&self, row: &rusqlite::Row) -> Result<InscriptionInfo, String> {
        let inscription_id_str: String = row.get(0).map_err(|e| {
            let msg = format!(
                "Failed to get inscription_id column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;
        let inscription_id = InscriptionId::from_str(&inscription_id_str).map_err(|e| {
            let msg = format!(
                "Failed to parse inscription_id '{}' from database: {}",
                inscription_id_str, e
            );
            error!("{}", msg);
            msg
        })?;

        let inscription_number: i32 = row.get(1).map_err(|e| {
            let msg = format!(
                "Failed to get inscription_number column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        let genesis_block_height: u64 = row.get::<_, i64>(2).map_err(|e| {
            let msg = format!(
                "Failed to get genesis_block_height column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })? as u64;

        let genesis_timestamp: u32 = row.get::<_, i64>(3).map_err(|e| {
            let msg = format!(
                "Failed to get genesis_timestamp column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })? as u32;

        let genesis_satpoint_str: String = row.get(4).map_err(|e| {
            let msg = format!(
                "Failed to get genesis_satpoint column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;
        let genesis_satpoint = SatPoint::from_str(&genesis_satpoint_str).map_err(|e| {
            let msg = format!(
                "Failed to parse genesis_satpoint '{}' from database: {}",
                genesis_satpoint_str, e
            );
            error!("{}", msg);
            msg
        })?;

        let commit_txid_str: String = row.get(5).map_err(|e| {
            let msg = format!("Failed to get commit_txid column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let commit_txid = Txid::from_str(&commit_txid_str).map_err(|e| {
            let msg = format!(
                "Failed to parse commit_txid '{}' from database: {}",
                commit_txid_str, e
            );
            error!("{}", msg);
            msg
        })?;

        let value_sat: i64 = row.get(6).map_err(|e| {
            let msg = format!("Failed to get value column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let value = Amount::from_sat(value_sat as u64);

        let content: String = row.get(7).map_err(|e| {
            let msg = format!("Failed to get content column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;

        let op_str: String = row.get(8).map_err(|e| {
            let msg = format!("Failed to get op column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let op = InscriptionOperation::from_str(&op_str).map_err(|e| {
            let msg = format!(
                "Failed to parse operation '{}' from database: {}",
                op_str, e
            );
            error!("{}", msg);
            msg
        })?;

        let creator_str: String = row.get(9).map_err(|e| {
            let msg = format!("Failed to get creator column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let creator = Address::from_str(&creator_str).map_err(|e| {
            let msg = format!(
                "Failed to parse creator address '{}' from database: {}",
                creator_str, e
            );
            error!("{}", msg);
            msg
        })?;
        if !creator.is_valid_for_network(self.network) {
            let msg = format!(
                "Creator address '{}' has invalid network: {}",
                creator_str, self.network
            );
            error!("{}", msg);
            return Err(msg);
        };

        let owner_str: String = row.get(10).map_err(|e| {
            let msg = format!("Failed to get owner column from database row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let owner = Address::from_str(&owner_str).map_err(|e| {
            let msg = format!(
                "Failed to parse owner address '{}' from database: {}",
                owner_str, e
            );
            error!("{}", msg);
            msg
        })?;
        if !owner.is_valid_for_network(self.network) {
            let msg = format!(
                "Owner address {} has invalid network: {}",
                owner_str, self.network
            );
            error!("{}", msg);
            return Err(msg);
        };

        let last_block_height: u64 = row.get::<_, i64>(11).map_err(|e| {
            let msg = format!(
                "Failed to get last_block_height column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })? as u64;

        let transfer_count: u64 = row.get::<_, i64>(12).map_err(|e| {
            let msg = format!(
                "Failed to get transfer_count column from database row: {}",
                e
            );
            error!("{}", msg);
            msg
        })? as u64;

        Ok(InscriptionInfo {
            inscription_id,
            inscription_number,
            genesis_block_height,
            genesis_timestamp,
            genesis_satpoint,
            commit_txid,
            value,
            content,
            op,
            creator,
            owner,
            last_block_height,
            transfer_count,
        })
    }

    pub fn get_inscription(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<InscriptionInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT
                    *
                 FROM inscriptions
                 WHERE inscription_id = ?1",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query to get inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to fetch row for inscription {}: {}",
                inscription_id, e
            );
            error!("{}", msg);
            msg
        })? {
            let info = self.row_to_info(row)?;

            Ok(Some(info))
        } else {
            warn!("Inscription {} not found in database", inscription_id);

            Ok(None)
        }
    }

    pub fn get_inscriptions_by_block(
        &self,
        block_height: u64,
    ) -> Result<Vec<InscriptionInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT
                    *
                 FROM inscriptions
                 WHERE genesis_block_height = ?1",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get inscriptions at block {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query to get inscriptions at block {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut inscriptions = Vec::new();

        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to fetch row for inscriptions at block {}: {}",
                block_height, e
            );
            error!("{}", msg);
            msg
        })? {
            let info = self.row_to_info(row)?;
            inscriptions.push(info);
        }

        Ok(inscriptions)
    }

    pub fn get_inscription_content(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT
                    content
                 FROM inscriptions
                 WHERE inscription_id = ?1",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get content for inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query to get content for inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to fetch row for content of inscription {}: {}",
                inscription_id, e
            );
            error!("{}", msg);
            msg
        })? {
            let content: String = row.get(0).map_err(|e| {
                let msg = format!(
                    "Failed to get content column from database row for inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                msg
            })?;

            Ok(Some(content))
        } else {
            warn!("Inscription {} not found in database", inscription_id);

            Ok(None)
        }
    }
}

pub type InscriptionStorageRef = std::sync::Arc<InscriptionStorage>;
