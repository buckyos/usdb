use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};
use crate::balance::{BalanceMonitor, MockBalanceBackend, MockResponse, SerialBalanceLoader};
use crate::config::{ConfigManager, IndexerConfig};
use crate::index::MintValidationErrorCode;
use crate::index::content::{MinerPassState, USDBInscription, USDBMint};
use crate::index::energy::PassEnergyManager;
use crate::index::energy_formula::{calc_growth_delta, calc_penalty_from_delta};
use crate::index::pass::MinerPassManager;
use crate::index::transfer::{InscriptionCreateInfo, TransferTrackSeed};
use crate::index::{
    BalanceHistoryCommitApi, BlockHintProvider, IndexStatusApi, InscriptionIndexer,
    PassBlockCommitEntry, TransferTrackerApi,
};
use crate::inscription::{
    DiscoveredInscription, DiscoveredInvalidMint, DiscoveredMint, DiscoveredMintBatch,
    InscriptionSource, InscriptionTransferItem,
};
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
use usdb_util::USDBScriptHash;

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
    updates: Mutex<Vec<(Option<u32>, Option<u32>, Option<String>)>>,
    upstream_reorg_recovery_pending: AtomicBool,
}

impl MockStatus {
    fn new(latest_height: u32) -> Self {
        let snapshot = BalanceHistorySnapshotInfo {
            stable_height: latest_height,
            stable_block_hash: Some("11".repeat(32)),
            latest_block_commit: Some("22".repeat(32)),
            stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
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
}

#[derive(Clone)]
struct MockCreateInfo {
    satpoint: ordinals::SatPoint,
    value: Amount,
    address: Option<USDBScriptHash>,
    commit_txid: Txid,
    commit_outpoint: OutPoint,
}

#[derive(Default)]
struct MockTransferTracker {
    create_infos: HashMap<String, MockCreateInfo>,
    transfers_by_height: HashMap<u32, Vec<InscriptionTransferItem>>,
    added: Mutex<Vec<(InscriptionId, USDBScriptHash, ordinals::SatPoint)>>,
    init_called: AtomicBool,
    staged_blocks: Mutex<HashSet<u32>>,
    commit_calls: AtomicU32,
    rollback_calls: AtomicU32,
    reload_calls: AtomicU32,
    fail_commit_count: AtomicU32,
    fail_reload_count: AtomicU32,
}

impl MockTransferTracker {
    fn with_create_info(mut self, inscription_id: &InscriptionId, info: MockCreateInfo) -> Self {
        self.create_infos.insert(inscription_id.to_string(), info);
        self
    }

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

    fn calc_create_satpoint<'a>(
        &'a self,
        inscription_id: &'a InscriptionId,
    ) -> Pin<Box<dyn Future<Output = Result<InscriptionCreateInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            let info = self
                .create_infos
                .get(&inscription_id.to_string())
                .ok_or_else(|| {
                    format!(
                        "Missing mock create satpoint info for inscription {}",
                        inscription_id
                    )
                })?;

            Ok(InscriptionCreateInfo {
                satpoint: info.satpoint,
                value: info.value,
                address: info.address,
                commit_txid: info.commit_txid,
                commit_outpoint: info.commit_outpoint,
            })
        })
    }

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
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
struct MockInscriptionSource {
    mints_by_height: HashMap<u32, Vec<DiscoveredMint>>,
    invalid_mints_by_height: HashMap<u32, Vec<DiscoveredInvalidMint>>,
}

impl MockInscriptionSource {
    fn with_mints(mut self, block_height: u32, mints: Vec<DiscoveredMint>) -> Self {
        self.mints_by_height.insert(block_height, mints);
        self
    }

    fn with_invalid_mints(
        mut self,
        block_height: u32,
        invalid_mints: Vec<DiscoveredInvalidMint>,
    ) -> Self {
        self.invalid_mints_by_height
            .insert(block_height, invalid_mints);
        self
    }
}

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

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        _block_hint: Option<Arc<Block>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DiscoveredMint>, String>> + Send + 'a>> {
        Box::pin(async move {
            Ok(self
                .mints_by_height
                .get(&block_height)
                .cloned()
                .unwrap_or_default())
        })
    }

    fn load_block_mint_batch<'a>(
        &'a self,
        block_height: u32,
        _block_hint: Option<Arc<Block>>,
    ) -> Pin<Box<dyn Future<Output = Result<DiscoveredMintBatch, String>> + Send + 'a>> {
        Box::pin(async move {
            Ok(DiscoveredMintBatch {
                valid_mints: self
                    .mints_by_height
                    .get(&block_height)
                    .cloned()
                    .unwrap_or_default(),
                invalid_mints: self
                    .invalid_mints_by_height
                    .get(&block_height)
                    .cloned()
                    .unwrap_or_default(),
            })
        })
    }
}

fn make_active_pass(
    inscription_id: InscriptionId,
    owner: USDBScriptHash,
    mint_height: u32,
) -> MinerPassInfo {
    MinerPassInfo {
        inscription_id,
        inscription_number: 1,
        mint_txid: Txid::from_slice(&[7u8; 32]).unwrap(),
        mint_block_height: mint_height,
        mint_owner: owner,
        satpoint: test_satpoint(7, 0, 0),
        eth_main: "0x1111111111111111111111111111111111111111".to_string(),
        eth_collab: None,
        prev: Vec::new(),
        invalid_code: None,
        invalid_reason: None,
        owner,
        state: MinerPassState::Active,
    }
}

fn make_discovered_mint(
    inscription_id: InscriptionId,
    block_height: u32,
    prev: Vec<InscriptionId>,
) -> DiscoveredMint {
    let prev_strings = prev.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let content = USDBInscription::Mint(USDBMint {
        eth_main: "0x1111111111111111111111111111111111111111".to_string(),
        eth_collab: None,
        prev: prev_strings,
    });

    DiscoveredMint {
        inscription_id,
        inscription_number: 1,
        block_height,
        timestamp: 0,
        satpoint: Some(test_satpoint(8, 0, 0)),
        content_string: "{\"p\":\"usdb\",\"op\":\"mint\",\"eth_main\":\"0x1111111111111111111111111111111111111111\",\"prev\":[]}".to_string(),
        content,
    }
}

fn make_discovered_invalid_mint(
    inscription_id: InscriptionId,
    block_height: u32,
    content_string: &str,
    error_code: MintValidationErrorCode,
    error_reason: &str,
) -> DiscoveredInvalidMint {
    DiscoveredInvalidMint {
        inscription_id,
        inscription_number: 1,
        block_height,
        timestamp: 0,
        satpoint: Some(test_satpoint(18, 0, 0)),
        content_string: content_string.to_string(),
        error_code,
        error_reason: error_reason.to_string(),
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
    let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());

    let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
    let backend = Arc::new(MockBalanceBackend::new(backend_responses));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let balance_monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let energy_storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
    let pass_energy_manager = Arc::new(PassEnergyManager::new_with_deps(
        config.clone(),
        energy_storage,
        energy_provider,
    ));
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
        stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
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
) -> HashSet<USDBScriptHash> {
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
    assert!(
        !fixture
            .indexer
            .has_active_block_mutation_collection_for_test()
    );

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

    let existing_pass = make_active_pass(pass_id.clone(), owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id.clone(),
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
async fn test_sync_block_without_events_negative_delta_applies_penalty_and_resets_active_height() {
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

    let existing_pass = make_active_pass(pass_id.clone(), owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id.clone(),
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
    let expected_at_120 = 10_000u64
        .saturating_add(calc_growth_delta(400_000, block_height - base_height))
        .saturating_sub(calc_penalty_from_delta(-50_000));
    assert_eq!(energy_record.block_height, block_height);
    assert_eq!(energy_record.owner_balance, 350_000);
    assert_eq!(energy_record.owner_delta, -50_000);
    assert_eq!(energy_record.active_block_height, block_height);
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
    // - h102: negative delta (growth + penalty, active_block_height reset)
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

    let existing_pass = make_active_pass(pass_id.clone(), owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id.clone(),
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
    let expected_101 = 100u64.saturating_add(calc_growth_delta(300_000, h101 - base_height));
    assert_eq!(record_101.energy, expected_101);
    assert_eq!(record_101.owner_balance, 320_000);
    assert_eq!(record_101.owner_delta, 20_000);
    assert_eq!(record_101.active_block_height, base_height);

    // Negative delta at h102: growth from previous owner_balance then penalty, and reset active height.
    let record_102 = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, h102)
        .unwrap()
        .unwrap();
    let expected_102 = expected_101
        .saturating_add(calc_growth_delta(320_000, h102 - base_height))
        .saturating_sub(calc_penalty_from_delta(-50_000));
    assert_eq!(record_102.energy, expected_102);
    assert_eq!(record_102.owner_balance, 270_000);
    assert_eq!(record_102.owner_delta, -50_000);
    assert_eq!(record_102.active_block_height, h102);

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
async fn test_sync_blocks_energy_projects_growth_without_intermediate_records() {
    // Mint once, then keep owner balance unchanged for multiple blocks:
    // no extra energy records should be written, but query-time projection must still grow.
    let owner = test_script_hash(26);
    let mint_height = 300u32;
    let query_height = 305u32;
    let mint_tx = build_test_tx(62);
    let pass_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(mint_height, build_test_block(vec![mint_tx]))
            .with_block(mint_height + 1, build_test_block(vec![]))
            .with_block(mint_height + 2, build_test_block(vec![]))
            .with_block(mint_height + 3, build_test_block(vec![]))
            .with_block(mint_height + 4, build_test_block(vec![]))
            .with_block(mint_height + 5, build_test_block(vec![])),
    );
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(
            mint_height,
            vec![make_discovered_mint(pass_id.clone(), mint_height, vec![])],
        ));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(26, 0, 0),
        value: Amount::from_sat(12_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[26u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[26u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&pass_id, create_info));

    let backend_responses = vec![
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height,
            balance: 250_000,
            delta: 0,
        }]])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height + 1,
            balance: 250_000,
            delta: 0,
        }]])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height + 2,
            balance: 250_000,
            delta: 0,
        }]])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height + 3,
            balance: 250_000,
            delta: 0,
        }]])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height + 4,
            balance: 250_000,
            delta: 0,
        }]])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: mint_height + 5,
            balance: 250_000,
            delta: 0,
        }]])),
    ];

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_energy_projection_without_records",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        backend_responses,
        Arc::new(MockBalanceProvider::default().with_height(owner, mint_height, 250_000, 0)),
    );
    fixture
        .storage
        .update_synced_btc_block_height(mint_height - 1)
        .unwrap();

    let synced = fixture
        .indexer
        .sync_blocks_for_test(mint_height..=query_height)
        .await
        .unwrap();
    assert_eq!(synced, query_height);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 6);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // Mint height has the baseline record written by on_new_pass.
    let mint_record = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, mint_height)
        .unwrap()
        .unwrap();
    assert_eq!(mint_record.state, MinerPassState::Active);
    assert_eq!(mint_record.owner_balance, 250_000);
    assert_eq!(mint_record.energy, 0);

    // No subsequent record should be written when all settle deltas are zero.
    for h in (mint_height + 1)..=query_height {
        assert!(
            fixture
                .pass_energy_manager
                .get_pass_energy_record_exact(&pass_id, h)
                .unwrap()
                .is_none(),
            "unexpected energy record found at height {}",
            h
        );
    }

    // Query-time energy must still project growth from mint height to query height.
    let energy = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, query_height)
        .await
        .unwrap()
        .unwrap();
    let expected = calc_growth_delta(250_000, query_height - mint_height);
    assert_eq!(energy.state, MinerPassState::Active);
    assert_eq!(energy.energy, expected);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_energy_projects_growth_over_long_window_without_new_records() {
    // Long-window projection regression:
    // mint once at h400, then settle through h460 with unchanged balance and stale at-or-before height.
    // No new energy records should be written after mint, but query-time projection must remain exact.
    let owner = test_script_hash(27);
    let mint_height = 400u32;
    let query_height = 460u32;
    let mint_tx = build_test_tx(63);
    let pass_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };

    let mut block_provider =
        MockBlockHintProvider::default().with_block(mint_height, build_test_block(vec![mint_tx]));
    for h in (mint_height + 1)..=query_height {
        block_provider = block_provider.with_block(h, build_test_block(vec![]));
    }
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(block_provider);

    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(
            mint_height,
            vec![make_discovered_mint(pass_id.clone(), mint_height, vec![])],
        ));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(27, 0, 0),
        value: Amount::from_sat(15_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[27u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[27u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&pass_id, create_info));

    let mut backend_responses = Vec::new();
    for h in mint_height..=query_height {
        let effective_balance_height = if h == mint_height { h } else { mint_height };
        backend_responses.push(MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height: effective_balance_height,
                balance: 260_000,
                delta: 0,
            },
        ]])));
    }

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_energy_projection_long_window_without_records",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        backend_responses,
        Arc::new(MockBalanceProvider::default().with_height(owner, mint_height, 260_000, 0)),
    );
    fixture
        .storage
        .update_synced_btc_block_height(mint_height - 1)
        .unwrap();

    let synced = fixture
        .indexer
        .sync_blocks_for_test(mint_height..=query_height)
        .await
        .unwrap();
    assert_eq!(synced, query_height);
    assert_eq!(
        fixture.backend.call_count(),
        (query_height - mint_height + 1) as usize
    );
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // Mint record should exist.
    let mint_record = fixture
        .pass_energy_manager
        .get_pass_energy_record_exact(&pass_id, mint_height)
        .unwrap()
        .unwrap();
    assert_eq!(mint_record.state, MinerPassState::Active);
    assert_eq!(mint_record.owner_balance, 260_000);
    assert_eq!(mint_record.energy, 0);

    // No incremental record should be written for zero-delta windows.
    for h in (mint_height + 1)..=query_height {
        assert!(
            fixture
                .pass_energy_manager
                .get_pass_energy_record_exact(&pass_id, h)
                .unwrap()
                .is_none(),
            "unexpected energy record found at height {}",
            h
        );
    }

    for h in [410u32, 430u32, query_height] {
        let snapshot = fixture
            .pass_energy_manager
            .get_pass_energy(&pass_id, h)
            .await
            .unwrap()
            .unwrap();
        let expected = calc_growth_delta(260_000, h - mint_height);
        assert_eq!(snapshot.state, MinerPassState::Active);
        assert_eq!(snapshot.energy, expected);
    }

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_settle_failure_rolls_back_and_synced_height_not_advance() {
    let owner = test_script_hash(2);
    let block_height = 200;
    let mint_tx = build_test_tx(52);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![mint_tx])),
    );

    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(9, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[9u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[9u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_settle_fail_rollback",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![]))],
        Arc::new(MockBalanceProvider::default().with_height(owner, block_height, 200_000, 100)),
    );

    let err = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(err.contains("Address balance batch size mismatch"));

    let synced = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_ne!(synced, Some(200));

    let pass = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap();
    assert!(pass.is_none());

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap();
    assert!(snapshot.is_none());
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 0);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_strict_same_height_conflict_rolls_back_atomically() {
    let owner = test_script_hash(24);
    let block_height = 210u32;
    let mint_tx = build_test_tx(61);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![mint_tx])),
    );
    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(24, 0, 0),
        value: Amount::from_sat(20_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[24u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[24u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    // on_new_pass writes same-height record from energy provider;
    // settle stage provides conflicting same-height balance/delta.
    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_strict_same_height_conflict_rollback",
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
        Arc::new(MockBalanceProvider::default().with_height(owner, block_height, 300_000, 10_000)),
    );
    fixture
        .storage
        .update_synced_btc_block_height(block_height - 1)
        .unwrap();
    fixture
        .pass_energy_manager
        .set_force_strict_settle_consistency_for_test(true);

    let err = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(err.contains("conflicts with existing record at same block"));

    // SQLite state should rollback atomically.
    assert_eq!(
        fixture.storage.get_synced_btc_block_height().unwrap(),
        Some(block_height - 1)
    );
    assert!(
        fixture
            .storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .storage
            .get_pass_history_count_in_height_range(&mint_id, block_height, block_height)
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
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 0);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

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

    let existing_pass = make_active_pass(pass_id.clone(), owner, base_height);
    fixture
        .storage
        .add_new_mint_pass_at_height(&existing_pass, existing_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .insert_pass_energy_record_for_test(&PassEnergyRecord {
            inscription_id: pass_id.clone(),
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
async fn test_sync_blocks_partial_commit_then_retry_produces_consistent_result() {
    let owner = test_script_hash(3);
    let commit_block_height = 300;
    let mint_block_height = 301;
    let mint_tx = build_test_tx(53);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(commit_block_height, build_test_block(vec![]))
            .with_block(mint_block_height, build_test_block(vec![mint_tx])),
    );

    let mint = make_discovered_mint(mint_id.clone(), mint_block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(mint_block_height, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(10, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[10u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[10u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_partial_commit_then_retry",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: mint_block_height,
                balance: 7_777,
                delta: 1,
            }]])),
        ],
        Arc::new(MockBalanceProvider::default().with_height(
            owner,
            mint_block_height,
            200_000,
            100,
        )),
    );

    let first_err = fixture
        .indexer
        .sync_blocks_for_test(commit_block_height..=mint_block_height)
        .await
        .unwrap_err();
    assert!(first_err.contains("Address balance batch size mismatch"));

    let synced_after_first = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_after_first, Some(commit_block_height));

    let snapshot_300 = fixture
        .storage
        .get_active_balance_snapshot(commit_block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_300.total_balance, 0);
    assert_eq!(snapshot_300.active_address_count, 0);

    let pass_after_first = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap();
    assert!(pass_after_first.is_none());
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    let second_ok = fixture
        .indexer
        .sync_blocks_for_test(mint_block_height..=mint_block_height)
        .await
        .unwrap();
    assert_eq!(second_ok, mint_block_height);

    let synced_after_second = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_after_second, Some(mint_block_height));

    let pass_after_second = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass_after_second.owner, owner);
    assert_eq!(pass_after_second.state, MinerPassState::Active);

    let snapshot_301 = fixture
        .storage
        .get_active_balance_snapshot(mint_block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_301.total_balance, 7_777);
    assert_eq!(snapshot_301.active_address_count, 1);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 2);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_retry_without_reconcile_recovers_energy_pending_failure() {
    let owner = test_script_hash(31);
    let block_height = 320u32;
    let mint_tx = build_test_tx(71);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![mint_tx])),
    );

    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(31, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[31u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[31u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_retry_without_reconcile_pending_energy_failure",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height,
                balance: 8_888,
                delta: 1,
            }]])),
        ],
        Arc::new(MockBalanceProvider::default().with_height(owner, block_height, 200_000, 100)),
    );

    let first_err = fixture
        .indexer
        .sync_blocks_without_reconcile_for_test(block_height..=block_height)
        .await
        .unwrap_err();
    assert!(first_err.contains("Address balance batch size mismatch"));
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
        None
    );

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
    assert_eq!(
        fixture
            .pass_energy_manager
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );

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

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_retry_without_reconcile_finalized_energy_failure",
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
        Some(0)
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
async fn test_sync_blocks_restart_after_failed_block_replay_matches_fresh_run() {
    // Purpose:
    // 1) Simulate block failure at h701 after h700 committed.
    // 2) Recreate indexer (restart) and replay h701.
    // 3) Compare final state with a fresh successful run over h700..h701.
    let owner = test_script_hash(22);
    let h700 = 700u32;
    let h701 = 701u32;
    let mint_tx = build_test_tx(57);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h700, build_test_block(vec![]))
            .with_block(h701, build_test_block(vec![mint_tx])),
    );

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(23, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[23u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[23u8; 32]).unwrap(),
            vout: 0,
        },
    };

    let restart_root = test_root_dir("indexer_behavior", "restart_replay_consistency");
    let restart_energy_provider =
        Arc::new(MockBalanceProvider::default().with_height(owner, h701, 260_000, 100));

    // First run: commit h700, fail at h701 during balance settle.
    {
        let inscription_source: Arc<dyn InscriptionSource> =
            Arc::new(MockInscriptionSource::default().with_mints(
                h701,
                vec![make_discovered_mint(mint_id.clone(), h701, vec![])],
            ));
        let transfer_tracker = Arc::new(
            MockTransferTracker::default().with_create_info(&mint_id, create_info.clone()),
        );
        let fixture = build_indexer_fixture_with_hint_provider_at_root(
            restart_root.clone(),
            inscription_source,
            block_hint_provider.clone(),
            transfer_tracker,
            vec![
                // h701: force settle failure by returning mismatched batch length
                MockResponse::Immediate(Ok(vec![])),
            ],
            restart_energy_provider.clone(),
        );

        let err = fixture
            .indexer
            .sync_blocks_for_test(h700..=h701)
            .await
            .unwrap_err();
        assert!(err.contains("Address balance batch size mismatch"));

        assert_eq!(
            fixture.storage.get_synced_btc_block_height().unwrap(),
            Some(h700)
        );
        assert!(
            fixture
                .storage
                .get_pass_by_inscription_id(&mint_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
        assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);
    }

    // Restart run: rebuild indexer from same root and replay h701.
    let (restart_pass, restart_energy, restart_snap_700, restart_snap_701) = {
        let inscription_source: Arc<dyn InscriptionSource> =
            Arc::new(MockInscriptionSource::default().with_mints(
                h701,
                vec![make_discovered_mint(mint_id.clone(), h701, vec![])],
            ));
        let transfer_tracker = Arc::new(
            MockTransferTracker::default().with_create_info(&mint_id, create_info.clone()),
        );
        let fixture = build_indexer_fixture_with_hint_provider_at_root(
            restart_root.clone(),
            inscription_source,
            block_hint_provider.clone(),
            transfer_tracker,
            vec![
                // h701 replay succeeds
                MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                    block_height: h701,
                    balance: 9_999,
                    delta: 999,
                }]])),
            ],
            restart_energy_provider.clone(),
        );

        let synced = fixture
            .indexer
            .sync_blocks_for_test(h701..=h701)
            .await
            .unwrap();
        assert_eq!(synced, h701);
        assert_eq!(
            fixture.storage.get_synced_btc_block_height().unwrap(),
            Some(h701)
        );
        assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
        assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

        let pass = fixture
            .storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .unwrap();
        let energy = fixture
            .pass_energy_manager
            .get_pass_energy(&mint_id, h701)
            .await
            .unwrap()
            .unwrap();
        let snap_700 = fixture
            .storage
            .get_active_balance_snapshot(h700)
            .unwrap()
            .unwrap();
        let snap_701 = fixture
            .storage
            .get_active_balance_snapshot(h701)
            .unwrap()
            .unwrap();
        (pass, energy, snap_700, snap_701)
    };

    // Fresh baseline run over same blocks should produce identical final state.
    let baseline_root = test_root_dir("indexer_behavior", "restart_replay_baseline");
    let baseline_energy_provider =
        Arc::new(MockBalanceProvider::default().with_height(owner, h701, 260_000, 100));
    let (baseline_pass, baseline_energy, baseline_snap_700, baseline_snap_701) = {
        let inscription_source: Arc<dyn InscriptionSource> =
            Arc::new(MockInscriptionSource::default().with_mints(
                h701,
                vec![make_discovered_mint(mint_id.clone(), h701, vec![])],
            ));
        let transfer_tracker = Arc::new(
            MockTransferTracker::default().with_create_info(&mint_id, create_info.clone()),
        );
        let fixture = build_indexer_fixture_with_hint_provider_at_root(
            baseline_root.clone(),
            inscription_source,
            block_hint_provider,
            transfer_tracker,
            vec![
                // h701
                MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                    block_height: h701,
                    balance: 9_999,
                    delta: 999,
                }]])),
            ],
            baseline_energy_provider,
        );

        let synced = fixture
            .indexer
            .sync_blocks_for_test(h700..=h701)
            .await
            .unwrap();
        assert_eq!(synced, h701);

        let pass = fixture
            .storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .unwrap();
        let energy = fixture
            .pass_energy_manager
            .get_pass_energy(&mint_id, h701)
            .await
            .unwrap()
            .unwrap();
        let snap_700 = fixture
            .storage
            .get_active_balance_snapshot(h700)
            .unwrap()
            .unwrap();
        let snap_701 = fixture
            .storage
            .get_active_balance_snapshot(h701)
            .unwrap()
            .unwrap();
        (pass, energy, snap_700, snap_701)
    };

    assert_eq!(restart_pass.owner, baseline_pass.owner);
    assert_eq!(restart_pass.state, baseline_pass.state);
    assert_eq!(restart_pass.satpoint, baseline_pass.satpoint);
    assert_eq!(restart_energy.state, baseline_energy.state);
    assert_eq!(restart_energy.energy, baseline_energy.energy);
    assert_eq!(
        (
            restart_snap_700.total_balance,
            restart_snap_700.active_address_count
        ),
        (
            baseline_snap_700.total_balance,
            baseline_snap_700.active_address_count
        )
    );
    assert_eq!(
        (
            restart_snap_701.total_balance,
            restart_snap_701.active_address_count
        ),
        (
            baseline_snap_701.total_balance,
            baseline_snap_701.active_address_count
        )
    );

    cleanup_temp_dir(&restart_root);
    cleanup_temp_dir(&baseline_root);
}

#[tokio::test]
async fn test_sync_blocks_failed_settle_keeps_pass_history_and_snapshot_atomic() {
    // Purpose: verify failed block does not advance sqlite state.
    // We first commit block h-1, then force settle failure at block h and assert:
    // miner_passes / miner_pass_state_history / active_balance_snapshot all stay at h-1.
    let owner1 = test_script_hash(14);
    let owner2 = test_script_hash(15);
    let committed_height = 320;
    let failed_height = 321;

    let mint_tx1 = build_test_tx(55);
    let mint_tx2 = build_test_tx(56);
    let mint_id1 = InscriptionId {
        txid: mint_tx1.compute_txid(),
        index: 0,
    };
    let mint_id2 = InscriptionId {
        txid: mint_tx2.compute_txid(),
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(committed_height, build_test_block(vec![mint_tx1]))
            .with_block(failed_height, build_test_block(vec![mint_tx2])),
    );

    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                committed_height,
                vec![make_discovered_mint(
                    mint_id1.clone(),
                    committed_height,
                    vec![],
                )],
            )
            .with_mints(
                failed_height,
                vec![make_discovered_mint(
                    mint_id2.clone(),
                    failed_height,
                    vec![],
                )],
            ),
    );

    let create_info1 = MockCreateInfo {
        satpoint: test_satpoint(12, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner1),
        commit_txid: Txid::from_slice(&[12u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[12u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info2 = MockCreateInfo {
        satpoint: test_satpoint(13, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner2),
        commit_txid: Txid::from_slice(&[13u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[13u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&mint_id1, create_info1)
            .with_create_info(&mint_id2, create_info2),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_failed_settle_keeps_atomic",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            // block=320 settle succeeds for owner1
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: committed_height,
                balance: 6_000,
                delta: 10,
            }]])),
            // block=321 settle fails by returning mismatched batch length
            MockResponse::Immediate(Ok(vec![])),
        ],
        Arc::new(
            MockBalanceProvider::default()
                .with_height(owner1, committed_height, 210_000, 100)
                .with_height(owner2, failed_height, 220_000, 100),
        ),
    );

    let err = fixture
        .indexer
        .sync_blocks_for_test(committed_height..=failed_height)
        .await
        .unwrap_err();
    assert!(err.contains("Address balance batch size mismatch"));

    // synced_height must remain at committed block.
    let synced = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced, Some(committed_height));

    // miner_passes must not include failed block mint.
    let pass1 = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id1)
        .unwrap()
        .unwrap();
    assert_eq!(pass1.owner, owner1);
    assert_eq!(pass1.state, MinerPassState::Active);
    let pass2 = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id2)
        .unwrap();
    assert!(pass2.is_none());

    // history must not include failed block mint event.
    let history1_at_failed = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&mint_id1, failed_height)
        .unwrap()
        .unwrap();
    assert_eq!(history1_at_failed.block_height, committed_height);
    assert_eq!(history1_at_failed.state, MinerPassState::Active);
    let history2_at_failed = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&mint_id2, failed_height)
        .unwrap();
    assert!(history2_at_failed.is_none());

    // active owner set at failed height must remain same as committed height.
    let expected_active = HashSet::from([owner1]);
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, failed_height),
        expected_active
    );

    // snapshot must exist only for committed block.
    let snapshot_committed = fixture
        .storage
        .get_active_balance_snapshot(committed_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_committed.active_address_count, 1);
    assert_eq!(snapshot_committed.total_balance, 6_000);
    let snapshot_failed = fixture
        .storage
        .get_active_balance_snapshot(failed_height)
        .unwrap();
    assert!(snapshot_failed.is_none());

    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_single_mint_success_updates_height_and_snapshot() {
    let owner = test_script_hash(4);
    let block_height = 400;
    let mint_tx = build_test_tx(54);
    let mint_id = InscriptionId {
        txid: mint_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![mint_tx])),
    );

    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(11, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[11u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[11u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture_with_hint_provider(
        "sync_blocks_single_mint_success",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 8_888,
                delta: 1,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default().with_height(owner, block_height, 220_000, 100)),
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(block_height..=block_height)
        .await
        .unwrap();
    assert_eq!(synced, block_height);

    let synced_height = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_height, Some(block_height));

    let pass = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass.owner, owner);
    assert_eq!(pass.state, MinerPassState::Active);

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.total_balance, 8_888);
    assert_eq!(snapshot.active_address_count, 1);

    let pass_block_commit = fixture.storage.get_pass_block_commit(block_height).unwrap();
    let pass_block_commit = pass_block_commit.unwrap();
    assert_eq!(pass_block_commit.block_height, block_height);
    assert_eq!(pass_block_commit.balance_history_block_height, block_height);
    assert_eq!(
        pass_block_commit.balance_history_block_commit,
        "33".repeat(32)
    );
    assert_eq!(pass_block_commit.commit_protocol_version, "1.0.0");
    assert_eq!(pass_block_commit.commit_hash_algo, "sha256");
    assert_eq!(pass_block_commit.mutation_root.len(), 64);
    assert_eq!(pass_block_commit.block_commit.len(), 64);

    assert!(fixture.status.update_count() > 0);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_same_block_transfer_then_mint_uses_transfered_prev_state() {
    let block_height = 500;
    let owner_a = test_script_hash(10);
    let owner_b = test_script_hash(11);
    let prev_pass_id = test_inscription_id(12, 0);

    let transfer_tx = build_test_tx(21);
    let mint_tx = build_test_tx(22);
    let transfer_txid = transfer_tx.compute_txid();
    let mint_txid = mint_tx.compute_txid();
    let transfer_prev_outpoint = transfer_tx.input[0].previous_output;
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(block_height, build_test_block(vec![transfer_tx, mint_tx])),
    );

    let mint_id = InscriptionId {
        txid: mint_txid,
        index: 0,
    };
    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![prev_pass_id.clone()]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let transfer_item = InscriptionTransferItem {
        inscription_id: prev_pass_id.clone(),
        block_height,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner_a,
        to_address: Some(owner_b),
    };
    let create_info = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner_b),
        commit_txid: Txid::from_slice(&[23u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[23u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&mint_id, create_info)
            .with_transfers(block_height, vec![transfer_item]),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 450, 220_000, 100)
            .with_height(owner_b, block_height, 230_000, 100),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "same_block_transfer_then_mint",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 9_000,
                delta: 1,
            },
        ]]))],
        energy_provider,
    );

    let prev_pass = make_active_pass(prev_pass_id.clone(), owner_a, 450);
    fixture
        .storage
        .add_new_mint_pass_at_height(&prev_pass, prev_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .on_new_pass(&prev_pass_id, &owner_a, 450, 0)
        .await
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let prev_pass = fixture
        .storage
        .get_pass_by_inscription_id(&prev_pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(prev_pass.owner, owner_b);
    assert_eq!(prev_pass.state, MinerPassState::Consumed);

    let new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_b);
    assert_eq!(new_pass.state, MinerPassState::Active);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_same_block_mint_then_transfer_keeps_mint_before_later_transfer() {
    let block_height = 510;
    let owner_a = test_script_hash(31);
    let owner_b = test_script_hash(32);
    let prev_pass_id = test_inscription_id(33, 0);

    let mint_tx = build_test_tx(41);
    let transfer_tx = build_test_tx(42);
    let mint_txid = mint_tx.compute_txid();
    let transfer_txid = transfer_tx.compute_txid();
    let transfer_prev_outpoint = transfer_tx.input[0].previous_output;
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(block_height, build_test_block(vec![mint_tx, transfer_tx])),
    );

    let mint_id = InscriptionId {
        txid: mint_txid,
        index: 0,
    };
    let mint = make_discovered_mint(mint_id.clone(), block_height, vec![prev_pass_id.clone()]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint]));

    let transfer_item = InscriptionTransferItem {
        inscription_id: prev_pass_id.clone(),
        block_height,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner_a,
        to_address: Some(owner_b),
    };
    let create_info = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner_a),
        commit_txid: Txid::from_slice(&[43u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[43u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&mint_id, create_info)
            .with_transfers(block_height, vec![transfer_item]),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 460, 250_000, 100)
            .with_height(owner_a, block_height, 260_000, 100),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "same_block_mint_then_transfer",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height,
                balance: 10_000,
                delta: 1,
            },
        ]]))],
        energy_provider,
    );

    let prev_pass = make_active_pass(prev_pass_id.clone(), owner_a, 460);
    fixture
        .storage
        .add_new_mint_pass_at_height(&prev_pass, prev_pass.mint_block_height)
        .unwrap();
    fixture
        .pass_energy_manager
        .on_new_pass(&prev_pass_id, &owner_a, 460, 0)
        .await
        .unwrap();

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let prev_pass = fixture
        .storage
        .get_pass_by_inscription_id(&prev_pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(prev_pass.owner, owner_b);
    assert_eq!(prev_pass.state, MinerPassState::Consumed);

    let new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_a);
    assert_eq!(new_pass.state, MinerPassState::Active);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_records_invalid_mint_with_error_code() {
    let block_height = 520;
    let owner = test_script_hash(40);
    let invalid_tx = build_test_tx(61);
    let invalid_id = InscriptionId {
        txid: invalid_tx.compute_txid(),
        index: 0,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(block_height, build_test_block(vec![invalid_tx])),
    );

    let invalid_mint = make_discovered_invalid_mint(
        invalid_id.clone(),
        block_height,
        "{\"p\":\"usdb\",\"op\":\"mint\",\"eth_main\":\"0x123\"}",
        MintValidationErrorCode::InvalidEthMain,
        "Invalid eth_main format",
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default().with_invalid_mints(block_height, vec![invalid_mint]),
    );

    let create_info = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: invalid_id.txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[62u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[62u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&invalid_id, create_info));

    let fixture = build_indexer_fixture_with_hint_provider(
        "invalid_mint_recorded",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![],
        Arc::new(MockBalanceProvider::default()),
    );

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let stored = fixture
        .storage
        .get_pass_by_inscription_id(&invalid_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, MinerPassState::Invalid);
    assert_eq!(stored.owner, owner);
    assert_eq!(
        stored.invalid_code.as_deref(),
        Some(MintValidationErrorCode::InvalidEthMain.as_str())
    );
    assert!(
        stored
            .invalid_reason
            .as_deref()
            .unwrap_or_default()
            .contains("Invalid eth_main format")
    );

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.active_address_count, 0);
    assert_eq!(snapshot.total_balance, 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_block_marks_mints_invalid_when_reveal_input_is_ambiguous() {
    let block_height = 530;
    let owner = test_script_hash(50);
    let mint_tx = build_test_tx(71);
    let mint_txid = mint_tx.compute_txid();
    let mint_id0 = InscriptionId {
        txid: mint_txid,
        index: 0,
    };
    let mint_id1 = InscriptionId {
        txid: mint_txid,
        index: 1,
    };
    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default().with_block(block_height, build_test_block(vec![mint_tx])),
    );

    let mint0 = make_discovered_mint(mint_id0.clone(), block_height, vec![]);
    let mint1 = make_discovered_mint(mint_id1.clone(), block_height, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(block_height, vec![mint0, mint1]));

    let shared_commit_outpoint = OutPoint {
        txid: Txid::from_slice(&[72u8; 32]).unwrap(),
        vout: 0,
    };
    let create_info0 = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: shared_commit_outpoint.txid,
        commit_outpoint: shared_commit_outpoint,
    };
    let create_info1 = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_txid,
                vout: 0,
            },
            offset: 1,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: shared_commit_outpoint.txid,
        commit_outpoint: shared_commit_outpoint,
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&mint_id0, create_info0)
            .with_create_info(&mint_id1, create_info1),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "ambiguous_reveal_input_invalid",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![],
        Arc::new(MockBalanceProvider::default()),
    );

    fixture
        .indexer
        .sync_block_for_test(block_height)
        .await
        .unwrap();

    let stored0 = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id0)
        .unwrap()
        .unwrap();
    let stored1 = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id1)
        .unwrap()
        .unwrap();
    assert_eq!(stored0.state, MinerPassState::Invalid);
    assert_eq!(stored1.state, MinerPassState::Invalid);
    assert_eq!(
        stored0.invalid_code.as_deref(),
        Some(MintValidationErrorCode::AmbiguousRevealInput.as_str())
    );
    assert_eq!(
        stored1.invalid_code.as_deref(),
        Some(MintValidationErrorCode::AmbiguousRevealInput.as_str())
    );

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(block_height)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.active_address_count, 0);
    assert_eq!(snapshot.total_balance, 0);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_timeline_mint_transfer_burn_remint_replay() {
    // End-to-end timeline:
    // h100 mint(A, owner1)
    // h101 transfer(A, owner1 -> owner1)
    // h102 transfer(A, owner1 -> owner2)
    // h103 burn(A)
    // h104 mint(B, owner2, prev=[A])
    let h100 = 100u32;
    let h101 = 101u32;
    let h102 = 102u32;
    let h103 = 103u32;
    let h104 = 104u32;
    let owner1 = test_script_hash(61);
    let owner2 = test_script_hash(62);

    let mint_a_tx = build_test_tx(81);
    let transfer_same_tx = build_test_tx(82);
    let transfer_cross_tx = build_test_tx(83);
    let burn_tx = build_test_tx(84);
    let mint_b_tx = build_test_tx(85);

    let mint_a_txid = mint_a_tx.compute_txid();
    let transfer_same_txid = transfer_same_tx.compute_txid();
    let transfer_cross_txid = transfer_cross_tx.compute_txid();
    let burn_txid = burn_tx.compute_txid();
    let mint_b_txid = mint_b_tx.compute_txid();

    let transfer_same_prev_outpoint = transfer_same_tx.input[0].previous_output;
    let transfer_cross_prev_outpoint = transfer_cross_tx.input[0].previous_output;
    let burn_prev_outpoint = burn_tx.input[0].previous_output;

    let pass_a_id = InscriptionId {
        txid: mint_a_txid,
        index: 0,
    };
    let pass_b_id = InscriptionId {
        txid: mint_b_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h100, build_test_block(vec![mint_a_tx]))
            .with_block(h101, build_test_block(vec![transfer_same_tx]))
            .with_block(h102, build_test_block(vec![transfer_cross_tx]))
            .with_block(h103, build_test_block(vec![burn_tx]))
            .with_block(h104, build_test_block(vec![mint_b_tx])),
    );

    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                h100,
                vec![make_discovered_mint(pass_a_id.clone(), h100, vec![])],
            )
            .with_mints(
                h104,
                vec![make_discovered_mint(
                    pass_b_id.clone(),
                    h104,
                    vec![pass_a_id.clone()],
                )],
            ),
    );

    let create_info_a = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_a_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner1),
        commit_txid: Txid::from_slice(&[91u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[91u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_b = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_b_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner2),
        commit_txid: Txid::from_slice(&[92u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[92u8; 32]).unwrap(),
            vout: 0,
        },
    };

    let transfer_same_item = InscriptionTransferItem {
        inscription_id: pass_a_id.clone(),
        block_height: h101,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_same_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_same_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner1,
        to_address: Some(owner1),
    };
    let transfer_cross_item = InscriptionTransferItem {
        inscription_id: pass_a_id.clone(),
        block_height: h102,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_cross_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_cross_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner1,
        to_address: Some(owner2),
    };
    let burn_item = InscriptionTransferItem {
        inscription_id: pass_a_id.clone(),
        block_height: h103,
        prev_satpoint: ordinals::SatPoint {
            outpoint: burn_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: burn_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner2,
        to_address: None,
    };

    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&pass_a_id, create_info_a)
            .with_create_info(&pass_b_id, create_info_b)
            .with_transfers(h101, vec![transfer_same_item])
            .with_transfers(h102, vec![transfer_cross_item])
            .with_transfers(h103, vec![burn_item]),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner1, h100, 200_000, 100)
            .with_range(
                owner1,
                (h100 + 1)..(h101 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h101,
                    balance: 201_000,
                    delta: 1_000,
                }],
            )
            .with_range(
                owner1,
                (h101 + 1)..(h102 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h102,
                    balance: 180_000,
                    delta: -21_000,
                }],
            )
            .with_height(owner2, h104, 250_000, 500),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "timeline_mint_transfer_burn_remint_replay",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h100,
                balance: 5_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h101,
                balance: 5_500,
                delta: 500,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h104,
                balance: 8_000,
                delta: 2_500,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h100..=h104)
        .await
        .unwrap();
    assert_eq!(synced, h104);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 5);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // Check 1: active owner set at each height.
    let expected_active_100 = HashSet::from([owner1]);
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h100),
        expected_active_100
    );
    let expected_active_101 = HashSet::from([owner1]);
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h101),
        expected_active_101
    );
    assert!(active_owner_set_at_height(&fixture.storage, h102).is_empty());
    assert!(active_owner_set_at_height(&fixture.storage, h103).is_empty());
    let expected_active_104 = HashSet::from([owner2]);
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h104),
        expected_active_104
    );

    // Check 2: pass state timeline from history snapshots.
    let pass_a_100 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_a_id, h100)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a_100.state, MinerPassState::Active);
    assert_eq!(pass_a_100.owner, owner1);
    let pass_a_101 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_a_id, h101)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a_101.state, MinerPassState::Active);
    assert_eq!(pass_a_101.owner, owner1);
    let pass_a_102 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_a_id, h102)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a_102.state, MinerPassState::Dormant);
    assert_eq!(pass_a_102.owner, owner2);
    let pass_a_103 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_a_id, h103)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a_103.state, MinerPassState::Burned);
    assert_eq!(pass_a_103.owner, owner2);
    let pass_a_104 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_a_id, h104)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a_104.state, MinerPassState::Burned);
    assert_eq!(pass_a_104.owner, owner2);

    assert!(
        fixture
            .storage
            .get_last_pass_history_at_or_before_height(&pass_b_id, h103)
            .unwrap()
            .is_none()
    );
    let pass_b_104 = fixture
        .storage
        .get_last_pass_history_at_or_before_height(&pass_b_id, h104)
        .unwrap()
        .unwrap();
    assert_eq!(pass_b_104.state, MinerPassState::Active);
    assert_eq!(pass_b_104.owner, owner2);

    // Check 3: energy snapshots at each height.
    let expected_a_101 = calc_growth_delta(200_000, 1);
    let expected_a_102 = expected_a_101
        .saturating_add(calc_growth_delta(201_000, 2))
        .saturating_sub(calc_penalty_from_delta(-21_000));

    let energy_a_100 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h100)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_100.state, MinerPassState::Active);
    assert_eq!(energy_a_100.energy, 0);
    let energy_a_101 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_101.state, MinerPassState::Active);
    assert_eq!(energy_a_101.energy, expected_a_101);
    let energy_a_102 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h102)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_102.state, MinerPassState::Dormant);
    assert_eq!(energy_a_102.energy, expected_a_102);
    let energy_a_103 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h103)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_103.state, MinerPassState::Dormant);
    assert_eq!(energy_a_103.energy, expected_a_102);
    let energy_a_104 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h104)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_104.state, MinerPassState::Dormant);
    assert_eq!(energy_a_104.energy, expected_a_102);

    let energy_b_104 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_b_id, h104)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_b_104.state, MinerPassState::Active);
    assert_eq!(energy_b_104.energy, 0);

    // Burned prev should not be consumed/inherited by remint.
    let current_a = fixture
        .storage
        .get_pass_by_inscription_id(&pass_a_id)
        .unwrap()
        .unwrap();
    assert_eq!(current_a.state, MinerPassState::Burned);
    let current_b = fixture
        .storage
        .get_pass_by_inscription_id(&pass_b_id)
        .unwrap()
        .unwrap();
    assert_eq!(current_b.state, MinerPassState::Active);

    // Balance settlement snapshots by height.
    let snap_100 = fixture
        .storage
        .get_active_balance_snapshot(h100)
        .unwrap()
        .unwrap();
    assert_eq!(snap_100.active_address_count, 1);
    assert_eq!(snap_100.total_balance, 5_000);
    let snap_101 = fixture
        .storage
        .get_active_balance_snapshot(h101)
        .unwrap()
        .unwrap();
    assert_eq!(snap_101.active_address_count, 1);
    assert_eq!(snap_101.total_balance, 5_500);
    let snap_102 = fixture
        .storage
        .get_active_balance_snapshot(h102)
        .unwrap()
        .unwrap();
    assert_eq!(snap_102.active_address_count, 0);
    assert_eq!(snap_102.total_balance, 0);
    let snap_103 = fixture
        .storage
        .get_active_balance_snapshot(h103)
        .unwrap()
        .unwrap();
    assert_eq!(snap_103.active_address_count, 0);
    assert_eq!(snap_103.total_balance, 0);
    let snap_104 = fixture
        .storage
        .get_active_balance_snapshot(h104)
        .unwrap()
        .unwrap();
    assert_eq!(snap_104.active_address_count, 1);
    assert_eq!(snap_104.total_balance, 8_000);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_passive_transfer_keeps_receiver_active_and_transferred_pass_dormant() {
    // Scenario:
    // h600 mint(B, owner_b)
    // h601 mint(A, owner_a)
    // h602 transfer(A, owner_a -> owner_b)
    //
    // Expected:
    // - owner_b keeps the original active pass B
    // - transferred pass A becomes Dormant under owner_b (passive receive)
    // - active owner set at h602 contains only owner_b
    let h600 = 600u32;
    let h601 = 601u32;
    let h602 = 602u32;
    let owner_a = test_script_hash(71);
    let owner_b = test_script_hash(72);

    let mint_b_tx = build_test_tx(101);
    let mint_a_tx = build_test_tx(102);
    let transfer_tx = build_test_tx(103);
    let mint_b_txid = mint_b_tx.compute_txid();
    let mint_a_txid = mint_a_tx.compute_txid();
    let transfer_txid = transfer_tx.compute_txid();
    let transfer_prev_outpoint = transfer_tx.input[0].previous_output;

    let pass_b_id = InscriptionId {
        txid: mint_b_txid,
        index: 0,
    };
    let pass_a_id = InscriptionId {
        txid: mint_a_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h600, build_test_block(vec![mint_b_tx]))
            .with_block(h601, build_test_block(vec![mint_a_tx]))
            .with_block(h602, build_test_block(vec![transfer_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                h600,
                vec![make_discovered_mint(pass_b_id.clone(), h600, vec![])],
            )
            .with_mints(
                h601,
                vec![make_discovered_mint(pass_a_id.clone(), h601, vec![])],
            ),
    );

    let create_info_b = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_b_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner_b),
        commit_txid: Txid::from_slice(&[111u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[111u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_a = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_a_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner_a),
        commit_txid: Txid::from_slice(&[112u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[112u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_item = InscriptionTransferItem {
        inscription_id: pass_a_id.clone(),
        block_height: h602,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner_a,
        to_address: Some(owner_b),
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&pass_b_id, create_info_b)
            .with_create_info(&pass_a_id, create_info_a)
            .with_transfers(h602, vec![transfer_item]),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_b, h600, 240_000, 0)
            .with_height(owner_a, h601, 230_000, 0)
            .with_range(
                owner_a,
                h602..(h602 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h602,
                    balance: 231_000,
                    delta: 1_000,
                }],
            ),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "passive_transfer_keeps_receiver_active",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h600,
                balance: 7_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![
                vec![balance_history::AddressBalance {
                    block_height: h601,
                    balance: 3_000,
                    delta: 0,
                }],
                vec![balance_history::AddressBalance {
                    block_height: h601,
                    balance: 7_000,
                    delta: 0,
                }],
            ])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h602,
                balance: 8_000,
                delta: 1_000,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h600..=h602)
        .await
        .unwrap();
    assert_eq!(synced, h602);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 3);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // 1) pass state assertion
    let pass_b = fixture
        .storage
        .get_pass_by_inscription_id(&pass_b_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass_b.owner, owner_b);
    assert_eq!(pass_b.state, MinerPassState::Active);

    let pass_a = fixture
        .storage
        .get_pass_by_inscription_id(&pass_a_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass_a.owner, owner_b);
    assert_eq!(pass_a.state, MinerPassState::Dormant);

    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h600),
        HashSet::from([owner_b])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h601),
        HashSet::from([owner_a, owner_b])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h602),
        HashSet::from([owner_b])
    );

    // 2) energy assertion
    let energy_b_602 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_b_id, h602)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_b_602.state, MinerPassState::Active);
    assert_eq!(energy_b_602.energy, calc_growth_delta(240_000, 2));

    // Record query still returns the last persisted snapshot.
    let energy_b_record_602 = fixture
        .pass_energy_manager
        .get_pass_energy_record_at_or_before(&pass_b_id, h602)
        .unwrap()
        .unwrap();
    assert_eq!(energy_b_record_602.block_height, h600);
    assert_eq!(energy_b_record_602.energy, 0);

    let energy_a_602 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_a_id, h602)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_602.state, MinerPassState::Dormant);
    assert_eq!(energy_a_602.energy, calc_growth_delta(230_000, 1));

    // 3) active balance snapshot assertion
    let snap_600 = fixture
        .storage
        .get_active_balance_snapshot(h600)
        .unwrap()
        .unwrap();
    assert_eq!(snap_600.active_address_count, 1);
    assert_eq!(snap_600.total_balance, 7_000);

    let snap_601 = fixture
        .storage
        .get_active_balance_snapshot(h601)
        .unwrap()
        .unwrap();
    assert_eq!(snap_601.active_address_count, 2);
    assert_eq!(snap_601.total_balance, 10_000);

    let snap_602 = fixture
        .storage
        .get_active_balance_snapshot(h602)
        .unwrap()
        .unwrap();
    assert_eq!(snap_602.active_address_count, 1);
    assert_eq!(snap_602.total_balance, 8_000);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_same_owner_multiple_mints_keep_only_latest_active() {
    // Scenario:
    // h610 mint(old, owner)
    // h611 mint(new, owner)
    //
    // Expected:
    // - old pass becomes Dormant
    // - new pass is Active
    // - active owner set remains a single owner across heights
    let h610 = 610u32;
    let h611 = 611u32;
    let owner = test_script_hash(81);

    let mint_old_tx = build_test_tx(121);
    let mint_new_tx = build_test_tx(122);
    let mint_old_txid = mint_old_tx.compute_txid();
    let mint_new_txid = mint_new_tx.compute_txid();
    let old_pass_id = InscriptionId {
        txid: mint_old_txid,
        index: 0,
    };
    let new_pass_id = InscriptionId {
        txid: mint_new_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h610, build_test_block(vec![mint_old_tx]))
            .with_block(h611, build_test_block(vec![mint_new_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                h610,
                vec![make_discovered_mint(old_pass_id.clone(), h610, vec![])],
            )
            .with_mints(
                h611,
                vec![make_discovered_mint(new_pass_id.clone(), h611, vec![])],
            ),
    );

    let create_info_old = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_old_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[123u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[123u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_new = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_new_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[124u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[124u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&old_pass_id, create_info_old)
            .with_create_info(&new_pass_id, create_info_new),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner, h610, 220_000, 0)
            .with_height(owner, h611, 221_000, 1_000)
            .with_range(
                owner,
                h611..(h611 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h611,
                    balance: 221_000,
                    delta: 1_000,
                }],
            ),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "same_owner_multiple_mints_keep_latest_active",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h610,
                balance: 4_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h611,
                balance: 4_500,
                delta: 500,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h610..=h611)
        .await
        .unwrap();
    assert_eq!(synced, h611);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 2);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // 1) pass state assertion
    let old_pass = fixture
        .storage
        .get_pass_by_inscription_id(&old_pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(old_pass.owner, owner);
    assert_eq!(old_pass.state, MinerPassState::Dormant);

    let new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&new_pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner);
    assert_eq!(new_pass.state, MinerPassState::Active);

    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h610),
        HashSet::from([owner])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h611),
        HashSet::from([owner])
    );

    // 2) energy assertion
    let old_energy_611 = fixture
        .pass_energy_manager
        .get_pass_energy(&old_pass_id, h611)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_energy_611.state, MinerPassState::Dormant);
    assert_eq!(old_energy_611.energy, calc_growth_delta(220_000, 1));

    let new_energy_611 = fixture
        .pass_energy_manager
        .get_pass_energy(&new_pass_id, h611)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy_611.state, MinerPassState::Active);
    assert_eq!(new_energy_611.energy, 0);

    // 3) active balance snapshot assertion
    let snap_610 = fixture
        .storage
        .get_active_balance_snapshot(h610)
        .unwrap()
        .unwrap();
    assert_eq!(snap_610.active_address_count, 1);
    assert_eq!(snap_610.total_balance, 4_000);

    let snap_611 = fixture
        .storage
        .get_active_balance_snapshot(h611)
        .unwrap()
        .unwrap();
    assert_eq!(snap_611.active_address_count, 1);
    assert_eq!(snap_611.total_balance, 4_500);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_multi_prev_inherit_sums_energy_and_consumes_all_prev() {
    // Scenario:
    // h620 mint(prev1, owner)
    // h621 mint(prev2, owner)        -> prev1 dormant
    // h622 mint(new, owner, prev=[prev1, prev2])
    //
    // Expected:
    // - prev1/prev2 are consumed
    // - new pass is active
    // - inherited energy = dormant(prev1@621) + dormant(prev2@622)
    let h620 = 620u32;
    let h621 = 621u32;
    let h622 = 622u32;
    let owner = test_script_hash(91);

    let mint_prev1_tx = build_test_tx(131);
    let mint_prev2_tx = build_test_tx(132);
    let mint_new_tx = build_test_tx(133);
    let mint_prev1_txid = mint_prev1_tx.compute_txid();
    let mint_prev2_txid = mint_prev2_tx.compute_txid();
    let mint_new_txid = mint_new_tx.compute_txid();
    let prev1_id = InscriptionId {
        txid: mint_prev1_txid,
        index: 0,
    };
    let prev2_id = InscriptionId {
        txid: mint_prev2_txid,
        index: 0,
    };
    let new_id = InscriptionId {
        txid: mint_new_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h620, build_test_block(vec![mint_prev1_tx]))
            .with_block(h621, build_test_block(vec![mint_prev2_tx]))
            .with_block(h622, build_test_block(vec![mint_new_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                h620,
                vec![make_discovered_mint(prev1_id.clone(), h620, vec![])],
            )
            .with_mints(
                h621,
                vec![make_discovered_mint(prev2_id.clone(), h621, vec![])],
            )
            .with_mints(
                h622,
                vec![make_discovered_mint(
                    new_id.clone(),
                    h622,
                    vec![prev1_id.clone(), prev2_id.clone()],
                )],
            ),
    );

    let create_info_prev1 = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_prev1_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[141u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[141u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_prev2 = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_prev2_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[142u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[142u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_new = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_new_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[143u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[143u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&prev1_id, create_info_prev1)
            .with_create_info(&prev2_id, create_info_prev2)
            .with_create_info(&new_id, create_info_new),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner, h620, 220_000, 0)
            .with_height(owner, h621, 230_000, 10_000)
            .with_height(owner, h622, 240_000, 10_000)
            .with_range(
                owner,
                h621..(h621 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h621,
                    balance: 230_000,
                    delta: 10_000,
                }],
            )
            .with_range(
                owner,
                h622..(h622 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h622,
                    balance: 240_000,
                    delta: 10_000,
                }],
            ),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "multi_prev_inherit_sums_energy",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h620,
                balance: 6_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h621,
                balance: 6_500,
                delta: 500,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h622,
                balance: 7_000,
                delta: 500,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h620..=h622)
        .await
        .unwrap();
    assert_eq!(synced, h622);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 3);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // 1) pass state assertion
    let prev1 = fixture
        .storage
        .get_pass_by_inscription_id(&prev1_id)
        .unwrap()
        .unwrap();
    assert_eq!(prev1.state, MinerPassState::Consumed);
    let prev2 = fixture
        .storage
        .get_pass_by_inscription_id(&prev2_id)
        .unwrap()
        .unwrap();
    assert_eq!(prev2.state, MinerPassState::Consumed);
    let new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&new_id)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.state, MinerPassState::Active);
    assert_eq!(new_pass.owner, owner);

    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h620),
        HashSet::from([owner])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h621),
        HashSet::from([owner])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h622),
        HashSet::from([owner])
    );

    // 2) energy assertion
    let expected_prev1_dormant = calc_growth_delta(220_000, 1);
    let expected_prev2_dormant = calc_growth_delta(230_000, 1);
    let expected_inherited = expected_prev1_dormant.saturating_add(expected_prev2_dormant);

    let prev1_energy_621 = fixture
        .pass_energy_manager
        .get_pass_energy(&prev1_id, h621)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev1_energy_621.state, MinerPassState::Dormant);
    assert_eq!(prev1_energy_621.energy, expected_prev1_dormant);

    let prev1_energy_622 = fixture
        .pass_energy_manager
        .get_pass_energy(&prev1_id, h622)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev1_energy_622.state, MinerPassState::Consumed);
    assert_eq!(prev1_energy_622.energy, 0);

    let prev2_energy_622 = fixture
        .pass_energy_manager
        .get_pass_energy(&prev2_id, h622)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev2_energy_622.state, MinerPassState::Consumed);
    assert_eq!(prev2_energy_622.energy, 0);

    let new_energy_622 = fixture
        .pass_energy_manager
        .get_pass_energy(&new_id, h622)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy_622.state, MinerPassState::Active);
    assert_eq!(new_energy_622.energy, expected_inherited);

    // 3) active balance snapshot assertion
    let snap_620 = fixture
        .storage
        .get_active_balance_snapshot(h620)
        .unwrap()
        .unwrap();
    assert_eq!(snap_620.active_address_count, 1);
    assert_eq!(snap_620.total_balance, 6_000);
    let snap_621 = fixture
        .storage
        .get_active_balance_snapshot(h621)
        .unwrap()
        .unwrap();
    assert_eq!(snap_621.active_address_count, 1);
    assert_eq!(snap_621.total_balance, 6_500);
    let snap_622 = fixture
        .storage
        .get_active_balance_snapshot(h622)
        .unwrap()
        .unwrap();
    assert_eq!(snap_622.active_address_count, 1);
    assert_eq!(snap_622.total_balance, 7_000);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_double_inherit_same_prev_only_first_gets_energy() {
    // Scenario:
    // h630 mint(prev, owner)
    // h631 mint(first_new, owner, prev=[prev])   -> consume prev, inherit energy
    // h632 mint(second_new, owner, prev=[prev])  -> prev already consumed, no inheritance
    let h630 = 630u32;
    let h631 = 631u32;
    let h632 = 632u32;
    let owner = test_script_hash(101);

    let mint_prev_tx = build_test_tx(151);
    let mint_first_new_tx = build_test_tx(152);
    let mint_second_new_tx = build_test_tx(153);
    let mint_prev_txid = mint_prev_tx.compute_txid();
    let mint_first_new_txid = mint_first_new_tx.compute_txid();
    let mint_second_new_txid = mint_second_new_tx.compute_txid();
    let prev_id = InscriptionId {
        txid: mint_prev_txid,
        index: 0,
    };
    let first_new_id = InscriptionId {
        txid: mint_first_new_txid,
        index: 0,
    };
    let second_new_id = InscriptionId {
        txid: mint_second_new_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h630, build_test_block(vec![mint_prev_tx]))
            .with_block(h631, build_test_block(vec![mint_first_new_tx]))
            .with_block(h632, build_test_block(vec![mint_second_new_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(
        MockInscriptionSource::default()
            .with_mints(
                h630,
                vec![make_discovered_mint(prev_id.clone(), h630, vec![])],
            )
            .with_mints(
                h631,
                vec![make_discovered_mint(
                    first_new_id.clone(),
                    h631,
                    vec![prev_id.clone()],
                )],
            )
            .with_mints(
                h632,
                vec![make_discovered_mint(
                    second_new_id.clone(),
                    h632,
                    vec![prev_id.clone()],
                )],
            ),
    );

    let create_info_prev = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_prev_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[161u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[161u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_first_new = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_first_new_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[162u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[162u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let create_info_second_new = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_second_new_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[163u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[163u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&prev_id, create_info_prev)
            .with_create_info(&first_new_id, create_info_first_new)
            .with_create_info(&second_new_id, create_info_second_new),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner, h630, 230_000, 0)
            .with_height(owner, h631, 240_000, 10_000)
            .with_height(owner, h632, 245_000, 5_000)
            .with_range(
                owner,
                h631..(h631 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h631,
                    balance: 240_000,
                    delta: 10_000,
                }],
            )
            .with_range(
                owner,
                h632..(h632 + 1),
                vec![balance_history::AddressBalance {
                    block_height: h632,
                    balance: 245_000,
                    delta: 5_000,
                }],
            ),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "double_inherit_same_prev",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h630,
                balance: 8_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h631,
                balance: 8_500,
                delta: 500,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h632,
                balance: 9_000,
                delta: 500,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h630..=h632)
        .await
        .unwrap();
    assert_eq!(synced, h632);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 3);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // 1) pass state assertion
    let prev_pass = fixture
        .storage
        .get_pass_by_inscription_id(&prev_id)
        .unwrap()
        .unwrap();
    assert_eq!(prev_pass.state, MinerPassState::Consumed);

    let first_new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&first_new_id)
        .unwrap()
        .unwrap();
    assert_eq!(first_new_pass.state, MinerPassState::Dormant);
    let second_new_pass = fixture
        .storage
        .get_pass_by_inscription_id(&second_new_id)
        .unwrap()
        .unwrap();
    assert_eq!(second_new_pass.state, MinerPassState::Active);

    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h630),
        HashSet::from([owner])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h631),
        HashSet::from([owner])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h632),
        HashSet::from([owner])
    );

    // 2) energy assertion
    let expected_prev_dormant = calc_growth_delta(230_000, 1);
    let expected_first_new_dormant =
        expected_prev_dormant.saturating_add(calc_growth_delta(240_000, 1));

    let prev_energy_631 = fixture
        .pass_energy_manager
        .get_pass_energy(&prev_id, h631)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev_energy_631.state, MinerPassState::Consumed);
    assert_eq!(prev_energy_631.energy, 0);

    let first_new_energy_631 = fixture
        .pass_energy_manager
        .get_pass_energy(&first_new_id, h631)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_new_energy_631.state, MinerPassState::Active);
    assert_eq!(first_new_energy_631.energy, expected_prev_dormant);

    let first_new_energy_632 = fixture
        .pass_energy_manager
        .get_pass_energy(&first_new_id, h632)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_new_energy_632.state, MinerPassState::Dormant);
    assert_eq!(first_new_energy_632.energy, expected_first_new_dormant);

    let second_new_energy_632 = fixture
        .pass_energy_manager
        .get_pass_energy(&second_new_id, h632)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_new_energy_632.state, MinerPassState::Active);
    assert_eq!(second_new_energy_632.energy, 0);

    // 3) active balance snapshot assertion
    let snap_630 = fixture
        .storage
        .get_active_balance_snapshot(h630)
        .unwrap()
        .unwrap();
    assert_eq!(snap_630.active_address_count, 1);
    assert_eq!(snap_630.total_balance, 8_000);
    let snap_631 = fixture
        .storage
        .get_active_balance_snapshot(h631)
        .unwrap()
        .unwrap();
    assert_eq!(snap_631.active_address_count, 1);
    assert_eq!(snap_631.total_balance, 8_500);
    let snap_632 = fixture
        .storage
        .get_active_balance_snapshot(h632)
        .unwrap()
        .unwrap();
    assert_eq!(snap_632.active_address_count, 1);
    assert_eq!(snap_632.total_balance, 9_000);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_balance_threshold_and_penalty_applied_before_dormant_transfer() {
    // Scenario:
    // h640 mint(pass, owner_a)
    // h643 transfer(pass, owner_a -> owner_b) triggers energy finalize to h643
    //
    // owner_a balance timeline for update range [641, 643]:
    // - h641: negative delta (penalty + reset active height)
    // - h642: balance below threshold in previous record (no growth)
    // - h643: growth resumes from above-threshold balance with one-step incremental growth
    let h640 = 640u32;
    let h641 = 641u32;
    let h642 = 642u32;
    let h643 = 643u32;
    let owner_a = test_script_hash(111);
    let owner_b = test_script_hash(112);

    let mint_tx = build_test_tx(171);
    let transfer_tx = build_test_tx(172);
    let mint_txid = mint_tx.compute_txid();
    let transfer_txid = transfer_tx.compute_txid();
    let transfer_prev_outpoint = transfer_tx.input[0].previous_output;
    let pass_id = InscriptionId {
        txid: mint_txid,
        index: 0,
    };

    let block_hint_provider: Arc<dyn BlockHintProvider> = Arc::new(
        MockBlockHintProvider::default()
            .with_block(h640, build_test_block(vec![mint_tx]))
            .with_block(h641, build_test_block(vec![]))
            .with_block(h642, build_test_block(vec![]))
            .with_block(h643, build_test_block(vec![transfer_tx])),
    );
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(
            h640,
            vec![make_discovered_mint(pass_id.clone(), h640, vec![])],
        ));

    let create_info = MockCreateInfo {
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: mint_txid,
                vout: 0,
            },
            offset: 0,
        },
        value: Amount::from_sat(10_000),
        address: Some(owner_a),
        commit_txid: Txid::from_slice(&[181u8; 32]).unwrap(),
        commit_outpoint: OutPoint {
            txid: Txid::from_slice(&[181u8; 32]).unwrap(),
            vout: 0,
        },
    };
    let transfer_item = InscriptionTransferItem {
        inscription_id: pass_id.clone(),
        block_height: h643,
        prev_satpoint: ordinals::SatPoint {
            outpoint: transfer_prev_outpoint,
            offset: 0,
        },
        satpoint: ordinals::SatPoint {
            outpoint: OutPoint {
                txid: transfer_txid,
                vout: 0,
            },
            offset: 0,
        },
        from_address: owner_a,
        to_address: Some(owner_b),
    };
    let transfer_tracker = Arc::new(
        MockTransferTracker::default()
            .with_create_info(&pass_id, create_info)
            .with_transfers(h643, vec![transfer_item]),
    );

    let energy_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, h640, 150_000, 0)
            .with_range(
                owner_a,
                h641..(h643 + 1),
                vec![
                    balance_history::AddressBalance {
                        block_height: h641,
                        balance: 90_000,
                        delta: -60_000,
                    },
                    balance_history::AddressBalance {
                        block_height: h642,
                        balance: 120_000,
                        delta: 30_000,
                    },
                    balance_history::AddressBalance {
                        block_height: h643,
                        balance: 120_001,
                        delta: 1,
                    },
                ],
            ),
    );

    let fixture = build_indexer_fixture_with_hint_provider(
        "balance_threshold_and_penalty_before_dormant_transfer",
        inscription_source,
        block_hint_provider,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h640,
                balance: 4_000,
                delta: 0,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h641,
                balance: 4_100,
                delta: 100,
            }]])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: h642,
                balance: 4_200,
                delta: 100,
            }]])),
        ],
        energy_provider,
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(h640..=h643)
        .await
        .unwrap();
    assert_eq!(synced, h643);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 4);
    assert_eq!(fixture.transfer_tracker.rollback_call_count(), 0);

    // 1) pass state assertion
    let pass = fixture
        .storage
        .get_pass_by_inscription_id(&pass_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass.owner, owner_b);
    assert_eq!(pass.state, MinerPassState::Dormant);

    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h640),
        HashSet::from([owner_a])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h641),
        HashSet::from([owner_a])
    );
    assert_eq!(
        active_owner_set_at_height(&fixture.storage, h642),
        HashSet::from([owner_a])
    );
    assert!(active_owner_set_at_height(&fixture.storage, h643).is_empty());

    // 2) energy assertion
    let expected_h641 =
        calc_growth_delta(150_000, 1).saturating_sub(calc_penalty_from_delta(-60_000));
    assert_eq!(expected_h641, 0);
    let expected_h642 = expected_h641.saturating_add(calc_growth_delta(90_000, 1));
    assert_eq!(expected_h642, 0);
    let expected_h643 = expected_h642.saturating_add(
        calc_growth_delta(120_000, 2).saturating_sub(calc_growth_delta(120_000, 1)),
    );

    let energy_642 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, h642)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_642.state, MinerPassState::Active);
    assert_eq!(energy_642.energy, expected_h642);

    let energy_643 = fixture
        .pass_energy_manager
        .get_pass_energy(&pass_id, h643)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_643.state, MinerPassState::Dormant);
    assert_eq!(energy_643.energy, expected_h643);

    // 3) active balance snapshot assertion
    let snap_640 = fixture
        .storage
        .get_active_balance_snapshot(h640)
        .unwrap()
        .unwrap();
    assert_eq!(snap_640.active_address_count, 1);
    assert_eq!(snap_640.total_balance, 4_000);
    let snap_641 = fixture
        .storage
        .get_active_balance_snapshot(h641)
        .unwrap()
        .unwrap();
    assert_eq!(snap_641.active_address_count, 1);
    assert_eq!(snap_641.total_balance, 4_100);
    let snap_642 = fixture
        .storage
        .get_active_balance_snapshot(h642)
        .unwrap()
        .unwrap();
    assert_eq!(snap_642.active_address_count, 1);
    assert_eq!(snap_642.total_balance, 4_200);
    let snap_643 = fixture
        .storage
        .get_active_balance_snapshot(h643)
        .unwrap()
        .unwrap();
    assert_eq!(snap_643.active_address_count, 0);
    assert_eq!(snap_643.total_balance, 0);

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
