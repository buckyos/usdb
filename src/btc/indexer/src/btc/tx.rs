use super::utxo::UTXOValueManager;
use bitcoincore_rpc::bitcoin::address::{Address, NetworkUnchecked};
use bitcoincore_rpc::bitcoin::{Amount, OutPoint, Txid};
use bitcoincore_rpc::bitcoincore_rpc_json::GetRawTransactionResult;
use ordinals::SatPoint;

pub struct TxVOutItem {
    pub outpoint: OutPoint,
    pub value: Amount,

    // FIXME: should we cache address here too?
    pub address: Option<Address<NetworkUnchecked>>,
}

pub struct TxItem {
    pub txid: Txid,
    pub vin: Vec<OutPoint>,
    pub vout: Vec<TxVOutItem>,
}

pub struct SatPointResult {
    pub satpoint: SatPoint,
    pub value: Amount,
    pub address: Option<Address<NetworkUnchecked>>,
}

impl TxItem {
    pub fn from_tx(tx: &GetRawTransactionResult) -> Self {
        let mut vin = Vec::new();
        let mut vout = Vec::new();

        for item in &tx.vin {
            if item.coinbase.is_some() {
                continue;
            }

            let outpoint = OutPoint {
                txid: item.txid.unwrap(),
                vout: item.vout.unwrap(),
            };

            vin.push(outpoint);
        }

        for (i, item) in tx.vout.iter().enumerate() {
            let outpoint = OutPoint {
                txid: tx.txid,
                vout: i as u32,
            };

            let address = if let Some(address) = &item.script_pub_key.address {
                Some(address.clone())
            } else {
                None
            };

            vout.push(TxVOutItem {
                outpoint,
                value: item.value,
                address,
            });
        }

        Self {
            txid: tx.txid,
            vin,
            vout,
        }
    }

    pub async fn calc_next_satpoint(
        &self,
        satpoint: SatPoint,
        utxo_cache: &UTXOValueManager,
    ) -> Result<Option<SatPointResult>, String> {
        // Find by outpoint in vin and got the index
        let ret = self.vin.iter().position(|v| v == &satpoint.outpoint);
        if ret.is_none() {
            return Ok(None);
        }
        let vin_index = ret.unwrap();

        // Calc the sat position in this tx inputs
        let mut pos = 0;
        for i in 0..vin_index {
            let vin_outpoint = &self.vin[i];
            let amount = utxo_cache.get_utxo(vin_outpoint).await?;

            pos += amount.to_sat();
        }

        pos += satpoint.offset;

        // Find which vout contains this sat position
        let mut current = 0;
        for (i, vout_item) in self.vout.iter().enumerate() {
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

                info!(
                    "Found ordinal {} -> {}, address: {:?}",
                    satpoint, point, vout_item.address
                );

                return Ok(Some(SatPointResult {
                    satpoint,
                    value: vout_item.value,
                    address: vout_item.address.clone(),
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
                vout: self.vout.len() as u32,
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
