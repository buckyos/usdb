//! Real HTTP observations must remain independent of Core latency and fail closed for BH.

use crate::index::test_http_rpc as http_rpc;

use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn snapshot(hash: &str) -> BalanceHistorySnapshotInfo {
    BalanceHistorySnapshotInfo {
        stable_height: 103,
        stable_block_hash: Some(hash.into()),
        latest_block_commit: Some("commit".into()),
        balance_query_floor: 103,
        history_query_floor: 103,
        stable_lag: 10,
        balance_history_api_version: "v1".into(),
        balance_history_semantics_version: "v1".into(),
        commit_protocol_version: "v1".into(),
        commit_hash_algo: "sha256".into(),
    }
}

fn readiness() -> Value {
    json!({
        "service":"balance-history", "rpc_alive":true, "query_ready":true,
        "consensus_ready":true, "phase":"Synced", "current":103, "total":103,
        "stable_height":103, "stable_block_hash":"hash-a", "latest_block_commit":"commit",
        "balance_query_floor":103, "history_query_floor":103, "blockers":[],
        "script_registry": {
            "state":"disabled", "coverage_mode":"post_snapshot_only", "policy":"append-only",
            "capabilities":{"script_registry_lookup":true, "script_registry_complete_coverage":false}
        }
    })
}

fn bh_server(fail: Arc<AtomicBool>) -> http_rpc::RpcServer {
    http_rpc::RpcServer::new(move |request| {
        let result = match request["method"].as_str().unwrap() {
            "get_sync_status" => json!({"phase":"Synced", "current":103, "total":103}),
            "get_snapshot_info" if fail.load(Ordering::Relaxed) => return (503, json!(null)),
            "get_snapshot_info" => serde_json::to_value(snapshot("hash-a")).unwrap(),
            "get_readiness" => readiness(),
            method => panic!("Unexpected observation: {method}"),
        };
        (
            200,
            json!({"id":request["id"], "result":result, "error":null}),
        )
    })
}

fn manager(btc_url: &str, bh_url: &str) -> StatusManager {
    StatusManager {
        btc_client: Arc::new(
            BTCRpcClient::new(btc_url.into(), bitcoincore_rpc::Auth::None).unwrap(),
        ),
        balance_history_client: Arc::new(BalanceHistoryRpcClient::new(bh_url).unwrap()),
        output: Arc::new(crate::output::IndexOutput::new()),
        poll_interval: Duration::from_millis(100),
        upstream_changed: tokio::sync::watch::channel(0).0,
        shutdown: tokio::sync::watch::channel(false).0,
        usdb_status: Arc::new(Mutex::new(USDBInscriptionIndexStatus::new(103))),
        latest_balance_history_snapshot: Arc::new(Mutex::new(None)),
        latest_balance_history_readiness: Arc::new(Mutex::new(None)),
        runtime_readiness: Arc::new(Mutex::new(RuntimeReadinessStatus::default())),
    }
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn blocked_core_observation_does_not_block_bh_on_single_thread_executor() {
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let (release, released) = std::sync::mpsc::channel();
    let btc = http_rpc::RpcServer::new(move |_| {
        notify.notify_one();
        let _ = released.recv_timeout(Duration::from_secs(5));
        (503, json!(null))
    });
    let bh = bh_server(Arc::new(AtomicBool::new(false)));
    let manager = manager(&btc.url, &bh.url);
    let observer = manager.clone();
    let btc_task = tokio::spawn(async move { observer.update_bitcoin_status().await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    // A blocked synchronous call on this executor would prevent the BH future from running.
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        manager.update_balance_history_status(),
    )
    .await;
    release.send(()).unwrap();
    assert!(result.unwrap().is_ok());
    assert!(btc_task.await.unwrap().is_err());
    assert_eq!(manager.balance_history_stable_height(), Some(103));
    assert!(manager.balance_history_readiness().unwrap().consensus_ready);
}

#[tokio::test]
async fn bh_failure_invalidates_cached_readiness_and_shutdown_interrupts_backoff() {
    let fail = Arc::new(AtomicBool::new(false));
    let bh = bh_server(fail.clone());
    let manager = manager("http://127.0.0.1:1", &bh.url);
    let observer = manager.clone();
    let task = tokio::spawn(async move { observer.monitor_loop(false).await });
    wait_for(|| manager.balance_history_readiness().is_some()).await;
    fail.store(true, Ordering::Relaxed);
    wait_for(|| manager.balance_history_snapshot().is_none()).await;
    assert!(manager.balance_history_readiness().is_none());
    manager.set_shutdown_requested(true);
    tokio::time::timeout(Duration::from_millis(250), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn notification_retains_same_height_hash_changes_and_readiness_recovery() {
    let manager = manager("http://127.0.0.1:1", "http://127.0.0.1:1");
    let mut changes = manager.subscribe_upstream();
    manager.set_balance_history_snapshot(Some(snapshot("hash-a")));
    changes.changed().await.unwrap();
    manager.set_balance_history_snapshot(Some(snapshot("hash-a")));
    assert!(!changes.has_changed().unwrap());
    // Height-only notification would miss this reorg, including a change before await.
    manager.set_balance_history_snapshot(Some(snapshot("hash-b")));
    changes.changed().await.unwrap();
    assert_eq!(
        manager
            .balance_history_snapshot()
            .unwrap()
            .stable_block_hash
            .as_deref(),
        Some("hash-b")
    );
    let mut ready: BalanceHistoryReadinessInfo = serde_json::from_value(readiness()).unwrap();
    ready.consensus_ready = false;
    manager.set_balance_history_readiness(Some(ready.clone()));
    changes.changed().await.unwrap();
    ready.consensus_ready = true;
    manager.set_balance_history_readiness(Some(ready));
    changes.changed().await.unwrap();
}
