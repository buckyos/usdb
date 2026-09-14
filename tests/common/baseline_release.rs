//! Small producer DB for CLI/crash tests. State semantics use the separate real-chain library tests.

use balance_history::*;
use bitcoincore_rpc::bitcoin::{
    Block, Network, OutPoint, ScriptBuf, Txid, consensus, hashes::Hash,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use usdb_util::{ToBtcScriptHash, UTXOEntry};

pub struct Fixture {
    pub source: PathBuf,
    pub block: PathBuf,
    pub hash: String,
    pub key_root: PathBuf,
    pub signing: PathBuf,
    pub trust: PathBuf,
    pub core: PathBuf,
    pub registry: PathBuf,
}

pub fn fixture(root: &Path) -> Fixture {
    let key_root = root.join("keys");
    let keys = balance_history::tool::generate_snapshot_key_files(
        &key_root,
        "baseline-workflow-test",
        false,
    )
    .unwrap();
    let source = root.join("source");
    let mut config = BalanceHistoryConfig {
        root_dir: source.clone(),
        ..Default::default()
    };
    config.btc.network = Network::Regtest;
    config.snapshot.signing_key_file = Some(keys.signing_key_file.clone());
    let config = Arc::new(config);
    let chain: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/fixtures/assumeutxo-p5/chain.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let block: Block =
        consensus::encode::deserialize_hex(chain["blocks"][103].as_str().unwrap()).unwrap();
    let hash = block.block_hash().to_string();
    // Real managed producers replay the saved Core chain through a local RPC fixture.
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/assumeutxo-p5")
        .canonicalize()
        .unwrap();
    let mut managed = BalanceHistoryConfig::default();
    managed.btc.network = Network::Regtest;
    managed.btc.rpc_url = Some("http://127.0.0.1:1".into());
    managed.btc.auth = Some(usdb_util::BTCAuth::None);
    managed.btc.data_dir = Some(root.join("unused-bitcoin"));
    managed.sync.utxo_max_cache_bytes = 16 * 1024 * 1024;
    managed.sync.balance_max_cache_bytes = 16 * 1024 * 1024;
    std::fs::write(
        root.join("managed-full.toml"),
        toml::to_string(&managed).unwrap(),
    )
    .unwrap();
    managed.bootstrap = Some(serde_json::from_value(serde_json::json!({
        "snapshot_file":fixtures.join("snapshot.dat"),
        "identity":{
            "snapshot":serde_json::from_slice::<serde_json::Value>(&std::fs::read(fixtures.join("identity.json")).unwrap()).unwrap(),
            "origin_height":103,"origin_block_hash":hash,
            "regtest_checkpoint":serde_json::from_slice::<serde_json::Value>(&std::fs::read(fixtures.join("checkpoint.json")).unwrap()).unwrap()
        },"import_batch_size":31,"replay_batch_size":1
    })).unwrap());
    std::fs::write(
        root.join("managed-native.toml"),
        toml::to_string(&managed).unwrap(),
    )
    .unwrap();
    let block_file = root.join("genesis.block");
    std::fs::write(&block_file, consensus::serialize(&block)).unwrap();
    let db =
        Arc::new(BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap());
    let mut outputs = Vec::new();
    let mut histories = Vec::new();
    for index in 1..=40u8 {
        let script = ScriptBuf::from_bytes(vec![0x51, index]);
        let script_hash = script.to_btc_script_hash();
        outputs.push(UTXOEntry {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([index; 32]),
                vout: 0,
            },
            script_hash,
            value: index as u64,
        });
        histories.push(BalanceHistoryEntry {
            script_hash,
            block_height: 100,
            delta: index as i64,
            balance: index as u64,
        });
        db.put_script_registry_entries(&[ScriptRegistryEntry {
            script_hash,
            script_pubkey: script,
        }])
        .unwrap();
    }
    db.put_utxos(&outputs).unwrap();
    db.put_address_history_async(&histories).unwrap();
    db.put_block_commits_async(&[BlockCommitEntry {
        block_height: 103,
        btc_block_hash: block.block_hash(),
        balance_delta_root: [1; 32],
        block_commit: [2; 32],
    }])
    .unwrap();
    db.put_btc_block_height(103).unwrap();
    let indexer = SnapshotIndexer::new(
        config,
        db,
        Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new()))),
    );
    let core = indexer
        .run_core_to_path(103, &root.join("core.db"))
        .unwrap();
    let registry = indexer
        .run_registry_to_path(&core.manifest, &root.join("registry.db"))
        .unwrap();
    drop(indexer);
    Fixture {
        source,
        block: block_file,
        hash,
        key_root,
        signing: keys.signing_key_file,
        trust: keys.trusted_keys_file,
        core: core.manifest_path,
        registry: registry.manifest_path,
    }
}
