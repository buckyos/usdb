use crate::btc::{BTCClient, BTCClientRef, ElectrsClient, ElectrsClientRef, TxFullItem};
use crate::config::ConfigManagerRef;
use crate::storage::{AddressBalanceStorage, AddressBalanceStorageRef};
use bitcoincore_rpc::bitcoin::address::{Address, NetworkChecked};
use bitcoincore_rpc::bitcoin::{ScriptBuf, Txid};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub(crate) struct WatchedAddressInfo {
    address: Address<NetworkChecked>,
    block_height: u64,
    balance: u64,
}

pub struct WatchedAddressManager {
    addresses: Mutex<HashMap<ScriptBuf, WatchedAddressInfo>>,
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
            let script = addr.address.script_pubkey();
            let info = WatchedAddressInfo {
                address: addr.address.clone(),
                block_height: addr.block_height,
                balance: addr.balance,
            };
            let ret = addresses_lock.insert(script, info);
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
            if let Some(info) = addresses_lock.get(&vin.script_pubkey) {
                if found_addresses.iter().all(|a| a.address != info.address) {
                    found_addresses.push(info.clone());
                }
            }
        }

        for vout in &tx.vout {
            if let Some(info) = addresses_lock.get(&vout.script_pubkey) {
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
            let delta = self.amount_delta_from_tx(&tx, &addr_info.address).await?;

            self.balance_storage
                .update_balance(&addr_info.address, block_height, delta)?;
        }

        Ok(())
    }

    // Additional methods for BalanceIndexer can be added here in range[start, end)
    pub async fn index_address_balance(
        &self,
        address: &Address<NetworkChecked>,
        block_range: Range<u64>,
    ) -> Result<(), String> {
        // First load existing balance from electrs
        let list = self.electrs_client.get_history(address).await?;
        for item in list {
            // Check if the item is in mempool or unconfirmed
            if item.height <= 0 {
                continue;
            }

            if (item.height as u64) < block_range.start {
                continue;
            }

            if (item.height as u64) >= block_range.end {
                break;
            }

            // Load tx from btc client
            let tx = self.electrs_client.expand_tx(&item.tx_hash).await?;

            let delta = self.amount_delta_from_tx(&tx, address).await?;

            self.balance_storage
                .update_balance(address, item.height as u64, delta)?;
        }

        // Implementation to index the balance for the given address
        // This is a placeholder implementation
        Ok(())
    }

    async fn amount_delta_from_tx(
        &self,
        tx: &TxFullItem,
        address: &Address<NetworkChecked>,
    ) -> Result<i64, String> {
        let mut delta: i64 = 0;

        let address_script = address.script_pubkey();
        for vin in &tx.vin {
            if vin.script_pubkey == address_script {
                delta -= vin.value.to_sat() as i64;
            }
        }

        for vout in &tx.vout {
            if vout.script_pubkey == address_script {
                delta += vout.value.to_sat() as i64;
            }
        }

        Ok(delta)
    }
}
