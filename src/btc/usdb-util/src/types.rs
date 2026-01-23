use crate::USDBScriptHash;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, Txid};
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
