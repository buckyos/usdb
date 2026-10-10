// Real block execution pauses at publication boundaries while RPCs use separate readers.
use super::*;
use crate::index::publication_faults::{FaultMode, FaultPoint, PUBLICATION_POINTS};
use crate::index::test_miner_pipeline::Pipeline;
use crate::index::test_miner_queries::{QueryScenario, catalog, params};
use serde_json::{Value, json};
use std::time::Duration;

async fn fixture(name: &str) -> (Pipeline, QueryScenario) {
    let scenario = QueryScenario::new(false);
    let pipeline = scenario.pipeline(name, &catalog()).await;
    pipeline.sync_to(10).await.unwrap();
    pipeline.status.set_rpc_alive(true);
    pipeline
        .status
        .set_balance_history_readiness(Some(ready_balance_history_readiness(12)));
    pipeline
        .status
        .set_balance_history_snapshot(Some(pipeline.history.lock().unwrap().snapshot(12)));
    (pipeline, scenario)
}

async fn wait_pause(observed: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || observed.recv_timeout(Duration::from_secs(20)).unwrap())
        .await
        .unwrap();
}

fn economic_answers(rpc: &UsdbIndexerRpcServer, scenario: &QueryScenario, height: u32) -> Value {
    let id = scenario.ids[0].to_string();
    let profile = rpc.get_pass_economic_profile(params(json!({"view_version":USDB_ECONOMIC_STATE_VIEW_VERSION,"pass_id":id,"block_height":height}))).unwrap();
    let context = ConsensusQueryContext::from(&profile.external_state);
    let common = json!({"view_version":USDB_ECONOMIC_STATE_VIEW_VERSION,"block_height":height,"context":context});
    let mut candidate = common.clone();
    candidate["usdb_main"] = json!(profile.pass.usdb_main);
    let mut page = common.clone();
    page["limit"] = json!(100);
    let mut collab = page.clone();
    collab["leader_pass_id"] = json!(id);
    json!({
        "state":rpc.get_state_ref_at_height(params(json!({"block_height":height,"context":context}))).unwrap(),
        "snapshot":rpc.get_pass_snapshot(params(json!({"inscription_id":id,"at_height":height,"context":context}))).unwrap(),
        "energy":rpc.get_pass_energy(params(json!({"inscription_id":id,"block_height":height,"context":context}))).unwrap(),
        "profile":profile,
        "candidate":rpc.resolve_miner_candidate(params(candidate)).unwrap(),
        "candidates":rpc.get_candidate_set_view(params(page)).unwrap(),
        "collabs":rpc.get_collab_breakdown(params(collab)).unwrap(),
        "aggregate":rpc.get_miner_economic_aggregate(params(common)).unwrap(),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_profile_retries_during_backfill_then_recovers_with_the_same_identity() {
    let (p, scenario) = fixture("query_backfill_retry").await;
    let baseline = economic_answers(&p.rpc(), &scenario, 9);
    // Model a legacy database with complete business state and a missing anchor.
    let p = p
        .while_stopped(|root| {
            let conn = rusqlite::Connection::open(
                root.join("data").join(crate::constants::MINER_PASS_DB_FILE),
            )
            .unwrap();
            conn.execute(
                "DELETE FROM balance_history_snapshot_history WHERE block_height = 9",
                [],
            )
            .unwrap();
        })
        .await;
    p.status.set_rpc_alive(true);
    p.status
        .set_balance_history_readiness(Some(ready_balance_history_readiness(10)));
    p.status
        .set_balance_history_snapshot(Some(p.history.lock().unwrap().snapshot(10)));
    let rpc = p.rpc();
    let context: ConsensusQueryContext = ConsensusQueryContext::from(
        &serde_json::from_value::<PassEconomicProfileView>(baseline["profile"].clone())
            .unwrap()
            .external_state,
    );
    let query = json!({"view_version":USDB_ECONOMIC_STATE_VIEW_VERSION,
        "pass_id":scenario.ids[0].to_string(),"block_height":9,"context":context});
    let error = rpc
        .get_pass_economic_profile(params(query.clone()))
        .unwrap_err();
    assert_eq!(
        error.code,
        ErrorCode::ServerError(ConsensusRpcErrorCode::SnapshotNotReady.code())
    );
    assert_eq!(
        decode_consensus_error_data(&error).requested_height,
        Some(9)
    );
    assert_eq!(
        rpc.get_readiness().unwrap().snapshot_history_pending_from,
        Some(9)
    );
    // Complete heights remain usable while this selected historical height waits.
    economic_answers(&rpc, &scenario, 10);

    // Use the production backfill loop, without replacing the RPC/indexer instance.
    p.sync_to(10).await.unwrap();
    assert_eq!(
        rpc.get_readiness().unwrap().snapshot_history_pending_from,
        None
    );
    assert_eq!(
        serde_json::to_value(
            rpc.get_pass_economic_profile(params(query.clone()))
                .unwrap()
        )
        .unwrap(),
        baseline["profile"]
    );
    assert_eq!(economic_answers(&rpc, &scenario, 9), baseline);

    // A missing business snapshot is not repaired by anchor backfill.
    let conn = rusqlite::Connection::open(
        p.root
            .join("data")
            .join(crate::constants::MINER_PASS_DB_FILE),
    )
    .unwrap();
    conn.execute(
        "DELETE FROM active_balance_snapshots WHERE block_height = 9",
        [],
    )
    .unwrap();
    let error = rpc
        .get_pass_economic_profile(params(query.clone()))
        .unwrap_err();
    assert_eq!(
        error.code,
        ErrorCode::ServerError(ConsensusRpcErrorCode::HistoryNotAvailable.code())
    );
    // Likewise, a missing anchor outside the durable pending range is a fault,
    // not evidence that an automatic recovery job can repair it in this process.
    conn.execute(
        "DELETE FROM balance_history_snapshot_history WHERE block_height = 9",
        [],
    )
    .unwrap();
    let error = rpc.get_pass_economic_profile(params(query)).unwrap_err();
    assert_eq!(
        error.code,
        ErrorCode::ServerError(ConsensusRpcErrorCode::HistoryNotAvailable.code())
    );
    assert!(
        decode_consensus_error_data(&error)
            .detail
            .unwrap()
            .contains("backfill_pending=false")
    );
    drop(conn);
    drop(rpc);
    p.cleanup();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_block_writes_serve_old_state_and_publish_each_committed_head() {
    let (p, scenario) = fixture("concurrent_committed_prefix").await;
    let baseline = economic_answers(&p.rpc(), &scenario, 10);
    let (first, resume_first) = p
        .indexer
        .pause_publication_for_test(11, FaultPoint::PassCommitWritten);
    let indexer = p.indexer.clone();
    let job = tokio::spawn(async move {
        indexer
            .sync_blocks_without_reconcile_for_test(11..=12)
            .await
    });
    wait_pause(first).await;
    // The writer sees the new collab and finalized energy, but public queries must not.
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&scenario.ids[2])
            .unwrap()
            .is_some()
    );
    assert!(
        p.indexer
            .rpc_pass_storage()
            .get_pass_by_inscription_id(&scenario.ids[2])
            .unwrap()
            .is_none()
    );
    let ready = p.rpc().get_readiness().unwrap();
    assert_eq!(ready.synced_block_height, Some(10));
    assert!(ready.committed_query_ready);
    assert!(!ready.consensus_ready);
    assert!(
        ready
            .blockers
            .contains(&ReadinessBlocker::BlockProcessingPending)
    );
    assert_eq!(economic_answers(&p.rpc(), &scenario, 10), baseline);
    let error = p
        .rpc()
        .get_state_ref_at_height(params(json!({"block_height":11})))
        .unwrap_err();
    assert_eq!(
        error.code,
        ErrorCode::ServerError(ConsensusRpcErrorCode::HeightNotSynced.code())
    );

    let (second, resume_second) = p
        .indexer
        .pause_publication_for_test(12, FaultPoint::EventsApplied);
    resume_first.send(()).unwrap();
    wait_pause(second).await;
    let ready = p.rpc().get_readiness().unwrap();
    assert_eq!(ready.synced_block_height, Some(11));
    assert!(ready.committed_query_ready);
    assert_eq!(
        p.rpc()
            .get_snapshot_info()
            .unwrap()
            .unwrap()
            .balance_history_stable_height,
        11
    );
    let at11 = economic_answers(&p.rpc(), &scenario, 11);
    assert_eq!(economic_answers(&p.rpc(), &scenario, 10), baseline);
    resume_second.send(()).unwrap();
    job.await.unwrap().unwrap();
    assert_eq!(economic_answers(&p.rpc(), &scenario, 11), at11);
    assert!(p.rpc().get_readiness().unwrap().consensus_ready);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_publication_window_preserves_prefix_then_failed_attempt_blocks_until_retry() {
    for point in PUBLICATION_POINTS {
        let (p, scenario) = fixture(&format!("query_publication_{point:?}")).await;
        let baseline = economic_answers(&p.rpc(), &scenario, 10);
        let (observed, resume) = p.indexer.pause_publication_for_test(11, point);
        p.indexer
            .arm_publication_fault_for_test(11, point, FaultMode::Error);
        let indexer = p.indexer.clone();
        let job = tokio::spawn(async move {
            indexer
                .sync_blocks_without_reconcile_for_test(11..=11)
                .await
        });
        wait_pause(observed).await;
        if point == FaultPoint::SqliteCommitted {
            // Publication finished, but the runtime still owns the pending block marker.
            assert!(!p.rpc().get_readiness().unwrap().committed_query_ready);
        } else {
            assert!(
                p.rpc().get_readiness().unwrap().committed_query_ready,
                "{point:?}"
            );
            assert_eq!(
                economic_answers(&p.rpc(), &scenario, 10),
                baseline,
                "{point:?}"
            );
        }
        resume.send(()).unwrap();
        assert!(job.await.unwrap().is_err());
        assert!(
            !p.rpc().get_readiness().unwrap().committed_query_ready,
            "{point:?}"
        );
        let error = p
            .rpc()
            .get_state_ref_at_height(params(json!({"block_height":10})))
            .unwrap_err();
        assert_eq!(
            error.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::SnapshotNotReady.code())
        );
        p.sync_to(11).await.unwrap();
        assert!(p.rpc().get_readiness().unwrap().committed_query_ready);
        assert_eq!(economic_answers(&p.rpc(), &scenario, 10), baseline);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reorg_waits_for_query_lease_and_new_queries_fail_without_waiting() {
    let (p, _) = fixture("query_reorg_lease").await;
    let lease = p.indexer.try_committed_query_lease().unwrap();
    let indexer = p.indexer.clone();
    let job = tokio::spawn(async move { indexer.rollback_and_resume_for_test(9).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !p
            .status
            .get_runtime_readiness()
            .upstream_reorg_recovery_pending
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        p.indexer
            .rpc_pass_storage()
            .get_committed_synced_btc_block_height()
            .unwrap(),
        Some(10)
    );
    assert!(
        p.rpc()
            .get_state_ref_at_height(params(json!({"block_height":10})))
            .is_err()
    );
    drop(lease);
    job.await.unwrap().unwrap();
    assert_eq!(
        p.indexer
            .rpc_pass_storage()
            .get_committed_synced_btc_block_height()
            .unwrap(),
        Some(9)
    );
    p.rpc()
        .get_state_ref_at_height(params(json!({"block_height":9})))
        .unwrap();
}
