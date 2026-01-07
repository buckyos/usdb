use crate::index::InscriptionOperation;
use bitcoincore_rpc::bitcoin::{Amount};
use ord::InscriptionId;
use ordinals::SatPoint;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use usdb_util::USDBScriptHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InscriptionTransferRecordItem {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: SatPoint,
    pub from_address: Option<USDBScriptHash>,
    pub to_address: Option<USDBScriptHash>, // None if burn as fee
    pub value: Amount,
    pub index: u64, // Index indicates the number of transfers
    pub op: InscriptionOperation,
}

pub struct InscriptionTransferStorage {
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl InscriptionTransferStorage {
    pub fn new(data_dir: &Path) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::TRANSFER_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!("Failed to open database at {:?}: {}", db_path, e);
            log::error!("{}", msg);
            msg
        })?;

        // Initialize the database schema if necessary
        conn.execute(
            "CREATE TABLE IF NOT EXISTS inscription_transfers (
                    inscription_id TEXT,
                    inscription_number INTEGER,
                    block_height INTEGER,
                    timestamp INTEGER,
                    satpoint TEXT,
                    from_address TEXT,
                    to_address TEXT,
                    value INTEGER,
                    idx INTEGER DEFAULT 0,
                    op TEXT,
                    PRIMARY KEY(inscription_id, timestamp)
                    );",
            [],
        )
        .map_err(|e| {
            let msg = format!("Failed to create transfers table: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        // Create index for faster queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_inscription_id ON inscription_transfers (inscription_id);",
            [],
        ).map_err(|e| {
            let msg = format!("Failed to create index on inscription_id: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let storage = Self {
            db_path,
            conn: Mutex::new(conn),
        };

        Ok(storage)
    }

    pub fn insert_transfer_record(
        &self,
        record: &InscriptionTransferRecordItem,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        let from_address = record.from_address.map(|v| v.to_string());
        let to_address = record.to_address.map(|v| v.to_string());

        conn.execute(
            "INSERT OR REPLACE INTO inscription_transfers (
                    inscription_id,
                    inscription_number,
                    block_height,
                    timestamp,
                    satpoint,
                    from_address,
                    to_address,
                    value,
                    idx,
                    op
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10);",
            rusqlite::params![
                record.inscription_id.to_string(),
                record.inscription_number as i64,
                record.block_height as i64,
                record.timestamp as i64,
                record.satpoint.to_string(),
                from_address,
                to_address,
                record.value.to_sat() as i64,
                record.index as i64,
                record.op.as_str(),
            ],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to insert transfer record: {}, {}",
                record.inscription_id, e
            );
            log::error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn row_to_record_item(
        row: &rusqlite::Row<'_>,
    ) -> Result<InscriptionTransferRecordItem, String> {
        let inscription_id_str = row.get::<_, String>(0).map_err(|e| {
            let msg = format!("Failed to get inscription_id from DB row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let inscription_id = InscriptionId::from_str(&inscription_id_str).map_err(|e| {
            let msg = format!("Invalid inscription_id in DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        let inscription_number = row.get::<_, i64>(1).map_err(|e| {
            let msg = format!("Failed to get inscription_number from DB row: {}", e);
            error!("{}", msg);
            msg
        })? as i32;

        let block_height = row.get::<_, i64>(2).map_err(|e| {
            let msg = format!("Failed to get block_height from DB row: {}", e);
            error!("{}", msg);
            msg
        })? as u32;

        let timestamp = row.get::<_, i64>(3).map_err(|e| {
            let msg = format!("Failed to get timestamp from DB row: {}", e);
            error!("{}", msg);
            msg
        })? as u32;

        let satpoint_str = row.get::<_, String>(4).map_err(|e| {
            let msg = format!("Failed to get satpoint from DB row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let satpoint = SatPoint::from_str(&satpoint_str).map_err(|e| {
            let msg = format!("Invalid satpoint in DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        let from_address = match row.get::<_, Option<String>>(5).map_err(|e| {
            let msg = format!("Failed to get from_address from DB row: {}", e);
            error!("{}", msg);
            msg
        })? {
            Some(addr_str) => Some(USDBScriptHash::from_str(&addr_str).map_err(|e| {
                let msg = format!("Invalid from_address in DB: {}", e);
                error!("{}", msg);
                msg
            })?),
            None => None,
        };

        let to_address = match row.get::<_, Option<String>>(6).map_err(|e| {
            let msg = format!("Failed to get to_address from DB row: {}", e);
            error!("{}", msg);
            msg
        })? {
            Some(addr_str) => Some(USDBScriptHash::from_str(&addr_str).map_err(|e| {
                let msg = format!("Invalid to_address in DB: {}", e);
                error!("{}", msg);
                msg
            })?),
            None => None,
        };

        let value = Amount::from_sat(row.get::<_, i64>(7).map_err(|e| {
            let msg = format!("Failed to get value from DB row: {}", e);
            error!("{}", msg);
            msg
        })? as u64);

        let index = row.get::<_, i64>(8).map_err(|e| {
            let msg = format!("Failed to get index from DB row: {}", e);
            error!("{}", msg);
            msg
        })? as u64;

        let op_str = row.get::<_, String>(9).map_err(|e| {
            let msg = format!("Failed to get operation from DB row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let op = InscriptionOperation::from_str(&op_str).map_err(|e| {
            let msg = format!("Invalid operation in DB: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(InscriptionTransferRecordItem {
            inscription_id,
            inscription_number,
            block_height,
            timestamp,
            satpoint,
            from_address,
            to_address,
            value,
            index,
            op,
        })
    }

    pub fn get_all_transfer_records(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Vec<InscriptionTransferRecordItem>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT 
                    *
                FROM inscription_transfers 
                WHERE inscription_id = ?1
                ORDER BY timestamp ASC;",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare select statement: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query for inscription_id {}: {}",
                    inscription_id, e
                );
                log::error!("{}", msg);
                msg
            })?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row from query result: {}", e);
            log::error!("{}", msg);
            msg
        })? {
            let record = Self::row_to_record_item(row)?;
            records.push(record);
        }

        Ok(records)
    }

    pub fn get_latest_transfer_record(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<InscriptionTransferRecordItem>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT 
                    *
                FROM inscription_transfers 
                WHERE inscription_id = ?1
                ORDER BY timestamp DESC
                LIMIT 1;",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare select statement: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query for inscription_id {}: {}",
                    inscription_id, e
                );
                log::error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row from query result: {}", e);
            log::error!("{}", msg);
            msg
        })? {
            let record = Self::row_to_record_item(row)?;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    pub fn get_first_transfer_record(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<InscriptionTransferRecordItem>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT 
                    *
                FROM inscription_transfers 
                WHERE inscription_id = ?1
                ORDER BY timestamp ASC
                LIMIT 1;",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare select statement: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string()])
            .map_err(|e| {
                let msg = format!(
                    "Failed to execute query for inscription_id {}: {}",
                    inscription_id, e
                );
                log::error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row from query result: {}", e);
            log::error!("{}", msg);
            msg
        })? {
            let record = Self::row_to_record_item(row)?;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    pub fn get_all_inscriptions_with_last_transfer(
        &self,
    ) -> Result<Vec<InscriptionTransferRecordItem>, String> {
        let conn = self.conn.lock().unwrap();

        let sql = format!(
            "
                SELECT it1.* FROM inscription_transfers it1
                JOIN (
                SELECT inscription_id, MAX(timestamp) AS max_timestamp
                FROM inscription_transfers
                GROUP BY inscription_id
                ) it2
                ON it1.inscription_id = it2.inscription_id AND it1.timestamp = it2.max_timestamp
                WHERE (it1.op = '{}' AND it1.idx = 0) OR it1.op = '{}'",
            InscriptionOperation::Transfer.as_str(),
            InscriptionOperation::Inscribe.as_str()
        );

        let mut stmt = conn.prepare(&sql).map_err(|e| {
            let msg = format!("Failed to prepare select statement: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let mut rows = stmt.query([]).map_err(|e| {
            let msg = format!("Failed to execute query: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row from query result: {}", e);
            log::error!("{}", msg);
            msg
        })? {
            let record = Self::row_to_record_item(row)?;
            records.push(record);
        }

        Ok(records)
    }
}

pub type InscriptionTransferStorageRef = Arc<InscriptionTransferStorage>;
