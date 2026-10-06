//! Full production indexer with isolated Core/Balance History RPC fixtures and real stores/tracker.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bitcoincore_rpc::bitcoin::{Block, ScriptBuf, hashes::Hash};
use serde_json::{Value, json};
use usdb_util::{BtcActivationRegistryCatalog, BtcScriptHash, ToBtcScriptHash};

use crate::config::{ConfigManager, IndexerConfig};
use crate::index::InscriptionIndexer;
use crate::index::test_miner_evidence as chain;
use crate::index::test_miner_state::{MintBlock, SpendKind};
use crate::output::IndexOutput;
use crate::service::UsdbIndexerRpcServer;
use crate::status::StatusManager;

pub const CATALOG: &str = include_str!("../fixtures/miner-pass-v2/catalog.json");

type Timeline = HashMap<BtcScriptHash, BTreeMap<u32, u64>>;

#[derive(Clone)]
pub struct History {
    pub values: Timeline,
    pub blocks: BTreeMap<u32, Block>,
    pub fail_commit: Option<u32>,
    pub fail_balances: bool,
}
impl History {
    fn commit(&self, height: u32) -> balance_history::BlockCommitInfo {
        balance_history::BlockCommitInfo {
            block_height: height,
            btc_block_hash: self.blocks[&height].block_hash().to_string(),
            balance_delta_root: format!("{height:064x}"),
            // Different canonical forks must produce different upstream commitments.
            block_commit: bitcoincore_rpc::bitcoin::hashes::sha256::Hash::hash(
                format!("fixture-bh:{height}:{}", self.blocks[&height].block_hash()).as_bytes(),
            )
            .to_string(),
            commit_protocol_version: "1.0.0".into(),
            commit_hash_algo: "sha256".into(),
        }
    }
    pub fn snapshot(&self, height: u32) -> balance_history::SnapshotInfo {
        let commit = self.commit(height);
        balance_history::SnapshotInfo {
            stable_height: height,
            stable_block_hash: Some(commit.btc_block_hash),
            latest_block_commit: Some(commit.block_commit),
            balance_query_floor: 0,
            history_query_floor: 0,
            stable_lag: 10,
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.into(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .into(),
            commit_protocol_version: commit.commit_protocol_version,
            commit_hash_algo: commit.commit_hash_algo,
        }
    }
    fn reference(&self, height: u32) -> balance_history::HistoricalSnapshotStateRef {
        let commit = self.commit(height);
        let identity = usdb_util::ConsensusSnapshotIdentity {
            source_chain: "btc".into(),
            network: "regtest".into(),
            stable_height: height,
            stable_block_hash: commit.btc_block_hash.clone(),
            stable_lag: 10,
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.into(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .into(),
        };
        balance_history::HistoricalSnapshotStateRef {
            block_height: height,
            stable_block_hash: commit.btc_block_hash,
            latest_block_commit: commit.block_commit,
            snapshot_id: usdb_util::build_consensus_snapshot_id(&identity),
            consensus_identity: identity,
            snapshot_id_hash_algo: usdb_util::CONSENSUS_SNAPSHOT_ID_HASH_ALGO.into(),
            snapshot_id_version: usdb_util::CONSENSUS_SNAPSHOT_ID_VERSION.into(),
            commit_protocol_version: "1.0.0".into(),
            commit_hash_algo: "sha256".into(),
        }
    }
    fn records(&self, owner: BtcScriptHash, params: &Value) -> Value {
        let empty = BTreeMap::from([(0, 0)]);
        let values = self.values.get(&owner).unwrap_or(&empty);
        let mut records = Vec::new();
        if let Some(height) = params["block_height"].as_u64() {
            let (&h, &balance) = values.range(..=height as u32).next_back().unwrap();
            let old = values.range(..h).next_back().map(|(_, v)| *v).unwrap_or(0);
            records.push(
                json!({"block_height":h,"balance":balance,"delta":balance as i64-old as i64}),
            );
        } else {
            let range = &params["block_range"];
            let start = range["start"].as_u64().unwrap() as u32;
            let end = range["end"].as_u64().unwrap() as u32;
            for (&h, &balance) in values.range(start..end) {
                let old = values.range(..h).next_back().map(|(_, v)| *v).unwrap_or(0);
                records.push(
                    json!({"block_height":h,"balance":balance,"delta":balance as i64-old as i64}),
                );
            }
        }
        json!(records)
    }
}

pub struct Pipeline {
    pub root: PathBuf,
    pub config: Arc<ConfigManager>,
    pub indexer: Arc<InscriptionIndexer>,
    pub status: Arc<StatusManager>,
    pub core: chain::ChainCore,
    pub history: Arc<Mutex<History>>,
    _history_rpc: chain::RpcServer,
}

impl Pipeline {
    pub async fn new(name: &str, batches: &[&MintBlock], origin: u32) -> Self {
        Self::with_catalog(name, batches, origin, CATALOG).await
    }

    pub async fn with_catalog(
        name: &str,
        batches: &[&MintBlock],
        origin: u32,
        catalog: &str,
    ) -> Self {
        let mut merged: BTreeMap<u32, (Block, Value)> = BTreeMap::new();
        for batch in batches {
            for (&height, (block, verbose)) in &batch.core.state.lock().unwrap().blocks {
                if let Some((existing, description)) = merged.get_mut(&height) {
                    existing.txdata.extend(block.txdata.iter().skip(1).cloned());
                    description["tx"]
                        .as_array_mut()
                        .unwrap()
                        .extend(verbose["tx"].as_array().unwrap().iter().skip(1).cloned());
                } else {
                    merged.insert(height, (block.clone(), verbose.clone()));
                }
            }
        }
        let mut previous = None;
        for (block, verbose) in merged.values_mut() {
            if let Some(hash) = previous {
                block.header.prev_blockhash = hash;
            }
            block.header.merkle_root = block.compute_merkle_root().unwrap();
            verbose["hash"] = json!(block.block_hash());
            previous = Some(block.block_hash());
        }
        let core = chain::ChainCore::new(
            merged
                .iter()
                .map(|(&h, (b, v))| (h, b.clone(), v.clone()))
                .collect(),
        );
        let mut values: Timeline = HashMap::new();
        let mut current = HashMap::new();
        for kind in [SpendKind::Legacy, SpendKind::Witness, SpendKind::Taproot] {
            let owner = chain::source_script(kind).to_btc_script_hash();
            current.insert(owner, 100_000_000i128);
            values.insert(owner, BTreeMap::from([(0, 100_000_000)]));
        }
        for (&height, (block, _)) in &merged {
            let inputs = core.client.get_block_prevouts(height, block).unwrap();
            for tx in &block.txdata {
                if !tx.is_coinbase() {
                    for input in &tx.input {
                        let output = &inputs.get(&input.previous_output).unwrap().txout;
                        *current
                            .entry(output.script_pubkey.to_btc_script_hash())
                            .or_default() -= i128::from(output.value.to_sat());
                    }
                }
                for output in &tx.output {
                    if !usdb_util::is_core_unspendable(&output.script_pubkey) {
                        *current
                            .entry(output.script_pubkey.to_btc_script_hash())
                            .or_default() += i128::from(output.value.to_sat());
                    }
                }
            }
            for (&owner, &balance) in &current {
                assert!(
                    balance >= 0,
                    "negative fixture balance at {height} for {owner}"
                );
                values
                    .entry(owner)
                    .or_insert_with(|| BTreeMap::from([(0, 0)]))
                    .insert(height, balance as u64);
            }
        }
        let history = Arc::new(Mutex::new(History {
            values,
            blocks: merged.into_iter().map(|(h, (b, _))| (h, b)).collect(),
            fail_commit: None,
            fail_balances: false,
        }));
        let shared = history.clone();
        let history_rpc = chain::RpcServer::new(move |request| {
            let history = shared.lock().unwrap();
            let params = &request["params"][0];
            let height = params["block_height"]
                .as_u64()
                .or_else(|| params.as_u64())
                .unwrap_or(0) as u32;
            let method = request["method"].as_str().unwrap();
            let failure = method == "get_block_commit" && history.fail_commit == Some(height)
                || method == "get_addresses_balances" && history.fail_balances;
            let result = if failure {
                Value::Null
            } else {
                match method {
                    "get_state_ref_at_height" => json!(history.reference(height)),
                    "get_block_commit" => json!(history.commit(height)),
                    "get_address_balance" => history.records(
                        serde_json::from_value(params["script_hash"].clone()).unwrap(),
                        params,
                    ),
                    "get_addresses_balances" => json!(
                        params["script_hashes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|owner| history
                                .records(serde_json::from_value(owner.clone()).unwrap(), params))
                            .collect::<Vec<_>>()
                    ),
                    _ => panic!("unexpected Balance History method {method}: {params}"),
                }
            };
            (
                200,
                json!({"jsonrpc":"2.0","id":request["id"],"result":result,
                "error":failure.then(||json!({"code":-32001,"message":"injected upstream unavailable"}))}),
            )
        });
        let root = std::env::temp_dir().join(format!(
            "usdb-v2-pipeline-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut cfg = IndexerConfig::default();
        cfg.bitcoin.network = bitcoincore_rpc::bitcoin::Network::Regtest;
        cfg.bitcoin.rpc_url = Some(core.url().into());
        cfg.bitcoin.auth = Some(usdb_util::BTCAuth::None);
        cfg.balance_history.rpc_url = history_rpc.url.clone();
        cfg.usdb.genesis_block_height = origin;
        cfg.usdb.inscription_source = "bitcoind".into();
        cfg.usdb.rules_scope = Some(
            BtcActivationRegistryCatalog::from_json(catalog)
                .unwrap()
                .current_registry()
                .scope
                .rules_scope()
                .into(),
        );
        cfg.usdb.activation_registry_id = Some(
            BtcActivationRegistryCatalog::from_json(catalog)
                .unwrap()
                .current_registry_id()
                .into(),
        );
        cfg.usdb.activation_registry_catalog_file = Some("catalog.json".into());
        cfg.usdb.balance_query_max_retries = 0;
        std::fs::write(root.join("config.json"), serde_json::to_vec(&cfg).unwrap()).unwrap();
        std::fs::write(root.join("catalog.json"), catalog).unwrap();
        let config = Arc::new(ConfigManager::load(Some(root.clone())).unwrap());
        let status =
            Arc::new(StatusManager::new(config.clone(), Arc::new(IndexOutput::new())).unwrap());
        let indexer = Arc::new(InscriptionIndexer::new(config.clone(), status.clone()).unwrap());
        indexer.init().await.unwrap();
        Self {
            root,
            config,
            indexer,
            status,
            core,
            history,
            _history_rpc: history_rpc,
        }
    }

    pub async fn reopen(self) -> Self {
        self.while_stopped(|_| {}).await
    }

    /// Execute an isolated crash child or copy a coherent checkpoint with every store closed.
    pub async fn while_stopped(self, action: impl FnOnce(&std::path::Path)) -> Self {
        let Self {
            root,
            config,
            indexer,
            status,
            core,
            history,
            _history_rpc,
        } = self;
        drop(indexer);
        drop(status);
        action(&root);
        let status =
            Arc::new(StatusManager::new(config.clone(), Arc::new(IndexOutput::new())).unwrap());
        let indexer = Arc::new(InscriptionIndexer::new(config.clone(), status.clone()).unwrap());
        indexer.init().await.unwrap();
        Self {
            root,
            config,
            indexer,
            status,
            core,
            history,
            _history_rpc,
        }
    }

    pub fn rpc(&self) -> UsdbIndexerRpcServer {
        let (tx, _) = tokio::sync::watch::channel(());
        UsdbIndexerRpcServer::new(
            self.config.clone(),
            self.status.clone(),
            self.indexer.clone(),
            "127.0.0.1:0".parse().unwrap(),
            tx,
        )
    }
    pub async fn sync(&self, from: u32, to: u32) -> Result<u32, String> {
        self.indexer.sync_blocks_for_test(from..=to).await
    }
    /// Use the ordinary upstream-reconciliation loop, including pending recovery and fork detection.
    pub async fn sync_to(&self, height: u32) -> Result<u32, String> {
        self.status
            .set_balance_history_snapshot(Some(self.history.lock().unwrap().snapshot(height)));
        self.indexer.sync_once_for_test().await
    }

    /// Change only the mock canonical upstream; local stores and tracker remain untouched.
    pub fn follow_chain(&self, other: &Self) {
        self.core.state.lock().unwrap().blocks = other.core.state.lock().unwrap().blocks.clone();
        *self.history.lock().unwrap() = other.history.lock().unwrap().clone();
    }

    pub fn cleanup(self) {
        let root = self.root.clone();
        drop(self);
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub fn cold_recipient(tag: u8) -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&bitcoincore_rpc::bitcoin::WPubkeyHash::from_byte_array(
        [tag; 20],
    ))
}

/// Rewrite a fixture reveal while keeping its actual spent-prevout evidence consistent.
pub fn replace_reveal(
    batch: &mut MintBlock,
    height: u32,
    tx: bitcoincore_rpc::bitcoin::Transaction,
    extra: HashMap<bitcoincore_rpc::bitcoin::OutPoint, usdb_util::SpentPrevout>,
) {
    let original = batch.core.state.lock().unwrap().blocks[&height].0.clone();
    let inputs = batch
        .core
        .client
        .get_block_prevouts(height, &original)
        .unwrap();
    let mut coins = extra;
    for input in &original.txdata[1].input {
        coins.insert(
            input.previous_output,
            inputs.get(&input.previous_output).unwrap().clone(),
        );
    }
    let mut block = chain::block(vec![tx]);
    block.header.prev_blockhash = original.header.prev_blockhash;
    batch.mints[0].inscription_id.txid = block.txdata[1].compute_txid();
    let verbose = chain::verbose(height, &block, &coins);
    batch
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(height, (block, verbose));
}

/// Add a transfer to a block without substituting mocked transfer-tracker results.
pub fn append_transaction(
    batch: &mut MintBlock,
    height: u32,
    tx: bitcoincore_rpc::bitcoin::Transaction,
    extra: HashMap<bitcoincore_rpc::bitcoin::OutPoint, usdb_util::SpentPrevout>,
) {
    let original = batch.core.state.lock().unwrap().blocks[&height].0.clone();
    let inputs = batch
        .core
        .client
        .get_block_prevouts(height, &original)
        .unwrap();
    let mut coins = extra;
    for tx in original.txdata.iter().skip(1) {
        for input in &tx.input {
            coins.insert(
                input.previous_output,
                inputs.get(&input.previous_output).unwrap().clone(),
            );
        }
    }
    let mut block = original;
    block.txdata.push(tx);
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    let verbose = chain::verbose(height, &block, &coins);
    batch
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(height, (block, verbose));
}
