//! Proposed bootstrap-origin commitment. Production startup and RPC activation remain separate.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use bitcoincore_rpc::bitcoin::{BlockHash, Network, blockdata::constants::genesis_block};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{BALANCE_HISTORY_DATA_MODEL_VERSION, BalanceHistoryConfig, BalanceHistoryDB};

/// Canonical identity schema for a business-origin state, independent of its import source.
pub const BOOTSTRAP_ORIGIN_SCHEMA: &str = "balance-history-bootstrap-origin:v1";
/// Proposed rolling-commit version. This does not activate an embedded network version.
pub const BOOTSTRAP_COMMIT_PROTOCOL_VERSION: &str = "2.0.0";

/// Ordered logical table digest; zero-valued UTXOs are retained, zero balances are omitted.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OriginTableDigest {
    /// Number of canonical projected rows.
    pub rows: u64,
    /// Sum of output values or current balances, in satoshi.
    pub total_sats: u64,
    /// Lowercase SHA-256 of the ordered fixed-width row encodings.
    pub sha256: String,
}

/// Complete hash input for the proposed origin commit. Physical storage and old commits are excluded.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootstrapOriginIdentity {
    /// Canonical schema and domain separator.
    pub schema_version: String,
    /// Proposed commit rules selected for a future network activation.
    pub commit_protocol_version: String,
    /// Bitcoin network; encoded in the hash by its genesis block hash.
    pub network: Network,
    /// Business origin after applying this BTC block, independently of the import baseline.
    pub origin_height: u32,
    /// Pinned canonical BTC block hash at the business origin.
    pub origin_block_hash: BlockHash,
    /// Logical balance/UTXO interpretation; not the physical RocksDB schema.
    pub data_model_version: String,
    /// All live outpoints in descending canonical encoded-key order.
    pub utxos: OriginTableDigest,
    /// Latest nonzero balance per script, in descending raw script-hash order.
    pub balances: OriginTableDigest,
}

/// Offline calculation result; never installed as a production commit by this tool.
#[derive(Debug, Serialize)]
pub struct BootstrapOriginReport {
    /// Auditable logical identity used by the hash function.
    pub identity: BootstrapOriginIdentity,
    /// Proposed first commit, before rolling subsequent blocks under the new protocol.
    pub origin_commit: String,
    /// Explicit distinction from an activated network or installed database migration.
    pub activated: bool,
    /// Wall time spent opening and scanning the local database.
    pub elapsed_seconds: f64,
}

/// Encode the proposed identity with length-prefixed UTF-8 strings and fixed-width binary fields.
/// See the P6 design document and independent Python golden vector for the exact wire encoding.
pub fn derive_origin_commit(identity: &BootstrapOriginIdentity) -> Result<String, String> {
    if identity.schema_version != BOOTSTRAP_ORIGIN_SCHEMA
        || identity.commit_protocol_version != BOOTSTRAP_COMMIT_PROTOCOL_VERSION
        || identity.data_model_version != BALANCE_HISTORY_DATA_MODEL_VERSION
        || identity.origin_height == 0
        || identity.utxos.total_sats != identity.balances.total_sats
    {
        return Err("Invalid or unsupported bootstrap origin identity".to_string());
    }
    let mut hash = Sha256::new();
    for value in [
        &identity.schema_version,
        &identity.commit_protocol_version,
        &identity.data_model_version,
    ] {
        let length = u32::try_from(value.len()).map_err(|_| "Origin identity string too long")?;
        hash.update(length.to_be_bytes());
        hash.update(value.as_bytes());
    }
    hash.update(genesis_block(identity.network).block_hash().as_ref() as &[u8]);
    hash.update(identity.origin_height.to_be_bytes());
    hash.update(identity.origin_block_hash.as_ref() as &[u8]);
    for table in [&identity.utxos, &identity.balances] {
        // Reject alternate spellings so the human-readable identity is canonical too.
        if table.sha256.len() != 64
            || !table
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(
                "Origin table digest must be 64 lowercase hexadecimal characters".to_string(),
            );
        }
        hash.update(table.rows.to_be_bytes());
        hash.update(table.total_sats.to_be_bytes());
        hash.update(crate::assumeutxo::format::hash_bytes(&table.sha256)?);
    }
    Ok(crate::assumeutxo::format::hex(&hash.finalize()))
}

/// Inspect a stopped exact-height database read-only, with no old core/registry snapshot or RPC.
/// The caller supplies the expected canonical height/hash; this function does not verify chainwork.
/// It commits to the indexed logical state, not to full historical metadata or registry coverage.
pub fn inspect_bootstrap_origin(
    state_dir: &Path,
    network: Network,
    origin_height: u32,
    origin_block_hash: BlockHash,
) -> Result<BootstrapOriginReport, String> {
    let result = inspect_origin_state(state_dir, network, origin_height, origin_block_hash);
    result.map_err(|error| {
        let message = format!(
            "Bootstrap origin inspection failed: state_dir={}, network={network}, height={origin_height}, block_hash={origin_block_hash}, error={error}",
            state_dir.display()
        );
        eprintln!("{message}");
        message
    })
}

// Keep state-directory context at the public boundary for all validation and I/O failures.
fn inspect_origin_state(
    state_dir: &Path,
    network: Network,
    origin_height: u32,
    origin_block_hash: BlockHash,
) -> Result<BootstrapOriginReport, String> {
    if !state_dir.is_absolute() || origin_height == 0 {
        return Err(
            "Origin inspection requires an absolute state directory and positive height"
                .to_string(),
        );
    }
    let started = Instant::now();
    eprintln!(
        "Bootstrap origin inspection started: state_dir={}, network={network}, height={origin_height}, block_hash={origin_block_hash}",
        state_dir.display()
    );
    let mut config = BalanceHistoryConfig {
        root_dir: state_dir.to_path_buf(),
        ..Default::default()
    };
    config.btc.network = network;
    let db = BalanceHistoryDB::open_read_only(Arc::new(config))?;
    let identity = db.bootstrap_origin_identity(network, origin_height, origin_block_hash)?;
    let origin_commit = derive_origin_commit(&identity)?;
    eprintln!(
        "Bootstrap origin inspection finished: height={origin_height}, origin_commit={origin_commit}, activated=false, elapsed_seconds={:.1}",
        started.elapsed().as_secs_f64()
    );
    Ok(BootstrapOriginReport {
        identity,
        origin_commit,
        activated: false,
        elapsed_seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_commit_matches_independent_python_vector() {
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/bootstrap-origin-v1.json"
        ))
        .unwrap();
        let mut identity: BootstrapOriginIdentity =
            serde_json::from_value(golden["identity"].clone()).unwrap();
        let expected = derive_origin_commit(&identity).unwrap();
        assert_eq!(expected, golden["origin_commit"].as_str().unwrap());
        identity.origin_height += 1;
        assert_ne!(derive_origin_commit(&identity).unwrap(), expected);
        identity.origin_height -= 1;
        identity.network = Network::Bitcoin;
        assert_ne!(derive_origin_commit(&identity).unwrap(), expected);
        identity.utxos.total_sats += 1;
        assert!(derive_origin_commit(&identity).is_err());
        identity.utxos.total_sats -= 1;
        identity.commit_protocol_version = "1.0.0".to_string();
        assert!(derive_origin_commit(&identity).is_err());
    }
}
