use super::super::content::{InscriptionContentLoader, USDBInscription};
use super::{DiscoveredMint, InscriptionSource, InscriptionSourceFuture};
use crate::btc::{OrdClient, OrdClientRef};
use crate::config::ConfigManagerRef;
use bitcoincore_rpc::bitcoin::Block;
use ord::api::Inscription;
use std::sync::Arc;

pub struct OrdInscriptionSource {
    config: ConfigManagerRef,
    ord_client: OrdClientRef,
}

impl OrdInscriptionSource {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;

        Ok(Self {
            config,
            ord_client: Arc::new(ord_client),
        })
    }

    async fn load_inscriptions_content(
        &self,
        inscriptions: &[Inscription],
    ) -> Result<Vec<Option<(Inscription, String, USDBInscription)>>, String> {
        const BATCH_SIZE: usize = 64;

        let mut contents = Vec::with_capacity(inscriptions.len());
        for chunk in inscriptions.chunks(BATCH_SIZE) {
            let mut handles = Vec::with_capacity(chunk.len());

            for inscription in chunk {
                let ord_client = self.ord_client.clone();
                let config = self.config.clone();
                let inscription = inscription.clone();

                let handle = tokio::spawn(async move {
                    InscriptionContentLoader::load_content(
                        &ord_client,
                        &inscription.id,
                        inscription.content_type.as_deref(),
                        &config,
                    )
                    .await
                    .map(|opt| opt.map(|(content, usdb)| (inscription, content, usdb)))
                });
                handles.push(handle);
            }

            for handle in handles {
                let content = handle.await.map_err(|e| {
                    let msg = format!("Failed to join task for loading inscription content: {}", e);
                    error!("{}", msg);
                    msg
                })??;
                contents.push(content);
            }
        }

        Ok(contents)
    }
}

impl InscriptionSource for OrdInscriptionSource {
    fn source_name(&self) -> &'static str {
        "ord"
    }

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        _block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
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

            let parsed = self.load_inscriptions_content(&inscriptions).await?;
            let mut mints = Vec::new();

            for (i, item) in parsed.into_iter().enumerate() {
                let Some((inscription, content_string, content)) = item else {
                    debug!(
                        "Inscription skipped after content parsing: module=inscription_source_ord, block_height={}, inscription_id={}",
                        block_height, inscription_ids[i]
                    );
                    continue;
                };

                if inscription.number < 0 {
                    warn!(
                        "Skipping negative inscription number from ord source: module=inscription_source_ord, block_height={}, inscription_id={}, number={}",
                        block_height, inscription.id, inscription.number
                    );
                    continue;
                }

                mints.push(DiscoveredMint {
                    inscription_id: inscription.id,
                    inscription_number: inscription.number,
                    block_height,
                    timestamp: inscription.timestamp as u32,
                    satpoint: Some(inscription.satpoint),
                    content_string,
                    content,
                });
            }

            Ok(mints)
        })
    }
}
