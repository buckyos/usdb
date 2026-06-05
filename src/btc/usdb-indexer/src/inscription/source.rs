use crate::index::{
    InscriptionContentLoader, MintValidationError, MintValidationErrorCode, ParsedMintContent,
    USDBInscription,
};
use bitcoincore_rpc::bitcoin::{Block, Network};
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
        network: Network,
    ) -> InscriptionSourceFuture<'a, Result<DiscoveredMintBatch, String>> {
        Box::pin(async move {
            let inscriptions = self
                .load_block_inscriptions(block_height, block_hint)
                .await?;
            classify_usdb_mints_from_inscriptions(inscriptions, network)
        })
    }

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        network: Network,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
        Box::pin(async move {
            let batch = self
                .load_block_mint_batch(block_height, block_hint, network)
                .await?;
            Ok(batch.valid_mints)
        })
    }
}

pub fn map_usdb_mints_from_inscriptions(
    inscriptions: Vec<DiscoveredInscription>,
    network: Network,
) -> Result<Vec<DiscoveredMint>, String> {
    let batch = classify_usdb_mints_from_inscriptions(inscriptions, network)?;
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
    network: Network,
) -> Result<DiscoveredMintBatch, String> {
    let mut batch = DiscoveredMintBatch::default();
    for inscription in inscriptions {
        let content_string = match &inscription.content_string {
            Some(value) => value.clone(),
            None => continue,
        };

        match InscriptionContentLoader::classify_mint_content_str_with_network(
            &inscription.inscription_id,
            &content_string,
            network,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoincore_rpc::bitcoin::{Address, PublicKey, Txid};

    fn test_inscription_id(tag: u8) -> InscriptionId {
        InscriptionId {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            index: 0,
        }
    }

    fn regtest_address() -> String {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[1; 32]).unwrap();
        let public_key = PublicKey::new(
            bitcoincore_rpc::bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret),
        );
        Address::p2pkh(public_key, Network::Regtest).to_string()
    }

    #[test]
    fn classify_usdb_mints_uses_supplied_network_for_leader_btc_addr() {
        let leader_btc_addr = regtest_address();
        let inscription_id = test_inscription_id(1);
        let content_string = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"leader_btc_addr":"{}","prev":[]}}"#,
            leader_btc_addr
        );
        let inscriptions = vec![DiscoveredInscription {
            inscription_id,
            inscription_number: 1,
            block_height: 10,
            timestamp: 100,
            satpoint: None,
            content_type: Some("application/json".to_string()),
            content_string: Some(content_string),
        }];

        let batch = classify_usdb_mints_from_inscriptions(inscriptions, Network::Regtest).unwrap();

        assert_eq!(batch.valid_mints.len(), 1);
        assert!(batch.invalid_mints.is_empty());
        match &batch.valid_mints[0].content {
            USDBInscription::Mint(mint) => {
                assert_eq!(mint.leader_btc_addr, Some(leader_btc_addr));
            }
        }
    }
}
