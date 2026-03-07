use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};
use crate::balance::{BalanceMonitor, MockBalanceBackend, MockResponse, SerialBalanceLoader};
use crate::config::ConfigManager;
use crate::index::MintValidationErrorCode;
use crate::index::content::{MinerPassState, USDBInscription, USDBMint};
use crate::index::energy::PassEnergyManager;
use crate::index::energy_formula::{calc_growth_delta, calc_penalty_from_delta};
use crate::index::pass::MinerPassManager;
use crate::index::transfer::{InscriptionCreateInfo, TransferTrackSeed};
use crate::index::{BlockHintProvider, IndexStatusApi, InscriptionIndexer, TransferTrackerApi};
use crate::inscription::{
    DiscoveredInscription, DiscoveredInvalidMint, DiscoveredMint, DiscoveredMintBatch,
    InscriptionSource, InscriptionTransferItem,
};
use crate::storage::{MinerPassInfo, MinerPassStorage, MinerPassStorageRef, PassEnergyStorage};
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
    updates: Mutex<Vec<(Option<u32>, Option<u32>, Option<String>)>>,
}

impl MockStatus {
    fn new(latest_height: u32) -> Self {
        Self {
            latest_height: AtomicU32::new(latest_height),
            updates: Mutex::new(Vec::new()),
        }
    }

    fn update_count(&self) -> usize {
        self.updates.lock().unwrap().len()
    }
}

impl IndexStatusApi for MockStatus {
    fn latest_depend_synced_block_height(&self) -> u32 {
        self.latest_height.load(Ordering::SeqCst)
    }

    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        self.updates.lock().unwrap().push((current, total, message));
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

    fn add_call_count(&self) -> usize {
        self.added.lock().unwrap().len()
    }

    fn commit_call_count(&self) -> u32 {
        self.commit_calls.load(Ordering::SeqCst)
    }

    fn rollback_call_count(&self) -> u32 {
        self.rollback_calls.load(Ordering::SeqCst)
    }
}

impl TransferTrackerApi for MockTransferTracker {
    fn init<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.init_called.store(true, Ordering::SeqCst);
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

fn build_indexer_fixture_with_hint_provider(
    test_name: &str,
    inscription_source: Arc<dyn InscriptionSource>,
    block_hint_provider: Arc<dyn BlockHintProvider>,
    transfer_tracker: Arc<MockTransferTracker>,
    backend_responses: Vec<MockResponse>,
    energy_provider: Arc<dyn crate::index::energy::BalanceProvider>,
) -> IndexerFixture {
    let root_dir = test_root_dir("indexer_behavior", test_name);
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

    let status = Arc::new(MockStatus::new(1_000_000));
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
        .get_pass_energy_at_or_before(&pass_a_id, h100)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_100.state, MinerPassState::Active);
    assert_eq!(energy_a_100.energy, 0);
    let energy_a_101 = fixture
        .pass_energy_manager
        .get_pass_energy_at_or_before(&pass_a_id, h101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_101.state, MinerPassState::Active);
    assert_eq!(energy_a_101.energy, expected_a_101);
    let energy_a_102 = fixture
        .pass_energy_manager
        .get_pass_energy_at_or_before(&pass_a_id, h102)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_102.state, MinerPassState::Dormant);
    assert_eq!(energy_a_102.energy, expected_a_102);
    let energy_a_103 = fixture
        .pass_energy_manager
        .get_pass_energy_at_or_before(&pass_a_id, h103)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_103.state, MinerPassState::Dormant);
    assert_eq!(energy_a_103.energy, expected_a_102);
    let energy_a_104 = fixture
        .pass_energy_manager
        .get_pass_energy_at_or_before(&pass_a_id, h104)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(energy_a_104.state, MinerPassState::Dormant);
    assert_eq!(energy_a_104.energy, expected_a_102);

    let energy_b_104 = fixture
        .pass_energy_manager
        .get_pass_energy_at_or_before(&pass_b_id, h104)
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
