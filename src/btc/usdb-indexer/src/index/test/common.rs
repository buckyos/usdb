use crate::index::energy::BalanceProvider;
use balance_history::AddressBalance;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
use ord::InscriptionId;
use ordinals::SatPoint;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

pub(super) fn test_root_dir(suite: &str, test_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("usdb_indexer_{}_{}_{}", suite, test_name, nanos))
}

pub(super) fn cleanup_temp_dir(root_dir: &PathBuf) {
    if root_dir.exists() {
        std::fs::remove_dir_all(root_dir).unwrap();
    }
}

pub(super) fn test_script_hash(tag: u8) -> USDBScriptHash {
    let script = ScriptBuf::from(vec![tag; 32]);
    script.to_usdb_script_hash()
}

pub(super) fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
    InscriptionId {
        txid: Txid::from_slice(&[tag; 32]).unwrap(),
        index,
    }
}

pub(super) fn test_satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
    SatPoint {
        outpoint: OutPoint {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            vout,
        },
        offset,
    }
}

#[derive(Default)]
pub(super) struct MockBalanceProvider {
    heights: Mutex<HashMap<(USDBScriptHash, u32), Vec<AddressBalance>>>,
    ranges: Mutex<HashMap<(USDBScriptHash, u32, u32), Vec<AddressBalance>>>,
}

impl MockBalanceProvider {
    pub(super) fn with_height(
        self,
        address: USDBScriptHash,
        block_height: u32,
        balance: u64,
        delta: i64,
    ) -> Self {
        self.heights.lock().unwrap().insert(
            (address, block_height),
            vec![AddressBalance {
                block_height,
                balance,
                delta,
            }],
        );
        self
    }

    pub(super) fn with_range(
        self,
        address: USDBScriptHash,
        block_range: Range<u32>,
        items: Vec<AddressBalance>,
    ) -> Self {
        self.ranges
            .lock()
            .unwrap()
            .insert((address, block_range.start, block_range.end), items);
        self
    }
}

impl BalanceProvider for MockBalanceProvider {
    fn get_balance_at_height<'a>(
        &'a self,
        address: USDBScriptHash,
        block_height: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move {
            let balances = self
                .heights
                .lock()
                .unwrap()
                .get(&(address, block_height))
                .cloned()
                .unwrap_or_default();
            Ok(balances)
        })
    }

    fn get_balance_at_range<'a>(
        &'a self,
        address: USDBScriptHash,
        block_range: Range<u32>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move {
            let balances = self
                .ranges
                .lock()
                .unwrap()
                .get(&(address, block_range.start, block_range.end))
                .cloned()
                .unwrap_or_default();
            Ok(balances)
        })
    }
}
