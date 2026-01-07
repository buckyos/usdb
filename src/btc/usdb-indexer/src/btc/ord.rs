use ord::{
    InscriptionId,
    api::{Inscription, Inscriptions},
};
use reqwest::Client;
use std::time::Duration;
use reqwest::header::CONTENT_TYPE;

#[derive(Debug, Clone)]
pub enum ContentBody {
    Text(String),
    Binary(Vec<u8>),
}

pub struct OrdClient {
    client: Client,
    server_url: String,
}

impl OrdClient {
    pub fn new(server_url: &str) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| {
                let msg = format!("Failed to create Ord client: {}", e);
                error!("{}", msg);
                msg
            })?;

        Ok(OrdClient {
            client,
            server_url: server_url.to_string(),
        })
    }

    pub async fn get_latest_block_height(&self) -> Result<u32, String> {
        let url = format!("{}/blockheight", self.server_url);
        let resp = self.client.get(&url).send().await.map_err(|e| {
            let msg = format!("Failed to send request to {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        if !resp.status().is_success() {
            let msg = format!(
                "Received non-success status code {} from {}",
                resp.status(),
                url
            );
            error!("{}", msg);
            return Err(msg);
        }

        let block_info: serde_json::Value = resp.json().await.map_err(|e| {
            let msg = format!("Failed to parse JSON response from {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        if let Some(height) = block_info.get("data").and_then(|h| h.as_u64()) {
            Ok(height as u32)
        } else {
            let msg = format!("Missing 'data' field in response from {}", url);
            error!("{}", msg);
            Err(msg)
        }
    }

    /*
    GET /inscription/<INSCRIPTION_ID>
    Description
    Fetch details about a specific inscription by its ID.

    Example

    curl -s -H "Accept: application/json" /
    http://0.0.0.0/inscription/6fb976ab49dcec017f1e201e84395983204ae1a7c2abf7ced0a85d692e442799i0

    return {Inscription}
     */
    pub async fn get_inscription(&self, inscription_id: &str) -> Result<Inscription, String> {
        let url = format!("{}/inscription/{}", self.server_url, inscription_id);
        let resp = self.client.get(&url).send().await.map_err(|e| {
            let msg = format!("Failed to send request to {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        if !resp.status().is_success() {
            let msg = format!(
                "Received non-success status code {} from {}",
                resp.status(),
                url
            );
            error!("{}", msg);
            return Err(msg);
        }

        let inscription: Inscription = resp.json().await.map_err(|e| {
            let msg = format!("Failed to parse JSON response from {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        Ok(inscription)
    }

    /*
    POST /inscriptions
    Description
    Fetch details for a list of inscription IDs.

    Example

    curl -s -X POST \
      -H "Accept: application/json" \
      -H "Content-Type: application/json" \
      -d '["ab924ff229beca227bf40221faf492a20b5e2ee4f084524c84a5f98b80fe527fi1", "ab924ff229beca227bf40221faf492a20b5e2ee4f084524c84a5f98b80fe527fi0"]' \
      http://0.0.0.0/inscriptions

    return [{Inscription}]
    */
    pub async fn get_inscriptions(
        &self,
        inscription_ids: &[InscriptionId],
    ) -> Result<Vec<Inscription>, String> {
        let url = format!("{}/inscriptions", self.server_url);
        let resp = self
            .client
            .post(&url)
            .json(&inscription_ids)
            .send()
            .await
            .map_err(|e| {
                let msg = format!("Failed to send request to {}: {}", url, e);
                error!("{}", msg);
                msg
            })?;

        if !resp.status().is_success() {
            let msg = format!(
                "Received non-success status code {} from {}",
                resp.status(),
                url
            );
            error!("{}", msg);
            return Err(msg);
        }

        let inscriptions: Vec<Inscription> = resp.json().await.map_err(|e| {
            let msg = format!("Failed to parse JSON response from {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        Ok(inscriptions)
    }

    /*
    GET /inscriptions/block/<BLOCKHEIGHT>
    Description
    Get inscriptions for a specific block.

    Example

    curl -s -H "Accept: application/json" \
    http://0.0.0.0/inscriptions/block/767430

    return
    {
    "ids": [
        "6fb976ab49dcec017f1e201e84395983204ae1a7c2abf7ced0a85d692e442799i0"
    ],
    "more": false,
    "page_index": 0
    }
     */

    pub async fn get_inscription_by_block(
        &self,
        block_height: u32,
    ) -> Result<Vec<InscriptionId>, String> {
        let mut page = 0;
        let mut inscription_ids = Vec::new();

        loop {
            let url = format!(
                "{}/inscriptions/block/{}/{}",
                self.server_url, block_height, page
            );
            let resp = self.client.get(&url).send().await.map_err(|e| {
                let msg = format!("Failed to send request to {}: {}", url, e);
                error!("{}", msg);
                msg
            })?;

            if !resp.status().is_success() {
                let msg = format!(
                    "Received non-success status code {} from {}",
                    resp.status(),
                    url
                );
                error!("{}", msg);
                return Err(msg);
            }

            let block_inscriptions: Inscriptions = resp.json().await.map_err(|e| {
                let msg = format!("Failed to parse JSON response from {}: {}", url, e);
                error!("{}", msg);
                msg
            })?;

            inscription_ids.extend(block_inscriptions.ids);

            if block_inscriptions.more {
                page += 1;
            } else {
                break;
            }
        }

        Ok(inscription_ids)
    }

    pub async fn get_content_by_inscription_id(
        &self,
        inscription_id: &InscriptionId,
    ) -> Result<Option<ContentBody>, String> {
        let url = format!("{}/content/{}", self.server_url, inscription_id);
        let resp = self.client.get(&url).send().await.map_err(|e| {
            let msg = format!("Failed to send request to {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        if !resp.status().is_success() {
            if resp.status().as_u16() == 404 {
                warn!("Content not found for inscription ID {}", inscription_id);
                return Ok(None);
            }

            let msg = format!(
                "Received non-success status code {} from {}",
                resp.status(),
                url
            );
            error!("{}", msg);
            return Err(msg);
        }

        // Check if the content type is text-based
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_lowercase());
        let is_text = content_type.as_ref().map_or(false, |ct| {
            ct.starts_with("text/")
                || ct.contains("application/json")
                || ct.contains("application/xml")
        });

        if is_text {
            let content = resp.text().await.map_err(|e| {
                let msg = format!("Failed to read response text from {}: {}", url, e);
                error!("{}", msg);
                msg
            })?;

            Ok(Some(ContentBody::Text(content)))
        } else {
            let content = resp.bytes().await.map_err(|e| {
                let msg = format!("Failed to read response bytes from {}: {}", url, e);
                error!("{}", msg);
                msg
            })?;

            Ok(Some(ContentBody::Binary(content.to_vec())))
        }
    }
}

pub type OrdClientRef = std::sync::Arc<OrdClient>;
