use super::super::content::InscriptionContentLoader;
use super::{DiscoveredMint, InscriptionSource, InscriptionSourceFuture};
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

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
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
                    if !InscriptionContentLoader::is_supported_content_type(
                        inscription.content_type(),
                    ) {
                        continue;
                    }

                    let Some(body) = inscription.body() else {
                        continue;
                    };

                    let content_string = match std::str::from_utf8(body) {
                        Ok(text) => text.to_string(),
                        Err(e) => {
                            debug!(
                                "Skipping non-utf8 inscription body from bitcoind source: module=inscription_source_bitcoind, block_height={}, inscription_id={}, error={}",
                                block_height, inscription_id, e
                            );
                            continue;
                        }
                    };

                    let Some(content) = InscriptionContentLoader::parse_content_str(
                        &inscription_id,
                        &content_string,
                    )?
                    else {
                        continue;
                    };

                    discovered.push(DiscoveredMint {
                        inscription_id,
                        inscription_number: inscription_id.index as i32,
                        block_height,
                        timestamp: block.header.time,
                        satpoint: None,
                        content_string,
                        content,
                    });
                }
            }

            Ok(discovered)
        })
    }
}
