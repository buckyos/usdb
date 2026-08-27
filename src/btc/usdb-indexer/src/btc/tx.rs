use super::utxo::UTXOValueManager;
use bitcoincore_rpc::bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, Txid};
use ordinals::SatPoint;
use usdb_util::{BtcScriptHash, ToBtcScriptHash, is_core_unspendable};

pub struct TxItem {
    pub txid: Txid,
    pub tx: Transaction,
}

pub struct SatPointResult {
    pub satpoint: SatPoint,
    pub value: Amount,
    // None means the sat has no usable owner because it was lost to fees or sent to a
    // Bitcoin Core unspendable output.
    pub address: Option<BtcScriptHash>,
}

fn usable_owner(script_pubkey: &ScriptBuf) -> Option<BtcScriptHash> {
    (!is_core_unspendable(script_pubkey)).then(|| script_pubkey.to_btc_script_hash())
}

impl TxItem {
    pub fn from_tx(tx: Transaction) -> Self {
        let txid = tx.compute_txid();
        TxItem { txid, tx }
    }

    // Given an input satpoint, calculate the output satpoint after this transaction
    pub async fn calc_output_satpoint(
        &self,
        satpoint: SatPoint,
        utxo_manager: &UTXOValueManager,
    ) -> Result<Option<SatPointResult>, String> {
        // Find by outpoint in vin and got the index
        let ret = self
            .tx
            .input
            .iter()
            .position(|v| v.previous_output == satpoint.outpoint);
        if ret.is_none() {
            return Ok(None);
        }
        let vin_index = ret.unwrap();

        // Calc the sat position in this tx inputs
        let mut pos = 0;
        for i in 0..vin_index {
            let vin_outpoint = &self.tx.input[i].previous_output;
            let amount = utxo_manager.get_utxo(vin_outpoint).await?;

            pos += amount.to_sat();
        }

        pos += satpoint.offset;

        // Find which vout contains this sat position
        let mut current = 0;
        for (i, vout_item) in self.tx.output.iter().enumerate() {
            let vout_value = vout_item.value.to_sat();

            if pos >= current && pos < current + vout_value {
                let offset = pos - current;
                let point = SatPoint {
                    outpoint: OutPoint {
                        txid: self.txid,
                        vout: i as u32,
                    },
                    offset,
                };

                let address = usable_owner(&vout_item.script_pubkey);
                info!(
                    "Found ordinal {} -> {}, owner: {:?}",
                    satpoint, point, address
                );

                return Ok(Some(SatPointResult {
                    satpoint: point,
                    value: vout_item.value,
                    address,
                }));
            }

            current += vout_value;
        }

        warn!(
            "Ordinal input {} is spent as fee in {}",
            satpoint, self.txid
        );

        let point = SatPoint {
            outpoint: OutPoint {
                txid: self.txid,
                vout: self.tx.output.len() as u32, // Use vout index equal to output count to indicate spent as fee
            },
            offset: 0,
        };

        Ok(Some(SatPointResult {
            satpoint: point,
            value: Amount::from_sat(0),
            address: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::{opcodes::all::OP_RETURN, script::Builder};

    #[test]
    fn test_usable_owner_rejects_core_unspendable_scripts() {
        let burn_script = Builder::new().push_opcode(OP_RETURN).into_script();
        assert_eq!(usable_owner(&burn_script), None);

        let oversized_script = ScriptBuf::from(vec![0x51; 10_001]);
        assert_eq!(usable_owner(&oversized_script), None);

        let spendable_script = ScriptBuf::new();
        assert_eq!(
            usable_owner(&spendable_script),
            Some(spendable_script.to_btc_script_hash())
        );
    }
}
