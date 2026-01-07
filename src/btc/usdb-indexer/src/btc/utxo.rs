use usdb_util::BTCRpcClientRef;
use bitcoincore_rpc::bitcoin::{Amount, OutPoint};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type UTXOValueCache = HashMap<String, Amount>;

pub struct UTXOValueManager {
    btc_client: BTCRpcClientRef,
    cache: Mutex<UTXOValueCache>,
}

impl UTXOValueManager {
    pub fn new(btc_client: BTCRpcClientRef) -> Self {
        let cache = Mutex::new(HashMap::new());

        Self { btc_client, cache }
    }

    pub async fn get_utxo(&self, outpoint: &OutPoint) -> Result<Amount, String> {
        let outpoint_str;
        // First get from cache
        {
            let cache = self.cache.lock().unwrap();

            outpoint_str = outpoint.to_string();
            if let Some(amount) = cache.get(&outpoint_str) {
                return Ok(amount.to_owned());
            }
        }

        // Not found in cache, search UTXO
        let (_address, amount) = self.btc_client.get_utxo(outpoint)?;

        // Store the result back in cache
        let mut cache = self.cache.lock().unwrap();
        cache.insert(outpoint_str, amount.clone());

        Ok(amount)
    }
}


pub type UTXOValueManagerRef = Arc<UTXOValueManager>;