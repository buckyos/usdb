//! Bootstrap retry acceptance using real snapshot/chain fixtures and HTTP RPC failures.

#[path = "common/native_bootstrap_rpc.rs"]
mod native_rpc;

use super::test_common::{Fixture, Workspace};
use super::*;
use crate::bootstrap::{NativeBootstrapPhase, prepare_native_bootstrap};
use crate::btc::{create_btc_rpc_client, create_native_bootstrap_rpc_client};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn fixture() -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        "blocks",
    )
}

fn connect(
    cfg: &Arc<BalanceHistoryConfig>,
    server: &native_rpc::RpcServer,
) -> Arc<BalanceHistoryConfig> {
    let mut cfg = (**cfg).clone();
    cfg.btc.rpc_url = Some(server.url.clone());
    cfg.btc.auth = Some(usdb_util::BTCAuth::None);
    Arc::new(cfg)
}

fn phase(journal: &[u8]) -> Value {
    serde_json::from_slice::<Value>(journal).unwrap_or(Value::Null)["phase"].clone()
}

#[test]
fn native_rpc_recovers_in_preflight_replay_and_after_verification_without_rescanning() {
    let chain = fixture();
    let reference_work = Workspace::new();
    let reference_cfg = chain.native_config(&reference_work.0, 103);
    let reference =
        prepare_native_bootstrap(reference_cfg, chain.client_with_stable_tip(), &|| false)
            .unwrap()
            .unwrap();

    for (fault_method, fault_phase) in [
        ("getblockcount", Value::Null),
        ("getblockhash", json!("replaying")),
        ("getblock", json!("replaying")),
        ("getblockcount", json!("verifying")),
        ("getblockhash", json!("verifying")),
    ] {
        let work = Workspace::new();
        let cfg = chain.native_config(&work.0, 103);
        let faults = Arc::new(AtomicUsize::new(0));
        let count = faults.clone();
        let saved_journal = Arc::new(Mutex::new(None));
        let saved = saved_journal.clone();
        let fault_label = format!("{fault_method}/{fault_phase}");
        let server = native_rpc::serve(
            chain.blocks.clone(),
            &cfg.root_dir,
            move |request, journal| {
                if let Some(ref expected) = *saved.lock().unwrap() {
                    assert_eq!(
                        journal, expected,
                        "Verification must not restart during RPC recovery"
                    );
                }
                if request["method"] == fault_method
                    && phase(journal) == fault_phase
                    && count.fetch_add(1, Ordering::Relaxed) == 0
                {
                    if fault_phase == "verifying" {
                        *saved.lock().unwrap() = Some(journal.to_vec());
                    }
                    return Some((
                        500,
                        json!({"id":request["id"],"result":null,
                    "error":{"code":-28,"message":"Loading block index"}}),
                    ));
                }
                None
            },
        );
        let cfg = connect(&cfg, &server);
        let cancelled = Arc::new(AtomicBool::new(false));
        let client = create_native_bootstrap_rpc_client(&cfg, cancelled).unwrap();
        let sealed = prepare_native_bootstrap(cfg.clone(), client, &|| false)
            .unwrap()
            .unwrap();
        assert!(
            faults.load(Ordering::Relaxed) >= 2,
            "Fault was not retried: {fault_label}"
        );
        assert_eq!(sealed.phase, NativeBootstrapPhase::Sealed);
        assert_eq!(sealed.origin_commit, reference.origin_commit);
        assert!(
            BalanceHistoryDB::open_read_only(cfg)
                .unwrap()
                .validate_native_bootstrap_service()
                .is_ok()
        );
    }
}

#[test]
fn native_rpc_recovery_does_not_hide_an_origin_change_before_sealing() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = chain.native_config(&work.0, 103);
    let count = Arc::new(AtomicUsize::new(0));
    let requests = count.clone();
    let server = native_rpc::serve(
        chain.blocks.clone(),
        &cfg.root_dir,
        move |request, journal| {
            if phase(journal) != "verifying" {
                return None;
            }
            if request["method"] == "getblockcount" && requests.fetch_add(1, Ordering::Relaxed) == 0
            {
                return Some((503, json!(null)));
            }
            if request["method"] == "getblockhash" {
                return Some((
                    200,
                    json!({"id":request["id"], "result":"00".repeat(32), "error":null}),
                ));
            }
            None
        },
    );
    let cfg = connect(&cfg, &server);
    let client =
        create_native_bootstrap_rpc_client(&cfg, Arc::new(AtomicBool::new(false))).unwrap();
    let error = prepare_native_bootstrap(cfg.clone(), client, &|| false).unwrap_err();
    assert!(
        error.contains("Native origin changed during verification"),
        "{error}"
    );
    assert_eq!(count.load(Ordering::Relaxed), 2);
    assert!(!cfg.db_dir().join("balance_history").exists());
}

#[test]
fn native_rpc_auth_protocol_and_invalid_parameters_fail_without_retry_or_publication() {
    for (status, code, malformed) in [
        (401, None, false),
        (200, Some(-8), false),
        (200, None, true),
    ] {
        let chain = fixture();
        let work = Workspace::new();
        let cfg = chain.native_config(&work.0, 103);
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let server = native_rpc::serve(
            chain.blocks.clone(),
            &cfg.root_dir,
            move |request, journal| {
                if phase(journal) != "verifying" {
                    return None;
                }
                count.fetch_add(1, Ordering::Relaxed);
                Some((
                    status,
                    json!({"id":request["id"],
                "result":if malformed { json!("not a block height") } else { Value::Null },
                "error":code.map(|code| json!({"code":code,"message":"timeout is not a retry classifier"}))}),
                ))
            },
        );
        let cfg = connect(&cfg, &server);
        let client =
            create_native_bootstrap_rpc_client(&cfg, Arc::new(AtomicBool::new(false))).unwrap();
        let error = prepare_native_bootstrap(cfg.clone(), client, &|| false).unwrap_err();
        assert!(error.contains("failed without retry"), "{error}");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!cfg.db_dir().join("balance_history").exists());
    }
}

#[test]
fn native_rpc_cancellation_preserves_unsealed_staging_for_next_start() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = chain.native_config(&work.0, 103);
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |_, journal| {
        if phase(journal) == "verifying" {
            stop.store(true, Ordering::Relaxed);
            return Some((503, json!(null)));
        }
        None
    });
    let cfg = connect(&cfg, &server);
    let client = create_native_bootstrap_rpc_client(&cfg, cancelled.clone()).unwrap();
    let error =
        prepare_native_bootstrap(cfg.clone(), client, &|| cancelled.load(Ordering::Relaxed))
            .unwrap_err();
    assert!(error.contains("RPC cancelled"), "{error}");
    assert!(!cfg.db_dir().join("balance_history").exists());
    let resumed = prepare_native_bootstrap(cfg, chain.client_with_stable_tip(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(resumed.phase, NativeBootstrapPhase::Sealed);
}

#[test]
fn ordinary_service_rpc_clients_use_shorter_runtime_retries() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = chain.native_config(&work.0, 103);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Some((503, json!(null)))
    });
    let cfg = connect(&cfg, &server);
    assert!(
        create_btc_rpc_client(&cfg)
            .unwrap()
            .get_latest_block_height()
            .is_err()
    );
    assert_eq!(calls.load(Ordering::Relaxed), 3);
}

#[test]
fn runtime_final_hash_retry_preserves_batch_work_and_still_rejects_reorg() {
    for changed_hash in [false, true] {
        let chain = fixture();
        let work = Workspace::new();
        let cfg = chain.native_config(&work.0, 101);
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false).unwrap();
        let end = chain.blocks.len() as u32 - 1;
        assert!(end > 101);
        let hash_calls = Arc::new(AtomicUsize::new(0));
        let payload_calls = Arc::new(AtomicUsize::new(0));
        let hashes = hash_calls.clone();
        let payloads = payload_calls.clone();
        let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |request, _| {
            if request["method"] == "getblock" {
                payloads.fetch_add(1, Ordering::Relaxed);
            }
            if request["method"] == "getblockhash" && request["params"][0] == end {
                let call = hashes.fetch_add(1, Ordering::Relaxed);
                if call == 1 {
                    return Some((503, json!(null)));
                }
                if call >= 2 && changed_hash {
                    return Some((
                        200,
                        json!({"id":request["id"],"result":"00".repeat(32),"error":null}),
                    ));
                }
            }
            None
        });
        let cfg = connect(&cfg, &server);
        let client = create_btc_rpc_client(&cfg).unwrap();
        let output = Arc::new(crate::output::IndexOutput::new(Arc::new(
            crate::status::SyncStatusManager::new(),
        )));
        let indexer =
            crate::index::BalanceHistoryIndexer::new_with_btc_client(cfg, output, client.clone())
                .unwrap();
        let result = indexer.process_block_batch(102..end + 1, end);
        if changed_hash {
            assert!(result.unwrap_err().contains("canonical batch end changed"));
            assert_eq!(indexer.db().get_btc_block_height().unwrap(), 101);
        } else {
            assert_eq!(result.unwrap(), end);
            assert_eq!(indexer.db().get_btc_block_height().unwrap(), end);
        }
        assert_eq!(hash_calls.load(Ordering::Relaxed), 3);
        assert_eq!(payload_calls.load(Ordering::Relaxed), (end - 101) as usize);
        assert_eq!(client.read_stats().retries, 1);
    }
}

#[test]
fn runtime_idle_poll_does_not_nest_data_retries() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = chain.native_config(&work.0, 103);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
        Some((503, json!(null)))
    });
    let client = create_btc_rpc_client(&connect(&cfg, &server)).unwrap();
    assert!(client.poll_block_height().is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(client.poll_block_hash(103).is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(client.read_stats().retries, 0);
}

#[test]
fn adaptive_sync_resumes_from_durable_prefix_with_smaller_batches() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = Arc::new(config(&work.0.join("service"), Network::Regtest));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let fault = Arc::new(AtomicBool::new(true));
    let failing = fault.clone();
    let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |request, _| {
        observed.lock().unwrap().push(request.clone());
        if request["method"] == "getblockhash"
            && request["params"][0] == 60
            && failing.load(Ordering::Relaxed)
        {
            return Some((503, json!(null)));
        }
        None
    });
    let cfg = connect(&cfg, &server);
    let client = create_btc_rpc_client(&cfg).unwrap();
    let output = Arc::new(crate::output::IndexOutput::new(Arc::new(
        crate::status::SyncStatusManager::new(),
    )));
    let indexer =
        crate::index::BalanceHistoryIndexer::new_with_btc_client(cfg, output, client.clone())
            .unwrap();
    assert!(indexer.sync_once().is_err());
    assert_eq!(indexer.db().get_btc_block_height().unwrap(), 32);
    assert_eq!(client.read_stats().exhausted, 1);
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r["method"] == "getblockhash" && r["params"][0].as_u64().unwrap() > 60)
    );
    requests.lock().unwrap().clear();
    fault.store(false, Ordering::Relaxed);
    // Let the production shared-read cooldown expire, as the outer sync backoff would.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(indexer.sync_once().unwrap(), 103);
    let requests = requests.lock().unwrap();
    // A 16-block resumed batch ends at 48, and subsequent successful commits end at 64/80/96.
    for end in [48, 64, 80, 96] {
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "getblockhash" && r["params"][0] == end)
                .count(),
            2
        );
    }
    let bodies: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "getblock")
        .collect();
    assert_eq!(bodies.len(), 103 - 32);
    for block in &chain.blocks[1..=32] {
        assert!(
            !bodies
                .iter()
                .any(|r| r["params"][0] == block.block_hash().to_string())
        );
    }
    let reference_root = work.0.join("fixed");
    let mut reference_cfg = config(&reference_root, Network::Regtest);
    reference_cfg.sync.batch_mode = crate::config::BatchMode::Fixed;
    let reference = crate::index::BalanceHistoryIndexer::new_with_btc_client(
        Arc::new(reference_cfg),
        Arc::new(crate::output::IndexOutput::new(Arc::new(
            crate::status::SyncStatusManager::new(),
        ))),
        chain.client_with_stable_tip(),
    )
    .unwrap();
    assert_eq!(reference.sync_once().unwrap(), 103);
    assert_eq!(
        indexer.db().get_block_commit(103).unwrap(),
        reference.db().get_block_commit(103).unwrap()
    );
}

#[test]
fn canonical_range_rejects_reorg_before_commit_in_local_and_rpc_modes() {
    use crate::btc::{BTCClient, CanonicalBlockLoader};
    for (local, drift_during_load) in [(false, true), (true, true), (false, false), (true, false)] {
        let chain = fixture();
        let work = Workspace::new();
        let mut cfg = (*chain.native_config(&work.0, 101)).clone();
        cfg.btc.data_dir = Some(work.0.join("node"));
        cfg.sync.local_loader_threshold = if local { 0 } else { 500 };
        // An empty local index exercises canonical local selection with RPC payload fallback.
        fs::create_dir_all(cfg.btc.data_dir().join("blocks")).unwrap();
        let cfg = Arc::new(cfg);
        prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false).unwrap();
        let hashes = Arc::new(AtomicUsize::new(0));
        let calls = hashes.clone();
        let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |request, _| {
            if request["method"] == "getblockhash" && request["params"][0] == 103 {
                let call = calls.fetch_add(1, Ordering::Relaxed);
                let change_at = if drift_during_load { 1 } else { 2 };
                if call >= change_at {
                    return Some((
                        200,
                        json!({"id":request["id"], "result":"00".repeat(32),"error":null}),
                    ));
                }
            }
            None
        });
        let cfg = connect(&cfg, &server);
        let client = CanonicalBlockLoader::new(create_btc_rpc_client(&cfg).unwrap(), &cfg).unwrap();
        let client = Arc::new(Box::new(client) as Box<dyn BTCClient>);
        let indexer = crate::index::BalanceHistoryIndexer::new_with_btc_client(
            cfg,
            Arc::new(crate::output::IndexOutput::new(Arc::new(
                crate::status::SyncStatusManager::new(),
            ))),
            client,
        )
        .unwrap();
        let error = indexer.process_block_batch(102..104, 103).unwrap_err();
        assert!(
            error.contains(if drift_during_load {
                "changed during retrieval"
            } else {
                "canonical batch end changed"
            }),
            "{error}"
        );
        assert_eq!(indexer.db().get_btc_block_height().unwrap(), 101);
        assert!(indexer.db().get_block_commit(102).unwrap().is_none());
    }
}

#[test]
fn runtime_range_cancellation_stops_before_requesting_the_suffix() {
    let chain = fixture();
    let work = Workspace::new();
    let cfg = Arc::new(config(&work.0, Network::Regtest));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let server = native_rpc::serve(chain.blocks.clone(), &cfg.root_dir, move |request, _| {
        count.fetch_add(1, Ordering::Relaxed);
        if request["method"] == "getblock" {
            entered_tx.send(()).unwrap();
            resume_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        None
    });
    let client = create_btc_rpc_client(&connect(&cfg, &server)).unwrap();
    let worker = client.clone();
    let handle = std::thread::spawn(move || worker.load_block_range(102, 103));
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    client.stop().unwrap();
    resume_tx.send(()).unwrap();
    assert!(handle.join().unwrap().unwrap_err().contains("cancelled"));
    assert_eq!(requests.load(Ordering::Relaxed), 2); // Hash and payload for block 102 only.
}
