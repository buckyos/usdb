use ord::InscriptionId;
use ordinals::SatPoint;
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, de::DeserializeOwned};
use std::str::FromStr;
use std::time::Duration;

const JSON_ERROR_PREVIEW_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub enum ContentBody {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct OrdInscriptionItem {
    pub id: InscriptionId,
    pub number: i32,
    pub timestamp: u32,
    pub satpoint: SatPoint,
    pub content_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrdInscriptionResponse {
    id: String,
    number: i64,
    timestamp: i64,
    satpoint: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    effective_content_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrdBlockInscriptionsResponse {
    #[serde(default)]
    ids: Vec<InscriptionId>,
    #[serde(default)]
    more: bool,
}

pub struct OrdClient {
    client: Client,
    server_url: String,
}

impl OrdClient {
    fn parse_inscription_item(
        url: &str,
        item: OrdInscriptionResponse,
    ) -> Result<OrdInscriptionItem, String> {
        let id = InscriptionId::from_str(&item.id).map_err(|e| {
            let msg = format!(
                "Failed to parse inscription id from ord response: url={}, id={}, error={}",
                url, item.id, e
            );
            error!("{}", msg);
            msg
        })?;
        let satpoint = SatPoint::from_str(&item.satpoint).map_err(|e| {
            let msg = format!(
                "Failed to parse satpoint from ord response: url={}, inscription_id={}, satpoint={}, error={}",
                url, id, item.satpoint, e
            );
            error!("{}", msg);
            msg
        })?;
        let number = i32::try_from(item.number).map_err(|e| {
            let msg = format!(
                "Inscription number out of range from ord response: url={}, inscription_id={}, number={}, error={}",
                url, id, item.number, e
            );
            error!("{}", msg);
            msg
        })?;
        let timestamp_u64 = u64::try_from(item.timestamp).map_err(|e| {
            let msg = format!(
                "Inscription timestamp out of range from ord response: url={}, inscription_id={}, timestamp={}, error={}",
                url, id, item.timestamp, e
            );
            error!("{}", msg);
            msg
        })?;
        let timestamp = u32::try_from(timestamp_u64).map_err(|e| {
            let msg = format!(
                "Inscription timestamp too large from ord response: url={}, inscription_id={}, timestamp={}, error={}",
                url, id, item.timestamp, e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(OrdInscriptionItem {
            id,
            number,
            timestamp,
            satpoint,
            content_type: item.content_type.or(item.effective_content_type),
        })
    }

    fn build_response_preview(body: &[u8]) -> String {
        let preview_len = body.len().min(JSON_ERROR_PREVIEW_BYTES);
        let preview = String::from_utf8_lossy(&body[..preview_len]).replace('\n', "\\n");
        if body.len() > preview_len {
            format!("{}...(truncated, total_bytes={})", preview, body.len())
        } else {
            preview
        }
    }

    async fn parse_json_response<T: DeserializeOwned>(
        resp: reqwest::Response,
        url: &str,
    ) -> Result<T, String> {
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("<missing>")
            .to_string();

        let body = resp.bytes().await.map_err(|e| {
            let msg = format!("Failed to read response body from {}: {}", url, e);
            error!("{}", msg);
            msg
        })?;

        serde_json::from_slice::<T>(&body).map_err(|e| {
            let preview = Self::build_response_preview(body.as_ref());
            let msg = format!(
                "Failed to parse JSON response from {}: {}, status={}, content_type={}, body_preview={}",
                url, e, status, content_type, preview
            );
            error!("{}", msg);
            msg
        })
    }

    pub fn new(server_url: &str) -> Result<Self, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .default_headers(headers)
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

        let block_info: serde_json::Value = Self::parse_json_response(resp, &url).await?;

        // Parse the block height from the JSON response as integer
        if block_info.is_number() {
            Ok(block_info.as_u64().unwrap_or(0) as u32)
        } else {
            let msg = format!(
                "Invalid block height format received from {}: {:?}",
                url, block_info
            );
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
    pub async fn get_inscription(&self, inscription_id: &str) -> Result<OrdInscriptionItem, String> {
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

        let item: OrdInscriptionResponse = Self::parse_json_response(resp, &url).await?;
        let inscription = Self::parse_inscription_item(&url, item)?;

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
    ) -> Result<Vec<OrdInscriptionItem>, String> {
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

        let items: Vec<OrdInscriptionResponse> =
            Self::parse_json_response(resp, &url).await.map_err(|e| {
                format!(
                    "Failed to parse inscriptions payload: url={}, ids_count={}, error={}",
                    url,
                    inscription_ids.len(),
                    e
                )
            })?;

        let mut inscriptions = Vec::with_capacity(items.len());
        for item in items {
            inscriptions.push(Self::parse_inscription_item(&url, item)?);
        }

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

            let block_inscriptions: OrdBlockInscriptionsResponse =
                Self::parse_json_response(resp, &url).await?;

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
