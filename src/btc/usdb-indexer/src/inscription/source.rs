use crate::index::{
    InscriptionContentLoader, MintValidationError, MintValidationErrorCode, ParsedMintContent,
    USDBInscription,
};
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

#[derive(Debug, Clone)]
pub struct DiscoveredInvalidMint {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: Option<SatPoint>,
    pub content_string: String,
    pub error_code: MintValidationErrorCode,
    pub error_reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiscoveredMintBatch {
    pub valid_mints: Vec<DiscoveredMint>,
    pub invalid_mints: Vec<DiscoveredInvalidMint>,
}

pub trait InscriptionSource: Send + Sync {
    fn source_name(&self) -> &'static str;

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>>;

    fn load_block_mint_batch<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<DiscoveredMintBatch, String>> {
        Box::pin(async move {
            let inscriptions = self
                .load_block_inscriptions(block_height, block_hint)
                .await?;
            classify_usdb_mints_from_inscriptions(inscriptions)
        })
    }

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
        Box::pin(async move {
            let batch = self.load_block_mint_batch(block_height, block_hint).await?;
            Ok(batch.valid_mints)
        })
    }
}

pub fn map_usdb_mints_from_inscriptions(
    inscriptions: Vec<DiscoveredInscription>,
) -> Result<Vec<DiscoveredMint>, String> {
    let batch = classify_usdb_mints_from_inscriptions(inscriptions)?;
    Ok(batch.valid_mints)
}

fn to_invalid_mint(
    inscription: DiscoveredInscription,
    content_string: String,
    err: MintValidationError,
) -> DiscoveredInvalidMint {
    DiscoveredInvalidMint {
        inscription_id: inscription.inscription_id,
        inscription_number: inscription.inscription_number,
        block_height: inscription.block_height,
        timestamp: inscription.timestamp,
        satpoint: inscription.satpoint,
        content_string,
        error_code: err.code,
        error_reason: err.reason,
    }
}

pub fn classify_usdb_mints_from_inscriptions(
    inscriptions: Vec<DiscoveredInscription>,
) -> Result<DiscoveredMintBatch, String> {
    let mut batch = DiscoveredMintBatch::default();
    for inscription in inscriptions {
        let content_string = match &inscription.content_string {
            Some(value) => value.clone(),
            None => continue,
        };

        match InscriptionContentLoader::classify_mint_content_str(
            &inscription.inscription_id,
            &content_string,
        )? {
            ParsedMintContent::NotUsdbMint => {}
            ParsedMintContent::Valid(content) => {
                batch.valid_mints.push(DiscoveredMint {
                    inscription_id: inscription.inscription_id,
                    inscription_number: inscription.inscription_number,
                    block_height: inscription.block_height,
                    timestamp: inscription.timestamp,
                    satpoint: inscription.satpoint,
                    content_string,
                    content,
                });
            }
            ParsedMintContent::Invalid(err) => {
                batch
                    .invalid_mints
                    .push(to_invalid_mint(inscription, content_string, err));
            }
        }
    }

    Ok(batch)
}
