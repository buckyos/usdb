use std::path::{PathBuf, Path};
use std::sync::Mutex;
use rusqlite::Connection;
use ord::InscriptionId;
use crate::index::MinerPassState;
use bitcoincore_rpc::bitcoin::address::{Address, NetworkUnchecked};
use bitcoincore_rpc::bitcoin::{Network, Txid};

pub struct MinerPassInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    
    pub mint_txid: Txid,
    pub mint_block_height: u64,

    pub owner: Address<NetworkUnchecked>,
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<InscriptionId>,

    pub state: MinerPassState,
}

pub struct MinerPassStorage {
    db_path: PathBuf,
    network: Network,
    conn: Mutex<Connection>,
}

impl MinerPassStorage {
    pub fn new(data_dir: &Path, network: Network) -> Result<Self, String> {
        let db_path = data_dir.join(crate::constants::MINER_PASS_DB_FILE);

        let conn = Connection::open(&db_path).map_err(|e| {
            let msg = format!("Failed to open MinerPassStorage database at {:?}: {}", db_path, e);
            error!("{}", msg);
            msg
        })?;

        // Init the database
        let storage = MinerPassStorage {
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
            CREATE TABLE IF NOT EXISTS miner_passes (
                sequence_number INTEGER PRIMARY KEY AUTOINCREMENT,

                inscription_id TEXT NOT NULL UNIQUE,
                inscription_number INTEGER NOT NULL,

                mint_txid TEXT NOT NULL,
                mint_block_height INTEGER NOT NULL,
                owner TEXT NOT NULL,

                eth_main TEXT NOT NULL,
                eth_collab TEXT,
                prev TEXT NOT NULL,
                state TEXT NOT NULL,

                INDEX idx_miner_pass_inscription_id (inscription_id),
            );
            ",
        ).map_err(|e| {
            let msg = format!("Failed to initialize MinerPassStorage database: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(())
    }

    pub fn add_new_mint_pass(&self, pass_info: &MinerPassInfo) -> Result<(), String> {
        let owner = pass_info.owner.clone().require_network(self.network).map_err(|e| {
            let msg = format!("Failed to convert owner address to network {}: {}", self.network, e);
            error!("{}", msg);
            msg
        })?.to_string();

        let prev_serialized = pass_info.prev.iter()
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
                owner,
                eth_main,
                eth_collab,
                prev,
                state
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9);
            ",
            rusqlite::params![
                pass_info.inscription_id.to_string(),
                pass_info.inscription_number,
                pass_info.mint_txid.to_string(),
                pass_info.mint_block_height as i64,
                &owner,
                pass_info.eth_main,
                pass_info.eth_collab,
                prev_serialized,
                pass_info.state.as_str(),
            ],
        ).map_err(|e| {
            let msg = format!("Failed to insert new miner pass into database: {}", e);
            error!("{}", msg);
            msg
        })?;

        info!("Added new miner pass with inscription_id {} to owner {}", pass_info.inscription_id, owner);

        Ok(())
    }

    pub fn update_state(&self, inscription_id: &InscriptionId, new_state: MinerPassState, prev_state: MinerPassState) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();

        let affected = conn.execute(
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
        ).map_err(|e| {
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

        info!("Updated miner pass {} state from {} to {}", inscription_id, prev_state.as_str(), new_state.as_str());

        Ok(())
    }
}