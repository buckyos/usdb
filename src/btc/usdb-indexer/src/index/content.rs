use std::str::FromStr;

use crate::btc::{ContentBody, OrdClient};
use crate::config::ConfigManager;
use crate::inscription::InscriptionOperation;
use ord::InscriptionId;
use serde::{Deserialize, Serialize};

// check content type at first
const VALID_CONTENT_TYPES: [&str; 3] = [
    "text/plain;charset=utf-8",
    "text/plain'",
    "application/json",
];

/*
{
  "p": "usdb",
  "op": "mint",
  "eth_main": "0x1234...NewEthAddr...",
  "eth_collab": "0x5678...CollabAddr...",
  "prev": [
    "old_inscription_id_a",
    "old_inscription_id_b"
  ]
}
*/

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum MinerPassState {
    Active = 0,
    Dormant = 1,
    Consumed = 2,
    Burned = 3,
    Invalid = 4,
}

impl MinerPassState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MinerPassState::Active => "active",
            MinerPassState::Dormant => "dormant",
            MinerPassState::Consumed => "consumed",
            MinerPassState::Burned => "burned",
            MinerPassState::Invalid => "invalid",
        }
    }

    pub fn as_int(&self) -> u32 {
        match self {
            MinerPassState::Active => 0,
            MinerPassState::Dormant => 1,
            MinerPassState::Consumed => 2,
            MinerPassState::Burned => 3,
            MinerPassState::Invalid => 4,
        }
    }
}

impl FromStr for MinerPassState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "active" => Ok(MinerPassState::Active),
            "dormant" => Ok(MinerPassState::Dormant),
            "consumed" => Ok(MinerPassState::Consumed),
            "burned" => Ok(MinerPassState::Burned),
            "invalid" => Ok(MinerPassState::Invalid),
            _ => Err(format!("Invalid MinerPassState string: {}", s)),
        }
    }
}

impl TryFrom<u32> for MinerPassState {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(MinerPassState::Active),
            1 => Ok(MinerPassState::Dormant),
            2 => Ok(MinerPassState::Consumed),
            3 => Ok(MinerPassState::Burned),
            4 => Ok(MinerPassState::Invalid),
            _ => Err(format!("Invalid MinerPassState integer: {}", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MintValidationErrorCode {
    InvalidSchema,
    InvalidEthMain,
    InvalidEthCollab,
    InvalidPrevId,
}

impl MintValidationErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MintValidationErrorCode::InvalidSchema => "INVALID_SCHEMA",
            MintValidationErrorCode::InvalidEthMain => "INVALID_ETH_MAIN",
            MintValidationErrorCode::InvalidEthCollab => "INVALID_ETH_COLLAB",
            MintValidationErrorCode::InvalidPrevId => "INVALID_PREV_ID",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintValidationError {
    pub code: MintValidationErrorCode,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum ParsedMintContent {
    NotUsdbMint,
    Valid(USDBInscription),
    Invalid(MintValidationError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct USDBMint {
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<String>,
}

impl USDBMint {
    pub fn prev_inscription_ids(&self) -> Result<Vec<InscriptionId>, String> {
        self.prev
            .iter()
            .map(|prev| {
                InscriptionId::from_str(prev).map_err(|e| {
                    format!(
                        "Failed to parse prev inscription id {} in USDBMint: {}",
                        prev, e
                    )
                })
            })
            .collect()
    }
}

// TODO: define different types of USDB inscriptions
#[derive(Debug, Clone)]
pub enum USDBInscription {
    Mint(USDBMint),
}

impl USDBInscription {
    pub fn is_mint(&self) -> bool {
        matches!(self, USDBInscription::Mint(_))
    }

    pub fn op(&self) -> InscriptionOperation {
        match self {
            USDBInscription::Mint(_) => InscriptionOperation::Inscribe,
        }
    }

    pub fn as_mint(&self) -> Option<&USDBMint> {
        match self {
            USDBInscription::Mint(mint) => Some(mint),
        }
    }
}

pub struct InscriptionContentLoader {}

impl InscriptionContentLoader {
    fn is_valid_eth_address(value: &str) -> bool {
        if value.len() != 42 {
            return false;
        }
        if !value.starts_with("0x") {
            return false;
        }
        value
            .as_bytes()
            .iter()
            .skip(2)
            .all(|b| (*b as char).is_ascii_hexdigit())
    }

    pub fn is_supported_content_type(content_type: Option<&str>) -> bool {
        if let Some(ct) = content_type {
            return VALID_CONTENT_TYPES.contains(&ct.to_ascii_lowercase().as_str());
        }

        true
    }

    pub async fn load_content(
        ord_client: &OrdClient,
        inscription_id: &InscriptionId,
        content_type: Option<&str>,
        _config: &ConfigManager,
    ) -> Result<Option<(String, USDBInscription)>, String> {
        let content = Self::load_content_data(ord_client, inscription_id, content_type).await?;
        if content.is_none() {
            return Ok(None);
        }

        let content = content.unwrap();

        // Check if content is valid json then parse it
        let value = match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Failed to parse content for inscription {} as JSON: {}",
                    inscription_id, e
                );
                debug!("{}", msg);
                return Ok(None);
            }
        };

        // Parse the content into USDBInscription
        let ret = Self::parse_content(inscription_id, &value)?;
        match ret {
            Some(usdb_inscription) => Ok(Some((content, usdb_inscription))),
            None => Ok(None),
        }
    }

    // Load content data which in text mode
    async fn load_content_data(
        ord_client: &OrdClient,
        inscription_id: &InscriptionId,
        content_type: Option<&str>,
    ) -> Result<Option<String>, String> {
        // Check content type at first
        if !Self::is_supported_content_type(content_type) {
            debug!(
                "Skipping content load for inscription {} due to unsupported content type: {}",
                inscription_id,
                content_type.unwrap_or_default()
            );
            return Ok(None);
        }

        let content_opt = ord_client
            .get_content_by_inscription_id(inscription_id)
            .await?;

        let content = if let Some(content) = content_opt {
            match content {
                ContentBody::Text(text) => text,
                ContentBody::Binary(_data) => {
                    // Ignore binary content for now
                    return Ok(None);
                }
            }
        } else {
            return Ok(None);
        };

        Ok(Some(content))
    }

    pub fn parse_content_str(
        inscription_id: &InscriptionId,
        content: &str,
    ) -> Result<Option<USDBInscription>, String> {
        match Self::classify_mint_content_str(inscription_id, content)? {
            ParsedMintContent::Valid(v) => Ok(Some(v)),
            ParsedMintContent::NotUsdbMint | ParsedMintContent::Invalid(_) => Ok(None),
        }
    }

    pub fn classify_mint_content_str(
        inscription_id: &InscriptionId,
        content: &str,
    ) -> Result<ParsedMintContent, String> {
        let value = match serde_json::from_str::<serde_json::Value>(content) {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    "Skipping non-JSON inscription content: module=content_loader, inscription_id={}, error={}",
                    inscription_id, e
                );
                return Ok(ParsedMintContent::NotUsdbMint);
            }
        };

        Self::classify_mint_content(inscription_id, &value)
    }

    pub fn parse_content(
        inscription_id: &InscriptionId,
        content: &serde_json::Value,
    ) -> Result<Option<USDBInscription>, String> {
        match Self::classify_mint_content(inscription_id, content)? {
            ParsedMintContent::Valid(v) => Ok(Some(v)),
            ParsedMintContent::NotUsdbMint | ParsedMintContent::Invalid(_) => Ok(None),
        }
    }

    pub fn classify_mint_content(
        inscription_id: &InscriptionId,
        content: &serde_json::Value,
    ) -> Result<ParsedMintContent, String> {
        if !content.is_object() {
            return Ok(ParsedMintContent::NotUsdbMint);
        }

        let content = content.as_object().unwrap();

        // First check protocol field 'p' is equal to 'usdb'
        let p_field = content.get("p");
        if p_field.is_none() || p_field.unwrap().as_str().unwrap_or("") != "usdb" {
            return Ok(ParsedMintContent::NotUsdbMint);
        }

        // For now, we only support 'mint' operation
        let op_field = content.get("op");
        if op_field.is_none() || op_field.unwrap().as_str().unwrap_or("") != "mint" {
            warn!(
                "Unsupported USDB operation for inscription {}: {:?}",
                inscription_id,
                op_field.unwrap_or(&serde_json::Value::Null)
            );
            return Ok(ParsedMintContent::NotUsdbMint);
        }

        let mint_inscription: USDBMint =
            match serde_json::from_value(serde_json::Value::Object(content.clone())) {
                Ok(mint) => mint,
                Err(e) => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidSchema,
                        reason: format!(
                            "Failed to parse USDB mint payload for inscription {}: {}",
                            inscription_id, e
                        ),
                    }));
                }
            };

        if !Self::is_valid_eth_address(&mint_inscription.eth_main) {
            return Ok(ParsedMintContent::Invalid(MintValidationError {
                code: MintValidationErrorCode::InvalidEthMain,
                reason: format!(
                    "Invalid eth_main format for inscription {}: {}",
                    inscription_id, mint_inscription.eth_main
                ),
            }));
        }

        if let Some(collab) = &mint_inscription.eth_collab {
            if !Self::is_valid_eth_address(collab) {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidEthCollab,
                    reason: format!(
                        "Invalid eth_collab format for inscription {}: {}",
                        inscription_id, collab
                    ),
                }));
            }
        }

        if let Err(e) = mint_inscription.prev_inscription_ids() {
            return Ok(ParsedMintContent::Invalid(MintValidationError {
                code: MintValidationErrorCode::InvalidPrevId,
                reason: e,
            }));
        }

        Ok(ParsedMintContent::Valid(USDBInscription::Mint(
            mint_inscription,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Txid;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        let txid = Txid::from_slice(&[tag; 32]).unwrap();
        InscriptionId { txid, index }
    }

    #[test]
    fn test_classify_mint_content_str_valid() {
        let inscription_id = test_inscription_id(1, 0);
        let content = r#"{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","eth_collab":"0x2222222222222222222222222222222222222222","prev":["1111111111111111111111111111111111111111111111111111111111111111i0"]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        assert!(matches!(result, ParsedMintContent::Valid(_)));
    }

    #[test]
    fn test_classify_mint_content_str_invalid_eth_main() {
        let inscription_id = test_inscription_id(2, 0);
        let content = r#"{"p":"usdb","op":"mint","eth_main":"0x123","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidEthMain)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_eth_collab() {
        let inscription_id = test_inscription_id(3, 0);
        let content = r#"{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","eth_collab":"0xabc","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidEthCollab)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_prev_id() {
        let inscription_id = test_inscription_id(4, 0);
        let content = r#"{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":["bad-prev-id"]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidPrevId)
            }
            _ => panic!("expected invalid mint content"),
        }
    }
}
