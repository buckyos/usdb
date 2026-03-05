use super::{DiscoveredInscription, InscriptionSource, InscriptionSourceFuture};
use usdb_util::BTCRpcClientRef;

use bitcoincore_rpc::bitcoin::Block;
use ord::{InscriptionId, ParsedEnvelope};
use std::sync::Arc;

pub struct BitcoindInscriptionSource {
    btc_client: BTCRpcClientRef,
}

impl BitcoindInscriptionSource {
    pub fn new(btc_client: BTCRpcClientRef) -> Self {
        Self { btc_client }
    }
}

impl InscriptionSource for BitcoindInscriptionSource {
    fn source_name(&self) -> &'static str {
        "bitcoind"
    }

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>> {
        Box::pin(async move {
            let block = match block_hint {
                Some(block) => block,
                None => Arc::new(self.btc_client.get_block(block_height)?),
            };

            let mut discovered = Vec::new();
            for tx in &block.txdata {
                let txid = tx.compute_txid();
                let envelopes = ParsedEnvelope::from_transaction(tx);

                for (index, envelope) in envelopes.into_iter().enumerate() {
                    let inscription_id = InscriptionId {
                        txid,
                        index: index as u32,
                    };

                    let inscription = envelope.payload;
                    let content_string = inscription
                        .body()
                        .and_then(|body| std::str::from_utf8(body).ok())
                        .map(|text| text.to_string());
                    let content_type = inscription.content_type().map(|ct| ct.to_string());

                    discovered.push(DiscoveredInscription {
                        inscription_id,
                        inscription_number: inscription_id.index as i32,
                        block_height,
                        timestamp: block.header.time,
                        satpoint: None,
                        content_type,
                        content_string,
                    });
                }
            }

            Ok(discovered)
        })
    }
}
