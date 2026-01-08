use crate::config::ConfigManagerRef;
use crate::storage::{AddressBalanceStorage, AddressBalanceStorageRef};
use bitcoincore_rpc::bitcoin::{Txid};
use std::collections::HashMap;
use std::sync::Mutex;
use usdb_util::{ElectrsClient, ElectrsClientRef, TxFullItem, USDBScriptHash, ToUSDBScriptHash};

#[derive(Debug, Clone)]
pub(crate) struct WatchedAddressInfo {
    address: USDBScriptHash,
    block_height: u32,
    balance: u64, // in Satoshi
}

pub struct WatchedAddressManager {
    addresses: Mutex<HashMap<USDBScriptHash, WatchedAddressInfo>>,
}

impl WatchedAddressManager {
    pub fn new() -> Self {
        Self {
            addresses: Mutex::new(HashMap::new()),
        }
    }

    pub fn init(&self, balance_storage: &AddressBalanceStorage) -> Result<(), String> {
        let stored_addresses = balance_storage.get_all_watched_addresses()?;
        let mut addresses_lock = self.addresses.lock().unwrap();
        for addr in stored_addresses {
            let info = WatchedAddressInfo {
                address: addr.address.clone(),
                block_height: addr.block_height,
                balance: addr.balance,
            };
            let ret = addresses_lock.insert(addr.address.clone(), info);
            assert!(
                ret.is_none(),
                "Duplicate address found in storage: {}",
                addr.address
            );
        }

        Ok(())
    }

    // Search for a watched address within a transaction if any contains it
    // May contain more then one, we should return all addresses found
    pub fn search_within_tx(&self, tx: &TxFullItem) -> Vec<WatchedAddressInfo> {
        let addresses_lock = self.addresses.lock().unwrap();

        let mut found_addresses: Vec<WatchedAddressInfo> = Vec::new();
        for vin in &tx.vin {
            if let Some(info) = addresses_lock.get(&vin.script_pubkey.to_usdb_script_hash()) {
                if found_addresses.iter().all(|a| a.address != info.address) {
                    found_addresses.push(info.clone());
                }
            }
        }

        for vout in &tx.vout {
            if let Some(info) = addresses_lock.get(&vout.script_pubkey.to_usdb_script_hash()) {
                if found_addresses.iter().all(|a| a.address != info.address) {
                    found_addresses.push(info.clone());
                }
            }
        }

        found_addresses
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::{Network, key::rand::seq::index};
    use crate::{config::ConfigManager, storage};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_address_balance_indexer() {
         let tmp_dir = std::env::temp_dir().join("usdb").join("test_address_balance_indexer");
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let test_db_path = tmp_dir.join(crate::constants::ADDRESS_BALANCE_DB_FILE);
        if test_db_path.exists() {
            std::fs::remove_file(&test_db_path).unwrap();
        }
        let storage =
            AddressBalanceStorage::new(&tmp_dir, Network::Bitcoin).unwrap();
        let storage = Arc::new(storage);

        let config = ConfigManager::new(None).expect("Failed to create config manager");
        let config = Arc::new(config);
    }
}