use bitcoincore_rpc::bitcoin::address::NetworkUnchecked;
use bitcoincore_rpc::bitcoin::{Address, Network};
use rusqlite::Connection;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

// The balance record for an address in a specific block
pub struct BalanceRecord {
    pub address: Address<NetworkUnchecked>,
    pub block_height: u64,
    pub delta: i64,   // The change in balance due to this block
    pub balance: u64, // The balance after all transactions in this block
}

pub struct AddressBalanceStorage {
    db_path: PathBuf,
    network: Network,
    conn: Mutex<Connection>,
}

impl AddressBalanceStorage {
    pub fn new(data_dir: &PathBuf, network: Network) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::ADDRESS_BALANCE_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!(
                "Failed to open AddressBalanceStorage database at {:?}: {}",
                db_path, e
            );
            error!("{}", msg);
            msg
        })?;

        // Init the database
        let storage = AddressBalanceStorage {
            db_path,
            network,
            conn: Mutex::new(conn),
        };
        storage.init_db()?;

        Ok(storage)
    }

    fn init_db(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS watched_addresses (
                address TEXT NOT NULL PRIMARY KEY,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS address_balances (
                id INTEGER PRIMARY KEY AUTOINCREMENT,

                address TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                delta INTEGER NOT NULL,
                balance INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_address_balances_address_block_height
            ON address_balances (address, block_height);

            -- Create trigger to ensure data integrity before inserting
            DROP TRIGGER IF EXISTS trig_check_balance_insert;
            CREATE TRIGGER trig_check_balance_insert
            BEFORE INSERT ON address_balances
            FOR EACH ROW
            BEGIN
                -- Check if the address is being watched already
                -- If it exists, all fields must be exactly the same (except for id)
                PERFORM 1 FROM address_balances 
                WHERE address = NEW.address 
                AND block_height = NEW.block_height;

                IF FOUND THEN
                    -- If it exists, all fields must be exactly the same (except for id)
                    SELECT RAISE(ABORT, 'duplicate block_height with different data')
                    FROM address_balances
                    WHERE address = NEW.address
                    AND block_height = NEW.block_height
                    AND (
                            delta != NEW.delta 
                        OR balance != NEW.balance
                    );
                END IF;

                -- Check if block_height is strictly increasing (greater than the current maximum)
                SELECT RAISE(ABORT, 'block_height must be greater than last recorded height')
                FROM (
                    SELECT MAX(block_height) AS max_h 
                    FROM address_balances 
                    WHERE address = NEW.address
                ) 
                WHERE max_h IS NOT NULL AND NEW.block_height <= max_h;
            END IF;
            END;
            ",
        )
        .map_err(|e| {
            let msg = format!("Failed to initialize AddressBalanceStorage database: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn add_watched_address(&self, address: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "
            INSERT OR IGNORE INTO watched_addresses (address)
            VALUES (?)
            ",
                [address],
            )
            .map_err(|e| {
                let msg = format!("Failed to add watched address {}: {}", address, e);
                error!("{}", msg);
                msg
            })?;

        if changed > 0 {
            info!("Added new watched address: {}", address);
        } else {
            // debug!("Address {} is already being watched", address);
        }

        Ok(())
    }

    // Add a balance record for an address at a specific block
    // And the block_height must be greater than the latest record's block_height!
    pub fn add_balance_record(&self, record: &BalanceRecord) -> Result<(), String> {
        let address = record
            .address
            .clone()
            .require_network(self.network)
            .map_err(|e| {
                let msg = format!(
                    "Failed to convert address to network {}: {}",
                    self.network, e
                );
                error!("{}", msg);
                msg
            })?
            .to_string();

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO address_balances (address, block_height, delta, balance)
            VALUES (?, ?, ?, ?)
            ",
            rusqlite::params![&address, record.block_height, record.delta, record.balance],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to insert balance record for address {} at block {}: {}",
                address, record.block_height, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn row_to_record_item(&self, row: &rusqlite::Row) -> Result<BalanceRecord, String> {
        let addr_str = row.get::<usize, String>(1).map_err(|e| {
            let msg = format!("Failed to get address string from row: {}", e);
            error!("{}", msg);
            msg
        })?;

        let address = Address::from_str(&addr_str).map_err(|e| {
            let msg = format!("Failed to parse address {}: {}", addr_str, e);
            error!("{}", msg);
            msg
        })?;
        if !address.is_valid_for_network(self.network) {
            let msg = format!("Address {} is not valid for Bitcoin network", addr_str);
            error!("{}", msg);
            return Err(msg);
        }

        Ok(BalanceRecord {
            address,
            block_height: row.get(2).map_err(|e| {
                let msg = format!("Failed to get block_height from row: {}", e);
                error!("{}", msg);
                msg
            })?,
            delta: row.get(3).map_err(|e| {
                let msg = format!("Failed to get delta from row: {}", e);
                error!("{}", msg);
                msg
            })?,
            balance: row.get(4).map_err(|e| {
                let msg = format!("Failed to get balance from row: {}", e);
                error!("{}", msg);
                msg
            })?,
        })
    }

    // Get the latest block height for which we have a balance record for the address
    pub fn get_latest_balance_block_height(&self, address: &str) -> Result<Option<u64>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT block_height
            FROM address_balances
            WHERE address = ?
            ORDER BY block_height DESC
            LIMIT 1
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement for latest balance block height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt.query([address]).map_err(|e| {
            let msg = format!("Failed to query latest balance block height: {}", e);
            error!("{}", msg);
            msg
        })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let block_height: u64 = row.get(0).map_err(|e| {
                let msg = format!("Failed to get block_height from row: {}", e);
                error!("{}", msg);
                msg
            })?;
            Ok(Some(block_height))
        } else {
            Ok(None)
        }
    }

    pub fn get_latest_balance(&self, address: &str) -> Result<Option<BalanceRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT *
            FROM address_balances
            WHERE address = ?
            ORDER BY block_height DESC
            LIMIT 1
            ",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare statement for latest balance: {}", e);
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt.query([address]).map_err(|e| {
            let msg = format!("Failed to query latest balance: {}", e);
            error!("{}", msg);
            msg
        })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let record = self.row_to_record_item(&row)?;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    // Get the balance of an address at a specific block height, find the latest record.block_height <= block_height
    pub fn get_balance_at_block(&self, address: &str, block_height: u64) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT *
            FROM address_balances
            WHERE address = ? AND block_height <= ?
            ORDER BY block_height DESC
            LIMIT 1
            ",
            )
            .map_err(|e| {
                let msg = format!("Failed to prepare statement for balance at block: {}", e);
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![address, block_height])
            .map_err(|e| {
                let msg = format!("Failed to query balance at block: {}", e);
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let record = self.row_to_record_item(&row)?;
            Ok(record.balance)
        } else {
            // No record found, return 0 balance
            Ok(0)
        }
    }
}


pub type AddressBalanceStorageRef = std::sync::Arc<AddressBalanceStorage>;