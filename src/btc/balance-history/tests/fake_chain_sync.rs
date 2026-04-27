use balance_history::btc::{BTCClient, BTCClientRef, BTCClientType};
use balance_history::config::BalanceHistoryConfig;
use balance_history::index::BalanceHistoryIndexer;
use balance_history::output::IndexOutput;
use balance_history::status::SyncStatusManager;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{
    Amount, Block, BlockHash, CompactTarget, OutPoint, ScriptBuf, Sequence, Transaction, TxIn,
    TxMerkleNode, TxOut, Witness, absolute, block, transaction,
};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use usdb_util::ToUSDBScriptHash;

#[derive(Clone)]
struct FakeChain {
    state: Arc<RwLock<FakeChainState>>,
}

struct FakeChainState {
    latest_height: u32,
    blocks: BTreeMap<u32, Block>,
}

impl FakeChain {
    fn new(latest_height: u32, blocks: BTreeMap<u32, Block>) -> Self {
        Self {
            state: Arc::new(RwLock::new(FakeChainState {
                latest_height,
                blocks,
            })),
        }
    }

    fn replace_block(&self, height: u32, block: Block) {
        self.state.write().unwrap().blocks.insert(height, block);
    }
}

#[async_trait::async_trait]
impl BTCClient for FakeChain {
    fn get_type(&self) -> BTCClientType {
        BTCClientType::RPC
    }

    fn init(&self) -> Result<(), String> {
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        Ok(())
    }

    fn on_sync_complete(&self, _block_height: u32) -> Result<(), String> {
        Ok(())
    }

    fn get_latest_block_height(&self) -> Result<u32, String> {
        Ok(self.state.read().unwrap().latest_height)
    }

    fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
        let state = self.state.read().unwrap();
        state
            .blocks
            .get(&block_height)
            .map(|block| block.block_hash())
            .ok_or_else(|| format!("missing fake block at height {block_height}"))
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        let state = self.state.read().unwrap();
        state
            .blocks
            .values()
            .find(|block| block.block_hash() == *block_hash)
            .cloned()
            .ok_or_else(|| format!("missing fake block hash {block_hash}"))
    }

    fn get_block_by_height(&self, block_height: u32) -> Result<Block, String> {
        let state = self.state.read().unwrap();
        state
            .blocks
            .get(&block_height)
            .cloned()
            .ok_or_else(|| format!("missing fake block at height {block_height}"))
    }

    async fn get_blocks(&self, start_height: u32, end_height: u32) -> Result<Vec<Block>, String> {
        (start_height..=end_height)
            .map(|height| self.get_block_by_height(height))
            .collect()
    }

    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        Err(format!("fake chain has no external UTXO for {outpoint}"))
    }
}

struct Harness {
    indexer: BalanceHistoryIndexer,
}

impl Harness {
    fn new(name: &str, fake_chain: FakeChain, max_sync_block_height: u32) -> Self {
        let mut config = BalanceHistoryConfig::default();
        config.root_dir = temp_root(name);
        config.sync.batch_size = 16;
        config.sync.max_sync_block_height = max_sync_block_height;
        config.sync.undo_retention_blocks = 16;
        config.sync.undo_cleanup_interval_blocks = 16;
        config.sync.utxo_max_cache_bytes = 4 * 1024 * 1024;
        config.sync.balance_max_cache_bytes = 4 * 1024 * 1024;

        let config = Arc::new(config);
        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let btc_client: BTCClientRef = Arc::new(Box::new(fake_chain) as Box<dyn BTCClient>);
        let indexer =
            BalanceHistoryIndexer::new_with_btc_client(config, output, btc_client).unwrap();

        Self { indexer }
    }
}

struct FakeScenario {
    chain: FakeChain,
    original_block_3: Block,
    reorg_block_3: Block,
    script_a: ScriptBuf,
    script_b: ScriptBuf,
    script_c: ScriptBuf,
    script_d: ScriptBuf,
    script_e: ScriptBuf,
    script_f: ScriptBuf,
    op_return_script: ScriptBuf,
    outpoint_b: OutPoint,
    outpoint_c: OutPoint,
    outpoint_d: OutPoint,
    outpoint_e: OutPoint,
    outpoint_f: OutPoint,
}

fn build_scenario() -> FakeScenario {
    let script_a = script(1);
    let script_b = script(2);
    let script_c = script(3);
    let script_d = script(4);
    let script_e = script(5);
    let script_f = script(6);
    let miner_2 = script(7);
    let miner_3 = script(8);
    let miner_3_reorg = script(9);
    let op_return_script = op_return_script(99);

    let coinbase_1 = coinbase_tx(
        1,
        vec![output(100, &script_a), output(0, &op_return_script)],
    );
    let outpoint_a = outpoint(&coinbase_1, 0);
    let block_1 = block(BlockHash::all_zeros(), 1, vec![coinbase_1]);

    let spend_a = spend_tx(
        vec![outpoint_a],
        vec![output(40, &script_b), output(60, &script_c)],
    );
    let outpoint_b = outpoint(&spend_a, 0);
    let outpoint_c = outpoint(&spend_a, 1);
    let same_block_spend_b = spend_tx(
        vec![outpoint_b],
        vec![
            output(25, &script_d),
            output(15, &script_a),
            output(0, &op_return_script),
        ],
    );
    let outpoint_d = outpoint(&same_block_spend_b, 0);
    let coinbase_2 = coinbase_tx(2, vec![output(50, &miner_2)]);
    let block_2 = block(
        block_1.block_hash(),
        2,
        vec![coinbase_2, spend_a, same_block_spend_b],
    );

    let spend_c_and_d = spend_tx(
        vec![outpoint_c, outpoint_d],
        vec![
            output(80, &script_e),
            output(5, &script_a),
            output(0, &op_return_script),
        ],
    );
    let outpoint_e = outpoint(&spend_c_and_d, 0);
    let coinbase_3 = coinbase_tx(3, vec![output(50, &miner_3)]);
    let original_block_3 = block(block_2.block_hash(), 3, vec![coinbase_3, spend_c_and_d]);

    let reorg_spend_c = spend_tx(
        vec![outpoint_c],
        vec![output(55, &script_f), output(5, &script_a)],
    );
    let outpoint_f = outpoint(&reorg_spend_c, 0);
    let reorg_coinbase_3 = coinbase_tx(33, vec![output(50, &miner_3_reorg)]);
    let reorg_block_3 = block(
        block_2.block_hash(),
        33,
        vec![reorg_coinbase_3, reorg_spend_c],
    );

    let chain = FakeChain::new(
        5,
        BTreeMap::from([(1, block_1), (2, block_2), (3, original_block_3.clone())]),
    );

    FakeScenario {
        chain,
        original_block_3,
        reorg_block_3,
        script_a,
        script_b,
        script_c,
        script_d,
        script_e,
        script_f,
        op_return_script,
        outpoint_b,
        outpoint_c,
        outpoint_d,
        outpoint_e,
        outpoint_f,
    }
}

#[test]
fn process_block_batch_indexes_coinbase_spends_op_return_and_registry() {
    let scenario = build_scenario();
    let harness = Harness::new("process_block_batch_shapes", scenario.chain.clone(), 3);
    let db = harness.indexer.db().clone();

    harness.indexer.process_block_batch(1..4, 3).unwrap();

    assert_eq!(db.get_btc_block_height().unwrap(), 3);
    assert_balance(&db, &scenario.script_a, 1, 100, 100);
    assert_balance(&db, &scenario.script_a, 2, -85, 15);
    assert_balance(&db, &scenario.script_a, 3, 5, 20);
    assert_balance(&db, &scenario.script_b, 2, 0, 0);
    assert_balance(&db, &scenario.script_c, 3, -60, 0);
    assert_balance(&db, &scenario.script_d, 3, -25, 0);
    assert_balance(&db, &scenario.script_e, 3, 80, 80);

    assert!(db.get_utxo(&scenario.outpoint_b).unwrap().is_none());
    assert!(db.get_utxo(&scenario.outpoint_c).unwrap().is_none());
    assert!(db.get_utxo(&scenario.outpoint_d).unwrap().is_none());
    assert!(db.get_utxo(&scenario.outpoint_e).unwrap().is_some());

    assert_registry(&db, &scenario.script_a, true);
    assert_registry(&db, &scenario.script_b, true);
    assert_registry(&db, &scenario.script_c, true);
    assert_registry(&db, &scenario.script_d, true);
    assert_registry(&db, &scenario.script_e, true);
    assert_registry(&db, &scenario.op_return_script, false);
    assert_eq!(
        db.get_block_commit(3).unwrap().unwrap().btc_block_hash,
        scenario.original_block_3.block_hash()
    );
}

#[test]
fn sync_once_rolls_back_reorg_and_restores_cross_batch_utxos() {
    let scenario = build_scenario();
    let harness = Harness::new("sync_once_reorg", scenario.chain.clone(), 3);
    let db = harness.indexer.db().clone();

    assert_eq!(harness.indexer.sync_once().unwrap(), 3);
    assert_balance(&db, &scenario.script_e, 3, 80, 80);
    assert!(db.get_utxo(&scenario.outpoint_d).unwrap().is_none());

    scenario
        .chain
        .replace_block(3, scenario.reorg_block_3.clone());

    assert_eq!(harness.indexer.sync_once().unwrap(), 3);

    assert_eq!(db.get_btc_block_height().unwrap(), 3);
    assert_eq!(
        db.get_block_commit(3).unwrap().unwrap().btc_block_hash,
        scenario.reorg_block_3.block_hash()
    );
    assert_balance(&db, &scenario.script_a, 3, 5, 20);
    assert_balance(&db, &scenario.script_c, 3, -60, 0);
    assert_balance(&db, &scenario.script_d, 2, 25, 25);
    assert_empty_balance(&db, &scenario.script_e, 3);
    assert_balance(&db, &scenario.script_f, 3, 55, 55);

    assert!(db.get_utxo(&scenario.outpoint_d).unwrap().is_some());
    assert!(db.get_utxo(&scenario.outpoint_e).unwrap().is_none());
    assert!(db.get_utxo(&scenario.outpoint_f).unwrap().is_some());
    assert!(db.get_block_undo_bundle(3).unwrap().is_some());
    assert_registry(&db, &scenario.script_f, true);
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("balance_history_{name}_{nanos}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn script(tag: u8) -> ScriptBuf {
    ScriptBuf::from(vec![tag; 32])
}

fn op_return_script(tag: u8) -> ScriptBuf {
    ScriptBuf::from(vec![0x6a, 0x01, tag])
}

fn output(value: u64, script_pubkey: &ScriptBuf) -> TxOut {
    TxOut {
        value: Amount::from_sat(value),
        script_pubkey: script_pubkey.clone(),
    }
}

fn coinbase_tx(tag: u8, output: Vec<TxOut>) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from(vec![tag]),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output,
    }
}

fn spend_tx(input: Vec<OutPoint>, output: Vec<TxOut>) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: input
            .into_iter()
            .map(|previous_output| TxIn {
                previous_output,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            })
            .collect(),
        output,
    }
}

fn outpoint(tx: &Transaction, vout: u32) -> OutPoint {
    OutPoint {
        txid: tx.compute_txid(),
        vout,
    }
}

fn block(prev_blockhash: BlockHash, nonce: u32, txdata: Vec<Transaction>) -> Block {
    let mut block = Block {
        header: block::Header {
            version: block::Version::TWO,
            prev_blockhash,
            merkle_root: TxMerkleNode::all_zeros(),
            time: 1_700_000_000 + nonce,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce,
        },
        txdata,
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    block
}

fn assert_balance(
    db: &balance_history::db::BalanceHistoryDB,
    script_pubkey: &ScriptBuf,
    height: u32,
    expected_delta: i64,
    expected_balance: u64,
) {
    let script_hash = script_pubkey.to_usdb_script_hash();
    let actual = db
        .get_balance_at_block_height(&script_hash, height)
        .unwrap();
    assert_eq!(actual.block_height, height, "script_hash={script_hash}");
    assert_eq!(actual.delta, expected_delta, "script_hash={script_hash}");
    assert_eq!(
        actual.balance, expected_balance,
        "script_hash={script_hash}"
    );
}

fn assert_empty_balance(
    db: &balance_history::db::BalanceHistoryDB,
    script_pubkey: &ScriptBuf,
    height: u32,
) {
    let script_hash = script_pubkey.to_usdb_script_hash();
    let actual = db
        .get_balance_at_block_height(&script_hash, height)
        .unwrap();
    assert_eq!(actual.block_height, 0, "script_hash={script_hash}");
    assert_eq!(actual.delta, 0, "script_hash={script_hash}");
    assert_eq!(actual.balance, 0, "script_hash={script_hash}");
}

fn assert_registry(
    db: &balance_history::db::BalanceHistoryDB,
    script_pubkey: &ScriptBuf,
    expected_present: bool,
) {
    let script_hash = script_pubkey.to_usdb_script_hash();
    let actual = db.get_script_registry_entry(&script_hash).unwrap();
    assert_eq!(
        actual.is_some(),
        expected_present,
        "script_hash={script_hash}"
    );
    if expected_present {
        assert_eq!(actual.unwrap(), *script_pubkey);
    }
}
