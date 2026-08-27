use crate::types::BalanceHistoryData;
use crate::{BtcScriptHash, ToBtcScriptHash, is_core_unspendable};
use bitcoincore_rpc::bitcoin::blockdata::transaction::TxOut;
use bitcoincore_rpc::bitcoin::{Script, ScriptBuf, Transaction, Txid};
use electrum_client::{Client, ElectrumApi, GetBalanceRes, GetHistoryRes, Param};

pub struct TxFullItem {
    pub vin: Vec<TxOut>,
    pub vout: Vec<TxOut>,
}

#[derive(Debug, Clone)]
pub struct ElectrsBalanceHistory {
    pub balance: u64,
    pub script_buf: ScriptBuf,
}

#[derive(Debug, Clone)]
pub struct ElectrsBalanceHistoryList {
    pub history: Vec<BalanceHistoryData>,
    pub script_buf: ScriptBuf,
}

impl TxFullItem {
    pub fn amount_delta_from_tx(
        &self,
        script_hash: &BtcScriptHash,
    ) -> Result<(i64, ScriptBuf), String> {
        let mut delta: i64 = 0;

        let mut script_buf = None;
        for vin in &self.vin {
            if vin.script_pubkey.to_btc_script_hash() == *script_hash {
                delta -= vin.value.to_sat() as i64;
                if script_buf.is_none() {
                    script_buf = Some(vin.script_pubkey.clone());
                }
            }
        }

        for vout in &self.vout {
            if !is_core_unspendable(&vout.script_pubkey)
                && vout.script_pubkey.to_btc_script_hash() == *script_hash
            {
                delta += vout.value.to_sat() as i64;
                if script_buf.is_none() {
                    script_buf = Some(vout.script_pubkey.clone());
                }
            }
        }

        let script_buf = script_buf.ok_or_else(|| {
            format!(
                "Spendable script not found for script hash {} in transaction",
                script_hash
            )
        })?;

        Ok((delta, script_buf))
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

        Ok(Self { client })
    }

    // Get address balance
    pub async fn get_balance(&self, script_hash: &BtcScriptHash) -> Result<u64, String> {
        let script_hash_str = format!("{:x}", script_hash);

        let params = vec![Param::String(script_hash_str)];
        let result = self
            .client
            .raw_call("blockchain.scripthash.get_balance", params)
            .map_err(|e| {
                let msg = format!(
                    "Failed to get balance for script hash {}: {}",
                    script_hash, e
                );
                error!("{}", msg);
                msg
            })?;

        let balance_res: GetBalanceRes = serde_json::from_value(result).map_err(|e| {
            let msg = format!(
                "Failed to parse balance for script hash {}: {}",
                script_hash, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(balance_res.confirmed)
    }

    pub async fn get_balances(&self, script_hashes: &[BtcScriptHash]) -> Result<Vec<u64>, String> {
        let mut batch = electrum_client::Batch::default();
        for script_hash in script_hashes {
            let script_hash_str = format!("{:x}", script_hash);
            let params = vec![Param::String(script_hash_str)];
            batch.raw(String::from("blockchain.scripthash.get_balance"), params);
        }

        let ret = self.client.batch_call(&batch).map_err(|e| {
            let msg = format!("Failed to send batch request: {}", e);
            error!("{}", msg);
            msg
        })?;
        let mut balances = Vec::with_capacity(script_hashes.len());
        for value in ret {
            let balance_res: GetBalanceRes = serde_json::from_value(value).map_err(|e| {
                let msg = format!("Failed to parse balance in batch response: {}", e);
                error!("{}", msg);
                msg
            })?;
            balances.push(balance_res.confirmed);
        }

        Ok(balances)
    }

    // Get address history
    pub async fn get_history(
        &self,
        script_hash: &BtcScriptHash,
    ) -> Result<Vec<GetHistoryRes>, String> {
        let script_hash_str = format!("{:x}", script_hash);

        let params = vec![Param::String(script_hash_str)];
        let result = self
            .client
            .raw_call("blockchain.scripthash.get_history", params)
            .map_err(|e| {
                let msg = format!(
                    "Failed to get history for script hash {}: {}",
                    script_hash, e
                );
                error!("{}", msg);
                msg
            })?;

        let his: Vec<GetHistoryRes> = serde_json::from_value(result).map_err(|e| {
            let msg = format!(
                "Failed to parse history for script hash {}: {}",
                script_hash, e
            );
            error!("{}", msg);
            msg
        })?;
        Ok(his)
    }

    pub async fn get_history_by_script(
        &self,
        script: &Script,
    ) -> Result<Vec<GetHistoryRes>, String> {
        let his = self.client.script_get_history(script).map_err(|e| {
            let msg = format!("Failed to get history for script {}: {}", script, e);
            error!("{}", msg);
            msg
        })?;

        Ok(his)
    }

    // Calculate balance for an address at a specific block height
    pub async fn calc_balance(
        &self,
        script_hash: &BtcScriptHash,
        block_height: u32,
    ) -> Result<ElectrsBalanceHistory, String> {
        let history = self.get_history(script_hash).await?;

        let mut balance: i64 = 0;
        let mut script_buf = None;
        for item in history {
            if item.height > block_height as i32 {
                break;
            }
            // Load tx from btc client
            let tx = self.expand_tx(&item.tx_hash).await?;

            let (delta, script_buf_inner) = tx.amount_delta_from_tx(script_hash)?;
            if script_buf.is_none() {
                script_buf = Some(script_buf_inner);
            }
            balance = balance
                .checked_add(delta)
                .ok_or_else(|| format!("Balance overflow for script hash {}", script_hash))?;
            if balance < 0 {
                return Err(format!(
                    "Balance went negative for script hash {} at block height {}",
                    script_hash, item.height
                ));
            }
        }

        info!(
            "Calculated balance for script hash {} at block height {}: {}",
            script_hash, block_height, balance
        );

        let script_buf = script_buf.ok_or_else(|| {
            format!(
                "No spendable confirmed history found for script hash {} at block height {}",
                script_hash, block_height
            )
        })?;
        let ret = ElectrsBalanceHistory {
            balance: balance as u64,
            script_buf,
        };
        Ok(ret)
    }

    // Calculate balance history for an address up to a specific block height
    pub async fn calc_balance_history(
        &self,
        script_hash: &BtcScriptHash,
        block_height: u32,
    ) -> Result<ElectrsBalanceHistoryList, String> {
        let history = self.get_history(script_hash).await?;

        let mut balance: i64 = 0;
        let mut result = Vec::with_capacity(history.len());
        let mut script_buf = None;
        for item in history {
            if item.height > block_height as i32 {
                break;
            }

            // Load tx from btc client
            let tx = self.expand_tx(&item.tx_hash).await?;

            let (delta, script_buf_inner) = tx.amount_delta_from_tx(script_hash)?;
            if script_buf.is_none() {
                script_buf = Some(script_buf_inner);
            }
            balance = balance
                .checked_add(delta)
                .ok_or_else(|| format!("Balance overflow for script hash {}", script_hash))?;
            if balance < 0 {
                return Err(format!(
                    "Balance went negative for script hash {} at block height {}",
                    script_hash, item.height
                ));
            }

            let data = BalanceHistoryData {
                block_height: item.height as u32,
                delta,
                balance: balance as u64,
            };
            result.push(data);
        }

        info!(
            "Calculated balance history for script hash {}: {} entries",
            script_hash,
            result.len()
        );

        let script_buf = script_buf.ok_or_else(|| {
            format!(
                "No spendable confirmed history found for script hash {} at block height {}",
                script_hash, block_height
            )
        })?;
        let ret = ElectrsBalanceHistoryList {
            history: result,
            script_buf,
        };
        Ok(ret)
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
    use bitcoincore_rpc::bitcoin::{Address, Amount, Network, Txid};
    use std::str::FromStr;

    #[test]
    fn amount_delta_excludes_core_unspendable_outputs() {
        let oversized_script = ScriptBuf::from(vec![0x51; 10_001]);
        let script_hash = oversized_script.to_btc_script_hash();
        let tx = TxFullItem {
            vin: Vec::new(),
            vout: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: oversized_script,
            }],
        };

        let error = tx.amount_delta_from_tx(&script_hash).unwrap_err();
        assert!(error.contains("Spendable script not found"));
    }

    #[test]
    fn amount_delta_keeps_nonstandard_but_core_spendable_scripts() {
        let script = ScriptBuf::from(vec![0x51, 0x51]);
        let script_hash = script.to_btc_script_hash();
        let tx = TxFullItem {
            vin: Vec::new(),
            vout: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: script.clone(),
            }],
        };

        assert_eq!(
            tx.amount_delta_from_tx(&script_hash).unwrap(),
            (50_000, script)
        );
    }

    #[tokio::test]
    #[ignore = "Requires Electrs server running at tcp://127.0.0.1:50001 and specific transactions in the history"]
    async fn test_electrs_client() {
        let server_url = "tcp://127.0.0.1:50001";
        let client = ElectrsClient::new(server_url).expect("Failed to create Electrs client");
        let address = Address::from_str("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh")
            .expect("Failed to parse address");
        let address = address.assume_checked();
        let history = client
            .get_history(&address.script_pubkey().to_btc_script_hash())
            .await
            .expect("Failed to get history");
        assert!(!history.is_empty());

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
        println!(
            "Full Transaction: vin={:?}, vout={:?}",
            full_tx.vin, full_tx.vout
        );

        let address = Address::from_str("bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h")
            .expect("Failed to parse address");
        let address = address.require_network(Network::Bitcoin).unwrap();
        let (delta, _) = full_tx
            .amount_delta_from_tx(&address.script_pubkey().to_btc_script_hash())
            .expect("Failed to compute amount delta");
        println!(
            "Amount delta for address {} in tx {}: {:?}",
            address, txid, delta
        );
        assert!(delta == -2045555); // Example value

        // Test another address
        let address = Address::from_str("bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h").unwrap();
        let address = address.require_network(Network::Bitcoin).unwrap();
        let script_hash = address.script_pubkey().to_btc_script_hash();
        let history = client
            .get_history(&script_hash)
            .await
            .expect("Failed to get history");
        assert!(!history.is_empty());
        println!("History for address {}: {}", address, history.len());
    }
}
