use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};

use crate::balance::{BalanceMonitor, MockBalanceBackend, MockResponse, SerialBalanceLoader};
use crate::config::{ConfigManager, IndexerConfig};
use crate::index::content::{MinerPassKind, MinerPassState};
use crate::index::energy::PassEnergyManager;
use crate::index::energy_formula::{Energy, calc_balance_penalty_energy, calc_growth_delta};
use crate::index::pass::MinerPassManager;
use crate::index::transfer::TransferTrackSeed;
use crate::index::{
    BalanceHistoryCommitApi, BlockHintProvider, IndexStatusApi, InscriptionIndexer,
    PassBlockCommitEntry, TransferTrackerApi,
};
use crate::inscription::{DiscoveredInscription, InscriptionSource, InscriptionTransferItem};
use crate::storage::{
    MinerPassInfo, MinerPassStorage, MinerPassStorageRef, PassEnergyRecord, PassEnergyStorage,
};
use balance_history::SnapshotInfo as BalanceHistorySnapshotInfo;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{
    Amount, Block, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute, constants, transaction,
};
use ord::InscriptionId;
use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use usdb_util::BtcScriptHash;

#[path = "../../../../../../tests/assumeutxo_stale_anchor.rs"]
mod assumeutxo_stale_anchor;

type StatusUpdateRecord = (Option<u32>, Option<u32>, Option<String>);

fn expected_balance_penalty(
    balance_before: u64,
    balance_after: u64,
    active_block_height: u32,
    event_block_height: u32,
) -> Energy {
    calc_balance_penalty_energy(
        balance_before,
        balance_after,
        active_block_height,
        event_block_height,
    )
}

fn regtest_stable_lag() -> u32 {
    usdb_util::embedded_btc_stable_lag_blocks(Network::Regtest).unwrap()
}

#[derive(Default)]
struct MockBlockHintProvider {
    blocks_by_height: HashMap<u32, Arc<Block>>,
}

impl MockBlockHintProvider {
    fn with_block(mut self, block_height: u32, block: Arc<Block>) -> Self {
        self.blocks_by_height.insert(block_height, block);
        self
    }
}

impl BlockHintProvider for MockBlockHintProvider {
    fn load_block_hint(&self, block_height: u32) -> Result<Option<Arc<Block>>, String> {
        Ok(self.blocks_by_height.get(&block_height).cloned())
    }
}

#[derive(Default)]
struct MockStatus {
    latest_height: AtomicU32,
    snapshot: Mutex<Option<BalanceHistorySnapshotInfo>>,
    updates: Mutex<Vec<StatusUpdateRecord>>,
    upstream_reorg_recovery_pending: AtomicBool,
    block_processing_pending_height: Mutex<Option<u32>>,
}

impl MockStatus {
    fn new(latest_height: u32) -> Self {
        let snapshot = BalanceHistorySnapshotInfo {
            stable_height: latest_height,
            stable_block_hash: Some("11".repeat(32)),
            latest_block_commit: Some("22".repeat(32)),
            balance_query_floor: 0,
            history_query_floor: 0,
            stable_lag: regtest_stable_lag(),
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.to_string(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .to_string(),
            commit_protocol_version: "1.0.0".to_string(),
            commit_hash_algo: "sha256".to_string(),
        };
        Self {
            latest_height: AtomicU32::new(latest_height),
            snapshot: Mutex::new(Some(snapshot)),
            updates: Mutex::new(Vec::new()),
            upstream_reorg_recovery_pending: AtomicBool::new(false),
            block_processing_pending_height: Mutex::new(None),
        }
    }

    fn update_count(&self) -> usize {
        self.updates.lock().unwrap().len()
    }

    fn set_snapshot(&self, snapshot: BalanceHistorySnapshotInfo) {
        self.latest_height
            .store(snapshot.stable_height, Ordering::SeqCst);
        *self.snapshot.lock().unwrap() = Some(snapshot);
    }
}

impl IndexStatusApi for MockStatus {
    fn set_block_processing_pending_height(&self, height: Option<u32>) {
        *self.block_processing_pending_height.lock().unwrap() = height;
    }

    fn balance_history_stable_height(&self) -> Option<u32> {
        Some(self.latest_height.load(Ordering::SeqCst))
    }

    fn balance_history_snapshot(&self) -> Option<BalanceHistorySnapshotInfo> {
        self.snapshot.lock().unwrap().clone()
    }

    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        self.updates.lock().unwrap().push((current, total, message));
    }

    fn set_upstream_reorg_recovery_pending(&self, pending: bool) {
        self.upstream_reorg_recovery_pending
            .store(pending, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct MockBalanceHistoryCommitProvider {
    commits: Mutex<HashMap<u32, Option<balance_history::BlockCommitInfo>>>,
    state_refs: Mutex<HashMap<u32, balance_history::HistoricalSnapshotStateRef>>,
    state_ref_calls: Mutex<Vec<u32>>,
    fail_state_ref_at: AtomicU32,
}

impl MockBalanceHistoryCommitProvider {
    fn default_commit(block_height: u32) -> balance_history::BlockCommitInfo {
        balance_history::BlockCommitInfo {
            block_height,
            btc_block_hash: "11".repeat(32),
            balance_delta_root: "22".repeat(32),
            block_commit: "33".repeat(32),
            commit_protocol_version: "1.0.0".to_string(),
            commit_hash_algo: "sha256".to_string(),
        }
    }

    fn set_block_commit(
        &self,
        block_height: u32,
        commit: Option<balance_history::BlockCommitInfo>,
    ) {
        self.commits.lock().unwrap().insert(block_height, commit);
    }

    fn default_state_ref(block_height: u32) -> balance_history::HistoricalSnapshotStateRef {
        let commit = Self::default_commit(block_height);
        Self::state_ref_from_commit(&commit)
    }

    fn state_ref_from_commit(
        commit: &balance_history::BlockCommitInfo,
    ) -> balance_history::HistoricalSnapshotStateRef {
        let block_height = commit.block_height;
        let stable_block_hash = commit.btc_block_hash.clone();
        let latest_block_commit = commit.block_commit.clone();
        let consensus_identity = usdb_util::ConsensusSnapshotIdentity {
            source_chain: usdb_util::CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
            network: "regtest".to_string(),
            stable_height: block_height,
            stable_block_hash: stable_block_hash.clone(),
            stable_lag: regtest_stable_lag(),
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.to_string(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .to_string(),
        };
        balance_history::HistoricalSnapshotStateRef {
            block_height,
            stable_block_hash,
            latest_block_commit,
            snapshot_id: usdb_util::build_consensus_snapshot_id(&consensus_identity),
            consensus_identity,
            snapshot_id_hash_algo: usdb_util::CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
            snapshot_id_version: usdb_util::CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
            commit_protocol_version: commit.commit_protocol_version.clone(),
            commit_hash_algo: commit.commit_hash_algo.clone(),
        }
    }

    fn set_state_ref(
        &self,
        block_height: u32,
        state_ref: balance_history::HistoricalSnapshotStateRef,
    ) {
        self.state_refs
            .lock()
            .unwrap()
            .insert(block_height, state_ref);
    }
}

impl BalanceHistoryCommitApi for MockBalanceHistoryCommitProvider {
    fn get_block_commit<'a>(
        &'a self,
        block_height: u32,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<balance_history::BlockCommitInfo>, String>>
                + Send
                + 'a,
        >,
    > {
        let commit = self
            .commits
            .lock()
            .unwrap()
            .get(&block_height)
            .cloned()
            .unwrap_or_else(|| Some(Self::default_commit(block_height)));
        Box::pin(async move { Ok(commit) })
    }

    fn get_state_ref_at_height<'a>(
        &'a self,
        block_height: u32,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<balance_history::HistoricalSnapshotStateRef, String>>
                + Send
                + 'a,
        >,
    > {
        self.state_ref_calls.lock().unwrap().push(block_height);
        if self.fail_state_ref_at.load(Ordering::SeqCst) == block_height && block_height != 0 {
            return Box::pin(async move {
                Err(format!(
                    "Injected history RPC failure at height {}",
                    block_height
                ))
            });
        }
        let state_ref = self
            .state_refs
            .lock()
            .unwrap()
            .get(&block_height)
            .cloned()
            .unwrap_or_else(|| {
                let commit = self
                    .commits
                    .lock()
                    .unwrap()
                    .get(&block_height)
                    .cloned()
                    .flatten();
                commit
                    .as_ref()
                    .map(Self::state_ref_from_commit)
                    .unwrap_or_else(|| Self::default_state_ref(block_height))
            });
        Box::pin(async move { Ok(state_ref) })
    }
}

#[path = "../../../../../../tests/indexer_snapshot_anchors.rs"]
mod snapshot_anchor_acceptance;

#[derive(Default)]
struct MockTransferTracker {
    transfers_by_height: HashMap<u32, Vec<InscriptionTransferItem>>,
    added: Mutex<Vec<(InscriptionId, BtcScriptHash, ordinals::SatPoint)>>,
    init_called: AtomicBool,
    staged_blocks: Mutex<HashSet<u32>>,
    commit_calls: AtomicU32,
    rollback_calls: AtomicU32,
    reload_calls: AtomicU32,
    fail_commit_count: AtomicU32,
    fail_reload_count: AtomicU32,
}

impl MockTransferTracker {
    fn with_transfers(mut self, block_height: u32, items: Vec<InscriptionTransferItem>) -> Self {
        self.transfers_by_height.insert(block_height, items);
        self
    }

    fn with_commit_failures(self, count: u32) -> Self {
        self.fail_commit_count.store(count, Ordering::SeqCst);
        self
    }

    fn with_reload_failures(self, count: u32) -> Self {
        self.fail_reload_count.store(count, Ordering::SeqCst);
        self
    }

    fn add_call_count(&self) -> usize {
        self.added.lock().unwrap().len()
    }

    fn commit_call_count(&self) -> u32 {
        self.commit_calls.load(Ordering::SeqCst)
    }

    fn rollback_call_count(&self) -> u32 {
        self.rollback_calls.load(Ordering::SeqCst)
    }

    fn reload_call_count(&self) -> u32 {
        self.reload_calls.load(Ordering::SeqCst)
    }
}

impl TransferTrackerApi for MockTransferTracker {
    fn init<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.init_called.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    fn reload_from_storage<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.reload_calls.fetch_add(1, Ordering::SeqCst);
            let remaining_failures = self.fail_reload_count.load(Ordering::SeqCst);
            if remaining_failures > 0 {
                self.fail_reload_count.fetch_sub(1, Ordering::SeqCst);
                return Err("Injected mock transfer reload failure".to_string());
            }
            self.staged_blocks.lock().unwrap().clear();
            Ok(())
        })
    }

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        satpoint: ordinals::SatPoint,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.added
                .lock()
                .unwrap()
                .push((inscription_id, owner, satpoint));
            Ok(())
        })
    }

    fn process_block_with_hint<'a>(
        &'a self,
        block_height: u32,
        _block_hint: Option<Arc<Block>>,
        _extra_tracked_inscriptions: Vec<TransferTrackSeed>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<InscriptionTransferItem>, String>> + Send + 'a>>
    {
        Box::pin(async move {
            self.staged_blocks.lock().unwrap().insert(block_height);
            Ok(self
                .transfers_by_height
                .get(&block_height)
                .cloned()
                .unwrap_or_default())
        })
    }

    fn commit_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let remaining_failures = self.fail_commit_count.load(Ordering::SeqCst);
            if remaining_failures > 0 {
                self.fail_commit_count.fetch_sub(1, Ordering::SeqCst);
                return Err(format!(
                    "Injected mock transfer commit failure at block {}",
                    block_height
                ));
            }

            let removed = self.staged_blocks.lock().unwrap().remove(&block_height);
            if !removed {
                return Err(format!(
                    "Missing staged block {} when committing mock transfer tracker",
                    block_height
                ));
            }
            self.commit_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn rollback_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let removed = self.staged_blocks.lock().unwrap().remove(&block_height);
            if !removed {
                return Err(format!(
                    "Missing staged block {} when rolling back mock transfer tracker",
                    block_height
                ));
            }
            self.rollback_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[derive(Default)]
struct MockInscriptionSource {}

impl InscriptionSource for MockInscriptionSource {
    fn source_name(&self) -> &'static str {
        "mock"
    }

    fn load_block_inscriptions<'a>(
        &'a self,
        _block_height: u32,
        _block_hint: Option<Arc<Block>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DiscoveredInscription>, String>> + Send + 'a>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

fn make_active_pass(
    inscription_id: InscriptionId,
    owner: BtcScriptHash,
    mint_height: u32,
) -> MinerPassInfo {
    MinerPassInfo {
        inscription_id,
        inscription_number: 1,
        mint_txid: Txid::from_slice(&[7u8; 32]).unwrap(),
        mint_block_height: mint_height,
        mint_owner: owner,
        satpoint: test_satpoint(7, 0, 0),
        mint_version: usdb_util::MINER_PASS_MINT_SCHEMA_VERSION,
        pass_kind: MinerPassKind::Standard,
        usdb_main: "0x1111111111111111111111111111111111111111".to_string(),
        leader_pass_id: None,
        leader_btc_addr: None,
        leader_btc_owner: None,
        prev: Vec::new(),
        invalid_code: None,
        invalid_reason: None,
        owner,
        state: MinerPassState::Active,
    }
}

struct IndexerFixture {
    root_dir: PathBuf,
    storage: MinerPassStorageRef,
    backend: Arc<MockBalanceBackend>,
    pass_energy_manager: Arc<PassEnergyManager>,
    status: Arc<MockStatus>,
    transfer_tracker: Arc<MockTransferTracker>,
    indexer: InscriptionIndexer,
}

#[allow(clippy::too_many_arguments)]
fn build_indexer_fixture_with_runtime_deps_at_root(
    root_dir: PathBuf,
    inscription_source: Arc<dyn InscriptionSource>,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    transfer_tracker: Arc<MockTransferTracker>,
    backend_responses: Vec<MockResponse>,
    energy_provider: Arc<dyn crate::index::energy::BalanceProvider>,
    status: Arc<MockStatus>,
    balance_history_commit_provider: Arc<dyn BalanceHistoryCommitApi>,
) -> IndexerFixture {
    std::fs::create_dir_all(&root_dir).unwrap();
    let config = Arc::new(crate::test_config::load(Some(root_dir.clone())).unwrap());

    let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
    let backend = Arc::new(MockBalanceBackend::new(backend_responses));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let balance_monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let energy_storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
    let pass_energy_manager = Arc::new(
        PassEnergyManager::new_with_deps(config.clone(), energy_storage, energy_provider).unwrap(),
    );
    let miner_pass_manager = Arc::new(
        MinerPassManager::new(config.clone(), storage.clone(), pass_energy_manager.clone())
            .unwrap(),
    );

    let status_api: Arc<dyn IndexStatusApi> = status.clone();
    let transfer_tracker_api: Arc<dyn TransferTrackerApi> = transfer_tracker.clone();

    let indexer = InscriptionIndexer::new_with_deps_for_test(
        config,
        block_hint_provider.clone(),
        inscription_source,
        transfer_tracker_api,
        storage.clone(),
        balance_monitor,
        pass_energy_manager.clone(),
        miner_pass_manager,
        balance_history_commit_provider,
        status_api,
    );

    IndexerFixture {
        root_dir,
        storage,
        backend,
        pass_energy_manager,
        status,
        transfer_tracker,
        indexer,
    }
}

fn build_indexer_fixture_with_hint_provider_at_root(
    root_dir: PathBuf,
    inscription_source: Arc<dyn InscriptionSource>,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    transfer_tracker: Arc<MockTransferTracker>,
    backend_responses: Vec<MockResponse>,
    energy_provider: Arc<dyn crate::index::energy::BalanceProvider>,
) -> IndexerFixture {
    build_indexer_fixture_with_runtime_deps_at_root(
        root_dir,
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        backend_responses,
        energy_provider,
        Arc::new(MockStatus::new(1_000_000)),
        Arc::new(MockBalanceHistoryCommitProvider::default()),
    )
}

fn build_indexer_fixture_with_hint_provider(
    test_name: &str,
    inscription_source: Arc<dyn InscriptionSource>,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    transfer_tracker: Arc<MockTransferTracker>,
    backend_responses: Vec<MockResponse>,
    energy_provider: Arc<dyn crate::index::energy::BalanceProvider>,
) -> IndexerFixture {
    let root_dir = test_root_dir("indexer_behavior", test_name);
    build_indexer_fixture_with_hint_provider_at_root(
        root_dir,
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        backend_responses,
        energy_provider,
    )
}

fn build_indexer_fixture(
    test_name: &str,
    inscription_source: Arc<dyn InscriptionSource>,
    transfer_tracker: Arc<MockTransferTracker>,
    backend_responses: Vec<MockResponse>,
    energy_provider: Arc<dyn crate::index::energy::BalanceProvider>,
) -> IndexerFixture {
    let block_hint_provider: Arc<dyn BlockHintProvider> =
        Arc::new(MockBlockHintProvider::default());
    build_indexer_fixture_with_hint_provider(
        test_name,
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        backend_responses,
        energy_provider,
    )
}

fn build_test_tx(tag: u8) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::new(),
        }],
    }
}

fn build_test_block(txs: Vec<Transaction>) -> Arc<Block> {
    Arc::new(Block {
        header: constants::genesis_block(Network::Bitcoin).header,
        txdata: txs,
    })
}

fn write_test_config(root_dir: &PathBuf, genesis_block_height: u32) {
    std::fs::create_dir_all(root_dir).unwrap();
    let mut config = IndexerConfig::default();
    config.usdb.genesis_block_height = genesis_block_height;
    std::fs::write(
        root_dir.join("config.json"),
        serde_json::to_vec_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn mock_balance_history_commit(
    block_height: u32,
    btc_block_byte: &str,
    delta_root_byte: &str,
    block_commit_byte: &str,
) -> balance_history::BlockCommitInfo {
    balance_history::BlockCommitInfo {
        block_height,
        btc_block_hash: btc_block_byte.repeat(64),
        balance_delta_root: delta_root_byte.repeat(64),
        block_commit: block_commit_byte.repeat(64),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    }
}

fn snapshot_from_commit(commit: &balance_history::BlockCommitInfo) -> BalanceHistorySnapshotInfo {
    BalanceHistorySnapshotInfo {
        stable_height: commit.block_height,
        stable_block_hash: Some(commit.btc_block_hash.clone()),
        latest_block_commit: Some(commit.block_commit.clone()),
        balance_query_floor: 0,
        history_query_floor: 0,
        stable_lag: regtest_stable_lag(),
        balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.to_string(),
        balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
            .to_string(),
        commit_protocol_version: commit.commit_protocol_version.clone(),
        commit_hash_algo: commit.commit_hash_algo.clone(),
    }
}

fn active_owner_set_at_height(
    storage: &MinerPassStorageRef,
    block_height: u32,
) -> HashSet<BtcScriptHash> {
    let mut owners = HashSet::new();
    let mut page = 0usize;
    loop {
        let rows = storage
            .get_all_active_pass_by_page_from_history_at_height(page, 128, block_height)
            .unwrap();
        if rows.is_empty() {
            break;
        }

        for row in rows {
            assert!(
                owners.insert(row.owner),
                "Duplicate active owner found in history snapshot: block_height={}, owner={}",
                block_height,
                row.owner
            );
        }

        page += 1;
    }

    owners
}

#[tokio::test]
async fn test_sync_block_without_events_still_settles_balance_snapshot() {
    let owner = test_script_hash(1);
    let existing_pass_id = test_inscription_id(1, 0);
    let block_height = 120;

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_block_empty_still_settle",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 5_000,
                delta: 0,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default()),
    );

    let existing_pass = make_active_pass(existing_pass_id, owner, 100);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.block_height, block_height);
    assert_eq!(snapshot.active_address_count, 1);
    assert_eq!(snapshot.total_balance, 5_000);
    assert_eq!(fixture.backend.call_count(), 1);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_missing_block_hint_does_not_leak_collector_state() {
    let block_height = 120u32;
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture(
        "sync_block_missing_hint_clears_collector",
        inscription_source,
        transfer_tracker,
        vec![],
        Arc::new(MockBalanceProvider::default()),
    );

    let first_err = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(first_err.contains("Missing required block hint"));
    assert_eq!(
        *fixture
            .status
            .block_processing_pending_height
            .lock()
            .unwrap(),
        Some(block_height)
    );
    assert!(
        !fixture
            .indexer
            .has_active_block_mutation_collection_for_test()
    );
    assert_eq!(
        fixture
            .indexer
            .pass_energy_manager()
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    let second_err = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(second_err.contains("Missing required block hint"));
    assert!(!second_err.contains("collector is already active"));
    assert!(
        !fixture
            .indexer
            .has_active_block_mutation_collection_for_test()
    );
    assert_eq!(
        fixture
            .indexer
            .pass_energy_manager()
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_without_events_updates_energy_on_positive_owner_delta() {
    let owner = test_script_hash(21);
    let pass_id = test_inscription_id(21, 0);
    let base_height = 100u32;
    let block_height = 120u32;

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_block_empty_updates_energy_positive_delta",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 320_000,
                delta: 20_000,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default()),
    );

    let existing_pass = make_active_pass(pass_id, owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id,
            block_height: base_height,
            state: MinerPassState::Active,
            active_block_height: base_height,
            owner_address: owner,
            owner_balance: 300_000,
            owner_delta: 0,
            energy: 0,
        })
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let energy_record = fixture
        .pass_energy_manager
        .get_pass_energy_record_at_or_before(&pass_id, block_height)
        .unwrap()
        .unwrap();
    assert_eq!(energy_record.block_height, block_height);
    assert_eq!(energy_record.owner_balance, 320_000);
    assert_eq!(energy_record.owner_delta, 20_000);
    let expected_at_120 = calc_growth_delta(300_000, block_height - base_height);
    assert_eq!(energy_record.energy, expected_at_120);

    let energy = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, block_height)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy.state, MinerPassState::Active);
    assert_eq!(energy.energy, expected_at_120);

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.active_address_count, 1);
    assert_eq!(snapshot.total_balance, 320_000);
    assert_eq!(fixture.backend.call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_without_events_partial_unit_loss_applies_penalty_and_keeps_active_height()
{
    let owner = test_script_hash(22);
    let pass_id = test_inscription_id(22, 0);
    let base_height = 100u32;
    let block_height = 120u32;

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_block_empty_updates_energy_negative_delta",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 350_000,
                delta: -50_000,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default()),
    );

    let existing_pass = make_active_pass(pass_id, owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id,
            block_height: base_height,
            state: MinerPassState::Active,
            active_block_height: base_height,
            owner_address: owner,
            owner_balance: 400_000,
            owner_delta: 0,
            energy: 10_000,
        })
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let energy_record = fixture
        .pass_energy_manager
        .get_pass_energy_record_at_or_before(&pass_id, block_height)
        .unwrap()
        .unwrap();
    let expected_at_120 = 10_000u128
        .saturating_add(calc_growth_delta(400_000, block_height - base_height))
        .saturating_sub(expected_balance_penalty(
            400_000,
            350_000,
            base_height,
            block_height,
        ));
    assert_eq!(energy_record.block_height, block_height);
    assert_eq!(energy_record.owner_balance, 350_000);
    assert_eq!(energy_record.owner_delta, -50_000);
    assert_eq!(energy_record.active_block_height, base_height);
    assert_eq!(energy_record.energy, expected_at_120);

    let energy_125 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, 125)
        .await
        .unwrap()
        .unwrap();
    let expected_at_125 = expected_at_120.saturating_add(calc_growth_delta(350_000, 5));
    assert_eq!(energy_125.state, MinerPassState::Active);
    assert_eq!(energy_125.energy, expected_at_125);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_energy_numeric_assertions_positive_negative_and_projection() {
    // Numeric-assertion scenario:
    // - h101: positive delta (growth only)
    // - h102: negative delta (growth + penalty, active_block_height remains because units stay positive)
    // - h103: zero delta (no new record, projected energy must follow formula)
    let owner = test_script_hash(23);
    let pass_id = test_inscription_id(23, 0);
    let base_height = 100u32;
    let h101 = 101u32;
    let h102 = 102u32;
    let h103 = 103u32;

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h101, build_test_block(vec![]))
            .with_block(h102, build_test_block(vec![]))
            .with_block(h103, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_energy_numeric_assertions_positive_negative_projection",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h101,
                balance: 320_000,
                delta: 20_000,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h102,
                balance: 270_000,
                delta: -50_000,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h103,
                balance: 270_000,
                delta: 0,
            }]])),
        ],
        Arc::new(MockBalanceProvider::default()),
    );

    let existing_pass = make_active_pass(pass_id, owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id,
            block_height: base_height,
            state: MinerPassState::Active,
            active_block_height: base_height,
            owner_address: owner,
            owner_balance: 300_000,
            owner_delta: 0,
            energy: 100,
        })
        .unwrap();
    fixture
        .storage
        .update_synced_btc_block_height(base_height)
        .unwrap();

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h101..=h103)
        .await
        .unwrap();
    assert_eq!(synced, h103);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 3);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);
    assert_eq!(fixture.backend.call_count(), 3);

    // Positive delta at h101: growth only, keep active_block_height.
    let record_101 = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, h101)
        .unwrap()
        .unwrap();
    let expected_101 = 100u128.saturating_add(calc_growth_delta(300_000, h101 - base_height));
    assert_eq!(record_101.energy, expected_101);
    assert_eq!(record_101.owner_balance, 320_000);
    assert_eq!(record_101.owner_delta, 20_000);
    assert_eq!(record_101.active_block_height, base_height);

    // Negative delta at h102: growth from previous owner_balance then penalty.
    let record_102 = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, h102)
        .unwrap()
        .unwrap();
    let expected_102 = expected_101
        .saturating_add(calc_growth_delta(320_000, h102 - h101))
        .saturating_sub(expected_balance_penalty(
            320_000,
            270_000,
            base_height,
            h102,
        ));
    assert_eq!(record_102.energy, expected_102);
    assert_eq!(record_102.owner_balance, 270_000);
    assert_eq!(record_102.owner_delta, -50_000);
    assert_eq!(record_102.active_block_height, base_height);

    // Zero delta at h103 should not create a new record; get_pass_energy must return projected energy.
    assert!(
        fixture
            .pass_energy_manager
            .get_pass_energy_record_exact(&pass_id, h103)
            .unwrap()
            .is_none()
    );
    let energy_103 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, h103)
        .await
        .unwrap()
        .unwrap();
    let expected_103 = expected_102.saturating_add(calc_growth_delta(270_000, h103 - h102));
    assert_eq!(energy_103.state, MinerPassState::Active);
    assert_eq!(energy_103.energy, expected_103);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_strict_direction_conflict_rolls_back_atomically() {
    let owner = test_script_hash(25);
    let pass_id = test_inscription_id(25, 0);
    let base_height = 200u32;
    let block_height = 220u32;

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    // Direction conflict: delta > 0 but balance goes down from last record.
    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_strict_direction_conflict_rollback",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 350_000,
                delta: 10_000,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default()),
    );

    let existing_pass = make_active_pass(pass_id, owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id,
            block_height: base_height,
            state: MinerPassState::Active,
            active_block_height: base_height,
            owner_address: owner,
            owner_balance: 400_000,
            owner_delta: 0,
            energy: 1_000,
        })
        .unwrap();
    fixture
        .storage
        .update_synced_btc_block_height(base_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .set_force_strict_settle_consistency_for_test(true);

    let err = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(err.contains("inconsistent settle direction"));

    // SQLite state should rollback atomically.
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(base_height)
    );
    let current_pass = fixture
        .storage
        .get_pass_by_inscription_id(&pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(current_pass.state, MinerPassState::Active);
    assert_eq!(current_pass.owner, owner);
    assert_eq!(
        fixture
            .storage
            .get_pass_history_count_in_height_range(&pass_id, block_height, block_height)
            .unwrap(),
        0
    );
    assert!(
        fixture
            .storage
            .get_active_balance_snapshot(block_height)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, block_height),
        HashSet::from([owner])
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 0);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_retry_without_reconcile_recovers_finalized_energy_failure() {
    let block_height = 330u32;
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default().with_commit_failures(1));

    let root = test_root_dir(
        "indexer_behavior",
        "sync_blocks_retry_without_reconcile_finalized_energy_failure",
    );
    write_test_config(&root, block_height);
    let fixture = build_indexer_fixture_with_hint_provider_at_root(
        root,
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![])),
            MockResponse::Immediate(Ok(vec![])),
        ],
        Arc::new(MockBalanceProvider::default()),
    );

    let first_err = fixture
        .indexer
        .sync_blocks_without_reconcile_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(first_err.contains("Injected mock transfer commit failure"));
    assert_eq!(fixture.storage.get_synced_btc_block_height().unwrap(), None);
    assert_eq!(
        fixture
            .pass_energy_manager
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );
    assert_eq!(
        fixture
            .pass_energy_manager
            .get_synced_block_height_for_test()
            .unwrap(),
        Some(block_height - 1)
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 0);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    let second_ok = fixture
        .indexer
        .sync_blocks_without_reconcile_for_test(block_height..=block_height)
        .await
        .unwrap();
    assert_eq!(second_ok, block_height);
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(block_height)
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_same_owner_transfer_block_end_settlement_is_idempotent() {
    let base_height = 500u32;
    let block_height = 515u32;
    let owner = test_script_hash(34);
    let pass_id = test_inscription_id(35, 0);

    let transfer_tx = build_test_tx(46);
    let transfer_txid = transfer_tx.compute_txid();
    let transfer_prev_outpoint = transfer_tx.input[0].previous_output;
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(block_height, build_test_block(vec![transfer_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());

    let new_satpoint = ordinals::SatPoint {
        outpoint: OutPoint {
            txid: transfer_txid,
            vout: 0,
        },
        offset: 3,
    };
    let transfer_item = InscriptionTransferItem {
        inscription_id: pass_id,
        block_height,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_prev_outpoint,
            offset: 0,
        },
        satpoint: new_satpoint,
        from_address: owner,
        to_address: Some(owner),
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_transfers(block_height, vec![transfer_item]));

    let energy_provider = Arc::new(MockBalanceProvider::default().with_range(
        owner,
        (base_height + 1)..(block_height + 1),
        vec![balance_history::AddressBalance {
            block_height,
            balance: 280_000,
            delta: -500,
        }],
    ));
    let fixture = build_indexer_fixture_with_hint_provider(
        "same_owner_transfer_block_end_settlement_idempotent",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 280_000,
                delta: -500,
            },
        ]]))],
        energy_provider,
    );

    let existing_pass = make_active_pass(pass_id, owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id,
            block_height: base_height,
            state: MinerPassState::Active,
            active_block_height: base_height,
            owner_address: owner,
            owner_balance: 300_000,
            owner_delta: 0,
            energy: 10_000,
        })
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let pass = fixture
        .storage
        .get_pass_by_inscription_id(&pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass.owner, owner);
    assert_eq!(pass.state, MinerPassState::Active);
    assert_eq!(pass.satpoint, new_satpoint);

    let energy_record = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, block_height)
        .unwrap()
        .unwrap();
    let expected_energy = 10_000u128
        .saturating_add(calc_growth_delta(300_000, block_height - base_height))
        .saturating_sub(expected_balance_penalty(
            300_000,
            280_000,
            base_height,
            block_height,
        ));
    assert_eq!(energy_record.state, MinerPassState::Active);
    assert_eq!(energy_record.active_block_height, base_height);
    assert_eq!(energy_record.owner_balance, 280_000);
    assert_eq!(energy_record.owner_delta, -500);
    assert_eq!(energy_record.energy, expected_energy);
    assert_eq!(
        fixture
            .pass_energy_manager
            .count_pass_energy_records_in_height_range(&pass_id, block_height, block_height)
            .unwrap(),
        1
    );

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.active_address_count, 1);
    assert_eq!(snapshot.total_balance, 280_000);
    assert_eq!(fixture.backend.call_count(), 1);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_once_rejects_upstream_stable_lag_mismatch_before_persistence() {
    let root_dir = test_root_dir("indexer_behavior", "sync_once_stable_lag_mismatch");
    write_test_config(&root_dir, 100);

    let status = Arc::new(MockStatus::new(100));
    let upstream_commit = mock_balance_history_commit(100, "a", "b", "c");
    let mut mismatched_snapshot = snapshot_from_commit(&upstream_commit);
    mismatched_snapshot.stable_lag = regtest_stable_lag() + 1;
    status.set_snapshot(mismatched_snapshot);

    let fixture = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default()),
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status,
        Arc::new(MockBalanceHistoryCommitProvider::default()),
    );

    let err = fixture.indexer.sync_once_for_test().await.unwrap_err();

    assert!(err.contains("stable lag does not match"));
    assert!(err.contains("source=current snapshot"));
    assert!(err.contains(&format!("upstream_stable_lag={}", regtest_stable_lag() + 1)));
    assert!(err.contains(&format!("expected_stable_lag={}", regtest_stable_lag())));
    assert!(
        fixture
            .storage
            .get_balance_history_snapshot_anchor()
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.storage.get_synced_btc_block_height().unwrap(), None);
    assert_eq!(fixture.status.update_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_once_rolls_back_when_upstream_height_regresses() {
    let root_dir = test_root_dir("indexer_behavior", "sync_once_height_regresses");
    write_test_config(&root_dir, 100);

    let status = Arc::new(MockStatus::new(100));
    let commit_provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let upstream_commit_100 = mock_balance_history_commit(100, "a", "b", "c");
    status.set_snapshot(snapshot_from_commit(&upstream_commit_100));
    commit_provider.set_block_commit(100, Some(upstream_commit_100.clone()));

    let fixture = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default()),
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status,
        commit_provider,
    );

    let old_upstream_commit_105 = mock_balance_history_commit(105, "d", "e", "f");
    fixture
        .storage
        .upsert_active_balance_snapshot(100, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(105, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 100,
            balance_history_block_height: 100,
            balance_history_block_commit: upstream_commit_100.block_commit.clone(),
            mutation_root: "1".repeat(64),
            block_commit: "2".repeat(64),
            commit_protocol_version: upstream_commit_100.commit_protocol_version.clone(),
            commit_hash_algo: upstream_commit_100.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 105,
            balance_history_block_height: 105,
            balance_history_block_commit: old_upstream_commit_105.block_commit.clone(),
            mutation_root: "3".repeat(64),
            block_commit: "4".repeat(64),
            commit_protocol_version: old_upstream_commit_105.commit_protocol_version.clone(),
            commit_hash_algo: old_upstream_commit_105.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(&old_upstream_commit_105))
        .unwrap();

    // Seed a coherent pair; a committed pass tip always has finalized energy metadata.
    fixture
        .pass_energy_manager
        .set_synced_block_height_for_test(105)
        .unwrap();

    let synced = fixture.indexer.sync_once_for_test().await.unwrap();

    assert_eq!(synced, 100);
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(100)
    );
    assert!(
        fixture
            .storage
            .get_pass_block_commit(105)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .storage
            .get_active_balance_snapshot(105)
            .unwrap()
            .is_none()
    );
    let anchor = fixture
        .storage
        .get_balance_history_snapshot_anchor()
        .unwrap()
        .unwrap();
    assert_eq!(anchor.stable_height, 100);
    assert_eq!(anchor.latest_block_commit, upstream_commit_100.block_commit);
    assert_eq!(fixture.transfer_tracker.reload_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_once_rolls_back_and_replays_same_height_reorg() {
    let root_dir = test_root_dir("indexer_behavior", "sync_once_same_height_reorg");
    write_test_config(&root_dir, 100);

    let reorg_height = 101u32;
    let common_commit_100 = mock_balance_history_commit(100, "a", "b", "c");
    let old_commit_101 = mock_balance_history_commit(101, "d", "e", "f");
    let new_commit_101 = mock_balance_history_commit(101, "1", "2", "3");

    let status = Arc::new(MockStatus::new(reorg_height));
    status.set_snapshot(snapshot_from_commit(&new_commit_101));
    let commit_provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    commit_provider.set_block_commit(100, Some(common_commit_100.clone()));
    commit_provider.set_block_commit(101, Some(new_commit_101.clone()));

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(reorg_height, build_test_block(vec![])),
    );
    let fixture = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        block_hint_provider,
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status,
        commit_provider,
    );

    fixture
        .storage
        .upsert_active_balance_snapshot(100, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(reorg_height, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 100,
            balance_history_block_height: 100,
            balance_history_block_commit: common_commit_100.block_commit.clone(),
            mutation_root: "4".repeat(64),
            block_commit: "5".repeat(64),
            commit_protocol_version: common_commit_100.commit_protocol_version.clone(),
            commit_hash_algo: common_commit_100.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: reorg_height,
            balance_history_block_height: reorg_height,
            balance_history_block_commit: old_commit_101.block_commit.clone(),
            mutation_root: "6".repeat(64),
            block_commit: "7".repeat(64),
            commit_protocol_version: old_commit_101.commit_protocol_version.clone(),
            commit_hash_algo: old_commit_101.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(&old_commit_101))
        .unwrap();

    // Seed a coherent pair; a committed pass tip always has finalized energy metadata.
    fixture
        .pass_energy_manager
        .set_synced_block_height_for_test(reorg_height)
        .unwrap();

    let synced = fixture.indexer.sync_once_for_test().await.unwrap();

    assert_eq!(synced, reorg_height);
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(reorg_height)
    );
    let replayed_commit = fixture
        .storage
        .get_pass_block_commit(reorg_height)
        .unwrap()
        .unwrap();
    assert_eq!(
        replayed_commit.balance_history_block_commit,
        new_commit_101.block_commit
    );
    let anchor = fixture
        .storage
        .get_balance_history_snapshot_anchor()
        .unwrap()
        .unwrap();
    assert_eq!(anchor.stable_height, reorg_height);
    assert_eq!(anchor.latest_block_commit, new_commit_101.block_commit);
    assert!(
        fixture
            .storage
            .get_active_balance_snapshot(reorg_height)
            .unwrap()
            .is_some()
    );
    assert_eq!(fixture.transfer_tracker.reload_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_once_retries_pending_reorg_recovery_after_energy_failure() {
    let root_dir = test_root_dir("indexer_behavior", "sync_once_retry_pending_energy_failure");
    write_test_config(&root_dir, 100);

    let status = Arc::new(MockStatus::new(100));
    let commit_provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let upstream_commit_100 = mock_balance_history_commit(100, "a", "b", "c");
    status.set_snapshot(snapshot_from_commit(&upstream_commit_100));
    commit_provider.set_block_commit(100, Some(upstream_commit_100.clone()));

    let fixture = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default()),
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status,
        commit_provider,
    );

    let old_upstream_commit_105 = mock_balance_history_commit(105, "d", "e", "f");
    fixture
        .storage
        .upsert_active_balance_snapshot(100, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(105, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 100,
            balance_history_block_height: 100,
            balance_history_block_commit: upstream_commit_100.block_commit.clone(),
            mutation_root: "1".repeat(64),
            block_commit: "2".repeat(64),
            commit_protocol_version: upstream_commit_100.commit_protocol_version.clone(),
            commit_hash_algo: upstream_commit_100.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 105,
            balance_history_block_height: 105,
            balance_history_block_commit: old_upstream_commit_105.block_commit.clone(),
            mutation_root: "3".repeat(64),
            block_commit: "4".repeat(64),
            commit_protocol_version: old_upstream_commit_105.commit_protocol_version.clone(),
            commit_hash_algo: old_upstream_commit_105.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(&old_upstream_commit_105))
        .unwrap();
    fixture
        .pass_energy_manager
        .set_synced_block_height_for_test(90)
        .unwrap();

    let first_err = fixture.indexer.sync_once_for_test().await.unwrap_err();
    assert!(first_err.contains("pending upstream reorg recovery"));
    assert_eq!(
        fixture
            .storage
            .get_upstream_reorg_recovery_pending_height()
            .unwrap(),
        Some(100)
    );
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(100)
    );

    fixture
        .pass_energy_manager
        .set_synced_block_height_for_test(100)
        .unwrap();

    let synced = fixture.indexer.sync_once_for_test().await.unwrap();
    assert_eq!(synced, 100);
    assert_eq!(
        fixture
            .storage
            .get_upstream_reorg_recovery_pending_height()
            .unwrap(),
        None
    );
    assert_eq!(
        fixture
            .pass_energy_manager
            .get_synced_block_height_for_test()
            .unwrap(),
        Some(100)
    );
    assert_eq!(fixture.transfer_tracker.reload_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_once_resumes_pending_reorg_recovery_after_restart() {
    let root_dir = test_root_dir("indexer_behavior", "sync_once_resume_pending_after_restart");
    write_test_config(&root_dir, 100);

    let upstream_commit_100 = mock_balance_history_commit(100, "a", "b", "c");
    let old_upstream_commit_105 = mock_balance_history_commit(105, "d", "e", "f");

    let status1 = Arc::new(MockStatus::new(100));
    status1.set_snapshot(snapshot_from_commit(&upstream_commit_100));
    let commit_provider1 = Arc::new(MockBalanceHistoryCommitProvider::default());
    commit_provider1.set_block_commit(100, Some(upstream_commit_100.clone()));
    let fixture1 = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default()),
        Arc::new(MockTransferTracker::default().with_reload_failures(1)),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status1,
        commit_provider1,
    );

    fixture1
        .storage
        .upsert_active_balance_snapshot(100, 0, 0)
        .unwrap();
    fixture1
        .storage
        .upsert_active_balance_snapshot(105, 0, 0)
        .unwrap();
    fixture1
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 100,
            balance_history_block_height: 100,
            balance_history_block_commit: upstream_commit_100.block_commit.clone(),
            mutation_root: "1".repeat(64),
            block_commit: "2".repeat(64),
            commit_protocol_version: upstream_commit_100.commit_protocol_version.clone(),
            commit_hash_algo: upstream_commit_100.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture1
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 105,
            balance_history_block_height: 105,
            balance_history_block_commit: old_upstream_commit_105.block_commit.clone(),
            mutation_root: "3".repeat(64),
            block_commit: "4".repeat(64),
            commit_protocol_version: old_upstream_commit_105.commit_protocol_version.clone(),
            commit_hash_algo: old_upstream_commit_105.commit_hash_algo.clone(),
        })
        .unwrap();
    fixture1
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(&old_upstream_commit_105))
        .unwrap();

    // Seed a coherent pair; a committed pass tip always has finalized energy metadata.
    fixture1
        .pass_energy_manager
        .set_synced_block_height_for_test(105)
        .unwrap();

    let first_err = fixture1.indexer.sync_once_for_test().await.unwrap_err();
    assert!(first_err.contains("Injected mock transfer reload failure"));
    assert_eq!(
        fixture1
            .storage
            .get_upstream_reorg_recovery_pending_height()
            .unwrap(),
        Some(100)
    );

    drop(fixture1);

    let status2 = Arc::new(MockStatus::new(100));
    status2.set_snapshot(snapshot_from_commit(&upstream_commit_100));
    let commit_provider2 = Arc::new(MockBalanceHistoryCommitProvider::default());
    commit_provider2.set_block_commit(100, Some(upstream_commit_100.clone()));
    let fixture2 = build_indexer_fixture_with_runtime_deps_at_root(
        root_dir.clone(),
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default()),
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status2,
        commit_provider2,
    );

    let synced = fixture2.indexer.sync_once_for_test().await.unwrap();
    assert_eq!(synced, 100);
    assert_eq!(
        fixture2
            .storage
            .get_upstream_reorg_recovery_pending_height()
            .unwrap(),
        None
    );
    assert_eq!(
        fixture2.storage.get_synced_btc_block_height().unwrap(),
        Some(100)
    );
    assert_eq!(fixture2.transfer_tracker.reload_call_count(), 1);

    cleanup_temp_dir(&fixture2.root_dir);
}

#[path = "../../../../../../tests/miner_pass_block_recovery.rs"]
mod miner_pass_block_recovery;
