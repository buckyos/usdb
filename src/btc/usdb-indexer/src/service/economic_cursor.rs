use super::rpc::EconomicExternalState;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use usdb_util::USDB_ECONOMIC_PAGE_MAX_LIMIT;

/// Maximum number of rows accepted by UIP-0006 cursor-paged queries.
pub(crate) const ECONOMIC_PAGE_MAX_LIMIT: usize = USDB_ECONOMIC_PAGE_MAX_LIMIT;

const ECONOMIC_CURSOR_VERSION: &str = "uip-0006-economic-cursor:v1";
const ECONOMIC_CURSOR_HASH_DOMAIN: &[u8] = b"usdb-indexer:uip-0006-economic-cursor:v1\0";
const MAX_ENCODED_CURSOR_LENGTH: usize = 32 * 1024;

/// Candidate-set continuation state bound to one immutable economic view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateSetCursor {
    pub view_version: String,
    pub external_state: EconomicExternalState,
    pub selection_rule: String,
    pub limit: usize,
    pub last_effective_energy: String,
    pub last_pass_id: String,
}

/// Collab-breakdown continuation state bound to one Leader and sort order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollabBreakdownCursor {
    pub view_version: String,
    pub external_state: EconomicExternalState,
    pub leader_pass_id: String,
    pub sort: String,
    pub limit: usize,
    pub last_collab_contribution: String,
    pub last_collab_pass_id: String,
}

/// Opaque UIP-0006 continuation payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "resource", content = "state", rename_all = "snake_case")]
pub(crate) enum EconomicPageCursor {
    CandidateSet(CandidateSetCursor),
    CollabBreakdown(CollabBreakdownCursor),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EconomicCursorEnvelope {
    cursor_version: String,
    payload: EconomicPageCursor,
    checksum: String,
}

/// Encode a cursor with a domain-separated checksum over its canonical payload.
///
/// The checksum detects corruption and schema drift. Cursor opacity is a query
/// contract, not an authorization boundary; callers must not parse or construct
/// cursor values.
pub(crate) fn encode_economic_cursor(cursor: EconomicPageCursor) -> Result<String, String> {
    let checksum = cursor_checksum(&cursor)?;
    let envelope = EconomicCursorEnvelope {
        cursor_version: ECONOMIC_CURSOR_VERSION.to_string(),
        payload: cursor,
        checksum,
    };
    let encoded = serde_json::to_vec(&envelope)
        .map_err(|e| format!("Failed to serialize economic cursor envelope: {}", e))?;
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

/// Decode and verify one opaque UIP-0006 cursor.
pub(crate) fn decode_economic_cursor(value: &str) -> Result<EconomicPageCursor, String> {
    if value.is_empty() || value.len() > MAX_ENCODED_CURSOR_LENGTH {
        return Err(format!(
            "Economic cursor length must be between 1 and {} bytes",
            MAX_ENCODED_CURSOR_LENGTH
        ));
    }

    let raw = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|e| format!("Failed to decode economic cursor: {}", e))?;
    if raw.len() > MAX_ENCODED_CURSOR_LENGTH {
        return Err("Decoded economic cursor exceeds the size limit".to_string());
    }

    let envelope: EconomicCursorEnvelope = serde_json::from_slice(&raw)
        .map_err(|e| format!("Failed to parse economic cursor: {}", e))?;
    if envelope.cursor_version != ECONOMIC_CURSOR_VERSION {
        return Err(format!(
            "Unsupported economic cursor version {}, expected {}",
            envelope.cursor_version, ECONOMIC_CURSOR_VERSION
        ));
    }

    let expected_checksum = cursor_checksum(&envelope.payload)?;
    if envelope.checksum != expected_checksum {
        return Err("Economic cursor checksum mismatch".to_string());
    }

    Ok(envelope.payload)
}

fn cursor_checksum(cursor: &EconomicPageCursor) -> Result<String, String> {
    let payload = serde_json::to_vec(cursor)
        .map_err(|e| format!("Failed to serialize economic cursor payload: {}", e))?;
    let mut hasher = Sha256::new();
    hasher.update(ECONOMIC_CURSOR_HASH_DOMAIN);
    hasher.update(ECONOMIC_CURSOR_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(payload);
    Ok(encode_hex(&hasher.finalize()))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Network;
    use usdb_util::embedded_btc_activation_registry;

    fn external_state() -> EconomicExternalState {
        let registry = embedded_btc_activation_registry(Network::Regtest).unwrap();
        let active_version_set = registry.lookup_active_version_set(120).unwrap();
        EconomicExternalState {
            btc_height: 120,
            snapshot_id: "snapshot".to_string(),
            stable_block_hash: "block".to_string(),
            stable_lag: registry.stable_lag_blocks(),
            local_state_commit: "local".to_string(),
            system_state_id: "system".to_string(),
            balance_history_api_version: "api".to_string(),
            balance_history_semantics_version: "semantics".to_string(),
            activation_registry_id: registry.activation_registry_id(),
            active_version_set_id: active_version_set.active_version_set_id(),
            active_version_set,
        }
    }

    #[test]
    fn test_economic_cursor_round_trip_preserves_all_bindings() {
        let cursor = EconomicPageCursor::CandidateSet(CandidateSetCursor {
            view_version: "view".to_string(),
            external_state: external_state(),
            selection_rule: "rule".to_string(),
            limit: 100,
            last_effective_energy: "123456789".to_string(),
            last_pass_id: "pass".to_string(),
        });

        let encoded = encode_economic_cursor(cursor.clone()).unwrap();
        assert_eq!(decode_economic_cursor(&encoded).unwrap(), cursor);
    }

    #[test]
    fn test_economic_cursor_rejects_tampered_payload() {
        let cursor = EconomicPageCursor::CollabBreakdown(CollabBreakdownCursor {
            view_version: "view".to_string(),
            external_state: external_state(),
            leader_pass_id: "leader".to_string(),
            sort: "collab_pass_id_asc".to_string(),
            limit: 10,
            last_collab_contribution: "50".to_string(),
            last_collab_pass_id: "collab".to_string(),
        });
        let encoded = encode_economic_cursor(cursor).unwrap();
        let mut raw = URL_SAFE_NO_PAD.decode(&encoded).unwrap();
        let position = raw
            .windows("collab".len())
            .rposition(|window| window == b"collab")
            .unwrap();
        raw[position] = b'd';
        let tampered = URL_SAFE_NO_PAD.encode(raw);

        assert!(
            decode_economic_cursor(&tampered)
                .unwrap_err()
                .contains("checksum mismatch")
        );
    }
}
