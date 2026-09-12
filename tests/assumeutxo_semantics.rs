//! P5 cross-module acceptance using real Core blocks, RPC handlers and replacement branches.

use super::test_common as common;
use super::*;
use crate::service::*;
use crate::status::{SyncPhase, SyncStatusManager};
use bitcoincore_rpc::bitcoin::{ScriptBuf, hashes::Hash};
use common::{Fixture, Workspace};
use usdb_util::{BtcScriptHash, ConsensusRpcErrorCode, ToBtcScriptHash};

fn fixture(branch: &str) -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        branch,
    )
}

fn server(
    root: &Path,
) -> (
    Arc<BalanceHistoryDB>,
    BalanceHistoryRpcServer,
    Arc<SyncStatusManager>,
) {
    let cfg = Arc::new(config(root, Network::Regtest));
    let db = Arc::new(BalanceHistoryDB::open(cfg.clone(), BalanceHistoryDBMode::Normal).unwrap());
    let status = Arc::new(SyncStatusManager::new());
    // In-process RPC contract harness; no listener or production readiness is claimed.
    status.set_rpc_alive(true);
    status.update_phase(SyncPhase::Synced, None);
    let tip = db.get_btc_block_height().unwrap() as u64;
    status.update_status(tip, tip, None);
    let (shutdown, _) = tokio::sync::watch::channel(());
    let rpc = BalanceHistoryRpcServer::new(
        cfg,
        "127.0.0.1:0".parse().unwrap(),
        status.clone(),
        db.clone(),
        shutdown,
    );
    (db, rpc, status)
}

fn json(value: impl Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

/// Capture real handler outputs for the downstream crate, which cannot depend on BH test internals.
/// Normal runs regenerate and compare this fixture; updating it is an explicit operator action.
fn verify_downstream_fixture(candidate: &BalanceHistoryRpcServer, full: &BalanceHistoryRpcServer) {
    let hashes: Vec<_> = [
        vec![0x51],
        vec![0x51, 0x51],
        vec![0x52],
        vec![0x51, 0x75, 0x51],
    ]
    .into_iter()
    .map(|script| ScriptBuf::from_bytes(script).to_btc_script_hash())
    .collect();
    let inputs: Vec<_> = (101..=103).map(|height| {
        let read = |rpc: &BalanceHistoryRpcServer| {
            let state_ref = rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: height, context: None,
            }).unwrap();
            let balances = rpc.get_addresses_balances(GetBalancesParams {
                script_hashes: hashes.clone(), block_height: Some(height), block_range: None,
            }).unwrap().iter().map(|rows| rows.first().map_or(0, |row| row.balance)).collect::<Vec<_>>();
            serde_json::json!({"state_ref":state_ref,
                "block_commit":rpc.get_block_commit(height).unwrap().unwrap(), "balances":balances})
        };
        serde_json::json!({"height":height,"candidate":read(candidate),"full_replay":read(full)})
    }).collect();
    let value = serde_json::json!({"schema":"assumeutxo-p5-downstream-inputs:v1",
        "scope":"real BH RPC outputs; downstream pass mutation stream is a separate deterministic test scenario",
        "script_hashes":hashes,"blocks":inputs});
    let path = fixture("blocks").fixture_dir.join("downstream-inputs.json");
    if std::env::var_os("USDB_P5_UPDATE_FIXTURE").as_deref() == Some(std::ffi::OsStr::new("1")) {
        write_report(&path, &value).unwrap();
    } else {
        let expected: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value, expected);
    }
}

#[test]
fn assumeutxo_p5_read_only_audit() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let options = chain.options(&work.0);
    let root = work.0.join("candidate");
    let _lock = AssumeUtxoWorkspaceLock::acquire(&root, true).unwrap();
    import_snapshot(&root, &options, 31).unwrap();
    replay_snapshot(&root, chain.client(), 103, 2).unwrap();
    let report = audit_snapshot(&root, 1).unwrap();
    assert_eq!(report["passed"], true);
    assert_eq!(report["target_height"], 103);
    assert_eq!(report["commits_checked"], 3);
    assert_eq!(report["live_service_readiness_checked"], false);
    let (_, rpc, _) = server(&root.join("state"));
    let (_, full, _) = server(&work.0.join("full-replay"));
    verify_downstream_fixture(&rpc, &full);
}

#[test]
fn assumeutxo_p5_registry_does_not_claim_complete_historical_coverage() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let options = chain.options(&work.0);
    let root = work.0.join("candidate");
    let _lock = AssumeUtxoWorkspaceLock::acquire(&root, true).unwrap();
    import_snapshot(&root, &options, 31).unwrap();
    let (_, rpc, _) = server(&root.join("state"));
    let old = ScriptBuf::from_bytes(vec![0x51, 0x61]).to_btc_script_hash();
    let live = ScriptBuf::from_bytes(vec![0x51, 0x51]).to_btc_script_hash();
    let result = rpc
        .resolve_script_hashes(ResolveScriptHashesParams {
            script_hashes: vec![old, live, old],
            include_script_pubkey: Some(true),
        })
        .unwrap();
    assert!(
        !result
            .registry
            .capabilities
            .script_registry_complete_coverage
    );
    assert_eq!(
        result.registry.coverage_mode,
        ScriptRegistryCoverageMode::PostSnapshotOnly
    );
    assert_eq!(result.registry.base_height, Some(101));
    result.registry.validate().unwrap();
    assert_eq!(
        result.items[0].status,
        ScriptHashResolutionStatus::Unresolved
    );
    assert_eq!(
        result.items[1].status,
        ScriptHashResolutionStatus::FoundOverlay
    );
    assert_eq!(
        result.items[2].status,
        ScriptHashResolutionStatus::Unresolved
    );
    let (_, full, _) = server(&work.0.join("full-replay"));
    assert_eq!(
        full.resolve_script_hashes(ResolveScriptHashesParams {
            script_hashes: vec![old],
            include_script_pubkey: Some(true)
        })
        .unwrap()
        .items[0]
            .status,
        ScriptHashResolutionStatus::FoundOverlay
    );
}

#[test]
fn assumeutxo_p5_real_forks_restart_and_baseline_rollback_limit() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let options = chain.options(&work.0);
    let root = work.0.join("candidate");
    let _lock = AssumeUtxoWorkspaceLock::acquire(&root, true).unwrap();
    import_snapshot(&root, &options, 31).unwrap();
    replay_snapshot(&root, chain.client(), 103, 2).unwrap();
    let alternate = fixture("fork_blocks");
    let deep = fixture("deep_fork_blocks");
    let alternate_core = work.0.join("alternate-core.db");
    alternate.reference(&work.0.join("alternate-full"), &alternate_core);
    {
        let (db, rpc, _) = server(&root.join("state"));
        let original = rpc
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 103,
                context: None,
            })
            .unwrap();
        // Offline replay intentionally refuses a changed saved branch rather than silently resetting data.
        let cfg = Arc::new(config(&root.join("state"), Network::Regtest));
        let processor = BatchBlockProcessor::new(
            alternate.client(),
            db.clone(),
            Arc::new(UTXOCache::new(cfg.clone(), CacheStrategy::Normal)),
            Arc::new(AddressBalanceCache::new(cfg, CacheStrategy::Normal)),
        );
        assert!(processor.process_blocks(104..105, 104, 288).is_err());
        assert_eq!(db.get_btc_block_height().unwrap(), 103);
        assert_eq!(
            json(original),
            json(
                rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
                    block_height: 103,
                    context: None
                })
                .unwrap()
            )
        );
        db.rollback_to_block_height(101).unwrap();
        db.flush_all().unwrap();
    }
    // Reopen after rollback before processing a real replacement branch with fresh caches.
    {
        let (db, rpc, _) = server(&root.join("state"));
        let cfg = Arc::new(config(&root.join("state"), Network::Regtest));
        let processor = BatchBlockProcessor::new(
            alternate.client(),
            db.clone(),
            Arc::new(UTXOCache::new(cfg.clone(), CacheStrategy::Normal)),
            Arc::new(AddressBalanceCache::new(cfg, CacheStrategy::Normal)),
        );
        processor.process_blocks(102..105, 104, 288).unwrap();
        db.flush_all().unwrap();
        assert!(
            db.compare_assumeutxo_core(&alternate_core, 104)
                .unwrap()
                .equal
        );
        let (_, full, _) = server(&work.0.join("alternate-full"));
        assert_eq!(
            json(
                rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
                    block_height: 104,
                    context: None
                })
                .unwrap()
            ),
            json(
                full.get_state_ref_at_height(GetStateRefAtHeightParams {
                    block_height: 104,
                    context: None
                })
                .unwrap()
            )
        );
        let before = db.get_block_commit(104).unwrap();
        assert!(db.rollback_to_block_height(100).is_err());
        assert_eq!(db.get_btc_block_height().unwrap(), 104);
        assert_eq!(before, db.get_block_commit(104).unwrap());
        let cfg = Arc::new(config(&root.join("state"), Network::Regtest));
        let processor = BatchBlockProcessor::new(
            deep.client(),
            db.clone(),
            Arc::new(UTXOCache::new(cfg.clone(), CacheStrategy::Normal)),
            Arc::new(AddressBalanceCache::new(cfg, CacheStrategy::Normal)),
        );
        assert!(processor.process_blocks(105..106, 105, 288).is_err());
        assert_eq!(db.get_btc_block_height().unwrap(), 104);
    }
    let (_, rpc, _) = server(&root.join("state"));
    assert_eq!(rpc.get_snapshot_info().unwrap().stable_height, 104);
}

#[test]
fn assumeutxo_p5_rpc_queries_state_refs_and_recovery_gates() {
    let work = Workspace::new();
    let chain = fixture("blocks");
    let options = chain.options(&work.0);
    let root = work.0.join("candidate");
    let _lock = AssumeUtxoWorkspaceLock::acquire(&root, true).unwrap();
    import_snapshot(&root, &options, 31).unwrap();
    replay_snapshot(&root, chain.client(), 103, 2).unwrap();
    let (db, rpc, status) = server(&root.join("state"));
    let (_, full, _) = server(&work.0.join("full-replay"));
    let mut hashes: Vec<_> = chain
        .blocks
        .iter()
        .flat_map(|b| b.txdata.iter())
        .flat_map(|tx| tx.output.iter())
        .map(|o| o.script_pubkey.to_btc_script_hash())
        .collect();
    hashes.push(BtcScriptHash::from_byte_array([0x99; 32]));
    hashes.sort();
    hashes.dedup();
    for hash in hashes {
        for height in 101..=103 {
            let params = GetBalanceParams {
                script_hash: hash,
                block_height: Some(height),
                block_range: None,
            };
            let actual = rpc.get_address_balance(params.clone()).unwrap();
            let expected = full.get_address_balance(params).unwrap();
            assert_eq!(actual.len(), expected.len());
            for (a, b) in actual.iter().zip(&expected) {
                assert_eq!(a.balance, b.balance);
                if b.block_height > 101 {
                    assert_eq!(json(a), json(b));
                }
            }
            if height > 101 {
                let params = GetBalanceParams {
                    script_hash: hash,
                    block_height: Some(height),
                    block_range: None,
                };
                assert_eq!(
                    json(rpc.get_address_balance_delta(params.clone()).unwrap()),
                    json(full.get_address_balance_delta(params).unwrap())
                );
            }
        }
        let range = GetBalanceParams {
            script_hash: hash,
            block_height: None,
            block_range: Some(102..104),
        };
        assert_eq!(
            json(rpc.get_address_balance(range.clone()).unwrap()),
            json(full.get_address_balance(range).unwrap())
        );
        let summary = GetAddressBalanceSummaryParams {
            script_hash: hash,
            block_range: 102..104,
        };
        assert_eq!(
            json(rpc.get_address_balance_summary(summary.clone()).unwrap()),
            json(full.get_address_balance_summary(summary).unwrap())
        );
        let buckets = GetAddressBalanceBucketsParams {
            script_hash: hash,
            block_range: 102..104,
            bucket_size: 1,
        };
        assert_eq!(
            json(rpc.get_address_balance_timeseries(buckets.clone()).unwrap()),
            json(
                full.get_address_balance_timeseries(buckets.clone())
                    .unwrap()
            )
        );
        assert_eq!(
            json(rpc.get_address_flow_buckets(buckets.clone()).unwrap()),
            json(full.get_address_flow_buckets(buckets).unwrap())
        );
    }
    let hash = ScriptBuf::from_bytes(vec![0x51, 0x51]).to_btc_script_hash();
    let baseline = rpc
        .get_address_balance(GetBalanceParams {
            script_hash: hash,
            block_height: Some(101),
            block_range: None,
        })
        .unwrap();
    assert_eq!(baseline[0].balance, 5_000_000_000);
    assert_eq!((baseline[0].block_height, baseline[0].delta), (101, 0));
    for (point, range) in [(Some(100), None), (None, Some(101..104))] {
        assert_eq!(
            rpc.get_address_balance(GetBalanceParams {
                script_hash: hash,
                block_height: point,
                block_range: range
            })
            .unwrap_err()
            .message,
            ConsensusRpcErrorCode::StateNotRetained.as_str()
        );
    }
    assert_eq!(
        rpc.get_address_balance_delta(GetBalanceParams {
            script_hash: hash,
            block_height: Some(101),
            block_range: None
        })
        .unwrap_err()
        .message,
        ConsensusRpcErrorCode::StateNotRetained.as_str()
    );
    assert_eq!(
        rpc.get_address_balance_summary(GetAddressBalanceSummaryParams {
            script_hash: hash,
            block_range: 101..104
        })
        .unwrap_err()
        .message,
        ConsensusRpcErrorCode::StateNotRetained.as_str()
    );
    assert_eq!(
        rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 100,
            context: None
        })
        .unwrap_err()
        .message,
        ConsensusRpcErrorCode::StateNotRetained.as_str()
    );
    for height in 101..=103 {
        let params = GetStateRefAtHeightParams {
            block_height: height,
            context: None,
        };
        assert_eq!(
            json(rpc.get_state_ref_at_height(params.clone()).unwrap()),
            json(full.get_state_ref_at_height(params).unwrap())
        );
    }
    let snapshot = rpc.get_snapshot_info().unwrap();
    // Preserve ordered, repeated and missing script selectors in batch responses.
    let selected = vec![hash, BtcScriptHash::from_byte_array([0x99; 32]), hash];
    let batch = rpc
        .get_addresses_balances(GetBalancesParams {
            script_hashes: selected.clone(),
            block_height: Some(103),
            block_range: None,
        })
        .unwrap();
    for (item, selected_hash) in batch.iter().zip(&selected) {
        assert_eq!(
            json(item),
            json(
                rpc.get_address_balance(GetBalanceParams {
                    script_hash: *selected_hash,
                    block_height: Some(103),
                    block_range: None,
                })
                .unwrap()
            )
        );
    }
    assert_eq!(batch.len(), selected.len());
    let exact = rpc
        .get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 103,
            context: None,
        })
        .unwrap();
    let mut context = usdb_util::ConsensusQueryContext {
        requested_height: Some(103),
        expected_state: (&exact).into(),
    };
    assert!(
        rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 103,
            context: Some(context.clone()),
        })
        .is_ok()
    );
    context.expected_state.snapshot_id = Some("00".repeat(32));
    assert_eq!(
        rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 103,
            context: Some(context),
        })
        .unwrap_err()
        .message,
        ConsensusRpcErrorCode::SnapshotIdMismatch.as_str()
    );

    let exporter = crate::index::SnapshotIndexer::new(
        Arc::new(config(&root.join("state"), Network::Regtest)),
        db.clone(),
        Arc::new(crate::output::IndexOutput::new(status.clone())),
    );
    let output = work.0.join("unsupported-core.db");
    assert!(
        exporter
            .run_core_to_path(103, &output)
            .unwrap_err()
            .contains("AssumeUTXO sources are unsupported")
    );
    assert!(!output.exists());
    assert_eq!(
        snapshot.stable_lag,
        full.get_snapshot_info().unwrap().stable_lag
    );
    assert_eq!(
        (snapshot.balance_query_floor, snapshot.history_query_floor),
        (101, 102)
    );
    status.set_rollback_in_progress(true);
    assert!(!rpc.get_readiness().unwrap().query_ready);
    assert_eq!(
        rpc.get_address_balance(GetBalanceParams {
            script_hash: hash,
            block_height: Some(103),
            block_range: None
        })
        .unwrap_err()
        .message,
        ConsensusRpcErrorCode::SnapshotNotReady.as_str()
    );
    status.set_rollback_in_progress(false);
    status.set_shutdown_requested(true);
    assert!(!rpc.get_readiness().unwrap().query_ready);
    status.set_shutdown_requested(false);
    assert_eq!(db.get_btc_block_height().unwrap(), 103);
}
