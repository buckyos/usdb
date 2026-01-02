use bitcoincore_rpc::bitcoin::address::{Address, NetworkChecked};
use bitcoincore_rpc::bitcoin::blockdata::transaction::TxOut;
use bitcoincore_rpc::bitcoin::{Transaction, Txid, Script};
use electrum_client::{Client, ElectrumApi, GetHistoryRes};

pub struct TxFullItem {
    pub vin: Vec<TxOut>,
    pub vout: Vec<TxOut>,
}

impl TxFullItem {
    pub fn amount_delta_from_tx(
        &self,
        address: &Script,
    ) -> Result<i64, String> {
        let mut delta: i64 = 0;

        for vin in &self.vin {
            if vin.script_pubkey == *address {
                delta -= vin.value.to_sat() as i64;
            }
        }

        for vout in &self.vout {
            if vout.script_pubkey == *address {
                delta += vout.value.to_sat() as i64;
            }
        }

        Ok(delta)
    }    
}

pub struct ElectrsClient {
    client: Client,
}

impl ElectrsClient {
    pub fn new(server_url: &str) -> Result<Self, String> {
        let client = Client::new(server_url).map_err(|e| {
            let msg = format!("Failed to create Electrs client: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            client,
        })
    }

    // Get address history
    pub async fn get_history(
        &self,
        address: &Address<NetworkChecked>,
    ) -> Result<Vec<GetHistoryRes>, String> {
        let his = self
            .client
            .script_get_history(&address.script_pubkey())
            .map_err(|e| {
                let msg = format!("Failed to get history for address {}: {}", address, e);
                error!("{}", msg);
                msg
            })?;

        Ok(his)
    }

    pub async fn get_history_by_script(
        &self,
        script: &Script,
    ) -> Result<Vec<GetHistoryRes>, String> {
        let his = self
            .client
            .script_get_history(&script)
            .map_err(|e| {
                let msg = format!("Failed to get history for script {}: {}", script, e);
                error!("{}", msg);
                msg
            })?;

        Ok(his)
    }

    // Calculate balance for an address at a specific block height
    pub async fn calc_balance_by_script(
        &self,
        address: &Script,
        block_height: u32,
    ) -> Result<u64, String> {
        let history = self.get_history_by_script(address).await?;

        let mut balance: i64 = 0;
        for item in history {
            if item.height > block_height as i32 {
                break;
            }
            // Load tx from btc client
            let tx = self.expand_tx(&item.tx_hash).await?;

            let delta = tx.amount_delta_from_tx(address)?;
            balance += delta;
            assert!(
                balance >= 0,
                "Balance went negative for address {}",
                address
            );
        }

        info!(
            "Calculated balance for address {} at block height {}: {}",
            address, block_height, balance
        );
        Ok(balance as u64)
    }

    // Calculate balance history for an address up to a specific block height
    pub async fn calc_balance_history_by_script(
        &self,
        address: &Script,
        block_height: u32,
    ) -> Result<Vec<(u32, i64, u64)>, String> {
        let history = self.get_history_by_script(address).await?;

        let mut balance: i64 = 0;
        let mut result = Vec::with_capacity(history.len());
        for item in history {
            if item.height > block_height as i32 {
                break;
            }
            
            // Load tx from btc client
            let tx = self.expand_tx(&item.tx_hash).await?;

            let delta = tx.amount_delta_from_tx(address)?;
            balance += delta;
            assert!(
                balance >= 0,
                "Balance went negative for address {}",
                address
            );

            result.push((item.height as u32, delta, balance as u64));
        }

        info!(
            "Calculated balance history for address {}: {} entries",
            address,
            result.len()
        );
        Ok(result)
    }

    pub async fn get_transaction(&self, txid: &Txid) -> Result<Transaction, String> {
        let tx = self.client.transaction_get(txid).map_err(|e| {
            let msg = format!("Failed to get transaction {}: {}", txid, e);
            error!("{}", msg);
            msg
        })?;

        Ok(tx)
    }

    // Expand a transaction to get full vin and vout details
    pub async fn expand_tx(&self, txid: &Txid) -> Result<TxFullItem, String> {
        let tx = self.client.transaction_get(txid).map_err(|e| {
            let msg = format!("Failed to get transaction {}: {}", txid, e);
            error!("{}", msg);
            msg
        })?;

        let mut vin = Vec::with_capacity(tx.input.len());
        for input in tx.input {
            let vin_tx = self
                .client
                .transaction_get(&input.previous_output.txid)
                .map_err(|e| {
                    let msg = format!(
                        "Failed to get vin transaction {}: {}",
                        input.previous_output.txid, e
                    );
                    error!("{}", msg);
                    msg
                })?;

            let vin_vout = input.previous_output.vout as usize;
            if vin_vout >= vin_tx.output.len() {
                let msg = format!(
                    "Invalid vout index {} for transaction {}",
                    vin_vout, input.previous_output.txid
                );
                error!("{}", msg);
                return Err(msg);
            }


            vin.push(vin_tx.output[vin_vout].clone());
        }

        let vout = tx.output.clone();

        Ok(TxFullItem { vin, vout })
    }
}

pub type ElectrsClientRef = std::sync::Arc<ElectrsClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Network;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_electrs_client() {
        let server_url = "tcp://127.0.0.1:50001";
        let client = ElectrsClient::new(server_url).expect("Failed to create Electrs client");
        let address = Address::from_str("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh")
            .expect("Failed to parse address");
        let address = address.assume_checked();
        let history = client
            .get_history(&address)
            .await
            .expect("Failed to get history");
        assert!(history.len() > 0);

        let txid =
            Txid::from_str("32939f1cb22341c54c6db5dc0833acffbcefe822b3f82e6adf0de289a424fd53")
                .expect("Failed to parse txid");
        let tx = client
            .get_transaction(&txid)
            .await
            .expect("Failed to get transaction");
        println!("Transaction: {:?}", tx);

        let full_tx = client
            .expand_tx(&txid)
            .await
            .expect("Failed to expand transaction");
        println!("Full Transaction: vin={:?}, vout={:?}", full_tx.vin, full_tx.vout);

        let address = Address::from_str("bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h")
            .expect("Failed to parse address");
        let address = address.require_network(Network::Bitcoin).unwrap();
        let delta = full_tx.amount_delta_from_tx(&address.script_pubkey())
            .expect("Failed to compute amount delta");
        println!(
            "Amount delta for address {} in tx {}: {}",
            address, txid, delta
        );
        assert!(delta == -2045555); // Example value


        // Test another address
        let address = Address::from_str("bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h").unwrap();
        let address = address.require_network(Network::Bitcoin).unwrap();
        let history = client
            .get_history(&address)
            .await
            .expect("Failed to get history");
        assert!(history.len() > 0);
        println!("History for address {}: {}", address, history.len());
    }
}
