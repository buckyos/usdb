use crate::USDBScriptHash;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, Txid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct UTXOEntry {
    pub outpoint: OutPoint,
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

impl UTXOEntry {
    pub fn outpoint_vec(&self) -> [u8; 36] {
        OutPointCodec::encode(&self.outpoint)
    }
}

#[derive(Debug, Clone)]
pub struct UTXOValue {
    pub script_hash: USDBScriptHash,
    pub value: u64,
}

impl UTXOValue {
    pub fn to_vec(&self) -> [u8; USDBScriptHash::LEN + 8] {
        Self::encode(&self.script_hash, self.value)
    }

    pub fn encode(script_hash: &USDBScriptHash, value: u64) -> [u8; USDBScriptHash::LEN + 8] {
        let mut data = [0u8; USDBScriptHash::LEN + 8];
        data[..USDBScriptHash::LEN].copy_from_slice(script_hash.as_ref() as &[u8]);
        data[USDBScriptHash::LEN..].copy_from_slice(&value.to_be_bytes());
        data
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, String> {
        if data.len() != USDBScriptHash::LEN + 8 {
            return Err("Invalid UTXOValue data length".to_string());
        }

        let script_hash = USDBScriptHash::from_slice(&data[0..USDBScriptHash::LEN])
            .map_err(|e| format!("Failed to parse script hash: {}", e))?;
        let value = u64::from_be_bytes(
            data[USDBScriptHash::LEN..USDBScriptHash::LEN + 8]
                .try_into()
                .map_err(|_| "Failed to parse value".to_string())?,
        );

        Ok(UTXOValue { script_hash, value })
    }
}

pub type UTXOEntryRef = Arc<UTXOValue>;
pub type OutPointRef = Arc<OutPoint>;

#[derive(Debug, Clone)]
pub struct BalanceHistoryData {
    pub block_height: u32,
    pub delta: i64,
    pub balance: u64,
}

pub type BalanceHistoryDataRef = Arc<BalanceHistoryData>;

/// Fixed source-chain tag used by BTC-side consensus snapshot identifiers.
pub const CONSENSUS_SOURCE_CHAIN_BTC: &str = "BTC";
/// Hash algorithm used by canonical consensus snapshot ids.
pub const CONSENSUS_SNAPSHOT_ID_HASH_ALGO: &str = "sha256";
/// Version tag of the canonical consensus snapshot-id serialization rule.
pub const CONSENSUS_SNAPSHOT_ID_VERSION: &str = "btc-consensus-snapshot:v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsensusSnapshotIdentity {
    /// Fixed chain namespace, currently `BTC`.
    pub source_chain: String,
    /// Bitcoin network name, such as `mainnet` or `regtest`.
    pub network: String,
    /// Stable BTC height committed by the upstream balance-history snapshot.
    pub stable_height: u32,
    /// Stable BTC block hash paired with `stable_height`.
    pub stable_block_hash: String,
    /// Fixed lag rule used when interpreting `stable_height`.
    pub stable_lag: u32,
    /// Externally visible RPC/API version of balance-history.
    pub balance_history_api_version: String,
    /// Historical query semantics version of balance-history.
    pub balance_history_semantics_version: String,
    /// Version of the usdb-index derived-state formula set.
    pub usdb_index_formula_version: String,
    /// Version of the usdb-index external protocol contract.
    pub usdb_index_protocol_version: String,
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

fn update_string_component(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u32).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub fn build_consensus_snapshot_id(identity: &ConsensusSnapshotIdentity) -> String {
    let mut hasher = Sha256::new();
    update_string_component(&mut hasher, CONSENSUS_SNAPSHOT_ID_VERSION);
    update_string_component(&mut hasher, &identity.source_chain);
    update_string_component(&mut hasher, &identity.network);
    hasher.update(identity.stable_height.to_be_bytes());
    update_string_component(&mut hasher, &identity.stable_block_hash);
    hasher.update(identity.stable_lag.to_be_bytes());
    update_string_component(&mut hasher, &identity.balance_history_api_version);
    update_string_component(&mut hasher, &identity.balance_history_semantics_version);
    update_string_component(&mut hasher, &identity.usdb_index_formula_version);
    update_string_component(&mut hasher, &identity.usdb_index_protocol_version);
    encode_hex(&hasher.finalize())
}

pub struct OutPointCodec;

pub const OUTPOINT_SIZE: usize = 36;

impl OutPointCodec {
    pub fn encode(outpoint: &OutPoint) -> [u8; OUTPOINT_SIZE] {
        let mut key = [0u8; OUTPOINT_SIZE];
        key[..32].copy_from_slice(outpoint.txid.as_ref());
        key[32..36].copy_from_slice(&outpoint.vout.to_be_bytes());
        key
    }

    pub fn decode(data: &[u8]) -> Result<OutPoint, String> {
        if data.len() != OUTPOINT_SIZE {
            return Err("Invalid data length".to_string());
        }

        let txid = Txid::from_slice(&data[0..32]).map_err(|e| e.to_string())?;
        let vout = u32::from_be_bytes(
            data[32..36]
                .try_into()
                .map_err(|_| "Failed to parse vout".to_string())?,
        );
        Ok(OutPoint { txid, vout })
    }
}
