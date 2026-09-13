//! Native bootstrap acceptance using real Core snapshots and the normal indexer/RPC implementations.

use super::test_common::{Fixture, Workspace};
use super::*;
use crate::bootstrap::{
    NativeBootstrapConfig, NativeBootstrapIdentity, NativeBootstrapPhase, prepare_native_bootstrap,
};
use crate::index::BalanceHistoryIndexer;
use crate::service::*;
use crate::status::{SyncPhase, SyncStatusManager};
use bitcoincore_rpc::bitcoin::{ScriptBuf, hashes::Hash};
use std::cell::Cell;
use usdb_util::{ConsensusRpcErrorCode, ToBtcScriptHash};

fn fixture(branch: &str) -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        branch,
    )
}

fn native_config(work: &Workspace, chain: &Fixture, origin: u32) -> Arc<BalanceHistoryConfig> {
    let source = work.0.join("source.dat");
    fs::copy(chain.fixture_dir.join("snapshot.dat"), &source).unwrap();
    let mut cfg = config(&work.0.join("native"), Network::Regtest);
    cfg.bootstrap = Some(NativeBootstrapConfig {
        snapshot_file: source,
        identity: NativeBootstrapIdentity {
            snapshot: serde_json::from_slice(
                &fs::read(chain.fixture_dir.join("identity.json")).unwrap(),
            )
            .unwrap(),
            regtest_checkpoint: Some(
                serde_json::from_slice(
                    &fs::read(chain.fixture_dir.join("checkpoint.json")).unwrap(),
                )
                .unwrap(),
            ),
            origin_height: origin,
            origin_block_hash: chain.blocks[origin as usize].block_hash(),
        },
        import_batch_size: 31,
        replay_batch_size: 1,
    });
    Arc::new(cfg)
}

fn staged_config(cfg: &Arc<BalanceHistoryConfig>) -> Arc<BalanceHistoryConfig> {
    let mut staged = (**cfg).clone();
    staged.root_dir = cfg.root_dir.join("bootstrap-staging");
    Arc::new(staged)
}

fn rpc(cfg: Arc<BalanceHistoryConfig>, db: Arc<BalanceHistoryDB>) -> BalanceHistoryRpcServer {
    let status = Arc::new(SyncStatusManager::new());
    status.set_rpc_alive(true);
    status.update_phase(SyncPhase::Synced, None);
    let height = db.get_btc_block_height().unwrap() as u64;
    status.update_status(height, height, None);
    let (shutdown, _) = tokio::sync::watch::channel(());
    BalanceHistoryRpcServer::new(cfg, "127.0.0.1:0".parse().unwrap(), status, db, shutdown)
}

fn verify_native_downstream_fixture(rpc: &BalanceHistoryRpcServer) {
    let legacy: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/assumeutxo-p5/downstream-inputs.json"
    ))
    .unwrap();
    let hashes: Vec<usdb_util::BtcScriptHash> =
        serde_json::from_value(legacy["script_hashes"].clone()).unwrap();
    let blocks: Vec<_> = [102, 103]
        .into_iter()
        .map(|height| {
            let state_ref = rpc
                .get_state_ref_at_height(GetStateRefAtHeightParams {
                    block_height: height,
                    context: None,
                })
                .unwrap();
            let balances: Vec<_> = rpc
                .get_addresses_balances(GetBalancesParams {
                    script_hashes: hashes.clone(),
                    block_height: Some(height),
                    block_range: None,
                })
                .unwrap()
                .iter()
                .map(|rows| rows.first().map_or(0, |row| row.balance))
                .collect();
            let previous = legacy["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["height"] == height)
                .unwrap();
            serde_json::json!({"height":height, "candidate":{"state_ref":state_ref,
            "block_commit":rpc.get_block_commit(height).unwrap().unwrap(),"balances":balances},
            "full_replay":previous["full_replay"]})
        })
        .collect();
    let value = serde_json::json!({"schema":"assumeutxo-native-downstream:v1", "script_hashes":hashes,"blocks":blocks});
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/assumeutxo-native-downstream.json");
    if std::env::var("USDB_UPDATE_NATIVE_DOWNSTREAM_FIXTURE").as_deref() == Ok("1") {
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
    }
    let expected: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value, expected);
}

#[test]
fn native_bootstrap_import_seal_restart_and_normal_replay_without_old_snapshots() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let cfg = native_config(&work, &chain, 102);
    let state = prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(state.phase, NativeBootstrapPhase::Sealed);
    assert!(state.source_verification.is_some());
    assert!(!work.0.join("reference.db").exists());
    assert!(!cfg.root_dir.join("input.json").exists());
    let expected = state.origin_commit.clone();
    {
        let db =
            Arc::new(BalanceHistoryDB::open(cfg.clone(), BalanceHistoryDBMode::Normal).unwrap());
        assert!(db.get_assumeutxo_import_state().unwrap().is_none());
        assert!(db.get_snapshot_install_provenance().unwrap().is_none());
        assert!(db.get_block_commit(101).unwrap().is_none());
        assert_eq!(db.get_query_retention_floors().unwrap(), (102, 103));
        assert_eq!(db.block_commit_protocol_version().unwrap(), "1.0.0");
        let rpc = rpc(cfg.clone(), db.clone());
        assert!(rpc.get_readiness().unwrap().consensus_ready);
        assert_eq!(
            rpc.get_snapshot_info().unwrap().commit_protocol_version,
            "1.0.0"
        );
        assert_eq!(
            rpc.get_bootstrap_info().unwrap().unwrap().origin_commit,
            expected
        );
        assert!(
            !rpc.get_readiness()
                .unwrap()
                .script_registry
                .capabilities
                .script_registry_complete_coverage
        );
        assert_eq!(
            rpc.get_block_commit(102).unwrap().unwrap().block_commit,
            expected.clone().unwrap()
        );
        let below = rpc
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 101,
                context: None,
            })
            .unwrap_err();
        assert_eq!(
            below.code,
            jsonrpc_core::ErrorCode::ServerError(ConsensusRpcErrorCode::StateNotRetained.code())
        );
    }
    // Source removal does not prevent restart or recovery after seal-before-rename interruption.
    fs::remove_file(&cfg.bootstrap.as_ref().unwrap().snapshot_file).unwrap();
    fs::rename(
        cfg.db_dir().join("balance_history"),
        staged_config(&cfg).db_dir().join("balance_history"),
    )
    .unwrap();
    assert_eq!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
            .unwrap()
            .unwrap()
            .origin_commit,
        expected
    );
    assert_eq!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
            .unwrap()
            .unwrap()
            .origin_commit,
        expected
    );
    let output = Arc::new(crate::output::IndexOutput::new(Arc::new(
        SyncStatusManager::new(),
    )));
    let indexer = BalanceHistoryIndexer::new_with_btc_client(
        cfg.clone(),
        output,
        chain.client_with_stable_tip(),
    )
    .unwrap();
    assert_eq!(indexer.sync_once().unwrap(), 103);
    let db = indexer.db();
    assert_eq!(db.get_btc_block_height().unwrap(), 103);
    let rpc = rpc(cfg.clone(), db.clone());
    let state_ref = rpc
        .get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 103,
            context: None,
        })
        .unwrap();
    assert_eq!(state_ref.commit_protocol_version, "1.0.0");
    verify_native_downstream_fixture(&rpc);
    let native_state = db
        .bootstrap_origin_identity(Network::Regtest, 103, chain.blocks[103].block_hash())
        .unwrap();
    let ref_root = work.0.join("full-replay");
    chain.reference(&ref_root, &work.0.join("oracle.db"));
    let full =
        BalanceHistoryDB::open_read_only(Arc::new(config(&ref_root, Network::Regtest))).unwrap();
    let full_state = full
        .bootstrap_origin_identity(Network::Regtest, 103, chain.blocks[103].block_hash())
        .unwrap();
    assert_eq!(native_state, full_state);
    for height in 102..=103 {
        assert_eq!(
            db.get_block_commit(height).unwrap(),
            full.get_block_commit(height).unwrap()
        );
    }
    assert_ne!(state.origin_state_digest, state.origin_commit);
    assert_eq!(
        chain
            .fallback_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn native_bootstrap_resumes_import_and_replay_without_publishing_partial_state() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let cfg = native_config(&work, &chain, 103);
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client(), &|| false)
            .unwrap_err()
            .contains("not stable yet")
    );
    assert!(!staged_config(&cfg).db_dir().exists());
    let calls = Cell::new(0);
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| {
            calls.set(calls.get() + 1);
            calls.get() > 4
        })
        .unwrap_err()
        .contains("cancelled")
    );
    assert!(!cfg.db_dir().join("balance_history").exists());
    {
        let staged = staged_config(&cfg);
        let db = Arc::new(BalanceHistoryDB::open_read_only(staged.clone()).unwrap());
        let saved = db.get_native_bootstrap_state().unwrap().unwrap();
        assert_eq!(saved.phase, NativeBootstrapPhase::Importing);
        assert!(saved.imported_coins > 0);
        let server = rpc(staged, db);
        let readiness = server.get_readiness().unwrap();
        assert!(!readiness.query_ready);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::NativeBootstrapNotReady)
        );
        assert!(server.get_snapshot_info().is_err());
    }
    let mut resumed = (*cfg).clone();
    resumed.bootstrap.as_mut().unwrap().import_batch_size = 47;
    let cfg = Arc::new(resumed);
    let progress = cfg.root_dir.join("bootstrap-progress.json");
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| {
            fs::read(&progress)
                .ok()
                .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
                .is_some_and(|v| v["phase"] == "replaying" && v["height"] == 102)
        })
        .unwrap_err()
        .contains("cancelled")
    );
    assert!(!cfg.db_dir().join("balance_history").exists());
    {
        let staged = BalanceHistoryDB::open_read_only(staged_config(&cfg)).unwrap();
        assert_eq!(staged.get_btc_block_height().unwrap(), 102);
    }
    prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false).unwrap();
    let db = BalanceHistoryDB::open_read_only(cfg).unwrap();
    assert_eq!(db.get_btc_block_height().unwrap(), 103);
}

#[test]
fn native_bootstrap_rejects_changed_identity_prefix_and_balances_with_equal_totals() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let cfg = native_config(&work, &chain, 103);
    let calls = Cell::new(0);
    prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| {
        calls.set(calls.get() + 1);
        calls.get() > 4
    })
    .unwrap_err();
    let mut wrong = (*cfg).clone();
    wrong
        .bootstrap
        .as_mut()
        .unwrap()
        .identity
        .snapshot
        .file_sha256 = "11".repeat(32);
    assert!(
        prepare_native_bootstrap(Arc::new(wrong), chain.client_with_stable_tip(), &|| false)
            .unwrap_err()
            .contains("identity mismatch")
    );
    {
        let db = BalanceHistoryDB::open(staged_config(&cfg), BalanceHistoryDBMode::Normal).unwrap();
        let mut first = None;
        scan_snapshot(
            &cfg.bootstrap.as_ref().unwrap().snapshot_file,
            &cfg.bootstrap.as_ref().unwrap().identity.snapshot,
            31,
            |coins, _| {
                if first.is_none() {
                    first = Some(coins[0].clone());
                }
                Ok(())
            },
        )
        .unwrap();
        let coin = first.unwrap();
        db.put_utxos(&[usdb_util::UTXOEntry {
            outpoint: coin.outpoint,
            script_hash: coin.script.to_btc_script_hash(),
            value: coin.value + 1,
        }])
        .unwrap();
        db.flush_all().unwrap();
    }
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
            .unwrap_err()
            .contains("prefix differs")
    );
    assert!(!cfg.db_dir().join("balance_history").exists());

    let work = Workspace::new();
    let cfg = native_config(&work, &chain, 103);
    let progress = cfg.root_dir.join("bootstrap-progress.json");
    prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| {
        fs::read(&progress)
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
            .is_some_and(|v| v["height"] == 103)
    })
    .unwrap_err();
    {
        let db = BalanceHistoryDB::open(staged_config(&cfg), BalanceHistoryDBMode::Normal).unwrap();
        let a = ScriptBuf::from_bytes(vec![0x51]).to_btc_script_hash();
        let b = ScriptBuf::from_bytes(vec![0x51, 0x51]).to_btc_script_hash();
        let av = db.get_latest_balance(&a).unwrap().balance;
        let bv = db.get_latest_balance(&b).unwrap().balance;
        assert!(av > 0);
        db.put_address_history_async(&vec![
            crate::BalanceHistoryEntry {
                script_hash: a,
                block_height: 103,
                balance: av - 1,
                delta: 0,
            },
            crate::BalanceHistoryEntry {
                script_hash: b,
                block_height: 103,
                balance: bv + 1,
                delta: 0,
            },
        ])
        .unwrap();
        db.flush_all().unwrap();
    }
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
            .unwrap_err()
            .contains("per-script balance verification failed")
    );
    assert!(!cfg.db_dir().join("balance_history").exists());
}

#[test]
fn native_bootstrap_reorg_restart_and_legacy_kind_are_explicit() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let fork = fixture("fork_blocks");
    let cfg = native_config(&work, &chain, 101);
    prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false).unwrap();
    let output = || {
        Arc::new(crate::output::IndexOutput::new(Arc::new(
            SyncStatusManager::new(),
        )))
    };
    {
        let indexer = BalanceHistoryIndexer::new_with_btc_client(
            cfg.clone(),
            output(),
            chain.client_with_stable_tip(),
        )
        .unwrap();
        assert_eq!(indexer.sync_once().unwrap(), 103);
    }
    {
        let indexer = BalanceHistoryIndexer::new_with_btc_client(
            cfg.clone(),
            output(),
            fork.client_with_stable_tip(),
        )
        .unwrap();
        assert_eq!(indexer.sync_once().unwrap(), 104);
        assert_eq!(
            indexer
                .db()
                .get_block_commit(104)
                .unwrap()
                .unwrap()
                .btc_block_hash,
            fork.blocks[104].block_hash()
        );
        indexer.db().rollback_to_block_height(101).unwrap();
        assert!(indexer.db().rollback_to_block_height(100).is_err());
    }
    let mut old_config = (*cfg).clone();
    {
        let deep = fixture("deep_fork_blocks");
        let indexer = BalanceHistoryIndexer::new_with_btc_client(
            cfg.clone(),
            output(),
            deep.client_with_stable_tip(),
        )
        .unwrap();
        assert!(
            indexer
                .sync_once()
                .unwrap_err()
                .contains("crosses native bootstrap origin")
        );
    }
    old_config.bootstrap = None;
    assert!(BalanceHistoryDB::open(Arc::new(old_config), BalanceHistoryDBMode::Normal).is_err());
    let old_root = work.0.join("legacy");
    let old_config = Arc::new(config(&old_root, Network::Regtest));
    drop(BalanceHistoryDB::open(old_config, BalanceHistoryDBMode::Normal).unwrap());
    let mut wrong = (*cfg).clone();
    wrong.root_dir = old_root;
    assert!(
        prepare_native_bootstrap(Arc::new(wrong), chain.client_with_stable_tip(), &|| false)
            .is_err()
    );
}

#[test]
fn native_bootstrap_rejects_corrupt_source_before_publication() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let cfg = native_config(&work, &chain, 103);
    let source = &cfg.bootstrap.as_ref().unwrap().snapshot_file;
    let mut bytes = fs::read(source).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(source, bytes).unwrap();
    assert!(
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false).is_err()
    );
    assert!(!cfg.db_dir().join("balance_history").exists());
    let staged = BalanceHistoryDB::open_read_only(staged_config(&cfg)).unwrap();
    let state = staged.get_native_bootstrap_state().unwrap().unwrap();
    assert_eq!(state.phase, NativeBootstrapPhase::Importing);
    assert!(state.source_verification.is_none());
    assert!(staged.validate_native_bootstrap_service().is_err());
}

#[test]
fn native_bootstrap_different_snapshot_heights_preserve_genesis_and_later_commits() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let reference = work.0.join("full-replay");
    chain.reference(&reference, &work.0.join("oracle.db"));
    let full =
        BalanceHistoryDB::open_read_only(Arc::new(config(&reference, Network::Regtest))).unwrap();
    let mut state_digests = Vec::new();
    for base in [101, 102] {
        let branch = Workspace::new();
        let mut cfg = (*native_config(&branch, &chain, 102)).clone();
        if base == 102 {
            let options = cfg.bootstrap.as_mut().unwrap();
            fs::copy(
                chain.fixture_dir.join("base-102/snapshot.dat"),
                &options.snapshot_file,
            )
            .unwrap();
            options.identity.snapshot = serde_json::from_slice(
                &fs::read(chain.fixture_dir.join("base-102/identity.json")).unwrap(),
            )
            .unwrap();
            let checkpoint = crate::bootstrap::inspect_bootstrap_checkpoint(
                &reference,
                options.identity.snapshot.clone(),
            )
            .unwrap();
            options.identity.regtest_checkpoint = Some(checkpoint);
        }
        let cfg = Arc::new(cfg);
        let state =
            prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
                .unwrap()
                .unwrap();
        assert_eq!(state.checkpoint.snapshot.base_height, base);
        assert_eq!(
            state.origin_commit.as_deref(),
            Some(
                crate::assumeutxo::format::hex(
                    &full.get_block_commit(102).unwrap().unwrap().block_commit
                )
                .as_str()
            )
        );
        state_digests.push(state.origin_state_digest);
        let output = Arc::new(crate::output::IndexOutput::new(Arc::new(
            SyncStatusManager::new(),
        )));
        let indexer =
            BalanceHistoryIndexer::new_with_btc_client(cfg, output, chain.client_with_stable_tip())
                .unwrap();
        assert_eq!(indexer.sync_once().unwrap(), 103);
        for height in 102..=103 {
            assert_eq!(
                indexer.db().get_block_commit(height).unwrap(),
                full.get_block_commit(height).unwrap()
            );
        }
        let actual = indexer
            .db()
            .bootstrap_origin_identity(Network::Regtest, 103, chain.blocks[103].block_hash())
            .unwrap();
        let expected = full
            .bootstrap_origin_identity(Network::Regtest, 103, chain.blocks[103].block_hash())
            .unwrap();
        assert_eq!(actual, expected);
    }
    assert_eq!(state_digests[0], state_digests[1]);
}

#[test]
fn native_bootstrap_rejects_changed_checkpoint_and_superseded_v2_metadata() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let cfg = native_config(&work, &chain, 102);
    let state = prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
        .unwrap()
        .unwrap();
    let mut changed = (*cfg).clone();
    changed
        .bootstrap
        .as_mut()
        .unwrap()
        .identity
        .regtest_checkpoint
        .as_mut()
        .unwrap()
        .block_commit = "11".repeat(32);
    assert!(
        prepare_native_bootstrap(Arc::new(changed), chain.client_with_stable_tip(), &|| false)
            .unwrap_err()
            .contains("configuration identity differs")
    );
    let mut old = serde_json::to_value(&state).unwrap();
    old["schema_version"] = "balance-history-native-bootstrap:v1".into();
    old["commit_protocol_version"] = "2.0.0".into();
    let old: crate::bootstrap::NativeBootstrapState = serde_json::from_value(old).unwrap();
    assert!(
        old.validate()
            .unwrap_err()
            .contains("Unsupported native bootstrap")
    );
    let mut changed = state;
    changed.checkpoint.block_commit = "11".repeat(32);
    assert!(changed.validate().is_err());
}
