use bitcoincore_rpc::bitcoin::address::{NetworkChecked};
use bitcoincore_rpc::bitcoin::{Address, Network};
use rusqlite::Connection;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

// The balance record for an address in a specific block
pub struct BalanceRecord {
    pub address: Address<NetworkChecked>,
    pub block_height: u64,
    pub delta: i64,   // The change in balance due to this block
    pub balance: u64, // The balance after all transactions in this block
}

#[derive(Debug, Clone)]
pub struct WatchedAddress {
    pub address: Address<NetworkChecked>,
    pub block_height: u64,
    pub balance: u64,
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
                block_height INTEGER NOT NULL DEFAULT 0,
                balance INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS address_balances (
                id INTEGER PRIMARY KEY AUTOINCREMENT,

                address TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                delta INTEGER NOT NULL,
                balance INTEGER NOT NULL,

                foreign key(address) references watched_addresses(address),
                UNIQUE(address, block_height)
            );

            CREATE INDEX IF NOT EXISTS idx_address_balances_address_block_height
            ON address_balances (address, block_height);

            -- Create trigger to ensure data integrity before inserting
            DROP TRIGGER IF EXISTS trig_check_balance_insert;
            CREATE TRIGGER trig_check_balance_insert
            BEFORE INSERT ON address_balances
            FOR EACH ROW
            BEGIN
                -- Reject same block_height with different data
                SELECT RAISE(ABORT, 'duplicate block_height with different data')
                FROM address_balances
                WHERE address = NEW.address
                AND block_height = NEW.block_height
                AND (delta != NEW.delta OR balance != NEW.balance);

                -- Enforce strictly increasing block_height
                SELECT RAISE(ABORT, 'block_height must be greater than last recorded height')
                FROM (
                    SELECT MAX(block_height) AS max_h
                    FROM address_balances
                    WHERE address = NEW.address
                )
                WHERE max_h IS NOT NULL AND NEW.block_height <= max_h;
            END;


            -- Update watched_addresses table after inserting into address_balances
            CREATE TRIGGER trig_watched_sync_after_insert
            AFTER INSERT ON address_balances
            FOR EACH ROW
            BEGIN
                INSERT OR REPLACE INTO watched_addresses (address, block_height, balance)
                VALUES (
                    NEW.address,
                    NEW.block_height,
                    NEW.balance
                );
                -- Use INSERT OR REPLACE to:
                --   - If exists → update block_height and balance
                --   - If not exists → insert (although foreign key theoretically won't reach this step)
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
            INSERT OR IGNORE INTO watched_addresses (address, block_height, balance)
            VALUES (?1, 0, 0)
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

    pub fn get_all_watched_addresses(&self) -> Result<Vec<WatchedAddress>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT address, block_height, balance
            FROM watched_addresses
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement for getting all watched addresses: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt.query([]).map_err(|e| {
            let msg = format!("Failed to query all watched addresses: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let addr_str: String = row.get(0).map_err(|e| {
                let msg = format!("Failed to get address from row: {}", e);
                error!("{}", msg);
                msg
            })?;
            let address = Address::from_str(&addr_str)
                .map_err(|e| {
                    let msg = format!("Failed to parse address {}: {}", addr_str, e);
                    error!("{}", msg);
                    msg
                })?
                .require_network(self.network)
                .map_err(|e| {
                    let msg = format!(
                        "Address {} is not valid for network {}: {}",
                        addr_str, self.network, e
                    );
                    error!("{}", msg);
                    msg
                })?;

            result.push(WatchedAddress {
                address,
                block_height: row.get(1).map_err(|e| {
                    let msg = format!("Failed to get block_height from row: {}", e);
                    error!("{}", msg);
                    msg
                })?,
                balance: row.get(2).map_err(|e| {
                    let msg = format!("Failed to get balance from row: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            });
        }

        Ok(result)
    }

    pub fn get_watched_address(&self, address: &str) -> Result<Option<WatchedAddress>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT address, block_height, balance
            FROM watched_addresses
            WHERE address = ?
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement for getting watched address {}: {}",
                    address, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt.query([address]).map_err(|e| {
            let msg = format!("Failed to query watched address {}: {}", address, e);
            error!("{}", msg);
            msg
        })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to fetch row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let addr_str: String = row.get(0).map_err(|e| {
                let msg = format!("Failed to get address from row: {}", e);
                error!("{}", msg);
                msg
            })?;
            let address = Address::from_str(&addr_str)
                .map_err(|e| {
                    let msg = format!("Failed to parse address {}: {}", addr_str, e);
                    error!("{}", msg);
                    msg
                })?
                .require_network(self.network)
                .map_err(|e| {
                    let msg = format!(
                        "Address {} is not valid for network {}: {}",
                        addr_str, self.network, e
                    );
                    error!("{}", msg);
                    msg
                })?;

            Ok(Some(WatchedAddress {
                address,
                block_height: row.get(1).map_err(|e| {
                    let msg = format!("Failed to get block_height from row: {}", e);
                    error!("{}", msg);
                    msg
                })?,
                balance: row.get(2).map_err(|e| {
                    let msg = format!("Failed to get balance from row: {}", e);
                    error!("{}", msg);
                    msg
                })?,
            }))
        } else {
            Ok(None)
        }
    }

    // Update the balance of a watched address at a specific block
    // Which will read the latest balance and apply the delta to it
    pub fn update_balance(
        &self,
        address: &Address<NetworkChecked>,
        block_height: u64,
        delta: i64,
    ) -> Result<(), String> {
        let address_str = address.to_string();

        // Get the latest balance
        let latest_balance = self.get_watched_address(&address_str)?;
        assert!(
            latest_balance.is_some(),
            "Address {} must be watched before updating balance",
            address_str
        );
        let latest_balance = latest_balance.unwrap();

        // Check block height must be greater than latest
        if block_height <= latest_balance.block_height {
            let msg = format!(
                "Block height {} must be greater than latest recorded height {} for address {}",
                block_height, latest_balance.block_height, address_str
            );
            error!("{}", msg);
            return Err(msg);
        }

        // Calculate new balance
        let new_balance = if delta.is_negative() {
            let delta_abs = delta.abs() as u64;
            if delta_abs > latest_balance.balance {
                let msg = format!(
                    "Balance underflow: trying to decrease {} by {} which is greater than current balance {} for address {}",
                    latest_balance.balance, delta_abs, latest_balance.balance, address_str
                );
                error!("{}", msg);
                return Err(msg);
            }
            latest_balance.balance - delta_abs
        } else {
            latest_balance.balance + (delta as u64)
        };

        // Update the balance record
        let record = BalanceRecord {
            address: latest_balance.address,
            block_height,
            delta,
            balance: new_balance,
        };

        self.add_balance_record(&record)?;

        info!(
            "Updated balance for address {} at block {}: delta {}, new balance {}",
            address_str, block_height, delta, new_balance
        );

        Ok(())
    }

    // Add a balance record for an address at a specific block
    // And the block_height must be greater than the latest record's block_height!
    pub fn add_balance_record(&self, record: &BalanceRecord) -> Result<(), String> {
        let address = record.address.to_string();

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
        let address = address.require_network(self.network).map_err(|e| {
            let msg = format!(
                "Address {} is not valid for network {}: {}",
                addr_str, self.network, e
            );
            error!("{}", msg);
            msg
        })?;

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


#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Network;
    use std::fs;

    #[test]
    fn test_address_balance_storage() {
        let tmp_dir = std::env::temp_dir().join("usdb").join("test_address_balance_storage");
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let test_db_path = tmp_dir.join(crate::constants::ADDRESS_BALANCE_DB_FILE);
        if test_db_path.exists() {
            fs::remove_file(&test_db_path).unwrap();
        }
        let storage =
            AddressBalanceStorage::new(&tmp_dir, Network::Bitcoin).unwrap();

        let test_address =
            Address::from_str("bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h")
                .unwrap()
                .require_network(Network::Bitcoin)
                .unwrap();

        // Add watched address
        storage
            .add_watched_address(&test_address.to_string())
            .unwrap();
        storage
            .add_watched_address(&test_address.to_string())
            .unwrap(); // Adding again should be no-op

        // Update balance
        storage
            .update_balance(&test_address, 1, 1000)
            .unwrap();
        // Update balance with same block height and same delta should be failed
        storage
            .update_balance(&test_address, 1, 1000)
            .unwrap_err();

        // Update balance with same block height and different delta should error
        let result = storage.update_balance(&test_address, 1, 500);
        assert!(result.is_err());

        // Check latest balance
        let latest_balance = storage
            .get_latest_balance(&test_address.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(latest_balance.balance, 1000);
        assert_eq!(latest_balance.block_height, 1);

        // Check watched address
        let watched_address = storage
            .get_watched_address(&test_address.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(watched_address.balance, 1000);
        assert_eq!(watched_address.block_height, 1);

        storage
            .update_balance(&test_address, 3, -500)
            .unwrap();

        // Get latest balance
        let latest_balance = storage
            .get_latest_balance(&test_address.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(latest_balance.balance, 500);

        // Get balance at block 1
        let balance_at_1 = storage
            .get_balance_at_block(&test_address.to_string(), 1)
            .unwrap();
        assert_eq!(balance_at_1, 1000);

        // Get balance at block 2
        let balance_at_2 = storage
            .get_balance_at_block(&test_address.to_string(), 2)
            .unwrap();
        assert_eq!(balance_at_2, 1000);

        // Get balance at block 3
        let balance_at_3 = storage
            .get_balance_at_block(&test_address.to_string(), 3)
            .unwrap();
        assert_eq!(balance_at_3, 500);

        // Get balance at block 4
        let balance_at_4 = storage
            .get_balance_at_block(&test_address.to_string(), 4)
            .unwrap();
        assert_eq!(balance_at_4, 500);

        // Insert balance with lower block height should error
        let result = storage.update_balance(&test_address, 2, 200);
        assert!(result.is_err());
        
        // Clean up
        fs::remove_file(&test_db_path).unwrap();
    }
}