use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::btc::{ContentBody, OrdClient};
use crate::config::ConfigManager;
use crate::inscription::InscriptionOperation;
use bitcoincore_rpc::bitcoin::Network;
use ord::InscriptionId;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use usdb_util::address_string_to_script_hash;

const USDB_PROTOCOL_ID: &str = "usdb";
const USDB_MINT_OP: &str = "mint";
const USDB_MINT_SCHEMA_VERSION: u32 = 1;
const USDB_MINT_SCHEMA_FIELDS: [&str; 7] = [
    "p",
    "op",
    "v",
    "usdb_main",
    "leader_pass_id",
    "leader_btc_addr",
    "prev",
];

/*
{
  "p": "usdb",
  "op": "mint",
  "usdb_main": "0x1234...NewUsdbAddr...",
  "usdb_collab": "0x5678...UsdbCollabAddr...",
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
    InvalidUsdbMain,
    InvalidUsdbCollab,
    InvalidLeaderPassId,
    InvalidLeaderBtcAddr,
    InvalidPrevId,
    AmbiguousRevealInput,
}

impl MintValidationErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MintValidationErrorCode::InvalidSchema => "INVALID_SCHEMA",
            MintValidationErrorCode::InvalidUsdbMain => "INVALID_USDB_MAIN",
            MintValidationErrorCode::InvalidUsdbCollab => "INVALID_USDB_COLLAB",
            MintValidationErrorCode::InvalidLeaderPassId => "INVALID_LEADER_PASS_ID",
            MintValidationErrorCode::InvalidLeaderBtcAddr => "INVALID_LEADER_BTC_ADDR",
            MintValidationErrorCode::InvalidPrevId => "INVALID_PREV_ID",
            MintValidationErrorCode::AmbiguousRevealInput => "AMBIGUOUS_REVEAL_INPUT",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MinerPassKind {
    Standard,
    Collab,
}

impl MinerPassKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MinerPassKind::Standard => "standard",
            MinerPassKind::Collab => "collab",
        }
    }
}

impl FromStr for MinerPassKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "standard" => Ok(MinerPassKind::Standard),
            "collab" => Ok(MinerPassKind::Collab),
            _ => Err(format!("Invalid MinerPassKind string: {}", s)),
        }
    }
}

impl Default for MinerPassKind {
    fn default() -> Self {
        MinerPassKind::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct USDBMint {
    #[serde(rename = "v")]
    pub version: u32,
    #[serde(skip)]
    pub pass_kind: MinerPassKind,
    pub usdb_main: String,
    pub usdb_collab: Option<String>,
    pub leader_pass_id: Option<String>,
    pub leader_btc_addr: Option<String>,
    pub prev: Vec<String>,
}

impl USDBMint {
    pub fn standard(usdb_main: String, prev: Vec<String>) -> Self {
        Self {
            version: USDB_MINT_SCHEMA_VERSION,
            pass_kind: MinerPassKind::Standard,
            usdb_main,
            usdb_collab: None,
            leader_pass_id: None,
            leader_btc_addr: None,
            prev,
        }
    }

    pub fn collab_with_leader_pass(leader_pass_id: String, prev: Vec<String>) -> Self {
        Self {
            version: USDB_MINT_SCHEMA_VERSION,
            pass_kind: MinerPassKind::Collab,
            usdb_main: String::new(),
            usdb_collab: None,
            leader_pass_id: Some(leader_pass_id),
            leader_btc_addr: None,
            prev,
        }
    }

    pub fn collab_with_leader_btc_addr(leader_btc_addr: String, prev: Vec<String>) -> Self {
        Self {
            version: USDB_MINT_SCHEMA_VERSION,
            pass_kind: MinerPassKind::Collab,
            usdb_main: String::new(),
            usdb_collab: None,
            leader_pass_id: None,
            leader_btc_addr: Some(leader_btc_addr),
            prev,
        }
    }

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

    pub fn leader_pass_inscription_id(&self) -> Result<Option<InscriptionId>, String> {
        match &self.leader_pass_id {
            Some(id) => InscriptionId::from_str(id)
                .map(Some)
                .map_err(|e| format!("Failed to parse leader_pass_id {}: {}", id, e)),
            None => Ok(None),
        }
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
    fn is_valid_evm_address(value: &str) -> bool {
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

    fn is_valid_btc_address(value: &str, network: Network) -> bool {
        address_string_to_script_hash(value, &network).is_ok()
    }

    pub fn is_supported_content_type(content_type: Option<&str>) -> bool {
        if let Some(ct) = content_type {
            let normalized = ct.trim().to_ascii_lowercase();
            let mut parts = normalized.split(';').map(str::trim);
            let media_type = parts.next().unwrap_or_default();
            if media_type != "application/json" && media_type != "text/plain" {
                return false;
            }

            for part in parts {
                if let Some(charset) = part.strip_prefix("charset=") {
                    return charset.trim_matches('"') == "utf-8";
                }
            }

            return true;
        }

        true
    }

    pub async fn load_content(
        ord_client: &OrdClient,
        inscription_id: &InscriptionId,
        content_type: Option<&str>,
        config: &ConfigManager,
    ) -> Result<Option<(String, USDBInscription)>, String> {
        let content = Self::load_content_data(ord_client, inscription_id, content_type).await?;
        if content.is_none() {
            return Ok(None);
        }

        let content = content.unwrap();
        let network = config.config().bitcoin.network();
        match Self::classify_mint_content_str_with_network(inscription_id, &content, network)? {
            ParsedMintContent::Valid(usdb_inscription) => Ok(Some((content, usdb_inscription))),
            ParsedMintContent::NotUsdbMint | ParsedMintContent::Invalid(_) => Ok(None),
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
        Self::classify_mint_content_str_with_network(inscription_id, content, Network::Bitcoin)
    }

    pub fn classify_mint_content_str_with_network(
        inscription_id: &InscriptionId,
        content: &str,
        network: Network,
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

        if !Self::looks_like_usdb_mint(&value) {
            return Ok(ParsedMintContent::NotUsdbMint);
        }

        let strict_object = match Self::parse_top_level_object_strict(content) {
            Ok(object) => object,
            Err(e) => {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidSchema,
                    reason: format!(
                        "Failed to strictly parse USDB mint payload for inscription {}: {}",
                        inscription_id, e
                    ),
                }));
            }
        };

        Self::classify_mint_object(inscription_id, &strict_object, network)
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
        Self::classify_mint_content_with_network(inscription_id, content, Network::Bitcoin)
    }

    pub fn classify_mint_content_with_network(
        inscription_id: &InscriptionId,
        content: &serde_json::Value,
        network: Network,
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

        Self::classify_mint_object(inscription_id, content, network)
    }

    fn looks_like_usdb_mint(content: &serde_json::Value) -> bool {
        let Some(content) = content.as_object() else {
            return false;
        };

        let p = content
            .get("p")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let op = content
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        p == USDB_PROTOCOL_ID && op == USDB_MINT_OP
    }

    fn classify_mint_object(
        inscription_id: &InscriptionId,
        content: &serde_json::Map<String, serde_json::Value>,
        network: Network,
    ) -> Result<ParsedMintContent, String> {
        for key in content.keys() {
            if key == "usdb_collab" {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidUsdbCollab,
                    reason: format!(
                        "usdb_collab is prohibited in USDB mint v1 for inscription {}",
                        inscription_id
                    ),
                }));
            }

            if !USDB_MINT_SCHEMA_FIELDS.contains(&key.as_str()) {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidSchema,
                    reason: format!(
                        "Unknown field {} in USDB mint payload for inscription {}",
                        key, inscription_id
                    ),
                }));
            }
        }

        let version = match content.get("v").and_then(|value| value.as_u64()) {
            Some(version) if version == USDB_MINT_SCHEMA_VERSION as u64 => USDB_MINT_SCHEMA_VERSION,
            Some(version) => {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidSchema,
                    reason: format!(
                        "Unsupported USDB mint schema version {} for inscription {}",
                        version, inscription_id
                    ),
                }));
            }
            None => {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidSchema,
                    reason: format!(
                        "Missing or invalid v field in USDB mint payload for inscription {}",
                        inscription_id
                    ),
                }));
            }
        };

        let has_usdb_main = content.contains_key("usdb_main");
        let has_leader_pass_id = content.contains_key("leader_pass_id");
        let has_leader_btc_addr = content.contains_key("leader_btc_addr");

        let pass_kind = match (has_usdb_main, has_leader_pass_id, has_leader_btc_addr) {
            (true, false, false) => MinerPassKind::Standard,
            (false, true, false) | (false, false, true) => MinerPassKind::Collab,
            _ => {
                return Ok(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidSchema,
                    reason: format!(
                        "USDB mint v1 must contain either usdb_main or exactly one leader binding field for inscription {}",
                        inscription_id
                    ),
                }));
            }
        };

        let usdb_main = if pass_kind == MinerPassKind::Standard {
            match content.get("usdb_main").and_then(|value| value.as_str()) {
                Some(usdb_main) if Self::is_valid_evm_address(usdb_main) => usdb_main.to_string(),
                Some(usdb_main) => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidUsdbMain,
                        reason: format!(
                            "Invalid usdb_main format for inscription {}: {}",
                            inscription_id, usdb_main
                        ),
                    }));
                }
                None => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidUsdbMain,
                        reason: format!(
                            "Missing or non-string usdb_main for inscription {}",
                            inscription_id
                        ),
                    }));
                }
            }
        } else {
            String::new()
        };

        let leader_pass_id = if has_leader_pass_id {
            match content
                .get("leader_pass_id")
                .and_then(|value| value.as_str())
            {
                Some(leader_pass_id) => match InscriptionId::from_str(leader_pass_id) {
                    Ok(_) => Some(leader_pass_id.to_string()),
                    Err(e) => {
                        return Ok(ParsedMintContent::Invalid(MintValidationError {
                            code: MintValidationErrorCode::InvalidLeaderPassId,
                            reason: format!(
                                "Invalid leader_pass_id for inscription {}: {} ({})",
                                inscription_id, leader_pass_id, e
                            ),
                        }));
                    }
                },
                None => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidLeaderPassId,
                        reason: format!(
                            "Missing or non-string leader_pass_id for inscription {}",
                            inscription_id
                        ),
                    }));
                }
            }
        } else {
            None
        };

        let leader_btc_addr = if has_leader_btc_addr {
            match content
                .get("leader_btc_addr")
                .and_then(|value| value.as_str())
            {
                Some(leader_btc_addr) if Self::is_valid_btc_address(leader_btc_addr, network) => {
                    Some(leader_btc_addr.to_string())
                }
                Some(leader_btc_addr) => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidLeaderBtcAddr,
                        reason: format!(
                            "Invalid leader_btc_addr for network {} in inscription {}: {}",
                            network, inscription_id, leader_btc_addr
                        ),
                    }));
                }
                None => {
                    return Ok(ParsedMintContent::Invalid(MintValidationError {
                        code: MintValidationErrorCode::InvalidLeaderBtcAddr,
                        reason: format!(
                            "Missing or non-string leader_btc_addr for inscription {}",
                            inscription_id
                        ),
                    }));
                }
            }
        } else {
            None
        };

        let prev = match Self::parse_prev_field(inscription_id, content) {
            Ok(prev) => prev,
            Err(invalid) => return Ok(invalid),
        };

        let mint_inscription = USDBMint {
            version,
            pass_kind,
            usdb_main,
            usdb_collab: None,
            leader_pass_id,
            leader_btc_addr,
            prev,
        };

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

    fn parse_prev_field(
        inscription_id: &InscriptionId,
        content: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Vec<String>, ParsedMintContent> {
        let Some(prev_value) = content.get("prev") else {
            return Ok(Vec::new());
        };

        let Some(prev_array) = prev_value.as_array() else {
            return Err(ParsedMintContent::Invalid(MintValidationError {
                code: MintValidationErrorCode::InvalidPrevId,
                reason: format!(
                    "prev must be an array of inscription ids for inscription {}",
                    inscription_id
                ),
            }));
        };

        let mut seen = BTreeSet::new();
        let mut prev = Vec::with_capacity(prev_array.len());
        for item in prev_array {
            let Some(id) = item.as_str() else {
                return Err(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "prev contains a non-string inscription id for inscription {}",
                        inscription_id
                    ),
                }));
            };

            if InscriptionId::from_str(id).is_err() {
                return Err(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Invalid prev inscription id {} for inscription {}",
                        id, inscription_id
                    ),
                }));
            }

            if !seen.insert(id.to_string()) {
                return Err(ParsedMintContent::Invalid(MintValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Duplicate prev inscription id {} for inscription {}",
                        id, inscription_id
                    ),
                }));
            }

            prev.push(id.to_string());
        }

        Ok(prev)
    }

    fn parse_top_level_object_strict(
        content: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, StrictJsonObjectError> {
        let mut deserializer = serde_json::Deserializer::from_str(content);
        let object = deserializer.deserialize_any(StrictJsonObjectVisitor)?;
        deserializer.end()?;
        Ok(object)
    }
}

#[derive(Debug)]
struct StrictJsonObjectError(String);

impl fmt::Display for StrictJsonObjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<serde_json::Error> for StrictJsonObjectError {
    fn from(value: serde_json::Error) -> Self {
        StrictJsonObjectError(value.to_string())
    }
}

struct StrictJsonObjectVisitor;

impl<'de> serde::de::Visitor<'de> for StrictJsonObjectVisitor {
    type Value = serde_json::Map<String, serde_json::Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object without duplicate top-level keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut seen = BTreeSet::new();
        let mut out = serde_json::Map::new();

        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON key {}",
                    key
                )));
            }
            let value = map.next_value::<serde_json::Value>()?;
            out.insert(key, value);
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{Network, Txid};

    const VALID_LEADER_PASS_ID: &str =
        "1111111111111111111111111111111111111111111111111111111111111111i0";
    const VALID_MAINNET_ADDRESS: &str = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
    const VALID_TESTNET_ADDRESS: &str = "tb1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        let txid = Txid::from_slice(&[tag; 32]).unwrap();
        InscriptionId { txid, index }
    }

    #[test]
    fn test_classify_mint_content_str_standard_valid() {
        let inscription_id = test_inscription_id(1, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":["{}"]}}"#,
            VALID_LEADER_PASS_ID
        );

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, &content).unwrap();
        match result {
            ParsedMintContent::Valid(USDBInscription::Mint(mint)) => {
                assert_eq!(mint.version, 1);
                assert_eq!(mint.pass_kind, MinerPassKind::Standard);
                assert_eq!(mint.usdb_main, "0x1111111111111111111111111111111111111111");
                assert_eq!(mint.prev, vec![VALID_LEADER_PASS_ID.to_string()]);
                assert!(mint.leader_pass_id.is_none());
                assert!(mint.leader_btc_addr.is_none());
            }
            _ => panic!("expected valid standard mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_collab_leader_pass_valid() {
        let inscription_id = test_inscription_id(2, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"leader_pass_id":"{}","prev":[]}}"#,
            VALID_LEADER_PASS_ID
        );

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, &content).unwrap();
        match result {
            ParsedMintContent::Valid(USDBInscription::Mint(mint)) => {
                assert_eq!(mint.version, 1);
                assert_eq!(mint.pass_kind, MinerPassKind::Collab);
                assert_eq!(mint.usdb_main, "");
                assert_eq!(mint.leader_pass_id, Some(VALID_LEADER_PASS_ID.to_string()));
                assert!(mint.leader_btc_addr.is_none());
            }
            _ => panic!("expected valid collab mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_collab_leader_btc_addr_valid() {
        let inscription_id = test_inscription_id(3, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"leader_btc_addr":"{}","prev":[]}}"#,
            VALID_MAINNET_ADDRESS
        );

        let result = InscriptionContentLoader::classify_mint_content_str_with_network(
            &inscription_id,
            &content,
            Network::Bitcoin,
        )
        .unwrap();
        match result {
            ParsedMintContent::Valid(USDBInscription::Mint(mint)) => {
                assert_eq!(mint.pass_kind, MinerPassKind::Collab);
                assert_eq!(
                    mint.leader_btc_addr,
                    Some(VALID_MAINNET_ADDRESS.to_string())
                );
                assert!(mint.leader_pass_id.is_none());
            }
            _ => panic!("expected valid collab mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_missing_prev_defaults_empty() {
        let inscription_id = test_inscription_id(4, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111"}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Valid(USDBInscription::Mint(mint)) => {
                assert!(mint.prev.is_empty());
            }
            _ => panic!("expected valid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_usdb_main() {
        let inscription_id = test_inscription_id(5, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x123","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidUsdbMain)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_leader_pass_id() {
        let inscription_id = test_inscription_id(6, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"leader_pass_id":"bad-pass-id","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidLeaderPassId)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_leader_btc_addr_for_network() {
        let inscription_id = test_inscription_id(7, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"leader_btc_addr":"{}","prev":[]}}"#,
            VALID_TESTNET_ADDRESS
        );

        let result = InscriptionContentLoader::classify_mint_content_str_with_network(
            &inscription_id,
            &content,
            Network::Bitcoin,
        )
        .unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidLeaderBtcAddr)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_invalid_prev_id() {
        let inscription_id = test_inscription_id(8, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":["bad-prev-id"]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidPrevId)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_duplicate_prev_invalid() {
        let inscription_id = test_inscription_id(9, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":["{}","{}"]}}"#,
            VALID_LEADER_PASS_ID, VALID_LEADER_PASS_ID
        );

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, &content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidPrevId)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_usdb_main_with_leader_invalid() {
        let inscription_id = test_inscription_id(10, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","leader_pass_id":"{}","prev":[]}}"#,
            VALID_LEADER_PASS_ID
        );

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, &content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_two_leader_bindings_invalid() {
        let inscription_id = test_inscription_id(11, 0);
        let content = format!(
            r#"{{"p":"usdb","op":"mint","v":1,"leader_pass_id":"{}","leader_btc_addr":"{}","prev":[]}}"#,
            VALID_LEADER_PASS_ID, VALID_MAINNET_ADDRESS
        );

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, &content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_missing_identity_invalid() {
        let inscription_id = test_inscription_id(12, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_usdb_collab_invalid() {
        let inscription_id = test_inscription_id(13, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","usdb_collab":"0x2222222222222222222222222222222222222222","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidUsdbCollab)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_unknown_field_invalid() {
        let inscription_id = test_inscription_id(14, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","unexpected":true,"prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_duplicate_key_invalid() {
        let inscription_id = test_inscription_id(15, 0);
        let content = r#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","usdb_main":"0x2222222222222222222222222222222222222222","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected invalid mint content"),
        }
    }

    #[test]
    fn test_classify_mint_content_str_pre_standard_payload_invalid() {
        let inscription_id = test_inscription_id(16, 0);
        let content = r#"{"p":"usdb","op":"mint","usdb_main":"0x1111111111111111111111111111111111111111","prev":[]}"#;

        let result =
            InscriptionContentLoader::classify_mint_content_str(&inscription_id, content).unwrap();
        match result {
            ParsedMintContent::Invalid(err) => {
                assert_eq!(err.code, MintValidationErrorCode::InvalidSchema)
            }
            _ => panic!("expected pre-standard payload to be invalid v1 mint"),
        }
    }

    #[test]
    fn test_supported_content_type_accepts_json_utf8() {
        assert!(InscriptionContentLoader::is_supported_content_type(Some(
            "application/json;charset=utf-8"
        )));
        assert!(InscriptionContentLoader::is_supported_content_type(Some(
            "APPLICATION/JSON; Charset=\"utf-8\""
        )));
        assert!(!InscriptionContentLoader::is_supported_content_type(Some(
            "application/json;charset=gbk"
        )));
    }
}
