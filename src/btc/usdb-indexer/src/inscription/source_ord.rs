use super::{DiscoveredInscription, InscriptionSource, InscriptionSourceFuture};
use crate::btc::{ContentBody, OrdClient, OrdClientRef, OrdInscriptionItem};
use crate::config::ConfigManagerRef;
use bitcoincore_rpc::bitcoin::Block;
use std::sync::Arc;

pub struct OrdInscriptionSource {
    ord_client: OrdClientRef,
}

impl OrdInscriptionSource {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;

        Ok(Self {
            ord_client: Arc::new(ord_client),
        })
    }

    async fn load_inscription_contents(
        &self,
        inscriptions: &[OrdInscriptionItem],
    ) -> Result<Vec<(OrdInscriptionItem, Option<String>)>, String> {
        const BATCH_SIZE: usize = 64;

        let mut results = Vec::with_capacity(inscriptions.len());
        for chunk in inscriptions.chunks(BATCH_SIZE) {
            let mut handles = Vec::with_capacity(chunk.len());

            for inscription in chunk {
                let ord_client = self.ord_client.clone();
                let inscription = inscription.clone();

                let handle = tokio::spawn(async move {
                    let content = ord_client
                        .get_content_by_inscription_id(&inscription.id)
                        .await
                        .map(|opt| match opt {
                            Some(ContentBody::Text(text)) => Some(text),
                            _ => None,
                        })?;
                    Ok::<(OrdInscriptionItem, Option<String>), String>((inscription, content))
                });
                handles.push(handle);
            }

            for handle in handles {
                let item = handle.await.map_err(|e| {
                    let msg = format!("Failed to join task for loading inscription content: {}", e);
                    error!("{}", msg);
                    msg
                })??;
                results.push(item);
            }
        }

        Ok(results)
    }
}

impl InscriptionSource for OrdInscriptionSource {
    fn source_name(&self) -> &'static str {
        "ord"
    }

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        _block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>> {
        Box::pin(async move {
            let inscription_ids = self
                .ord_client
                .get_inscription_by_block(block_height)
                .await?;
            if inscription_ids.is_empty() {
                return Ok(Vec::new());
            }

            let begin_tick = std::time::Instant::now();
            let inscriptions = self.ord_client.get_inscriptions(&inscription_ids).await?;
            debug!(
                "Loaded inscriptions from ord source: module=inscription_source_ord, block_height={}, count={}, elapsed_ms={}",
                block_height,
                inscriptions.len(),
                begin_tick.elapsed().as_millis()
            );

            if inscriptions.len() != inscription_ids.len() {
                let msg = format!(
                    "Ord inscription list size mismatch: module=inscription_source_ord, block_height={}, ids={}, inscriptions={}",
                    block_height,
                    inscription_ids.len(),
                    inscriptions.len()
                );
                error!("{}", msg);
                return Err(msg);
            }

            let parsed = self.load_inscription_contents(&inscriptions).await?;
            let mut discovered = Vec::new();

            for (i, item) in parsed.into_iter().enumerate() {
                let (inscription, content_string) = item;
                if inscription.number < 0 {
                    warn!(
                        "Skipping negative inscription number from ord source: module=inscription_source_ord, block_height={}, inscription_id={}, number={}",
                        block_height, inscription.id, inscription.number
                    );
                    continue;
                }

                if content_string.is_none() {
                    debug!(
                        "Inscription has no text content from ord source: module=inscription_source_ord, block_height={}, inscription_id={}",
                        block_height, inscription_ids[i]
                    );
                }

                discovered.push(DiscoveredInscription {
                    inscription_id: inscription.id,
                    inscription_number: inscription.number,
                    block_height,
                    timestamp: inscription.timestamp,
                    satpoint: Some(inscription.satpoint),
                    content_type: inscription.content_type,
                    content_string,
                });
            }

            Ok(discovered)
        })
    }
}
