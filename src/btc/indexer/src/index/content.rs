use crate::btc::{ContentBody, OrdClient};
use crate::config::ConfigManager;
use ord::InscriptionId;
use super::inscription::InscriptionOperation;

// check content type at first
const VALID_CONTENT_TYPES: [&str; 3] = [
    "text/plain;charset=utf-8",
    "text/plain'",
    "application/json",
];

// TODO: define different types of USDB inscriptions
#[derive(Debug, Clone)]
pub enum USDBInscription {
    MinerCertificate(serde_json::Value),
}

impl USDBInscription {
    pub fn is_miner_certificate(&self) -> bool {
        matches!(self, USDBInscription::MinerCertificate(_))
    }

    pub fn op(&self) -> InscriptionOperation {
       todo!("Implement op retrieval from USDBInscription");
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
        let usdb_inscription = Self::parse_content(inscription_id, &value)?;
        Ok(Some((content, usdb_inscription)))
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
    ) -> Result<USDBInscription, String> {
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

    pub fn parse_content(inscription_id: &InscriptionId, content: &serde_json::Value) -> Result<USDBInscription, String> {
        // Implement your parsing logic here
        Ok(USDBInscription::MinerCertificate(content.clone()))
    }
}
