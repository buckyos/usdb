//! Tiny immutable artifacts shared by full-comparison and Electrs sampling tests.

use balance_history::*;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, Network, OutPoint, ScriptBuf, Txid};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use usdb_util::{
    ConsensusSnapshotIdentity, ToBtcScriptHash, UTXOEntry, build_consensus_snapshot_id,
};

pub struct Fixture {
    pub core: PathBuf,
    pub registry: PathBuf,
    pub legacy: PathBuf,
}

pub fn fixture(root: &Path) -> Fixture {
    std::fs::create_dir_all(root).unwrap();
    let core = root.join("core_10.db");
    let registry = root.join("registry_10.db");
    let legacy = root.join("legacy_10.db");
    let identity = BalanceHistoryDBIdentity::for_network(Network::Regtest);
    let mut commits = Vec::new();
    let mut previous = [0; 32];
    for height in 9..=10u32 {
        let mut block_bytes = [height as u8; 32];
        block_bytes[0] += 1;
        let block_hash = BlockHash::from_byte_array(block_bytes);
        let root = [height as u8 + 1; 32];
        let mut hasher = Sha256::new();
        hasher.update(b"balance-history:block-commit:v1");
        hasher.update(height.to_be_bytes());
        hasher.update(block_hash.to_byte_array());
        hasher.update(root);
        hasher.update(previous);
        previous = hasher.finalize().into();
        commits.push(BlockCommitEntry {
            block_height: height,
            btc_block_hash: block_hash,
            balance_delta_root: root,
            block_commit: previous,
        });
    }
    let block_hash = commits[1].btc_block_hash.to_string();
    let consensus_identity = ConsensusSnapshotIdentity {
        source_chain: "btc".to_string(),
        network: "regtest".to_string(),
        stable_height: 10,
        stable_block_hash: block_hash.clone(),
        stable_lag: 10,
        balance_history_api_version: "1.0.0".to_string(),
        balance_history_semantics_version: "1".to_string(),
    };
    let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
    let state = HistoricalSnapshotStateRef {
        block_height: 10,
        stable_block_hash: block_hash.clone(),
        latest_block_commit: previous
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect(),
        consensus_identity,
        snapshot_id: snapshot_id.clone(),
        snapshot_id_hash_algo: usdb_util::CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: usdb_util::CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    };
    let mut core_db = CoreSnapshotDb::create(&core).unwrap();
    let mut registry_db = ScriptRegistrySnapshotDb::create(&registry).unwrap();
    for index in 1..=12u8 {
        let script = ScriptBuf::from_bytes(vec![0x51, index]);
        let script_hash = script.to_btc_script_hash();
        registry_db
            .put_entries(&[ScriptRegistryEntry {
                script_hash,
                script_pubkey: script,
            }])
            .unwrap();
        if index <= 8 {
            core_db
                .put_balance_history_entries(&[BalanceHistoryEntry {
                    script_hash,
                    block_height: 9,
                    balance: u64::from(index),
                    delta: i64::from(index),
                }])
                .unwrap();
            core_db
                .put_utxo_entries(&[UTXOEntry {
                    outpoint: OutPoint {
                        txid: Txid::from_byte_array([index; 32]),
                        vout: 0,
                    },
                    script_hash,
                    value: u64::from(index),
                }])
                .unwrap();
        }
    }
    core_db.put_block_commit_entries(&commits).unwrap();
    core_db
        .write_meta(&CoreSnapshotMeta {
            block_height: 10,
            balance_history_count: 8,
            utxo_count: 8,
            block_commit_count: 2,
            generated_at: 1,
            db_identity: identity.clone(),
            core_snapshot_id: snapshot_id.clone(),
        })
        .unwrap();
    core_db.finalize_for_distribution().unwrap();
    let core_manifest = CoreSnapshotManifest::build(
        "core_10.db".to_string(),
        SnapshotHash::calc_hash(&core).unwrap(),
        state,
        identity.clone(),
        None,
        1,
    )
    .unwrap();
    core_manifest
        .save(&core.with_extension("manifest.json"))
        .unwrap();
    let base = ScriptRegistryBaseIdentity {
        btc_network: "regtest".to_string(),
        btc_genesis_hash: identity.btc_genesis_hash,
        base_height: 10,
        base_block_hash: block_hash,
        core_snapshot_id: snapshot_id,
    };
    registry_db
        .write_meta(&ScriptRegistrySnapshotMeta {
            base: base.clone(),
            entry_count: 12,
            generated_at: 1,
        })
        .unwrap();
    registry_db.finalize_for_distribution().unwrap();
    ScriptRegistryManifest::build(
        "registry_10.db".to_string(),
        SnapshotHash::calc_hash(&registry).unwrap(),
        base,
        12,
        None,
        1,
    )
    .unwrap()
    .save(&registry.with_extension("manifest.json"))
    .unwrap();
    let old = Connection::open(&legacy).unwrap();
    old.execute("ATTACH DATABASE ?1 AS core", [core.to_str().unwrap()])
        .unwrap();
    old.execute(
        "ATTACH DATABASE ?1 AS registry",
        [registry.to_str().unwrap()],
    )
    .unwrap();
    old.execute_batch("CREATE TABLE meta(block_height INTEGER, balance_history_count INTEGER, utxo_count INTEGER, block_commit_count INTEGER, script_registry_count INTEGER, generated_at INTEGER, version INTEGER);
        INSERT INTO meta VALUES(10,8,8,2,12,1,2);
        CREATE TABLE balance_history AS SELECT * FROM core.balance_history;
        CREATE TABLE utxos AS SELECT * FROM core.utxos;
        CREATE TABLE block_commits AS SELECT * FROM core.block_commits;
        CREATE TABLE script_registry AS SELECT * FROM registry.script_registry;").unwrap();
    drop(old);
    Fixture {
        core,
        registry,
        legacy,
    }
}
