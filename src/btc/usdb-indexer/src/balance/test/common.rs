use crate::index::MinerPassState;
use crate::storage::MinerPassInfo;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
use ord::InscriptionId;
use ordinals::SatPoint;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

pub(super) fn test_data_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("usdb_balance_monitor_{}_{}", tag, nanos));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn cleanup_data_dir(dir: &PathBuf) {
    if dir.exists() {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub(super) fn script_hash(tag: u8) -> USDBScriptHash {
    ScriptBuf::from(vec![tag; 32]).to_usdb_script_hash()
}

pub(super) fn inscription_id(tag: u8, index: u32) -> InscriptionId {
    InscriptionId {
        txid: Txid::from_slice(&[tag; 32]).unwrap(),
        index,
    }
}

pub(super) fn satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
    SatPoint {
        outpoint: OutPoint {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            vout,
        },
        offset,
    }
}

pub(super) fn make_pass(
    tag: u8,
    index: u32,
    owner: USDBScriptHash,
    mint_block_height: u32,
) -> MinerPassInfo {
    MinerPassInfo {
        inscription_id: inscription_id(tag, index),
        inscription_number: index as i32 + 1,
        mint_txid: Txid::from_slice(&[tag.wrapping_add(1); 32]).unwrap(),
        mint_block_height,
        mint_owner: owner,
        satpoint: satpoint(tag, index, 0),
        eth_main: "0x1111111111111111111111111111111111111111".to_string(),
        eth_collab: None,
        prev: Vec::new(),
        owner,
        state: MinerPassState::Active,
    }
}
