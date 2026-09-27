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
fn ordinary_service_rpc_clients_do_not_acquire_bootstrap_retries() {
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
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
