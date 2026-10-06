//! Boundary execution and paired-store recovery through the production indexer pipeline.
use crate::config::ConfigManager;
use crate::index::indexer::publication_faults::{
    FaultMode, FaultPoint, PUBLICATION_POINTS, REORG_POINTS,
};
use crate::index::test_miner_pipeline::Pipeline;
use crate::index::test_miner_rules::{CONFORMANCE_ENERGY_TRIPLE, conformance_catalog};
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use crate::index::test_miner_upgrade::{Scenario, catalog, copy_tree, crash_child, fingerprint};
use crate::index::{InscriptionIndexer, MinerPassState, PassBlockMutationCollector};
use crate::output::IndexOutput;
use crate::status::StatusManager;
use std::sync::Arc;
use usdb_util::VersionFamily;

#[tokio::test]
async fn empty_boundaries_preserve_sparse_storage_and_commit_the_selected_rules() {
    let mint = MintBlock::new(
        8,
        vec![MintSpec::standard(
            90,
            source_script(SpendKind::Witness),
            1,
            vec![],
        )],
        false,
    );
    let empty: Vec<_> = (9..=22).map(|h| MintBlock::new(h, vec![], false)).collect();
    let mut blocks = vec![&mint];
    blocks.extend(empty.iter());
    let mut p = Pipeline::with_catalog("empty-boundaries", &blocks, 8, &catalog()).await;
    p.sync_to(9).await.unwrap();
    let id = mint.mints[0].inscription_id;
    let original = p
        .indexer
        .pass_energy_manager()
        .get_pass_energy_record_exact(&id, 8)
        .unwrap()
        .unwrap();
    for height in [10, 19, 20, 22] {
        p.sync_to(height).await.unwrap();
        let expected =
            crate::index::test_energy_reference::reference(original.clone(), height, &[]);
        let manager = p.indexer.pass_energy_manager();
        assert_eq!(
            manager
                .get_pass_energy(&id, height)
                .await
                .unwrap()
                .unwrap()
                .energy,
            expected.energy
        );
        assert!(
            manager
                .get_pass_energy_record_exact(&id, height)
                .unwrap()
                .is_none()
        );
        let commit = p
            .indexer
            .miner_pass_storage()
            .get_pass_block_commit(height)
            .unwrap()
            .unwrap();
        assert_eq!(
            commit.mutation_root,
            PassBlockMutationCollector::new(height)
                .mutation_root()
                .unwrap()
        );
        let before = fingerprint(&p, &[id], height).await;
        p = p.reopen().await.reopen().await;
        assert_eq!(fingerprint(&p, &[id], height).await, before);
    }
    p.cleanup();
}

#[tokio::test]
async fn empty_block_and_startup_reject_a_missing_conversion_before_any_mutation() {
    let bad = conformance_catalog(&[(
        VersionFamily::EnergyFormulaVersion,
        10,
        CONFORMANCE_ENERGY_TRIPLE,
    )]);
    let blocks: Vec<_> = (8..=11).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p = Pipeline::with_catalog(
        "missing-boundary",
        &blocks.iter().collect::<Vec<_>>(),
        8,
        &bad,
    )
    .await;
    p.sync_to(9).await.unwrap();
    let before = fingerprint(&p, &[], 9).await;
    let error = p.sync_to(10).await.unwrap_err();
    assert!(
        error.contains("boundary_height=10") && error.contains("Missing raw-energy transition"),
        "{error}"
    );
    assert_eq!(
        p.indexer
            .pass_energy_manager()
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_pass_block_commit(10)
            .unwrap()
            .is_none()
    );
    let p = p
        .while_stopped(|root| {
            let config = Arc::new(ConfigManager::load(Some(root.to_owned())).unwrap());
            let db = rusqlite::Connection::open(config.data_dir().join("miner_pass.db")).unwrap();
            db.execute(
                "UPDATE state SET value='11' WHERE name='btc_synced_block_height'",
                [],
            )
            .unwrap();
            let status =
                Arc::new(StatusManager::new(config.clone(), Arc::new(IndexOutput::new())).unwrap());
            let error = match InscriptionIndexer::new(config, status) {
                Ok(_) => panic!("accepted missing historical conversion"),
                Err(error) => error,
            };
            assert!(error.contains("boundary_height=10"), "{error}");
            db.execute(
                "UPDATE state SET value='9' WHERE name='btc_synced_block_height'",
                [],
            )
            .unwrap();
        })
        .await;
    assert_eq!(fingerprint(&p, &[], 9).await, before);
    p.cleanup();
}

#[tokio::test]
async fn every_publication_error_window_retries_to_the_clean_replay_state() {
    let scenario = Scenario::new(false);
    let clean = scenario.pipeline("errors-clean").await;
    clean.sync_to(22).await.unwrap();
    let expected = fingerprint(&clean, &scenario.ids, 22).await;
    for height in [8, 10, 20] {
        for point in PUBLICATION_POINTS {
            let p = scenario.pipeline("publication-error").await;
            p.sync_to(height - 1).await.unwrap();
            p.indexer
                .arm_publication_fault_for_test(height, point, FaultMode::Error);
            let error = p
                .indexer
                .sync_blocks_without_reconcile_for_test(height..=height)
                .await
                .unwrap_err();
            assert!(
                error.contains("Injected publication failure"),
                "{point:?}: {error}"
            );
            let durable = if point == FaultPoint::SqliteCommitted {
                height
            } else {
                height - 1
            };
            assert_eq!(
                p.indexer
                    .miner_pass_storage()
                    .get_committed_synced_btc_block_height()
                    .unwrap(),
                (durable >= 8).then_some(durable)
            );
            assert_eq!(
                p.indexer
                    .pass_energy_manager()
                    .get_synced_block_height_for_test()
                    .unwrap(),
                Some(durable)
            );
            assert_eq!(
                p.indexer
                    .pass_energy_manager()
                    .get_pending_block_height_for_test()
                    .unwrap(),
                None
            );
            assert_eq!(
                p.status
                    .get_runtime_readiness()
                    .block_processing_pending_height,
                Some(height)
            );
            p.sync_to(22).await.unwrap();
            assert_eq!(
                fingerprint(&p, &scenario.ids, 22).await,
                expected,
                "{height} {point:?}"
            );
            assert_eq!(
                p.status
                    .get_runtime_readiness()
                    .block_processing_pending_height,
                None
            );
            p.cleanup();
        }
    }
    clean.cleanup();
}

#[tokio::test]
#[ignore = "Only the parent recovery matrix may invoke this process-exit fixture"]
async fn upgrade_crash_child() {
    let root = std::path::PathBuf::from(
        std::env::var_os("USDB_UPGRADE_TEST_ROOT").expect("isolated root"),
    );
    let height = std::env::var("USDB_UPGRADE_TEST_HEIGHT")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let point = std::env::var("USDB_UPGRADE_TEST_POINT").unwrap();
    let point = PUBLICATION_POINTS
        .into_iter()
        .chain(REORG_POINTS)
        .find(|p| format!("{p:?}") == point)
        .unwrap();
    let config = Arc::new(ConfigManager::load(Some(root.clone())).unwrap());
    let status =
        Arc::new(StatusManager::new(config.clone(), Arc::new(IndexOutput::new())).unwrap());
    let indexer = InscriptionIndexer::new(config, status.clone()).unwrap();
    indexer.init().await.unwrap();
    indexer.arm_publication_fault_for_test(height, point, FaultMode::Crash);
    if std::env::var("USDB_UPGRADE_TEST_REORG").unwrap() == "yes" {
        let snapshot: balance_history::SnapshotInfo =
            serde_json::from_slice(&std::fs::read(root.join("crash-snapshot.json")).unwrap())
                .unwrap();
        status.set_balance_history_snapshot(Some(snapshot));
        indexer.sync_once_for_test().await.unwrap();
    } else {
        indexer
            .sync_blocks_without_reconcile_for_test(height..=height)
            .await
            .unwrap();
    }
    panic!("Crash failpoint was not reached: {point:?}");
}

#[tokio::test]
async fn process_exit_at_every_publication_window_recovers_without_replaying_committed_blocks() {
    let scenario = Scenario::new(false);
    let clean = scenario.pipeline("crash-clean").await;
    clean.sync_to(22).await.unwrap();
    let expected = fingerprint(&clean, &scenario.ids, 22).await;
    for height in [8, 10, 20] {
        for point in PUBLICATION_POINTS {
            let p = scenario.pipeline("publication-crash").await;
            p.sync_to(height - 1).await.unwrap();
            let p = p
                .while_stopped(|root| crash_child(root, height, point, false))
                .await;
            let durable = if point == FaultPoint::SqliteCommitted {
                height
            } else {
                height - 1
            };
            assert_eq!(
                p.indexer
                    .miner_pass_storage()
                    .get_committed_synced_btc_block_height()
                    .unwrap(),
                (durable >= 8).then_some(durable)
            );
            assert_eq!(
                p.indexer
                    .pass_energy_manager()
                    .get_synced_block_height_for_test()
                    .unwrap(),
                Some(durable)
            );
            assert_eq!(
                p.indexer
                    .pass_energy_manager()
                    .get_pending_block_height_for_test()
                    .unwrap(),
                None
            );
            let p = p.reopen().await;
            p.sync_to(22).await.unwrap();
            assert_eq!(
                fingerprint(&p, &scenario.ids, 22).await,
                expected,
                "{height} {point:?}"
            );
            p.cleanup();
        }
    }
    clean.cleanup();
}

#[tokio::test]
async fn coherent_checkpoints_in_each_epoch_restore_then_catch_up_to_clean_replay() {
    let scenario = Scenario::new(false);
    let clean = scenario.pipeline("checkpoint-clean").await;
    clean.sync_to(22).await.unwrap();
    let expected = fingerprint(&clean, &scenario.ids, 22).await;
    for height in [9, 10, 19, 20, 22] {
        let p = scenario.pipeline("epoch-checkpoint").await;
        p.sync_to(height).await.unwrap();
        let before = fingerprint(&p, &scenario.ids, height).await;
        let p = p
            .while_stopped(|root| copy_tree(&root.join("data"), &root.join("checkpoint")))
            .await;
        p.sync_to(22).await.unwrap();
        let p = p
            .while_stopped(|root| {
                std::fs::rename(root.join("data"), root.join("later-data")).unwrap();
                copy_tree(&root.join("checkpoint"), &root.join("data"));
            })
            .await;
        assert_eq!(fingerprint(&p, &scenario.ids, height).await, before);
        let p = p.reopen().await;
        p.sync_to(22).await.unwrap();
        assert_eq!(
            fingerprint(&p, &scenario.ids, 22).await,
            expected,
            "checkpoint {height}"
        );
        p.cleanup();
    }
    clean.cleanup();
}

#[tokio::test]
async fn actual_upstream_reorg_crosses_both_boundaries_and_replays_the_replacement_branch() {
    let old = Scenario::new(false);
    let new = Scenario::new(true);
    let p = old.pipeline("reorg-old").await;
    let clean = new.pipeline("reorg-clean").await;
    p.sync_to(22).await.unwrap();
    clean.sync_to(22).await.unwrap();
    let mut ids = old.ids.clone();
    ids.extend(new.ids.iter().copied());
    ids.sort();
    ids.dedup();
    let expected = fingerprint(&clean, &ids, 22).await;
    for _ in 0..2 {
        p.follow_chain(&clean);
        p.sync_to(22).await.unwrap();
        assert_eq!(fingerprint(&p, &ids, 22).await, expected);
    }
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_upstream_reorg_epoch()
            .unwrap(),
        1
    );
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&new.ids[1])
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Dormant
    );
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&old.ids[3])
            .unwrap()
            .is_none()
    );
    let p = p.reopen().await;
    assert_eq!(fingerprint(&p, &ids, 22).await, expected);
    p.cleanup();
    clean.cleanup();
}

#[tokio::test]
async fn reorg_recovery_failures_keep_the_durable_gate_and_resume_after_restart() {
    let old = Scenario::new(false);
    let new = Scenario::new(true);
    let clean = new.pipeline("reorg-failure-clean").await;
    clean.sync_to(22).await.unwrap();
    let mut ids = old.ids.clone();
    ids.extend(new.ids.iter().copied());
    ids.sort();
    ids.dedup();
    let expected = fingerprint(&clean, &ids, 22).await;
    for point in REORG_POINTS {
        let p = old.pipeline("reorg-error").await;
        p.sync_to(22).await.unwrap();
        p.follow_chain(&clean);
        p.indexer
            .arm_publication_fault_for_test(8, point, FaultMode::Error);
        let error = p.sync_to(22).await.unwrap_err();
        assert!(
            error.contains("Injected publication failure"),
            "{point:?}: {error}"
        );
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_upstream_reorg_recovery_pending_height()
                .unwrap(),
            Some(8)
        );
        assert!(
            p.status
                .get_runtime_readiness()
                .upstream_reorg_recovery_pending
        );
        let p = p.reopen().await.reopen().await;
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_upstream_reorg_recovery_pending_height()
                .unwrap(),
            None
        );
        p.sync_to(22).await.unwrap();
        assert_eq!(fingerprint(&p, &ids, 22).await, expected, "{point:?}");
        p.cleanup();
    }
    clean.cleanup();
}

#[tokio::test]
async fn inconsistent_energy_checkpoint_is_rejected_without_deleting_committed_records() {
    let scenario = Scenario::new(false);
    for (case, expected_error) in [
        ("lagging", "behind pass synced height"),
        ("pending", "overlaps committed"),
        ("missing", "missing for committed pass history"),
    ] {
        let p = scenario.pipeline("mismatched-checkpoint").await;
        p.sync_to(20).await.unwrap();
        let manager = p.indexer.pass_energy_manager();
        match case {
            "pending" => manager.begin_block_sync(20).unwrap(),
            "lagging" => manager.set_synced_block_height_for_test(19).unwrap(),
            "missing" => manager.clear_synced_block_height_for_test().unwrap(),
            _ => unreachable!(),
        }
        let error = p.indexer.init().await.unwrap_err();
        assert!(error.contains(expected_error), "{error}");
        let record = manager
            .get_pass_energy_record_exact(&scenario.ids[3], 20)
            .unwrap()
            .unwrap();
        assert!(record.energy > 0);
        assert_eq!(record.state, MinerPassState::Active);
        assert_eq!(
            manager.get_pending_block_height_for_test().unwrap(),
            (case == "pending").then_some(20)
        );
        p.cleanup();
    }
}

#[tokio::test]
async fn process_exit_during_reorg_resumes_the_durable_rollback_before_replay() {
    let old = Scenario::new(false);
    let new = Scenario::new(true);
    let clean = new.pipeline("reorg-crash-clean").await;
    clean.sync_to(22).await.unwrap();
    let mut ids = old.ids.clone();
    ids.extend(new.ids.iter().copied());
    ids.sort();
    ids.dedup();
    let expected = fingerprint(&clean, &ids, 22).await;
    for point in REORG_POINTS {
        let p = old.pipeline("reorg-crash").await;
        p.sync_to(22).await.unwrap();
        p.follow_chain(&clean);
        let snapshot = p.history.lock().unwrap().snapshot(22);
        let p = p
            .while_stopped(|root| {
                std::fs::write(
                    root.join("crash-snapshot.json"),
                    serde_json::to_vec(&snapshot).unwrap(),
                )
                .unwrap();
                crash_child(root, 8, point, true);
            })
            .await;
        // Startup must finish the durable rollback without an upstream tip or a new block.
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_committed_synced_btc_block_height()
                .unwrap(),
            Some(8)
        );
        assert_eq!(
            p.indexer
                .pass_energy_manager()
                .get_synced_block_height_for_test()
                .unwrap(),
            Some(8)
        );
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_upstream_reorg_recovery_pending_height()
                .unwrap(),
            None
        );
        let p = p.reopen().await;
        p.sync_to(22).await.unwrap();
        assert_eq!(fingerprint(&p, &ids, 22).await, expected, "{point:?}");
        p.cleanup();
    }
    clean.cleanup();
}

#[tokio::test]
async fn sqlite_write_failure_after_boundary_inheritance_restores_both_stores_and_tracker() {
    let scenario = Scenario::new(false);
    let clean = scenario.pipeline("sqlite-error-clean").await;
    clean.sync_to(22).await.unwrap();
    let expected = fingerprint(&clean, &scenario.ids, 22).await;
    let p = scenario.pipeline("sqlite-error").await;
    p.sync_to(19).await.unwrap();
    let before = fingerprint(&p, &scenario.ids, 19).await;
    let db = rusqlite::Connection::open(p.config.data_dir().join("miner_pass.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_height BEFORE UPDATE ON state
        WHEN NEW.name='btc_synced_block_height' AND NEW.value=20
        BEGIN SELECT RAISE(ABORT, 'injected boundary height persistence failure'); END;",
    )
    .unwrap();
    let error = p.sync_to(20).await.unwrap_err();
    assert!(
        error.contains("injected boundary height persistence failure"),
        "{error}"
    );
    assert_eq!(fingerprint(&p, &scenario.ids, 19).await, before);
    db.execute_batch("DROP TRIGGER fail_height").unwrap();
    drop(db);
    p.sync_to(22).await.unwrap();
    assert_eq!(fingerprint(&p, &scenario.ids, 22).await, expected);
    p.cleanup();
    clean.cleanup();
}
