use bitcoincore_rpc::bitcoin::address::{NetworkChecked, Address};
use serde::de;
use crate::config::ConfigManagerRef;
use crate::btc::{BTCClient, BTCClientRef, ElectrsClient, ElectrsClientRef};
use std::ops::Range;
use crate::storage::AddressBalanceStorageRef;

pub struct AddressBalanceIndexer {
    config: ConfigManagerRef,
    btc_client: BTCClientRef,
    electrs_client: ElectrsClientRef,

    balance_storage: AddressBalanceStorageRef,
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
        let electrs_client = ElectrsClient::new(
            config.config().electrs.rpc_url(),
        )?;

        let ret = Self {
            config,
            btc_client: std::sync::Arc::new(btc_client),
            electrs_client: std::sync::Arc::new(electrs_client),
            balance_storage,
        };

        Ok(ret)
    }

    // Additional methods for BalanceIndexer can be added here in range[start, end)
    pub async fn index_address_balance(&self, address: &Address<NetworkChecked>, block_range: Range<u64>) -> Result<(), String> {

        let address_script = address.script_pubkey();
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
            let tx = self.btc_client.get_transaction(&item.tx_hash).await.map_err(|e| {
                let msg = format!("Failed to get transaction {} for {} history: {}", item.tx_hash, address, e);
                error!("{}", msg);
                msg
            })?;

            // Process the transaction to update balance
            let mut delta: i64 = 0;
            for vin in &tx.vin {
                if vin.is_coinbase() {
                    continue;
                }

                if vin.script_sig.as_ref().unwrap().script().unwrap() == address_script {
                    delta -= vin.vout.unwrap() as i64;
                }
            }

            for vout in &tx.vout {
                // Check if the output is to the address
                if vout.script_pub_key.script().unwrap() == address_script {
                    delta += vout.value.to_sat() as i64;
                }
            }
        }
        // Implementation to index the balance for the given address
        // This is a placeholder implementation
        Ok(())
    }
}