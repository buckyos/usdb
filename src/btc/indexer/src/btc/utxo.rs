use super::btc::BTCClientRef;
use bitcoincore_rpc::bitcoin::{Amount, OutPoint};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type UTXOValueCache = HashMap<String, Amount>;

pub struct UTXOValueManager {
    btc_client: BTCClientRef,
    cache: Mutex<UTXOValueCache>,
}

impl UTXOValueManager {
    pub fn new(btc_client: BTCClientRef) -> Self {
        let cache = Mutex::new(HashMap::new());

        Self { btc_client, cache }
    }

    pub async fn get_utxo(&self, outpoint: &OutPoint) -> Result<Amount, String> {
        // First get from cache
        let cache = self.cache.lock().unwrap();

        let outpoint_str = outpoint.to_string();
        if let Some(amount) = cache.get(&outpoint_str) {
            return Ok(amount.to_owned());
        }

        // Cache miss, fetch utxo
        drop(cache);

        let amount = self.search_utxo(outpoint).await?;

        // Store the result back in cache
        let mut cache = self.cache.lock().unwrap();
        cache.insert(outpoint_str, amount.clone());

        Ok(amount)
    }

    async fn search_utxo(&self, outpoint: &OutPoint) -> Result<Amount, String> {
        // First get the transaction
        let tx = self.btc_client.get_raw_transaction(&outpoint.txid).await?;

        if tx.vout.len() <= outpoint.vout as usize {
            let msg = format!(
                "Output index {} out of bounds for transaction {}",
                outpoint.vout, outpoint.txid
            );
            error!("{}", msg);
            return Err(msg);
        }

        let vout = &tx.vout[outpoint.vout as usize];

        Ok(vout.value)
    }
}


pub type UTXOValueManagerRef = Arc<UTXOValueManager>;