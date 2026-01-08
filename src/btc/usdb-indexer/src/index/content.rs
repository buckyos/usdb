use std::str::FromStr;

use crate::btc::{ContentBody, OrdClient};
use crate::config::ConfigManager;
use ord::InscriptionId;
use serde::{Deserialize, Serialize};
use super::inscription::InscriptionOperation;

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
}

impl MinerPassState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MinerPassState::Active => "active",
            MinerPassState::Dormant => "dormant",
            MinerPassState::Consumed => "consumed",
        }
    }

    pub fn as_int(&self) -> u32 {
        match self {
            MinerPassState::Active => 0,
            MinerPassState::Dormant => 1,
            MinerPassState::Consumed => 2,
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
            _ => Err(format!("Invalid MinerPassState integer: {}", value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct USDBMint {
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<String>,
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
}

pub struct InscriptionContentLoader {}

impl InscriptionContentLoader {
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
        if let Some(ct) = content_type {
            if !VALID_CONTENT_TYPES.contains(&ct.to_ascii_lowercase().as_str()) {
                debug!(
                    "Skipping content load for inscription {} due to unsupported content type: {}",
                    inscription_id, ct
                );
                return Ok(None);
            }
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
        let value = match serde_json::from_str::<serde_json::Value>(content) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!(
                    "Failed to parse content for inscription {} as JSON: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                return Err(msg);
            }
        };

        Self::parse_content(inscription_id, &value)
    }

    pub fn parse_content(inscription_id: &InscriptionId, content: &serde_json::Value) -> Result<Option<USDBInscription>, String> {
        if !content.is_object() {
            return Ok(None);
        }

        let content = content.as_object().unwrap();

        // First check protocol field 'p' is equal to 'usdb'
        let p_field = content.get("p");
        if p_field.is_none() || p_field.unwrap().as_str().unwrap_or("") != "usdb" {
            return Ok(None);
        }

        // For now, we only support 'mint' operation
        let op_field = content.get("op");
        if op_field.is_none() || op_field.unwrap().as_str().unwrap_or("") != "mint" {
            warn!(
                "Unsupported USDB operation for inscription {}: {:?}",
                inscription_id,
                op_field.unwrap_or(&serde_json::Value::Null)
            );
            return Ok(None);
        }

        // Parse the fields for USDBMint
        let mint_inscription: USDBMint = match serde_json::from_value(serde_json::Value::Object(content.clone())) {
            Ok(mint) => mint,
            Err(e) => {
                let msg = format!(
                    "Failed to parse USDBMint content for inscription {}: {}",
                    inscription_id, e
                );
                error!("{}", msg);
                return Ok(None);
            }
        };

        Ok(Some(USDBInscription::Mint(mint_inscription)))
    }
}
