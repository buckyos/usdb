use super::utxo::UTXOValueManager;
use bitcoincore_rpc::bitcoin::{Amount, OutPoint, Transaction, Txid};
use ordinals::SatPoint;
use usdb_util::{ToUSDBScriptHash, USDBScriptHash};


pub struct TxItem {
    pub txid: Txid,
    pub tx: Transaction,
}

pub struct SatPointResult {
    pub satpoint: SatPoint,
    pub value: Amount,
    pub address: Option<USDBScriptHash>,
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

                let address = vout_item.script_pubkey.to_usdb_script_hash();
                info!(
                    "Found ordinal {} -> {}, address: {}",
                    satpoint, point, address
                );

                return Ok(Some(SatPointResult {
                    satpoint,
                    value: vout_item.value,
                    address: Some(address),
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
