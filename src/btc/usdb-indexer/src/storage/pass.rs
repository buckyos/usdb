use crate::index::{MinerPassState, PassBlockCommitEntry};
use balance_history::SnapshotInfo as BalanceHistorySnapshotInfo;
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
const BALANCE_HISTORY_SNAPSHOT_HEIGHT_KEY: &str = "balance_history_snapshot_height";
const BALANCE_HISTORY_SNAPSHOT_BLOCK_HASH_KEY: &str = "balance_history_snapshot_block_hash";
const BALANCE_HISTORY_SNAPSHOT_BLOCK_COMMIT_KEY: &str = "balance_history_snapshot_block_commit";
const BALANCE_HISTORY_SNAPSHOT_COMMIT_PROTOCOL_VERSION_KEY: &str =
    "balance_history_snapshot_commit_protocol_version";
const BALANCE_HISTORY_SNAPSHOT_COMMIT_HASH_ALGO_KEY: &str =
    "balance_history_snapshot_commit_hash_algo";

// Default savepoint name for miner pass operations
const SAVEPOINT_MINER_PASS_OPS: &str = "miner_pass_ops";

// Event types for miner pass history
const PASS_HISTORY_EVENT_MINT: &str = "mint";
const PASS_HISTORY_EVENT_INVALID_MINT: &str = "invalid_mint";
const PASS_HISTORY_EVENT_STATE_UPDATE: &str = "state_update";
const PASS_HISTORY_EVENT_OWNER_TRANSFER: &str = "owner_transfer";
const PASS_HISTORY_EVENT_SATPOINT_UPDATE: &str = "satpoint_update";

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
    pub invalid_code: Option<String>,
    pub invalid_reason: Option<String>,

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MinerPassStateStats {
    pub total_count: u64,
    pub active_count: u64,
    pub dormant_count: u64,
    pub consumed_count: u64,
    pub burned_count: u64,
    pub invalid_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinerPassHistoryInfo {
    pub event_id: i64,
    pub inscription_id: InscriptionId,
    pub block_height: u32,
    pub event_type: String,
    pub state: MinerPassState,
    pub owner: USDBScriptHash,
    pub satpoint: SatPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBalanceSnapshot {
    pub block_height: u32,
    pub total_balance: u64,
    pub active_address_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceHistorySnapshotAnchor {
    pub stable_height: u32,
    pub stable_block_hash: String,
    pub latest_block_commit: String,
    pub commit_protocol_version: String,
    pub commit_hash_algo: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPassBlockCommitEntry {
    // Local usdb-indexer block height of this persisted commit row.
    pub block_height: u32,
    // Upstream balance-history anchor height used when this local commit was built.
    // Pass commit v1 stores this explicitly even though it must equal block_height.
    pub balance_history_block_height: u32,
    // Upstream balance-history logical block commit captured as the external anchor.
    // This upstream commit already includes the upstream btc_block_hash in its own hash input.
    pub balance_history_block_commit: String,
    // Hash of this block's local pass mutation stream.
    pub mutation_root: String,
    // Rolling local pass block commit persisted for later chaining/query.
    pub block_commit: String,
    // Protocol version used to interpret/hash the stored commit payload.
    pub commit_protocol_version: String,
    // Hash algorithm name used to produce mutation_root and block_commit.
    pub commit_hash_algo: String,
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

            CREATE TABLE IF NOT EXISTS state_text (
                name TEXT PRIMARY KEY,
                value TEXT NOT NULL
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
                invalid_code TEXT,
                invalid_reason TEXT,

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

            CREATE TABLE IF NOT EXISTS miner_pass_state_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                inscription_id TEXT NOT NULL,
                block_height INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                prev_state TEXT,
                new_state TEXT NOT NULL,
                prev_owner TEXT,
                new_owner TEXT NOT NULL,
                prev_satpoint TEXT,
                new_satpoint TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_pass_history_height_id
            ON miner_pass_state_history (block_height, id);

            CREATE INDEX IF NOT EXISTS idx_pass_history_inscription_height_id
            ON miner_pass_state_history (inscription_id, block_height, id);

            CREATE TABLE IF NOT EXISTS pass_block_commits (
                block_height INTEGER PRIMARY KEY,
                balance_history_block_height INTEGER NOT NULL,
                balance_history_block_commit TEXT NOT NULL,
                mutation_root TEXT NOT NULL,
                block_commit TEXT NOT NULL,
                commit_protocol_version TEXT NOT NULL,
                commit_hash_algo TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )
        .map_err(|e| {
            let msg = format!("Failed to initialize MinerPassStorage database: {}", e);
            error!("{}", msg);
            msg
        })?;

        Self::ensure_column_exists(&conn, "miner_passes", "invalid_code", "TEXT")?;
        Self::ensure_column_exists(&conn, "miner_passes", "invalid_reason", "TEXT")?;

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

    fn ensure_column_exists(
        conn: &Connection,
        table: &str,
        column: &str,
        column_def: &str,
    ) -> Result<(), String> {
        let sql = format!(
            "ALTER TABLE {} ADD COLUMN {} {};",
            table, column, column_def
        );

        match conn.execute(&sql, []) {
            Ok(_) => {
                info!(
                    "Added missing column to sqlite table: table={}, column={}",
                    table, column
                );
                Ok(())
            }
            Err(e) => {
                let err = e.to_string();
                if err.contains("duplicate column name") {
                    return Ok(());
                }

                let msg = format!(
                    "Failed to ensure sqlite column exists: table={}, column={}, error={}",
                    table, column, e
                );
                error!("{}", msg);
                Err(msg)
            }
        }
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

    fn upsert_text_state(&self, name: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "
            INSERT INTO state_text (name, value)
            VALUES (?1, ?2)
            ON CONFLICT(name) DO UPDATE SET value = excluded.value;
            ",
            rusqlite::params![name, value],
        )
        .map_err(|e| {
            let msg = format!("Failed to update text state {} in database: {}", name, e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    fn get_text_state(&self, name: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare("SELECT value FROM state_text WHERE name = ?1")
            .map_err(|e| {
                let msg = format!("Failed to prepare text state statement for {}: {}", name, e);
                error!("{}", msg);
                msg
            })?;

        let value: Option<String> = stmt
            .query_row([name], |row| row.get::<usize, String>(0))
            .optional()
            .map_err(|e| {
                let msg = format!("Failed to query text state {}: {}", name, e);
                error!("{}", msg);
                msg
            })?;

        Ok(value)
    }

    pub fn upsert_balance_history_snapshot_anchor(
        &self,
        snapshot: &BalanceHistorySnapshotInfo,
    ) -> Result<(), String> {
        let stable_block_hash = snapshot.stable_block_hash.clone().ok_or_else(|| {
            let msg = format!(
                "Balance-history snapshot missing stable block hash at height {}",
                snapshot.stable_height
            );
            error!("{}", msg);
            msg
        })?;
        let latest_block_commit = snapshot.latest_block_commit.clone().ok_or_else(|| {
            let msg = format!(
                "Balance-history snapshot missing latest block commit at height {}",
                snapshot.stable_height
            );
            error!("{}", msg);
            msg
        })?;

        self.update_synced_btc_block_height(snapshot.stable_height)?;
        self.upsert_text_state(BALANCE_HISTORY_SNAPSHOT_BLOCK_HASH_KEY, &stable_block_hash)?;
        self.upsert_text_state(
            BALANCE_HISTORY_SNAPSHOT_BLOCK_COMMIT_KEY,
            &latest_block_commit,
        )?;
        self.upsert_text_state(
            BALANCE_HISTORY_SNAPSHOT_COMMIT_PROTOCOL_VERSION_KEY,
            &snapshot.commit_protocol_version,
        )?;
        self.upsert_text_state(
            BALANCE_HISTORY_SNAPSHOT_COMMIT_HASH_ALGO_KEY,
            &snapshot.commit_hash_algo,
        )?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO state (name, value)
            VALUES (?1, ?2)
            ON CONFLICT(name) DO UPDATE SET value = excluded.value;
            ",
            rusqlite::params![
                BALANCE_HISTORY_SNAPSHOT_HEIGHT_KEY,
                snapshot.stable_height as i64,
            ],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to update balance-history snapshot height in database: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_balance_history_snapshot_anchor(
        &self,
    ) -> Result<Option<BalanceHistorySnapshotAnchor>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT value FROM state WHERE name = ?1")
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement for balance-history snapshot height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
        let stable_height: Option<i64> = stmt
            .query_row([BALANCE_HISTORY_SNAPSHOT_HEIGHT_KEY], |row| {
                row.get::<usize, i64>(0)
            })
            .optional()
            .map_err(|e| {
                let msg = format!("Failed to query balance-history snapshot height: {}", e);
                error!("{}", msg);
                msg
            })?;
        drop(stmt);
        drop(conn);

        let Some(stable_height) = stable_height else {
            return Ok(None);
        };
        if stable_height < 0 {
            let msg = format!(
                "Invalid negative balance-history snapshot height: {}",
                stable_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let stable_block_hash = self
            .get_text_state(BALANCE_HISTORY_SNAPSHOT_BLOCK_HASH_KEY)?
            .ok_or_else(|| {
                let msg = "Missing balance-history snapshot block hash in database".to_string();
                error!("{}", msg);
                msg
            })?;
        let latest_block_commit = self
            .get_text_state(BALANCE_HISTORY_SNAPSHOT_BLOCK_COMMIT_KEY)?
            .ok_or_else(|| {
                let msg = "Missing balance-history snapshot block commit in database".to_string();
                error!("{}", msg);
                msg
            })?;
        let commit_protocol_version = self
            .get_text_state(BALANCE_HISTORY_SNAPSHOT_COMMIT_PROTOCOL_VERSION_KEY)?
            .ok_or_else(|| {
                let msg = "Missing balance-history snapshot commit protocol version in database"
                    .to_string();
                error!("{}", msg);
                msg
            })?;
        let commit_hash_algo = self
            .get_text_state(BALANCE_HISTORY_SNAPSHOT_COMMIT_HASH_ALGO_KEY)?
            .ok_or_else(|| {
                let msg = "Missing balance-history snapshot commit hash algorithm in database"
                    .to_string();
                error!("{}", msg);
                msg
            })?;

        Ok(Some(BalanceHistorySnapshotAnchor {
            stable_height: stable_height as u32,
            stable_block_hash,
            latest_block_commit,
            commit_protocol_version,
            commit_hash_algo,
        }))
    }

    pub fn upsert_pass_block_commit(&self, entry: &PassBlockCommitEntry) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO pass_block_commits (
                block_height,
                balance_history_block_height,
                balance_history_block_commit,
                mutation_root,
                block_commit,
                commit_protocol_version,
                commit_hash_algo
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(block_height) DO UPDATE SET
                balance_history_block_height = excluded.balance_history_block_height,
                balance_history_block_commit = excluded.balance_history_block_commit,
                mutation_root = excluded.mutation_root,
                block_commit = excluded.block_commit,
                commit_protocol_version = excluded.commit_protocol_version,
                commit_hash_algo = excluded.commit_hash_algo;
            ",
            rusqlite::params![
                entry.block_height as i64,
                entry.balance_history_block_height as i64,
                entry.balance_history_block_commit,
                entry.mutation_root,
                entry.block_commit,
                entry.commit_protocol_version,
                entry.commit_hash_algo,
            ],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to upsert pass block commit at height {}: {}",
                entry.block_height, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn get_pass_block_commit(
        &self,
        block_height: u32,
    ) -> Result<Option<StoredPassBlockCommitEntry>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
                SELECT
                    block_height,
                    balance_history_block_height,
                    balance_history_block_commit,
                    mutation_root,
                    block_commit,
                    commit_protocol_version,
                    commit_hash_algo
                FROM pass_block_commits
                WHERE block_height = ?1
                ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare pass block commit query at height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let entry = stmt
            .query_row(rusqlite::params![block_height as i64], |row| {
                Ok(StoredPassBlockCommitEntry {
                    block_height: row.get::<usize, i64>(0)? as u32,
                    balance_history_block_height: row.get::<usize, i64>(1)? as u32,
                    balance_history_block_commit: row.get(2)?,
                    mutation_root: row.get(3)?,
                    block_commit: row.get(4)?,
                    commit_protocol_version: row.get(5)?,
                    commit_hash_algo: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| {
                let msg = format!(
                    "Failed to load pass block commit at height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        Ok(entry)
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

    // Defensive guard for historical reads: fail fast if local state contains
    // any record beyond the target height, which usually indicates incomplete rollback.
    pub fn assert_no_data_after_block_height(&self, block_height: u32) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        // Check 1: synced height must not be ahead of the target block height.
        let mut synced_stmt = conn
            .prepare("SELECT value FROM state WHERE name = ?1")
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to validate synced block height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
        let synced_height: Option<i64> = synced_stmt
            .query_row([BTC_SYNCED_BLOCK_HEIGHT_KEY], |row| {
                row.get::<usize, i64>(0)
            })
            .optional()
            .map_err(|e| {
                let msg = format!(
                    "Failed to query synced block height when validating future data: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(synced_height) = synced_height {
            if synced_height < 0 {
                let msg = format!("Invalid negative synced block height: {}", synced_height);
                error!("{}", msg);
                return Err(msg);
            }

            if synced_height as u32 > block_height {
                let msg = format!(
                    "Future synced height detected: target_block_height={}, synced_block_height={}",
                    block_height, synced_height
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        // Check 2: there must be no miner pass minted after the target block height.
        let mut pass_stmt = conn
            .prepare(
                "
            SELECT
                inscription_id,
                mint_block_height
            FROM miner_passes
            WHERE mint_block_height > ?1
            ORDER BY mint_block_height ASC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to validate future miner pass data: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
        let mut pass_rows = pass_stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query future miner pass data for block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;
        if let Some(row) = pass_rows.next().map_err(|e| {
            let msg = format!("Failed to read future miner pass row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let inscription_id: String = row.get(0).map_err(|e| {
                let msg = format!(
                    "Failed to get inscription_id from future miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let mint_block_height: i64 = row.get(1).map_err(|e| {
                let msg = format!(
                    "Failed to get mint_block_height from future miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let msg = format!(
                "Future miner pass data exists: target_block_height={}, inscription_id={}, mint_block_height={}",
                block_height, inscription_id, mint_block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        // Check 3: there must be no active balance snapshot after the target block height.
        let mut snapshot_stmt = conn
            .prepare(
                "
            SELECT
                block_height
            FROM active_balance_snapshots
            WHERE block_height > ?1
            ORDER BY block_height ASC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to validate future active balance snapshots: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
        let mut snapshot_rows = snapshot_stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query future active balance snapshots for block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;
        if let Some(row) = snapshot_rows.next().map_err(|e| {
            let msg = format!("Failed to read future active balance snapshot row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let snapshot_height: i64 = row.get(0).map_err(|e| {
                let msg = format!(
                    "Failed to get block_height from future active balance snapshot row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let msg = format!(
                "Future active balance snapshot exists: target_block_height={}, snapshot_block_height={}",
                block_height, snapshot_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        // Check 4: there must be no pass state history entry after target height.
        let mut history_stmt = conn
            .prepare(
                "
            SELECT
                inscription_id,
                block_height,
                event_type
            FROM miner_pass_state_history
            WHERE block_height > ?1
            ORDER BY block_height ASC, id ASC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to validate future miner pass history: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
        let mut history_rows = history_stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query future miner pass history for block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;
        if let Some(row) = history_rows.next().map_err(|e| {
            let msg = format!("Failed to read future miner pass history row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let inscription_id: String = row.get(0).map_err(|e| {
                let msg = format!(
                    "Failed to get inscription_id from future miner pass history row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let history_height: i64 = row.get(1).map_err(|e| {
                let msg = format!(
                    "Failed to get block_height from future miner pass history row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let event_type: String = row.get(2).map_err(|e| {
                let msg = format!(
                    "Failed to get event_type from future miner pass history row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;
            let msg = format!(
                "Future miner pass history exists: target_block_height={}, inscription_id={}, history_block_height={}, event_type={}",
                block_height, inscription_id, history_height, event_type
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(())
    }

    // Fail-fast consistency check between synced block height and balance snapshots.
    // For synced_height < genesis, there must be no snapshots.
    // For synced_height >= genesis, latest snapshot must exist and exactly match synced_height.
    pub fn assert_balance_snapshot_consistency(
        &self,
        synced_height: u32,
        genesis_block_height: u32,
    ) -> Result<(), String> {
        let latest_snapshot = self.get_latest_active_balance_snapshot()?;

        if synced_height < genesis_block_height {
            if let Some(snapshot) = latest_snapshot {
                let msg = format!(
                    "Unexpected balance snapshot before genesis: synced_height={}, genesis_block_height={}, latest_snapshot_height={}",
                    synced_height, genesis_block_height, snapshot.block_height
                );
                error!("{}", msg);
                return Err(msg);
            }

            return Ok(());
        }

        let snapshot = latest_snapshot.ok_or_else(|| {
            let msg = format!(
                "Missing balance snapshot at synced height: synced_height={}, genesis_block_height={}",
                synced_height, genesis_block_height
            );
            error!("{}", msg);
            msg
        })?;

        if snapshot.block_height != synced_height {
            let msg = format!(
                "Balance snapshot height mismatch: synced_height={}, latest_snapshot_height={}",
                synced_height, snapshot.block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(())
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

    fn insert_pass_record(&self, pass_info: &MinerPassInfo) -> Result<(), String> {
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
                state,
                invalid_code,
                invalid_reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13);
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
                pass_info.invalid_code,
                pass_info.invalid_reason,
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

    fn append_pass_history_event(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        event_type: &str,
        prev_state: Option<MinerPassState>,
        new_state: MinerPassState,
        prev_owner: Option<USDBScriptHash>,
        new_owner: USDBScriptHash,
        prev_satpoint: Option<SatPoint>,
        new_satpoint: SatPoint,
    ) -> Result<(), String> {
        let prev_state = prev_state.map(|s| s.as_str().to_string());
        let prev_owner = prev_owner.map(|o| o.to_string());
        let prev_satpoint = prev_satpoint.map(|s| s.to_string());

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "
            INSERT INTO miner_pass_state_history (
                inscription_id,
                block_height,
                event_type,
                prev_state,
                new_state,
                prev_owner,
                new_owner,
                prev_satpoint,
                new_satpoint
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9);
            ",
            rusqlite::params![
                inscription_id.to_string(),
                block_height as i64,
                event_type,
                prev_state,
                new_state.as_str(),
                prev_owner,
                new_owner.to_string(),
                prev_satpoint,
                new_satpoint.to_string(),
            ],
        )
        .map_err(|e| {
            let msg = format!(
                "Failed to append miner pass history event: inscription_id={}, block_height={}, event_type={}, error={}",
                inscription_id, block_height, event_type, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn append_pass_history_event_for_test(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        event_type: &str,
        prev_state: Option<MinerPassState>,
        new_state: MinerPassState,
        prev_owner: Option<USDBScriptHash>,
        new_owner: USDBScriptHash,
        prev_satpoint: Option<SatPoint>,
        new_satpoint: SatPoint,
    ) -> Result<(), String> {
        self.append_pass_history_event(
            inscription_id,
            block_height,
            event_type,
            prev_state,
            new_state,
            prev_owner,
            new_owner,
            prev_satpoint,
            new_satpoint,
        )
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
        assert!(
            pass_info.invalid_code.is_none() && pass_info.invalid_reason.is_none(),
            "Active mint pass should not carry invalid reason metadata: id {}",
            pass_info.inscription_id
        );

        self.insert_pass_record(pass_info)
    }

    pub fn add_new_mint_pass_at_height(
        &self,
        pass_info: &MinerPassInfo,
        block_height: u32,
    ) -> Result<(), String> {
        self.add_new_mint_pass(pass_info)?;
        self.append_pass_history_event(
            &pass_info.inscription_id,
            block_height,
            PASS_HISTORY_EVENT_MINT,
            None,
            pass_info.state.clone(),
            None,
            pass_info.owner.clone(),
            None,
            pass_info.satpoint.clone(),
        )?;
        Ok(())
    }

    pub fn add_invalid_mint_pass(&self, pass_info: &MinerPassInfo) -> Result<(), String> {
        assert!(
            pass_info.state == MinerPassState::Invalid,
            "Invalid mint pass must be in Invalid state: id {}, state: {:?}",
            pass_info.inscription_id,
            pass_info.state.as_str()
        );
        assert!(
            pass_info.owner == pass_info.mint_owner,
            "Invalid mint pass owner must be the mint owner {} != {}",
            pass_info.owner,
            pass_info.mint_owner
        );
        assert!(
            pass_info.invalid_code.is_some() && pass_info.invalid_reason.is_some(),
            "Invalid mint pass must include invalid code and reason: id {}",
            pass_info.inscription_id
        );

        self.insert_pass_record(pass_info)
    }

    pub fn add_invalid_mint_pass_at_height(
        &self,
        pass_info: &MinerPassInfo,
        block_height: u32,
    ) -> Result<(), String> {
        self.add_invalid_mint_pass(pass_info)?;
        self.append_pass_history_event(
            &pass_info.inscription_id,
            block_height,
            PASS_HISTORY_EVENT_INVALID_MINT,
            None,
            pass_info.state.clone(),
            None,
            pass_info.owner.clone(),
            None,
            pass_info.satpoint.clone(),
        )?;
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

    pub fn transfer_owner_at_height(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &USDBScriptHash,
        new_satpoint: &SatPoint,
        block_height: u32,
    ) -> Result<(), String> {
        let current = self
            .get_pass_by_inscription_id(inscription_id)?
            .ok_or_else(|| {
                let msg = format!(
                    "Miner pass not found before transfer history append: inscription_id={}",
                    inscription_id
                );
                error!("{}", msg);
                msg
            })?;

        self.transfer_owner(inscription_id, new_owner, new_satpoint)?;
        self.append_pass_history_event(
            inscription_id,
            block_height,
            PASS_HISTORY_EVENT_OWNER_TRANSFER,
            Some(current.state.clone()),
            current.state,
            Some(current.owner.clone()),
            new_owner.clone(),
            Some(current.satpoint.clone()),
            new_satpoint.clone(),
        )?;

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

    pub fn update_satpoint_at_height(
        &self,
        inscription_id: &InscriptionId,
        prev_satpoint: &SatPoint,
        new_satpoint: &SatPoint,
        block_height: u32,
    ) -> Result<(), String> {
        let current = self
            .get_pass_by_inscription_id(inscription_id)?
            .ok_or_else(|| {
                let msg = format!(
                    "Miner pass not found before satpoint history append: inscription_id={}",
                    inscription_id
                );
                error!("{}", msg);
                msg
            })?;

        self.update_satpoint(inscription_id, prev_satpoint, new_satpoint)?;
        self.append_pass_history_event(
            inscription_id,
            block_height,
            PASS_HISTORY_EVENT_SATPOINT_UPDATE,
            Some(current.state.clone()),
            current.state,
            Some(current.owner.clone()),
            current.owner.clone(),
            Some(current.satpoint.clone()),
            new_satpoint.clone(),
        )?;

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

    pub fn update_state_at_height(
        &self,
        inscription_id: &InscriptionId,
        new_state: MinerPassState,
        prev_state: MinerPassState,
        block_height: u32,
    ) -> Result<(), String> {
        let current = self
            .get_pass_by_inscription_id(inscription_id)?
            .ok_or_else(|| {
                let msg = format!(
                    "Miner pass not found before state history append: inscription_id={}",
                    inscription_id
                );
                error!("{}", msg);
                msg
            })?;

        self.update_state(inscription_id, new_state.clone(), prev_state)?;
        self.append_pass_history_event(
            inscription_id,
            block_height,
            PASS_HISTORY_EVENT_STATE_UPDATE,
            Some(current.state.clone()),
            new_state,
            Some(current.owner.clone()),
            current.owner,
            Some(current.satpoint.clone()),
            current.satpoint,
        )?;

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
            invalid_code: row.get(11).map_err(|e| {
                let msg = format!(
                    "Failed to get invalid_code field from miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?,
            invalid_reason: row.get(12).map_err(|e| {
                let msg = format!(
                    "Failed to get invalid_reason field from miner pass row: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?,

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

    fn row_to_pass_history_item(row: &rusqlite::Row) -> Result<MinerPassHistoryInfo, String> {
        let event_id = row.get::<_, i64>(0).map_err(|e| {
            let msg = format!("Failed to get event_id from miner pass history row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let inscription_id = row
            .get::<_, String>(1)
            .map_err(|e| {
                let msg = format!("Failed to get inscription_id from history row: {}", e);
                error!("{}", msg);
                msg
            })?
            .parse::<InscriptionId>()
            .map_err(|e| {
                let msg = format!("Failed to parse inscription_id from history row: {}", e);
                error!("{}", msg);
                msg
            })?;
        let block_height = row.get::<_, i64>(2).map_err(|e| {
            let msg = format!("Failed to get block_height from history row: {}", e);
            error!("{}", msg);
            msg
        })?;
        if block_height < 0 {
            let msg = format!(
                "Invalid negative block_height in miner pass history row: {}",
                block_height
            );
            error!("{}", msg);
            return Err(msg);
        }
        let event_type = row.get::<_, String>(3).map_err(|e| {
            let msg = format!("Failed to get event_type from history row: {}", e);
            error!("{}", msg);
            msg
        })?;
        let state = MinerPassState::from_str(&row.get::<_, String>(4).map_err(|e| {
            let msg = format!("Failed to get new_state from history row: {}", e);
            error!("{}", msg);
            msg
        })?)?;
        let owner = row
            .get::<_, String>(5)
            .map_err(|e| {
                let msg = format!("Failed to get new_owner from history row: {}", e);
                error!("{}", msg);
                msg
            })?
            .parse::<USDBScriptHash>()
            .map_err(|e| {
                let msg = format!("Failed to parse new_owner from history row: {}", e);
                error!("{}", msg);
                msg
            })?;
        let satpoint = row
            .get::<_, String>(6)
            .map_err(|e| {
                let msg = format!("Failed to get new_satpoint from history row: {}", e);
                error!("{}", msg);
                msg
            })?
            .parse::<SatPoint>()
            .map_err(|e| {
                let msg = format!("Failed to parse new_satpoint from history row: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(MinerPassHistoryInfo {
            event_id,
            inscription_id,
            block_height: block_height as u32,
            event_type,
            state,
            owner,
            satpoint,
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

    // Get all transfer-trackable miner passes by pagination.
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
            WHERE state NOT IN (?1, ?2)
            ORDER BY mint_block_height DESC
            LIMIT ?3 OFFSET ?4;
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
                MinerPassState::Invalid.as_str(),
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

    pub fn get_last_pass_history_at_or_before_height(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<MinerPassHistoryInfo>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
            SELECT
                id,
                inscription_id,
                block_height,
                event_type,
                new_state,
                new_owner,
                new_satpoint
            FROM miner_pass_state_history
            WHERE inscription_id = ?1 AND block_height <= ?2
            ORDER BY block_height DESC, id DESC
            LIMIT 1;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get pass history snapshot at or before height: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![inscription_id.to_string(), block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query pass history snapshot for inscription {} at block height {}: {}",
                    inscription_id, block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        if let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to read pass history snapshot row: {}", e);
            error!("{}", msg);
            msg
        })? {
            let item = Self::row_to_pass_history_item(row)?;
            return Ok(Some(item));
        }

        Ok(None)
    }

    pub fn get_pass_history_by_page_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_height: u32,
        to_height: u32,
        page: usize,
        page_size: usize,
        desc: bool,
    ) -> Result<Vec<MinerPassHistoryInfo>, String> {
        if from_height > to_height {
            let msg = format!(
                "Invalid history height range: from_height {} > to_height {}",
                from_height, to_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let conn = self.conn.lock().unwrap();
        let offset = page * page_size;
        let order = if desc { "DESC" } else { "ASC" };
        let sql = format!(
            "
            SELECT
                id,
                inscription_id,
                block_height,
                event_type,
                new_state,
                new_owner,
                new_satpoint
            FROM miner_pass_state_history
            WHERE inscription_id = ?1 AND block_height >= ?2 AND block_height <= ?3
            ORDER BY block_height {}, id {}
            LIMIT ?4 OFFSET ?5;
            ",
            order, order
        );

        let mut stmt = conn.prepare(&sql).map_err(|e| {
            let msg = format!(
                "Failed to prepare statement to get pass history in range: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;

        let mut rows = stmt
            .query(rusqlite::params![
                inscription_id.to_string(),
                from_height as i64,
                to_height as i64,
                page_size as i64,
                offset as i64
            ])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query pass history in range: inscription_id={}, from_height={}, to_height={}, error={}",
                    inscription_id, from_height, to_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying pass history in range: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            events.push(Self::row_to_pass_history_item(row)?);
        }

        Ok(events)
    }

    pub fn get_pass_history_count_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_height: u32,
        to_height: u32,
    ) -> Result<u64, String> {
        if from_height > to_height {
            let msg = format!(
                "Invalid history height range: from_height {} > to_height {}",
                from_height, to_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "
                SELECT COUNT(*)
                FROM miner_pass_state_history
                WHERE inscription_id = ?1
                  AND block_height >= ?2
                  AND block_height <= ?3;
                ",
                rusqlite::params![
                    inscription_id.to_string(),
                    from_height as i64,
                    to_height as i64
                ],
                |row| row.get(0),
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to count pass history in range: inscription_id={}, from_height={}, to_height={}, error={}",
                    inscription_id, from_height, to_height, e
                );
                error!("{}", msg);
                msg
            })?;

        if count < 0 {
            let msg = format!(
                "Invalid negative pass history count: inscription_id={}, from_height={}, to_height={}, count={}",
                inscription_id, from_height, to_height, count
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(count as u64)
    }

    pub fn get_pass_state_stats_from_history_at_height(
        &self,
        block_height: u32,
    ) -> Result<MinerPassStateStats, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "
                WITH latest AS (
                    SELECT
                        inscription_id,
                        MAX(id) AS max_id
                    FROM miner_pass_state_history
                    WHERE block_height <= ?1
                    GROUP BY inscription_id
                )
                SELECT
                    h.new_state,
                    COUNT(*)
                FROM miner_pass_state_history h
                INNER JOIN latest l ON h.id = l.max_id
                GROUP BY h.new_state;
                ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get pass state stats from history: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![block_height as i64])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query pass state stats from history at block height {}: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut stats = MinerPassStateStats::default();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying pass state stats from history: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let state_text: String = row.get(0).map_err(|e| {
                let msg = format!("Failed to get state from pass state stats row: {}", e);
                error!("{}", msg);
                msg
            })?;
            let count: i64 = row.get(1).map_err(|e| {
                let msg = format!("Failed to get count from pass state stats row: {}", e);
                error!("{}", msg);
                msg
            })?;
            if count < 0 {
                let msg = format!(
                    "Invalid negative state count in pass state stats: state={}, count={}",
                    state_text, count
                );
                error!("{}", msg);
                return Err(msg);
            }

            let count = count as u64;
            stats.total_count = stats.total_count.checked_add(count).ok_or_else(|| {
                let msg = format!(
                    "Pass state stats total count overflow at block height {}",
                    block_height
                );
                error!("{}", msg);
                msg
            })?;

            let state = MinerPassState::from_str(&state_text).map_err(|e| {
                let msg = format!(
                    "Failed to parse pass state from stats row: state={}, error={}",
                    state_text, e
                );
                error!("{}", msg);
                msg
            })?;

            match state {
                MinerPassState::Active => stats.active_count = count,
                MinerPassState::Dormant => stats.dormant_count = count,
                MinerPassState::Consumed => stats.consumed_count = count,
                MinerPassState::Burned => stats.burned_count = count,
                MinerPassState::Invalid => stats.invalid_count = count,
            }
        }

        Ok(stats)
    }

    pub fn get_active_pass_count_from_history_at_height(
        &self,
        block_height: u32,
    ) -> Result<u64, String> {
        self.get_pass_count_from_history_at_height_by_states(
            block_height,
            &[MinerPassState::Active],
        )
    }

    pub fn get_pass_count_from_history_at_height_by_states(
        &self,
        block_height: u32,
        states: &[MinerPassState],
    ) -> Result<u64, String> {
        if states.is_empty() {
            return Ok(0);
        }

        let conn = self.conn.lock().unwrap();
        let placeholders = (0..states.len())
            .map(|idx| format!("?{}", idx + 2))
            .collect::<Vec<String>>()
            .join(", ");
        let sql = format!(
            "
                WITH latest AS (
                    SELECT
                        inscription_id,
                        MAX(id) AS max_id
                    FROM miner_pass_state_history
                    WHERE block_height <= ?1
                    GROUP BY inscription_id
                )
                SELECT COUNT(*)
                FROM miner_pass_state_history h
                INNER JOIN latest l ON h.id = l.max_id
                WHERE h.new_state IN ({});
                ",
            placeholders
        );

        let mut params = Vec::<rusqlite::types::Value>::with_capacity(1 + states.len());
        params.push(rusqlite::types::Value::Integer(block_height as i64));
        for state in states {
            params.push(rusqlite::types::Value::Text(state.as_str().to_string()));
        }

        let count: i64 = conn
            .query_row(&sql, rusqlite::params_from_iter(params), |row| row.get(0))
            .map_err(|e| {
                let state_names = states
                    .iter()
                    .map(|s| s.as_str().to_string())
                    .collect::<Vec<String>>()
                    .join(",");
                let msg = format!(
                    "Failed to count pass snapshot from history at block height {} with states [{}]: {}",
                    block_height, state_names, e
                );
                error!("{}", msg);
                msg
            })?;

        if count < 0 {
            let msg = format!(
                "Invalid negative pass count at block height {}: {}",
                block_height, count
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(count as u64)
    }

    pub fn get_all_active_pass_by_page_from_history_at_height(
        &self,
        page: usize,
        page_size: usize,
        block_height: u32,
    ) -> Result<Vec<ActiveMinerPassInfo>, String> {
        self.get_passes_by_page_from_history_at_height_by_states(
            page,
            page_size,
            block_height,
            &[MinerPassState::Active],
        )
    }

    pub fn get_passes_by_page_from_history_at_height_by_states(
        &self,
        page: usize,
        page_size: usize,
        block_height: u32,
        states: &[MinerPassState],
    ) -> Result<Vec<ActiveMinerPassInfo>, String> {
        if states.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let offset = page * page_size;

        let state_placeholders = (0..states.len())
            .map(|idx| format!("?{}", idx + 2))
            .collect::<Vec<String>>()
            .join(", ");
        let limit_placeholder = states.len() + 2;
        let offset_placeholder = states.len() + 3;
        let sql = format!(
            "
            WITH latest AS (
                SELECT
                    inscription_id,
                    MAX(id) AS max_id
                FROM miner_pass_state_history
                WHERE block_height <= ?1
                GROUP BY inscription_id
            )
            SELECT
                h.inscription_id,
                h.new_owner
            FROM miner_pass_state_history h
            INNER JOIN latest l ON h.id = l.max_id
            WHERE h.new_state IN ({})
            ORDER BY h.block_height DESC, h.id DESC
            LIMIT ?{} OFFSET ?{};
            ",
            state_placeholders, limit_placeholder, offset_placeholder
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get pass snapshot from history by page and states: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut params = Vec::<rusqlite::types::Value>::with_capacity(states.len() + 3);
        params.push(rusqlite::types::Value::Integer(block_height as i64));
        for state in states {
            params.push(rusqlite::types::Value::Text(state.as_str().to_string()));
        }
        params.push(rusqlite::types::Value::Integer(page_size as i64));
        params.push(rusqlite::types::Value::Integer(offset as i64));

        let mut rows = stmt
            .query(rusqlite::params_from_iter(params))
            .map_err(|e| {
                let msg = format!(
                    "Failed to query pass snapshot from history at block height {} with state filter: {}",
                    block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut passes = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to get next row when querying active pass snapshot from history: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let inscription_id = row
                .get::<_, String>(0)
                .map_err(|e| {
                    let msg = format!("Failed to get inscription_id from history row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse::<InscriptionId>()
                .map_err(|e| {
                    let msg = format!("Failed to parse inscription_id from history row: {}", e);
                    error!("{}", msg);
                    msg
                })?;
            let owner = row
                .get::<_, String>(1)
                .map_err(|e| {
                    let msg = format!("Failed to get owner from history row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse::<USDBScriptHash>()
                .map_err(|e| {
                    let msg = format!("Failed to parse owner from history row: {}", e);
                    error!("{}", msg);
                    msg
                })?;

            passes.push(ActiveMinerPassInfo {
                inscription_id,
                owner,
            });
        }

        Ok(passes)
    }

    pub fn get_owner_active_pass_from_history_at_height(
        &self,
        owner: &USDBScriptHash,
        block_height: u32,
    ) -> Result<Option<ActiveMinerPassInfo>, String> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn
            .prepare(
                "
            WITH latest AS (
                SELECT
                    inscription_id,
                    MAX(id) AS max_id
                FROM miner_pass_state_history
                WHERE block_height <= ?1
                GROUP BY inscription_id
            )
            SELECT
                h.inscription_id,
                h.new_owner
            FROM miner_pass_state_history h
            INNER JOIN latest l ON h.id = l.max_id
            WHERE h.new_state = ?2 AND h.new_owner = ?3
            ORDER BY h.block_height DESC, h.id DESC
            LIMIT 2;
            ",
            )
            .map_err(|e| {
                let msg = format!(
                    "Failed to prepare statement to get owner active pass from history: {}",
                    e
                );
                error!("{}", msg);
                msg
            })?;

        let mut rows = stmt
            .query(rusqlite::params![
                block_height as i64,
                MinerPassState::Active.as_str(),
                owner.to_string()
            ])
            .map_err(|e| {
                let msg = format!(
                    "Failed to query owner active pass from history: owner={}, block_height={}, error={}",
                    owner, block_height, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut results = Vec::<ActiveMinerPassInfo>::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!(
                "Failed to read owner active pass row from history query: {}",
                e
            );
            error!("{}", msg);
            msg
        })? {
            let inscription_id = row
                .get::<_, String>(0)
                .map_err(|e| {
                    let msg = format!(
                        "Failed to get inscription_id from owner active history row: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?
                .parse::<InscriptionId>()
                .map_err(|e| {
                    let msg = format!(
                        "Failed to parse inscription_id from owner active history row: {}",
                        e
                    );
                    error!("{}", msg);
                    msg
                })?;
            let owner = row
                .get::<_, String>(1)
                .map_err(|e| {
                    let msg = format!("Failed to get owner from owner active history row: {}", e);
                    error!("{}", msg);
                    msg
                })?
                .parse::<USDBScriptHash>()
                .map_err(|e| {
                    let msg = format!("Failed to parse owner from owner active history row: {}", e);
                    error!("{}", msg);
                    msg
                })?;
            results.push(ActiveMinerPassInfo {
                inscription_id,
                owner,
            });
        }

        if results.len() > 1 {
            let msg = format!(
                "Duplicate active owner detected in history snapshot: owner={}, block_height={}",
                owner, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(results.into_iter().next())
    }

    pub fn get_invalid_pass_count_in_height_range(
        &self,
        from_height: u32,
        to_height: u32,
        error_code: Option<&str>,
    ) -> Result<u64, String> {
        if from_height > to_height {
            let msg = format!(
                "Invalid invalid-pass height range: from_height {} > to_height {}",
                from_height, to_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let conn = self.conn.lock().unwrap();
        let (sql, params): (&str, Vec<rusqlite::types::Value>) = if let Some(code) = error_code {
            (
                "
                SELECT COUNT(*)
                FROM miner_passes
                WHERE state = ?1
                  AND mint_block_height >= ?2
                  AND mint_block_height <= ?3
                  AND invalid_code = ?4;
                ",
                vec![
                    MinerPassState::Invalid.as_str().to_string().into(),
                    (from_height as i64).into(),
                    (to_height as i64).into(),
                    code.to_string().into(),
                ],
            )
        } else {
            (
                "
                SELECT COUNT(*)
                FROM miner_passes
                WHERE state = ?1
                  AND mint_block_height >= ?2
                  AND mint_block_height <= ?3;
                ",
                vec![
                    MinerPassState::Invalid.as_str().to_string().into(),
                    (from_height as i64).into(),
                    (to_height as i64).into(),
                ],
            )
        };

        let mut stmt = conn.prepare(sql).map_err(|e| {
            let msg = format!(
                "Failed to prepare statement to count invalid passes in range: {}",
                e
            );
            error!("{}", msg);
            msg
        })?;
        let count: i64 = stmt
            .query_row(rusqlite::params_from_iter(params), |row| row.get(0))
            .map_err(|e| {
                let msg = format!(
                    "Failed to count invalid passes: from_height={}, to_height={}, error_code={:?}, error={}",
                    from_height, to_height, error_code, e
                );
                error!("{}", msg);
                msg
            })?;

        if count < 0 {
            let msg = format!(
                "Invalid negative invalid-pass count: from_height={}, to_height={}, error_code={:?}, count={}",
                from_height, to_height, error_code, count
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(count as u64)
    }

    pub fn get_invalid_passes_by_page_in_height_range(
        &self,
        from_height: u32,
        to_height: u32,
        error_code: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<MinerPassInfo>, String> {
        if from_height > to_height {
            let msg = format!(
                "Invalid invalid-pass height range: from_height {} > to_height {}",
                from_height, to_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let conn = self.conn.lock().unwrap();
        let offset = page * page_size;

        let (sql, params): (&str, Vec<rusqlite::types::Value>) = if let Some(code) = error_code {
            (
                "
                SELECT *
                FROM miner_passes
                WHERE state = ?1
                  AND mint_block_height >= ?2
                  AND mint_block_height <= ?3
                  AND invalid_code = ?4
                ORDER BY mint_block_height DESC, inscription_number DESC
                LIMIT ?5 OFFSET ?6;
                ",
                vec![
                    MinerPassState::Invalid.as_str().to_string().into(),
                    (from_height as i64).into(),
                    (to_height as i64).into(),
                    code.to_string().into(),
                    (page_size as i64).into(),
                    (offset as i64).into(),
                ],
            )
        } else {
            (
                "
                SELECT *
                FROM miner_passes
                WHERE state = ?1
                  AND mint_block_height >= ?2
                  AND mint_block_height <= ?3
                ORDER BY mint_block_height DESC, inscription_number DESC
                LIMIT ?4 OFFSET ?5;
                ",
                vec![
                    MinerPassState::Invalid.as_str().to_string().into(),
                    (from_height as i64).into(),
                    (to_height as i64).into(),
                    (page_size as i64).into(),
                    (offset as i64).into(),
                ],
            )
        };

        let mut stmt = conn.prepare(sql).map_err(|e| {
            let msg = format!("Failed to prepare statement to query invalid passes: {}", e);
            error!("{}", msg);
            msg
        })?;

        let mut rows = stmt
            .query(rusqlite::params_from_iter(params))
            .map_err(|e| {
                let msg = format!(
                    "Failed to query invalid passes: from_height={}, to_height={}, error_code={:?}, error={}",
                    from_height, to_height, error_code, e
                );
                error!("{}", msg);
                msg
            })?;

        let mut ret = Vec::new();
        while let Some(row) = rows.next().map_err(|e| {
            let msg = format!("Failed to get next row when querying invalid passes: {}", e);
            error!("{}", msg);
            msg
        })? {
            ret.push(Self::row_to_pass_item(row)?);
        }

        Ok(ret)
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
    use std::collections::{HashMap, HashSet};
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
            invalid_code: None,
            invalid_reason: None,
            owner,
            state,
        }
    }

    #[derive(Clone)]
    struct ModelPassEvent {
        block_height: u32,
        event_type: &'static str,
        state: MinerPassState,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    }

    fn model_last_event_at_or_before(
        events: &[ModelPassEvent],
        block_height: u32,
    ) -> Option<&ModelPassEvent> {
        events
            .iter()
            .rev()
            .find(|event| event.block_height <= block_height)
    }

    fn load_all_active_from_history(
        storage: &MinerPassStorage,
        block_height: u32,
        page_size: usize,
    ) -> Vec<ActiveMinerPassInfo> {
        let mut page = 0usize;
        let mut rows = Vec::new();
        loop {
            let current = storage
                .get_all_active_pass_by_page_from_history_at_height(page, page_size, block_height)
                .unwrap();
            if current.is_empty() {
                break;
            }
            rows.extend(current);
            page += 1;
        }
        rows
    }

    // Deterministic pseudo-random generator (LCG) for reproducible property-like tests.
    fn lcg_next(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *seed
    }

    fn random_owner(seed: &mut u64) -> USDBScriptHash {
        let tag = ((lcg_next(seed) % 16) as u8).wrapping_add(10);
        script_hash(tag)
    }

    fn has_other_active_owner(
        snapshots: &HashMap<InscriptionId, ModelPassEvent>,
        owner: USDBScriptHash,
        self_pass: Option<&InscriptionId>,
    ) -> bool {
        snapshots.iter().any(|(id, event)| {
            event.state == MinerPassState::Active
                && event.owner == owner
                && self_pass.map(|v| v != id).unwrap_or(true)
        })
    }

    fn random_owner_without_active_conflict(
        seed: &mut u64,
        snapshots: &HashMap<InscriptionId, ModelPassEvent>,
        self_pass: Option<&InscriptionId>,
    ) -> Option<USDBScriptHash> {
        for _ in 0..64 {
            let owner = random_owner(seed);
            if !has_other_active_owner(snapshots, owner, self_pass) {
                return Some(owner);
            }
        }
        None
    }

    fn random_satpoint(seed: &mut u64, nonce: u32) -> SatPoint {
        let tag = ((lcg_next(seed) & 0xff) as u8).wrapping_add((nonce & 0xff) as u8);
        let vout = (lcg_next(seed) % 4) as u32;
        let offset = lcg_next(seed) % 10_000;
        satpoint(tag, vout, offset)
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
    fn test_transfer_active_to_owner_with_active_requires_dormant_first() {
        let dir = test_data_dir("transfer_active_requires_dormant");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner_a = script_hash(61);
        let owner_b = script_hash(62);

        let p1 = make_pass(71, 0, owner_a, MinerPassState::Active, 100);
        let p2 = make_pass(72, 1, owner_b, MinerPassState::Active, 101);
        storage.add_new_mint_pass(&p1).unwrap();
        storage.add_new_mint_pass(&p2).unwrap();

        let new_satpoint = satpoint(73, 2, 33);
        let err = storage.transfer_owner(&p1.inscription_id, &owner_b, &new_satpoint);
        assert!(err.is_err());

        storage
            .update_state(
                &p1.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
            )
            .unwrap();
        storage
            .transfer_owner(&p1.inscription_id, &owner_b, &new_satpoint)
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&p1.inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.owner, owner_b);
        assert_eq!(updated.state, MinerPassState::Dormant);
        assert_eq!(updated.satpoint, new_satpoint);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_add_invalid_mint_pass_and_exclude_from_valid_tracking_query() {
        let dir = test_data_dir("invalid_pass_tracking");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner = script_hash(91);

        let mut invalid_pass = make_pass(92, 0, owner, MinerPassState::Invalid, 123);
        invalid_pass.prev = Vec::new();
        invalid_pass.invalid_code = Some("INVALID_ETH_MAIN".to_string());
        invalid_pass.invalid_reason = Some("Invalid eth_main format".to_string());

        storage.add_invalid_mint_pass(&invalid_pass).unwrap();

        let loaded = storage
            .get_pass_by_inscription_id(&invalid_pass.inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, MinerPassState::Invalid);
        assert_eq!(loaded.invalid_code.as_deref(), Some("INVALID_ETH_MAIN"));
        assert_eq!(
            loaded.invalid_reason.as_deref(),
            Some("Invalid eth_main format")
        );

        let valid_passes = storage.get_all_valid_pass_by_page(0, 10).unwrap();
        assert!(valid_passes.is_empty());

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
    fn test_pass_history_random_sequence_matches_reference_model_by_height() {
        // Purpose: generate a long deterministic random operation sequence and verify
        // history queries match an in-memory reference model at every height.
        let dir = test_data_dir("history_random_sequence_reference_model");
        let storage = MinerPassStorage::new(&dir).unwrap();

        let pass_specs: Vec<(u8, u32)> = vec![
            (131, 0),
            (132, 1),
            (133, 2),
            (134, 3),
            (135, 4),
            (136, 5),
            (137, 6),
            (138, 7),
        ];
        let pass_ids: Vec<InscriptionId> = pass_specs
            .iter()
            .map(|(tag, index)| inscription_id(*tag, *index))
            .collect();

        let mut model_events: HashMap<InscriptionId, Vec<ModelPassEvent>> = HashMap::new();
        let mut current_snapshot: HashMap<InscriptionId, ModelPassEvent> = HashMap::new();
        let mut minted = vec![false; pass_specs.len()];

        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let base_height = 1_000u32;
        let mut current_height = base_height;

        // Ensure the sequence starts with at least one minted pass.
        let owner0 =
            random_owner_without_active_conflict(&mut seed, &current_snapshot, None).unwrap();
        let pass0 = make_pass(
            pass_specs[0].0,
            pass_specs[0].1,
            owner0,
            MinerPassState::Active,
            base_height,
        );
        storage
            .add_new_mint_pass_at_height(&pass0, base_height)
            .unwrap();
        let first_event = ModelPassEvent {
            block_height: base_height,
            event_type: PASS_HISTORY_EVENT_MINT,
            state: MinerPassState::Active,
            owner: pass0.owner,
            satpoint: pass0.satpoint,
        };
        minted[0] = true;
        model_events
            .entry(pass0.inscription_id)
            .or_default()
            .push(first_event.clone());
        current_snapshot.insert(pass0.inscription_id, first_event);

        let operation_steps = 300usize;
        for step in 0..operation_steps {
            current_height = current_height.saturating_add((lcg_next(&mut seed) % 3) as u32);
            let op_kind = (lcg_next(&mut seed) % 4) as usize;

            // Prefer mint while there are still unminted passes.
            let has_unminted = minted.iter().any(|v| !*v);
            if op_kind == 0 || (!has_unminted && current_snapshot.is_empty()) {
                if has_unminted {
                    let candidates: Vec<usize> = minted
                        .iter()
                        .enumerate()
                        .filter_map(|(idx, is_minted)| if !*is_minted { Some(idx) } else { None })
                        .collect();
                    let pick = (lcg_next(&mut seed) % candidates.len() as u64) as usize;
                    let idx = candidates[pick];
                    let Some(owner) =
                        random_owner_without_active_conflict(&mut seed, &current_snapshot, None)
                    else {
                        continue;
                    };
                    let pass = make_pass(
                        pass_specs[idx].0,
                        pass_specs[idx].1,
                        owner,
                        MinerPassState::Active,
                        current_height,
                    );
                    storage
                        .add_new_mint_pass_at_height(&pass, current_height)
                        .unwrap();

                    let event = ModelPassEvent {
                        block_height: current_height,
                        event_type: PASS_HISTORY_EVENT_MINT,
                        state: MinerPassState::Active,
                        owner: pass.owner,
                        satpoint: pass.satpoint,
                    };
                    minted[idx] = true;
                    model_events
                        .entry(pass.inscription_id)
                        .or_default()
                        .push(event.clone());
                    current_snapshot.insert(pass.inscription_id, event);
                    continue;
                }
            }

            if current_snapshot.is_empty() {
                continue;
            }

            let active_ids: Vec<InscriptionId> = current_snapshot.keys().cloned().collect();
            let chosen_index = (lcg_next(&mut seed) % active_ids.len() as u64) as usize;
            let chosen_id = active_ids[chosen_index];
            let current = current_snapshot.get(&chosen_id).unwrap().clone();

            match op_kind {
                // Mint was already handled in the branch above; keep as explicit no-op here.
                0 => continue,
                // Random legal state transition (active/dormant only) keeps sequence valid.
                1 => {
                    let next_state = match current.state {
                        MinerPassState::Active => MinerPassState::Dormant,
                        MinerPassState::Dormant => {
                            if (lcg_next(&mut seed) & 1) == 0
                                && !has_other_active_owner(
                                    &current_snapshot,
                                    current.owner,
                                    Some(&chosen_id),
                                )
                            {
                                MinerPassState::Active
                            } else {
                                MinerPassState::Burned
                            }
                        }
                        MinerPassState::Burned
                        | MinerPassState::Consumed
                        | MinerPassState::Invalid => {
                            continue;
                        }
                    };

                    storage
                        .update_state_at_height(
                            &chosen_id,
                            next_state.clone(),
                            current.state.clone(),
                            current_height,
                        )
                        .unwrap();

                    let event = ModelPassEvent {
                        block_height: current_height,
                        event_type: PASS_HISTORY_EVENT_STATE_UPDATE,
                        state: next_state,
                        owner: current.owner,
                        satpoint: current.satpoint,
                    };
                    model_events
                        .entry(chosen_id)
                        .or_default()
                        .push(event.clone());
                    current_snapshot.insert(chosen_id, event);
                }
                // Owner transfer (state unchanged).
                2 => {
                    let new_owner = if current.state == MinerPassState::Active {
                        let Some(owner) = random_owner_without_active_conflict(
                            &mut seed,
                            &current_snapshot,
                            Some(&chosen_id),
                        ) else {
                            continue;
                        };
                        owner
                    } else {
                        random_owner(&mut seed)
                    };
                    let new_satpoint = random_satpoint(&mut seed, step as u32);
                    storage
                        .transfer_owner_at_height(
                            &chosen_id,
                            &new_owner,
                            &new_satpoint,
                            current_height,
                        )
                        .unwrap();

                    let event = ModelPassEvent {
                        block_height: current_height,
                        event_type: PASS_HISTORY_EVENT_OWNER_TRANSFER,
                        state: current.state,
                        owner: new_owner,
                        satpoint: new_satpoint,
                    };
                    model_events
                        .entry(chosen_id)
                        .or_default()
                        .push(event.clone());
                    current_snapshot.insert(chosen_id, event);
                }
                // Satpoint update (state/owner unchanged).
                3 => {
                    let new_satpoint = random_satpoint(&mut seed, step as u32 + 7_000);
                    storage
                        .update_satpoint_at_height(
                            &chosen_id,
                            &current.satpoint,
                            &new_satpoint,
                            current_height,
                        )
                        .unwrap();

                    let event = ModelPassEvent {
                        block_height: current_height,
                        event_type: PASS_HISTORY_EVENT_SATPOINT_UPDATE,
                        state: current.state,
                        owner: current.owner,
                        satpoint: new_satpoint,
                    };
                    model_events
                        .entry(chosen_id)
                        .or_default()
                        .push(event.clone());
                    current_snapshot.insert(chosen_id, event);
                }
                _ => unreachable!(),
            }
        }

        for query_height in base_height..=current_height {
            // Compare per-pass latest snapshot at or before query height.
            for pass_id in &pass_ids {
                let expected = model_events
                    .get(pass_id)
                    .and_then(|events| model_last_event_at_or_before(events, query_height));
                let actual = storage
                    .get_last_pass_history_at_or_before_height(pass_id, query_height)
                    .unwrap();

                match (expected, actual) {
                    (None, None) => {}
                    (Some(expected), Some(actual)) => {
                        assert_eq!(actual.block_height, expected.block_height);
                        assert_eq!(actual.event_type, expected.event_type);
                        assert_eq!(actual.state, expected.state);
                        assert_eq!(actual.owner, expected.owner);
                        assert_eq!(actual.satpoint, expected.satpoint);
                    }
                    (None, Some(actual)) => {
                        panic!(
                            "Unexpected history snapshot at height {} for pass {}: {:?}",
                            query_height, pass_id, actual
                        );
                    }
                    (Some(expected), None) => {
                        panic!(
                            "Missing history snapshot at height {} for pass {}: expected event_type={}, state={:?}",
                            query_height, pass_id, expected.event_type, expected.state
                        );
                    }
                }
            }

            // Compare active-pass snapshot page query vs reference model.
            let expected_active: HashMap<InscriptionId, USDBScriptHash> = pass_ids
                .iter()
                .filter_map(|pass_id| {
                    model_events
                        .get(pass_id)
                        .and_then(|events| model_last_event_at_or_before(events, query_height))
                        .and_then(|event| {
                            if event.state == MinerPassState::Active {
                                Some((pass_id.clone(), event.owner))
                            } else {
                                None
                            }
                        })
                })
                .collect();

            let actual_rows = load_all_active_from_history(&storage, query_height, 3);
            let mut actual_active = HashMap::<InscriptionId, USDBScriptHash>::new();
            let mut seen_pass = HashSet::<InscriptionId>::new();
            for row in actual_rows {
                assert!(
                    seen_pass.insert(row.inscription_id),
                    "Duplicate active pass row returned across pages: height={}, inscription_id={}",
                    query_height,
                    row.inscription_id
                );
                actual_active.insert(row.inscription_id, row.owner);
            }

            assert_eq!(
                actual_active, expected_active,
                "Active snapshot mismatch at height {}",
                query_height
            );
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_history_snapshot_query_tracks_state_timeline() {
        let dir = test_data_dir("history_snapshot_timeline");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(51);
        let owner2 = script_hash(52);

        let p = make_pass(81, 0, owner1, MinerPassState::Active, 100);
        storage.add_new_mint_pass_at_height(&p, 100).unwrap();

        let at_100 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 100)
            .unwrap();
        assert_eq!(at_100.len(), 1);
        assert_eq!(at_100[0].inscription_id, p.inscription_id);
        assert_eq!(at_100[0].owner, owner1);

        storage
            .update_state_at_height(
                &p.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                110,
            )
            .unwrap();
        let at_105 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 105)
            .unwrap();
        assert_eq!(at_105.len(), 1);
        assert_eq!(at_105[0].owner, owner1);
        let at_110 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 110)
            .unwrap();
        assert!(at_110.is_empty());

        let moved_satpoint = satpoint(82, 1, 7);
        storage
            .transfer_owner_at_height(&p.inscription_id, &owner2, &moved_satpoint, 120)
            .unwrap();
        storage
            .update_state_at_height(
                &p.inscription_id,
                MinerPassState::Active,
                MinerPassState::Dormant,
                130,
            )
            .unwrap();

        let at_125 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 125)
            .unwrap();
        assert!(at_125.is_empty());
        let at_130 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 130)
            .unwrap();
        assert_eq!(at_130.len(), 1);
        assert_eq!(at_130[0].owner, owner2);

        let history_at_129 = storage
            .get_last_pass_history_at_or_before_height(&p.inscription_id, 129)
            .unwrap()
            .unwrap();
        assert_eq!(history_at_129.state, MinerPassState::Dormant);
        assert_eq!(history_at_129.owner, owner2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_history_same_height_multi_event_order_keeps_last_state() {
        let dir = test_data_dir("history_same_height_event_order");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(61);
        let owner2 = script_hash(62);
        let pass = make_pass(91, 0, owner1, MinerPassState::Active, 100);

        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();

        // Apply multiple state/owner transitions at the same block height.
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();
        let moved_satpoint = satpoint(92, 1, 3);
        storage
            .transfer_owner_at_height(&pass.inscription_id, &owner2, &moved_satpoint, 120)
            .unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Active,
                MinerPassState::Dormant,
                120,
            )
            .unwrap();

        // At height 120, snapshot should resolve to the last event (state_update -> Active).
        let latest = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 120)
            .unwrap()
            .unwrap();
        assert_eq!(latest.block_height, 120);
        assert_eq!(latest.state, MinerPassState::Active);
        assert_eq!(latest.owner, owner2);
        assert_eq!(latest.satpoint, moved_satpoint);
        assert_eq!(latest.event_type, PASS_HISTORY_EVENT_STATE_UPDATE);

        let active = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 120)
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].inscription_id, pass.inscription_id);
        assert_eq!(active[0].owner, owner2);

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
    fn test_pass_history_savepoint_guard_rollback_and_commit() {
        let dir = test_data_dir("history_savepoint");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(41);
        let owner2 = script_hash(42);

        let base_pass = make_pass(101, 0, owner1, MinerPassState::Active, 100);
        storage
            .add_new_mint_pass_at_height(&base_pass, base_pass.mint_block_height)
            .unwrap();

        // Roll back newly inserted pass + history entries in one savepoint scope.
        let rollback_pass = make_pass(102, 1, owner2, MinerPassState::Active, 101);
        {
            let _guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage
                .add_new_mint_pass_at_height(&rollback_pass, rollback_pass.mint_block_height)
                .unwrap();
            storage
                .update_state_at_height(
                    &rollback_pass.inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    101,
                )
                .unwrap();
        }
        assert!(
            storage
                .get_pass_by_inscription_id(&rollback_pass.inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            storage
                .get_last_pass_history_at_or_before_height(&rollback_pass.inscription_id, u32::MAX)
                .unwrap()
                .is_none()
        );

        // Roll back history update on existing pass, ensure latest snapshot stays unchanged.
        {
            let _guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage
                .update_state_at_height(
                    &base_pass.inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    102,
                )
                .unwrap();
        }
        let after_rollback = storage
            .get_last_pass_history_at_or_before_height(&base_pass.inscription_id, u32::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(after_rollback.block_height, 100);
        assert_eq!(after_rollback.state, MinerPassState::Active);
        assert_eq!(after_rollback.event_type, PASS_HISTORY_EVENT_MINT);

        // Commit history update, then latest snapshot should move to committed height.
        {
            let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
            storage
                .update_state_at_height(
                    &base_pass.inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    103,
                )
                .unwrap();
            guard.commit().unwrap();
        }
        let after_commit = storage
            .get_last_pass_history_at_or_before_height(&base_pass.inscription_id, u32::MAX)
            .unwrap()
            .unwrap();
        assert_eq!(after_commit.block_height, 103);
        assert_eq!(after_commit.state, MinerPassState::Dormant);
        assert_eq!(after_commit.event_type, PASS_HISTORY_EVENT_STATE_UPDATE);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_history_active_pagination_is_stable_without_duplicates_or_gaps() {
        let dir = test_data_dir("history_active_paging_stable");
        let storage = MinerPassStorage::new(&dir).unwrap();

        let page_size = 17usize;
        let total_active = 73usize; // > 2 pages
        let inactive_count = 11usize;
        let base_height = 1_000u32;

        // Build many active passes in history.
        let mut expected_active_ids = std::collections::HashSet::<InscriptionId>::new();
        for i in 0..total_active {
            let tag = ((i % 200) as u8).wrapping_add(1);
            let owner = script_hash((i % 240) as u8 + 1);
            let pass = make_pass(
                tag,
                i as u32,
                owner,
                MinerPassState::Active,
                base_height + i as u32,
            );
            storage
                .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
                .unwrap();
            expected_active_ids.insert(pass.inscription_id);
        }

        // Add some non-active passes to ensure query filter correctness.
        for i in 0..inactive_count {
            let tag = ((i % 200) as u8).wrapping_add(120);
            let owner = script_hash((i % 240) as u8 + 120);
            let pass = make_pass(
                tag,
                (total_active + i) as u32,
                owner,
                MinerPassState::Active,
                base_height + total_active as u32 + i as u32,
            );
            storage
                .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
                .unwrap();
            storage
                .update_state_at_height(
                    &pass.inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    pass.mint_block_height + 1,
                )
                .unwrap();
        }

        let query_height = base_height + total_active as u32 + inactive_count as u32 + 10;
        let mut paged_ids = std::collections::HashSet::<InscriptionId>::new();
        let mut total_rows = 0usize;
        let mut page = 0usize;
        loop {
            let rows = storage
                .get_all_active_pass_by_page_from_history_at_height(page, page_size, query_height)
                .unwrap();
            if rows.is_empty() {
                break;
            }

            for row in rows {
                assert!(
                    paged_ids.insert(row.inscription_id),
                    "Duplicate inscription id returned across pages: {}",
                    row.inscription_id
                );
                total_rows += 1;
            }

            page += 1;
        }

        assert_eq!(total_rows, total_active);
        assert_eq!(paged_ids, expected_active_ids);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_history_height_boundary_semantics_h_minus_1_h_h_plus_1() {
        // Purpose: lock the history snapshot boundary contract.
        // A query at height `h` must include events written at `h` (inclusive),
        // while `h-1` must still observe the previous state.
        let dir = test_data_dir("history_height_boundary_semantics");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(71);
        let owner2 = script_hash(72);
        let pass = make_pass(111, 0, owner1, MinerPassState::Active, 100);

        storage
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();
        let moved_satpoint = satpoint(112, 1, 9);
        storage
            .transfer_owner_at_height(&pass.inscription_id, &owner2, &moved_satpoint, 140)
            .unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Active,
                MinerPassState::Dormant,
                150,
            )
            .unwrap();

        // state update boundary at height 120
        let at_119 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 119)
            .unwrap()
            .unwrap();
        assert_eq!(at_119.state, MinerPassState::Active);
        assert_eq!(at_119.owner, owner1);
        let at_120 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 120)
            .unwrap()
            .unwrap();
        assert_eq!(at_120.state, MinerPassState::Dormant);
        assert_eq!(at_120.owner, owner1);
        let at_121 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 121)
            .unwrap()
            .unwrap();
        assert_eq!(at_121.state, MinerPassState::Dormant);
        assert_eq!(at_121.owner, owner1);

        // owner transfer boundary at height 140
        let at_139 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 139)
            .unwrap()
            .unwrap();
        assert_eq!(at_139.state, MinerPassState::Dormant);
        assert_eq!(at_139.owner, owner1);
        let at_140 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 140)
            .unwrap()
            .unwrap();
        assert_eq!(at_140.state, MinerPassState::Dormant);
        assert_eq!(at_140.owner, owner2);
        let at_141 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 141)
            .unwrap()
            .unwrap();
        assert_eq!(at_141.state, MinerPassState::Dormant);
        assert_eq!(at_141.owner, owner2);

        // re-activation boundary at height 150
        let at_149 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 149)
            .unwrap()
            .unwrap();
        assert_eq!(at_149.state, MinerPassState::Dormant);
        assert_eq!(at_149.owner, owner2);
        let at_150 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 150)
            .unwrap()
            .unwrap();
        assert_eq!(at_150.state, MinerPassState::Active);
        assert_eq!(at_150.owner, owner2);
        let at_151 = storage
            .get_last_pass_history_at_or_before_height(&pass.inscription_id, 151)
            .unwrap()
            .unwrap();
        assert_eq!(at_151.state, MinerPassState::Active);
        assert_eq!(at_151.owner, owner2);

        // Also verify the active-pass set follows the same inclusive boundary semantics.
        let active_119 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 119)
            .unwrap();
        assert_eq!(active_119.len(), 1);
        assert_eq!(active_119[0].inscription_id, pass.inscription_id);
        assert_eq!(active_119[0].owner, owner1);

        let active_120 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 120)
            .unwrap();
        assert!(active_120.is_empty());

        let active_149 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 149)
            .unwrap();
        assert!(active_149.is_empty());

        let active_150 = storage
            .get_all_active_pass_by_page_from_history_at_height(0, 10, 150)
            .unwrap();
        assert_eq!(active_150.len(), 1);
        assert_eq!(active_150[0].inscription_id, pass.inscription_id);
        assert_eq!(active_150[0].owner, owner2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_history_stats_and_count_queries() {
        // Purpose: verify history-based count/stat APIs used by RPC pagination
        // and dashboard aggregation logic.
        let dir = test_data_dir("history_stats_and_count");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner1 = script_hash(81);
        let owner2 = script_hash(82);
        let owner3 = script_hash(83);

        let active_pass = make_pass(121, 0, owner1, MinerPassState::Active, 100);
        storage
            .add_new_mint_pass_at_height(&active_pass, active_pass.mint_block_height)
            .unwrap();

        let dormant_pass = make_pass(122, 0, owner2, MinerPassState::Active, 101);
        storage
            .add_new_mint_pass_at_height(&dormant_pass, dormant_pass.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &dormant_pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();

        let invalid_pass = make_pass(123, 0, owner3, MinerPassState::Invalid, 110);
        let mut invalid_pass = invalid_pass;
        invalid_pass.invalid_code = Some("INVALID_ETH_MAIN".to_string());
        invalid_pass.invalid_reason = Some("invalid eth_main in test".to_string());
        storage
            .add_invalid_mint_pass_at_height(&invalid_pass, invalid_pass.mint_block_height)
            .unwrap();

        let stats_105 = storage
            .get_pass_state_stats_from_history_at_height(105)
            .unwrap();
        assert_eq!(
            stats_105,
            MinerPassStateStats {
                total_count: 2,
                active_count: 2,
                dormant_count: 0,
                consumed_count: 0,
                burned_count: 0,
                invalid_count: 0,
            }
        );

        let stats_120 = storage
            .get_pass_state_stats_from_history_at_height(120)
            .unwrap();
        assert_eq!(
            stats_120,
            MinerPassStateStats {
                total_count: 3,
                active_count: 1,
                dormant_count: 1,
                consumed_count: 0,
                burned_count: 0,
                invalid_count: 1,
            }
        );

        let active_count_120 = storage
            .get_active_pass_count_from_history_at_height(120)
            .unwrap();
        assert_eq!(active_count_120, 1);

        let active_dormant_count_120 = storage
            .get_pass_count_from_history_at_height_by_states(
                120,
                &[MinerPassState::Active, MinerPassState::Dormant],
            )
            .unwrap();
        assert_eq!(active_dormant_count_120, 2);

        let all_state_count_120 = storage
            .get_pass_count_from_history_at_height_by_states(
                120,
                &[
                    MinerPassState::Active,
                    MinerPassState::Dormant,
                    MinerPassState::Consumed,
                    MinerPassState::Burned,
                    MinerPassState::Invalid,
                ],
            )
            .unwrap();
        assert_eq!(all_state_count_120, 3);

        let active_dormant_rows = storage
            .get_passes_by_page_from_history_at_height_by_states(
                0,
                10,
                120,
                &[MinerPassState::Active, MinerPassState::Dormant],
            )
            .unwrap();
        assert_eq!(active_dormant_rows.len(), 2);

        let empty_state_count = storage
            .get_pass_count_from_history_at_height_by_states(120, &[])
            .unwrap();
        assert_eq!(empty_state_count, 0);

        let empty_state_rows = storage
            .get_passes_by_page_from_history_at_height_by_states(0, 10, 120, &[])
            .unwrap();
        assert!(empty_state_rows.is_empty());

        let history_count = storage
            .get_pass_history_count_in_height_range(&dormant_pass.inscription_id, 100, 130)
            .unwrap();
        assert_eq!(history_count, 2);

        let invalid_all = storage
            .get_invalid_pass_count_in_height_range(100, 130, None)
            .unwrap();
        assert_eq!(invalid_all, 1);

        let invalid_code = storage
            .get_invalid_pass_count_in_height_range(100, 130, Some("INVALID_ETH_MAIN"))
            .unwrap();
        assert_eq!(invalid_code, 1);

        let invalid_other = storage
            .get_invalid_pass_count_in_height_range(100, 130, Some("INVALID_SCHEMA"))
            .unwrap();
        assert_eq!(invalid_other, 0);

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

    #[test]
    fn test_assert_no_data_after_block_height_detect_future_pass() {
        let dir = test_data_dir("guard_future_pass");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner = script_hash(41);

        let p = make_pass(61, 0, owner, MinerPassState::Active, 150);
        storage.add_new_mint_pass(&p).unwrap();

        let err = storage.assert_no_data_after_block_height(100).unwrap_err();
        assert!(err.contains("Future miner pass data exists"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_no_data_after_block_height_detect_future_snapshot() {
        let dir = test_data_dir("guard_future_snapshot");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_active_balance_snapshot(120, 1_000, 1)
            .unwrap();

        let err = storage.assert_no_data_after_block_height(100).unwrap_err();
        assert!(err.contains("Future active balance snapshot exists"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_no_data_after_block_height_detect_future_history() {
        let dir = test_data_dir("guard_future_history");
        let storage = MinerPassStorage::new(&dir).unwrap();
        let owner = script_hash(43);

        let p = make_pass(63, 0, owner, MinerPassState::Active, 100);
        storage.add_new_mint_pass_at_height(&p, 100).unwrap();
        storage
            .update_state_at_height(
                &p.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();

        let err = storage.assert_no_data_after_block_height(100).unwrap_err();
        assert!(err.contains("Future miner pass history exists"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_no_data_after_block_height_detect_future_synced_height() {
        let dir = test_data_dir("guard_future_synced");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage.update_synced_btc_block_height(130).unwrap();

        let err = storage.assert_no_data_after_block_height(100).unwrap_err();
        assert!(err.contains("Future synced height detected"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_balance_snapshot_consistency_allow_no_snapshot_before_genesis() {
        let dir = test_data_dir("snapshot_consistency_before_genesis_ok");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .assert_balance_snapshot_consistency(99, 100)
            .unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_balance_snapshot_consistency_reject_snapshot_before_genesis() {
        let dir = test_data_dir("snapshot_consistency_before_genesis_err");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage.upsert_active_balance_snapshot(99, 1000, 1).unwrap();

        let err = storage
            .assert_balance_snapshot_consistency(99, 100)
            .unwrap_err();
        assert!(err.contains("Unexpected balance snapshot before genesis"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_balance_snapshot_consistency_reject_missing_snapshot_after_genesis() {
        let dir = test_data_dir("snapshot_consistency_missing_after_genesis");
        let storage = MinerPassStorage::new(&dir).unwrap();

        let err = storage
            .assert_balance_snapshot_consistency(100, 100)
            .unwrap_err();
        assert!(err.contains("Missing balance snapshot at synced height"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_balance_snapshot_consistency_reject_mismatched_snapshot_height() {
        let dir = test_data_dir("snapshot_consistency_mismatch");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_active_balance_snapshot(120, 1234, 2)
            .unwrap();

        let err = storage
            .assert_balance_snapshot_consistency(121, 100)
            .unwrap_err();
        assert!(err.contains("Balance snapshot height mismatch"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_assert_balance_snapshot_consistency_accept_matched_snapshot_height() {
        let dir = test_data_dir("snapshot_consistency_match");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_active_balance_snapshot(130, 5678, 3)
            .unwrap();

        storage
            .assert_balance_snapshot_consistency(130, 100)
            .unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_balance_history_snapshot_anchor_round_trip() {
        let dir = test_data_dir("balance_history_snapshot_anchor_round_trip");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_balance_history_snapshot_anchor(&BalanceHistorySnapshotInfo {
                stable_height: 321,
                stable_block_hash: Some("ab".repeat(32)),
                latest_block_commit: Some("cd".repeat(32)),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let anchor = storage
            .get_balance_history_snapshot_anchor()
            .unwrap()
            .unwrap();
        assert_eq!(anchor.stable_height, 321);
        assert_eq!(anchor.stable_block_hash, "ab".repeat(32));
        assert_eq!(anchor.latest_block_commit, "cd".repeat(32));
        assert_eq!(anchor.commit_protocol_version, "1.0.0");
        assert_eq!(anchor.commit_hash_algo, "sha256");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_pass_block_commit_round_trip() {
        let dir = test_data_dir("pass_block_commit_round_trip");
        let storage = MinerPassStorage::new(&dir).unwrap();

        storage
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 456,
                balance_history_block_height: 456,
                balance_history_block_commit: "ab".repeat(32),
                mutation_root: "cd".repeat(32),
                block_commit: "ef".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let commit = storage.get_pass_block_commit(456).unwrap().unwrap();
        assert_eq!(commit.block_height, 456);
        assert_eq!(commit.balance_history_block_height, 456);
        assert_eq!(commit.balance_history_block_commit, "ab".repeat(32));
        assert_eq!(commit.mutation_root, "cd".repeat(32));
        assert_eq!(commit.block_commit, "ef".repeat(32));
        assert_eq!(commit.commit_protocol_version, "1.0.0");
        assert_eq!(commit.commit_hash_algo, "sha256");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
