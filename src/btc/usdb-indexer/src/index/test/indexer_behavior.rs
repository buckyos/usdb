use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};
use crate::balance::{BalanceMonitor, MockBalanceBackend, MockResponse, SerialBalanceLoader};
use crate::config::ConfigManager;
use crate::index::content::{MinerPassState, USDBInscription, USDBMint};
use crate::index::energy::PassEnergyManager;
use crate::index::pass::MinerPassManager;
use crate::index::transfer::InscriptionCreateInfo;
use crate::index::{BlockHintProvider, IndexStatusApi, InscriptionIndexer, TransferTrackerApi};
use crate::inscription::{
    DiscoveredInscription, DiscoveredMint, InscriptionSource, InscriptionTransferItem,
};
use crate::storage::{MinerPassInfo, MinerPassStorage, MinerPassStorageRef, PassEnergyStorage};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{
    Amount, Block, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute, constants, transaction,
};
use ord::InscriptionId;
use std::collections::HashMap;
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
}

#[derive(Default)]
struct MockTransferTracker {
    create_infos: HashMap<String, MockCreateInfo>,
    transfers_by_height: HashMap<u32, Vec<InscriptionTransferItem>>,
    added: Mutex<Vec<(InscriptionId, USDBScriptHash, ordinals::SatPoint)>>,
    init_called: AtomicBool,
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<InscriptionTransferItem>, String>> + Send + 'a>>
    {
        Box::pin(async move {
            Ok(self
                .transfers_by_height
                .get(&block_height)
                .cloned()
                .unwrap_or_default())
        })
    }
}

#[derive(Default)]
struct MockInscriptionSource {
    mints_by_height: HashMap<u32, Vec<DiscoveredMint>>,
}

impl MockInscriptionSource {
    fn with_mints(mut self, block_height: u32, mints: Vec<DiscoveredMint>) -> Self {
        self.mints_by_height.insert(block_height, mints);
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

#[tokio::test]
async fn test_sync_block_without_events_still_settles_balance_snapshot() {
    let owner = test_script_hash(1);
    let existing_pass_id = test_inscription_id(1, 0);

    let inscription_source: Arc<dyn InscriptionSource> = Arc::new(MockInscriptionSource::default());
    let transfer_tracker = Arc::new(MockTransferTracker::default());

    let fixture = build_indexer_fixture(
        "sync_block_empty_still_settle",
        inscription_source,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height: 120,
                balance: 5_000,
                delta: 0,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default()),
    );

    fixture
        .storage
        .add_new_mint_pass(&make_active_pass(existing_pass_id, owner, 100))
        .unwrap();

    fixture.indexer.sync_block_for_test(120).await.unwrap();

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(120)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.block_height, 120);
    assert_eq!(snapshot.active_address_count, 1);
    assert_eq!(snapshot.total_balance, 5_000);
    assert_eq!(fixture.backend.call_count(), 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_settle_failure_rolls_back_and_synced_height_not_advance() {
    let owner = test_script_hash(2);
    let mint_id = test_inscription_id(2, 0);

    let mint = make_discovered_mint(mint_id, 200, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(200, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(9, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[9u8; 32]).unwrap(),
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture(
        "sync_blocks_settle_fail_rollback",
        inscription_source,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![]))],
        Arc::new(MockBalanceProvider::default().with_height(owner, 200, 200_000, 100)),
    );

    let err = fixture
        .indexer
        .sync_blocks_for_test(200..=200)
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

    let snapshot = fixture.storage.get_active_balance_snapshot(200).unwrap();
    assert!(snapshot.is_none());

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_partial_commit_then_retry_produces_consistent_result() {
    let owner = test_script_hash(3);
    let mint_id = test_inscription_id(3, 0);

    let mint = make_discovered_mint(mint_id, 301, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(301, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(10, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[10u8; 32]).unwrap(),
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture(
        "sync_blocks_partial_commit_then_retry",
        inscription_source,
        transfer_tracker,
        vec![
            MockResponse::Immediate(Ok(vec![])),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: 301,
                balance: 7_777,
                delta: 1,
            }]])),
        ],
        Arc::new(MockBalanceProvider::default().with_height(owner, 301, 200_000, 100)),
    );

    let first_err = fixture
        .indexer
        .sync_blocks_for_test(300..=301)
        .await
        .unwrap_err();
    assert!(first_err.contains("Address balance batch size mismatch"));

    let synced_after_first = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_after_first, Some(300));

    let snapshot_300 = fixture
        .storage
        .get_active_balance_snapshot(300)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_300.total_balance, 0);
    assert_eq!(snapshot_300.active_address_count, 0);

    let pass_after_first = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap();
    assert!(pass_after_first.is_none());

    let second_ok = fixture
        .indexer
        .sync_blocks_for_test(301..=301)
        .await
        .unwrap();
    assert_eq!(second_ok, 301);

    let synced_after_second = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_after_second, Some(301));

    let pass_after_second = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass_after_second.owner, owner);
    assert_eq!(pass_after_second.state, MinerPassState::Active);

    let snapshot_301 = fixture
        .storage
        .get_active_balance_snapshot(301)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot_301.total_balance, 7_777);
    assert_eq!(snapshot_301.active_address_count, 1);

    cleanup_temp_dir(&fixture.root_dir);
}

#[tokio::test]
async fn test_sync_blocks_single_mint_success_updates_height_and_snapshot() {
    let owner = test_script_hash(4);
    let mint_id = test_inscription_id(4, 0);

    let mint = make_discovered_mint(mint_id, 400, vec![]);
    let inscription_source: Arc<dyn InscriptionSource> =
        Arc::new(MockInscriptionSource::default().with_mints(400, vec![mint]));

    let create_info = MockCreateInfo {
        satpoint: test_satpoint(11, 0, 0),
        value: Amount::from_sat(10_000),
        address: Some(owner),
        commit_txid: Txid::from_slice(&[11u8; 32]).unwrap(),
    };
    let transfer_tracker =
        Arc::new(MockTransferTracker::default().with_create_info(&mint_id, create_info));

    let fixture = build_indexer_fixture(
        "sync_blocks_single_mint_success",
        inscription_source,
        transfer_tracker,
        vec![MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height: 400,
                balance: 8_888,
                delta: 1,
            },
        ]]))],
        Arc::new(MockBalanceProvider::default().with_height(owner, 400, 220_000, 100)),
    );

    let synced = fixture
        .indexer
        .sync_blocks_for_test(400..=400)
        .await
        .unwrap();
    assert_eq!(synced, 400);

    let synced_height = fixture.storage.get_synced_btc_block_height().unwrap();
    assert_eq!(synced_height, Some(400));

    let pass = fixture
        .storage
        .get_pass_by_inscription_id(&mint_id)
        .unwrap()
        .unwrap();
    assert_eq!(pass.owner, owner);
    assert_eq!(pass.state, MinerPassState::Active);

    let snapshot = fixture
        .storage
        .get_active_balance_snapshot(400)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.total_balance, 8_888);
    assert_eq!(snapshot.active_address_count, 1);

    assert_eq!(fixture.transfer_tracker.add_call_count(), 1);
    assert!(fixture.status.update_count() > 0);

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
        prev_satpoint: test_satpoint(7, 0, 0),
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

    fixture
        .storage
        .add_new_mint_pass(&make_active_pass(prev_pass_id.clone(), owner_a, 450))
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
        prev_satpoint: test_satpoint(7, 0, 0),
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

    fixture
        .storage
        .add_new_mint_pass(&make_active_pass(prev_pass_id.clone(), owner_a, 460))
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

    cleanup_temp_dir(&fixture.root_dir);
}
