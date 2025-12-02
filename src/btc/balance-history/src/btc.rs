use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, ScriptBuf};
use bitcoincore_rpc::{Auth, Client, RpcApi};

pub struct BTCClient {
    client: Client,
}

impl BTCClient {
    pub fn new(rpc_url: String, auth: Auth) -> Result<Self, String> {
        let client = Client::new(&rpc_url, auth).map_err(|e| {
            let msg = format!("Failed to create BTC RPC client: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        let ret = Self { client };

        Ok(ret)
    }

    pub fn get_latest_block_height(&self) -> Result<u64, String> {
        self.client.get_block_count().map_err(|error| {
            let msg = format!("get_block_count failed: {}", error);
            error!("{}", msg);
            msg
        })
    }

    pub fn get_block(&self, block_height: u64) -> Result<Block, String> {
        // First get the block hash for the given height
        let hash = self.client.get_block_hash(block_height).map_err(|error| {
            let msg = format!("get_block_hash failed: {}", error);
            error!("{}", msg);
            msg
        })?;

        // Now get the block using the hash
        self.client.get_block(&hash).map_err(|error| {
            let msg = format!("get_block failed: {}", error);
            error!("{}", msg);
            msg
        })
    }

    // Get UTXO details for a given outpoint, maybe spent already
    // So we should get it from transaction and then parse it
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        let ret = self
            .client
            .get_raw_transaction(&outpoint.txid, None)
            .map_err(|e| {
                let msg = format!(
                    "Failed to get raw transaction for outpoint: {} {}",
                    outpoint, e
                );
                error!("{}", msg);
                msg
            })?;

        if outpoint.vout as usize >= ret.output.len() {
            let msg = format!("Invalid vout index for outpoint: {}", outpoint);
            error!("{}", msg);
            return Err(msg);
        }

        let tx_out = ret.output.get(outpoint.vout as usize).unwrap();
        Ok((tx_out.script_pubkey.clone(), tx_out.value))
    }
}

pub type BTCClientRef = std::sync::Arc<BTCClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Txid;
    use bitcoincore_rpc::bitcoin::Address;
    use std::str::FromStr;
    use usdb_util::BTCConfig;

    #[test]
    fn test_btc_client() {
        let config = BTCConfig::default();
        let rpc_url = config.rpc_url();
        let auth = config.auth();

        let client_result = BTCClient::new(rpc_url, auth);
        assert!(client_result.is_ok());

        let client = client_result.unwrap();
        let height_result = client.get_latest_block_height();
        assert!(height_result.is_ok());
        let height = height_result.unwrap();
        println!("Latest block height: {}", height);

        // Test get utxo with a known outpoint (this may fail if the outpoint doesn't exist in the test environment)
        let txid =
            Txid::from_str("adc4b0b0dd51518d5246ecf6aa91550a19b8d86b9dfca525b97bce18dabffc05")
                .unwrap();
        let outpoint = OutPoint::new(txid, 0);
        let utxo_result = client.get_utxo(&outpoint);
        match utxo_result {
            Ok((script, amount)) => {
                println!("UTXO Script: {:?}", script);
                let address = Address::from_script(&script, bitcoincore_rpc::bitcoin::Network::Bitcoin).expect("Invalid script");
                println!("UTXO Address: {}", address);
                assert_eq!(address.to_string(), "1CMb4HTBRQtweVanz79nfZmKXTDBcJC7Uu");

                println!("UTXO Amount: {}", amount);
                assert_eq!(amount.to_sat(), 61550000);
            }
            Err(e) => {
                println!("Failed to get UTXO: {}", e);
            }
        }

        // Get another utxo in current tx but out of range
        let outpoint = OutPoint::new(txid, 3);
        let utxo_result = client.get_utxo(&outpoint);
        assert!(utxo_result.is_err());
    }
}
