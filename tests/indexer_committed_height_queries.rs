// Exercise the public RPC methods with live readiness and real private stores.
use super::*;

fn catching_up_server(tag: &str) -> (UsdbIndexerRpcServer, PathBuf, MinerPassInfo) {
    let (server, root) = build_server_with_genesis(tag, 120, 100);
    let pass = make_active_pass(201, 211, 100);
    server
        .indexer
        .miner_pass_storage()
        .add_new_mint_pass_at_height(&pass, 100)
        .unwrap();
    for height in [110, 120] {
        seed_energy_record(&server, &pass, height, u128::from(height));
        seed_state_ref_context(&server, height);
    }
    server.status.set_rpc_alive(true);
    server
        .status
        .set_balance_history_snapshot(Some(ready_balance_history_snapshot(125)));
    let mut upstream = ready_balance_history_readiness(125);
    upstream.consensus_ready = false;
    upstream.phase = balance_history::SyncPhase::Indexing;
    upstream.total = 126;
    upstream.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
    server.status.set_balance_history_readiness(Some(upstream));
    (server, root, pass)
}

fn state_at(server: &UsdbIndexerRpcServer, height: u32) -> JsonResult<HistoricalStateRefInfo> {
    server.get_state_ref_at_height(GetStateRefAtHeightParams {
        block_height: height,
        context: None,
    })
}

fn assert_code(error: JsonError, expected: ConsensusRpcErrorCode) {
    assert_eq!(
        error.code,
        ErrorCode::ServerError(expected.code()),
        "{error:?}"
    );
}

#[test]
fn committed_queries_work_while_both_services_catch_up() {
    let (server, root, pass) = catching_up_server("committed_catch_up");
    let readiness = server.get_readiness().unwrap();
    assert!(readiness.query_ready);
    assert!(!readiness.consensus_ready);
    for blocker in [
        ReadinessBlocker::CatchingUp,
        ReadinessBlocker::UpstreamConsensusNotReady,
        ReadinessBlocker::HistoryBackfillPending,
    ] {
        assert!(readiness.blockers.contains(&blocker), "{readiness:?}");
    }
    for height in [110, 120] {
        let state = state_at(&server, height).unwrap();
        let profile = get_pass_economic_profile_for_test(&server, &pass, height);
        let context = ConsensusQueryContext::from(&profile.external_state);
        assert_eq!(profile.external_state.btc_height, height);
        assert_eq!(
            profile.external_state.system_state_id,
            state.system_state_info.system_state_id
        );
        let candidate = server
            .resolve_miner_candidate(ResolveMinerCandidateParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                usdb_main: pass.usdb_main.clone(),
                block_height: Some(height),
                context: Some(context.clone()),
            })
            .unwrap();
        assert_eq!(candidate.pass.pass_id, pass.inscription_id.to_string());
        assert_eq!(
            candidate.external_state.system_state_id,
            profile.external_state.system_state_id
        );
        let energy = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(height),
                context: Some(context.clone()),
                mode: Some("exact".into()),
            })
            .unwrap();
        assert_eq!(energy.query_block_height, height);
        assert_eq!(energy.raw_energy, height.to_string());
        assert!(
            server
                .get_pass_snapshot(GetPassSnapshotParams {
                    inscription_id: pass.inscription_id.to_string(),
                    at_height: Some(height),
                    context: Some(context),
                })
                .unwrap()
                .is_some()
        );
    }
    assert!(!server.get_readiness().unwrap().consensus_ready);
    let latest = server
        .resolve_miner_candidate(ResolveMinerCandidateParams {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            usdb_main: pass.usdb_main.clone(),
            block_height: None,
            context: None,
        })
        .unwrap();
    assert_eq!(latest.external_state.btc_height, 120);
    // Local-only catch-up and upstream-only catch-up both permit committed reads.
    server
        .status
        .set_balance_history_readiness(Some(ready_balance_history_readiness(125)));
    state_at(&server, 120).unwrap();
    let mut upstream = ready_balance_history_readiness(120);
    upstream.consensus_ready = false;
    upstream.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
    server
        .status
        .set_balance_history_snapshot(Some(ready_balance_history_snapshot(120)));
    server.status.set_balance_history_readiness(Some(upstream));
    state_at(&server, 120).unwrap();
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn uncommitted_height_and_failed_publication_cannot_leak_to_queries() {
    let (server, root, _) = catching_up_server("committed_pending_writer");
    let storage = server.indexer.miner_pass_storage();
    server.status.set_block_processing_pending_height(Some(121));
    storage.savepoint_begin().unwrap();
    storage.update_synced_btc_block_height(121).unwrap();
    assert_eq!(
        server.get_readiness().unwrap().synced_block_height,
        Some(120)
    );
    assert_code(
        state_at(&server, 121).unwrap_err(),
        ConsensusRpcErrorCode::HeightNotSynced,
    );
    assert_code(
        state_at(&server, 120).unwrap_err(),
        ConsensusRpcErrorCode::SnapshotNotReady,
    );
    storage.savepoint_rollback().unwrap();
    // Rollback alone does not clear the pending failure barrier.
    assert_code(
        state_at(&server, 120).unwrap_err(),
        ConsensusRpcErrorCode::SnapshotNotReady,
    );
    server.status.set_block_processing_pending_height(None);
    assert_eq!(state_at(&server, 120).unwrap().block_height, 120);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn catch_up_never_substitutes_heights_or_ignores_identity() {
    let (server, root, pass) = catching_up_server("committed_missing_state");
    assert_code(
        state_at(&server, 121).unwrap_err(),
        ConsensusRpcErrorCode::HeightNotSynced,
    );
    assert_code(
        state_at(&server, 111).unwrap_err(),
        ConsensusRpcErrorCode::HistoryNotAvailable,
    );
    assert_code(
        state_at(&server, 99).unwrap_err(),
        ConsensusRpcErrorCode::StateNotRetained,
    );
    let profile = get_pass_economic_profile_for_test(&server, &pass, 120);
    let mut context = ConsensusQueryContext::from(&profile.external_state);
    context.expected_state.snapshot_id = Some("ff".repeat(32));
    let error = server
        .resolve_miner_candidate(ResolveMinerCandidateParams {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            usdb_main: pass.usdb_main,
            block_height: Some(120),
            context: Some(context),
        })
        .unwrap_err();
    assert_code(error, ConsensusRpcErrorCode::SnapshotIdMismatch);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_and_upstream_safety_barriers_still_block_committed_queries() {
    let (server, root, _) = catching_up_server("committed_safety_barriers");
    let initial = state_at(&server, 120).unwrap();
    for barrier in [
        "pending",
        "shutdown",
        "recovery",
        "unknown",
        "rollback",
        "bootstrap",
        "unverified",
        "upstream_behind",
    ] {
        let upstream = server.status.balance_history_readiness().unwrap();
        match barrier {
            "pending" => server.status.set_block_processing_pending_height(Some(121)),
            "shutdown" => server.status.set_shutdown_requested(true),
            "recovery" => server.status.set_upstream_reorg_recovery_pending(true),
            "unknown" => server.status.set_balance_history_readiness(None),
            _ => {
                let mut unsafe_upstream = upstream.clone();
                if barrier == "upstream_behind" {
                    unsafe_upstream.stable_height = Some(119);
                } else {
                    unsafe_upstream.blockers.push(match barrier {
                        "rollback" => balance_history::ReadinessBlocker::RollbackInProgress,
                        "bootstrap" => balance_history::ReadinessBlocker::NativeBootstrapNotReady,
                        _ => balance_history::ReadinessBlocker::SnapshotInstallUnverified,
                    });
                }
                server
                    .status
                    .set_balance_history_readiness(Some(unsafe_upstream));
            }
        }
        assert_code(
            state_at(&server, 120).unwrap_err(),
            ConsensusRpcErrorCode::SnapshotNotReady,
        );
        // A barrier arising after derivation must prevent publishing the result too.
        assert_code(
            server
                .revalidate_economic_query_context(120, &initial)
                .unwrap_err(),
            ConsensusRpcErrorCode::SnapshotNotReady,
        );
        server.status.set_block_processing_pending_height(None);
        server.status.set_shutdown_requested(false);
        server.status.set_upstream_reorg_recovery_pending(false);
        server.status.set_balance_history_readiness(Some(upstream));
        state_at(&server, 120).unwrap();
    }
    // A restart cannot erase the durable recovery barrier.
    server
        .indexer
        .miner_pass_storage()
        .rollback_to_block_height_with_upstream_reorg_recovery_pending(120, None)
        .unwrap();
    assert_code(
        state_at(&server, 120).unwrap_err(),
        ConsensusRpcErrorCode::SnapshotNotReady,
    );
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}
