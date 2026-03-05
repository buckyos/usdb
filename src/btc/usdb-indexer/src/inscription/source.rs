use crate::index::{InscriptionContentLoader, USDBInscription};
use bitcoincore_rpc::bitcoin::Block;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type InscriptionSourceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct DiscoveredInscription {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: Option<SatPoint>,
    pub content_type: Option<String>,
    pub content_string: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredMint {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: Option<SatPoint>,
    pub content_string: String,
    pub content: USDBInscription,
}

pub trait InscriptionSource: Send + Sync {
    fn source_name(&self) -> &'static str;

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>>;

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
        Box::pin(async move {
            let inscriptions = self
                .load_block_inscriptions(block_height, block_hint)
                .await?;
            map_usdb_mints_from_inscriptions(inscriptions)
        })
    }
}

pub fn map_usdb_mints_from_inscriptions(
    inscriptions: Vec<DiscoveredInscription>,
) -> Result<Vec<DiscoveredMint>, String> {
    let mut mints = Vec::new();
    for inscription in inscriptions {
        let Some(content_string) = inscription.content_string else {
            continue;
        };

        let Some(content) = InscriptionContentLoader::parse_content_str(
            &inscription.inscription_id,
            &content_string,
        )?
        else {
            continue;
        };

        mints.push(DiscoveredMint {
            inscription_id: inscription.inscription_id,
            inscription_number: inscription.inscription_number,
            block_height: inscription.block_height,
            timestamp: inscription.timestamp,
            satpoint: inscription.satpoint,
            content_string,
            content,
        });
    }

    Ok(mints)
}
