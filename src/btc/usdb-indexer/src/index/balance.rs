use crate::btc::{BTCClient, BTCClientRef};
use crate::config::ConfigManagerRef;
use crate::storage::{AddressBalanceStorage, AddressBalanceStorageRef};
use bitcoincore_rpc::bitcoin::{Txid};
use std::collections::HashMap;
use std::sync::Mutex;
use usdb_util::{ElectrsClient, ElectrsClientRef, TxFullItem, USDBScriptHash, ToUSDBScriptHash};

#[derive(Debug, Clone)]
pub(crate) struct WatchedAddressInfo {
    address: USDBScriptHash,
    block_height: u64,
    balance: u64,
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

pub struct AddressBalanceIndexer {
    config: ConfigManagerRef,
    btc_client: BTCClientRef,
    electrs_client: ElectrsClientRef,

    balance_storage: AddressBalanceStorageRef,
    watched_address_manager: WatchedAddressManager,
}

impl AddressBalanceIndexer {
    pub fn new(
        config: ConfigManagerRef,
        balance_storage: AddressBalanceStorageRef,
    ) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        // Init electrs client
        let electrs_client = ElectrsClient::new(config.config().electrs.rpc_url())?;

        let ret = Self {
            config,
            btc_client: std::sync::Arc::new(btc_client),
            electrs_client: std::sync::Arc::new(electrs_client),
            balance_storage,
            watched_address_manager: WatchedAddressManager::new(),
        };

        Ok(ret)
    }

    pub async fn init(&self) -> Result<(), String> {
        self.watched_address_manager.init(&self.balance_storage)?;
        Ok(())
    }

    pub async fn sync_block(&self, block_height: u64) -> Result<(), String> {
        info!(
            "Syncing block height for watched addresses balance: {}",
            block_height
        );
        let block = self.btc_client.get_block(block_height).await?;

        // Get all inscription ids in this block
        let txs = self.btc_client.get_raw_transactions(&block.tx).await?;
        assert_eq!(
            txs.len(),
            block.tx.len(),
            "Mismatch in number of transactions fetched"
        );

        // Process each transaction in the block
        for tx in txs {
            self.async_tx(block_height, &tx.txid).await?;
        }

        Ok(())
    }

    async fn async_tx(&self, block_height: u64, txid: &Txid) -> Result<(), String> {
        let tx = self.electrs_client.expand_tx(txid).await?;

        // First check if any watched address is involved
        let found_address = self.watched_address_manager.search_within_tx(&tx);
        if found_address.is_empty() {
            debug!(
                "No watched address found in transaction {}",
                txid.to_string()
            );
            return Ok(());
        }

        // Update balance for each found address
        for addr_info in found_address {
            // TODO
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::{Network, key::rand::seq::index};
    use std::str::FromStr;
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

        let indexer =
            AddressBalanceIndexer::new(config, storage).expect("Failed to create indexer");
    }
}