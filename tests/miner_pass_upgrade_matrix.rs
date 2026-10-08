//! Combined schema/state/energy boundaries must survive operations, interruption and reorg.
use crate::index::MinerPassState;
use crate::index::indexer::publication_faults::{FaultMode, PUBLICATION_POINTS};
use crate::index::test_energy_reference::reference;
use crate::index::test_miner_epochs::{Scenario, catalogs};
use crate::index::test_miner_upgrade::{crash_child, fingerprint};

#[tokio::test]
async fn same_and_adjacent_boundaries_preserve_old_facts_and_settle_cross_schema_prev() {
    for gap in [0, 1] {
        let scenario = Scenario::new(gap, false);
        let p = scenario
            .pipeline("combined-semantics", &catalogs(gap)[2])
            .await;
        let height = 20 + scenario.gap;
        p.sync_to(height - 1).await.unwrap();
        let store = p.indexer.miner_pass_storage();
        let energy = p.indexer.pass_energy_manager();
        let mut expected = 0;
        for id in [scenario.ids[0], scenario.ids[2]] {
            let previous = energy
                .get_pass_energy_record_at_or_before(&id, height - 1)
                .unwrap()
                .unwrap();
            expected += reference(previous, height, &[]).energy * 75 / 100;
        }
        assert!(
            expected > 0,
            "fixture must exercise real nonzero inherited energy"
        );
        assert_eq!(
            store
                .get_pass_by_inscription_id(&scenario.ids[1])
                .unwrap()
                .unwrap()
                .state,
            MinerPassState::Active
        );
        assert_eq!(
            store
                .get_pass_by_inscription_id(&scenario.ids[3])
                .unwrap()
                .unwrap()
                .state,
            MinerPassState::Invalid
        );
        assert_eq!(
            store
                .get_mint_audit(&scenario.ids[3])
                .unwrap()
                .unwrap()
                .error_code
                .as_deref(),
            Some("INVALID_USDB_COLLAB")
        );
        p.sync_to(24).await.unwrap();
        assert_eq!(
            energy
                .get_pass_energy_record_exact(&scenario.ids[4], height)
                .unwrap()
                .unwrap()
                .energy,
            expected
        );
        for (slot, version, state) in [
            (0, 1, MinerPassState::Consumed),
            (1, 1, MinerPassState::Dormant),
            (2, 901, MinerPassState::Consumed),
            (4, 902, MinerPassState::Active),
            (5, 902, MinerPassState::Active),
        ] {
            let pass = store
                .get_pass_by_inscription_id(&scenario.ids[slot])
                .unwrap()
                .unwrap();
            assert_eq!((pass.mint_version, pass.state), (version, state));
        }
        let before = fingerprint(&p, &scenario.ids, 24).await;
        let p = p.reopen().await;
        assert_eq!(before, fingerprint(&p, &scenario.ids, 24).await);
        p.cleanup();
    }
}

#[tokio::test]
async fn combined_boundary_error_and_process_exit_matrices_match_clean_replay() {
    for gap in [0, 1] {
        let scenario = Scenario::new(gap, false);
        let catalog = &catalogs(gap)[2];
        let clean = scenario.pipeline("combined-clean", catalog).await;
        clean.sync_to(24).await.unwrap();
        let expected = fingerprint(&clean, &scenario.ids, 24).await;
        // Exercise every publication stage at both schema transitions, including prev consumption.
        for height in [10 + gap, 20 + gap] {
            for point in PUBLICATION_POINTS {
                for crash in [false, true] {
                    let mut p = scenario.pipeline("combined-recovery", catalog).await;
                    p.sync_to(height - 1).await.unwrap();
                    if crash {
                        p = p
                            .while_stopped(|root| crash_child(root, height, point, false))
                            .await;
                    } else {
                        p.indexer
                            .arm_publication_fault_for_test(height, point, FaultMode::Error);
                        let error = p
                            .indexer
                            .sync_blocks_without_reconcile_for_test(height..=height)
                            .await
                            .unwrap_err();
                        assert!(error.contains("Injected publication failure"), "{error}");
                    }
                    p.sync_to(24).await.unwrap();
                    assert_eq!(
                        fingerprint(&p, &scenario.ids, 24).await,
                        expected,
                        "gap={gap}, height={height}, point={point:?}, crash={crash}"
                    );
                    p.cleanup();
                }
            }
        }
        clean.cleanup();
    }
}

#[tokio::test]
async fn reorg_reverses_two_schema_changes_and_replays_combined_rule_boundaries() {
    for gap in [0, 1] {
        let old = Scenario::new(gap, false);
        let new = Scenario::new(gap, true);
        let catalog = &catalogs(gap)[2];
        let p = old.pipeline("combined-reorg", catalog).await;
        let clean = new.pipeline("combined-fork", catalog).await;
        p.sync_to(24).await.unwrap();
        clean.sync_to(24).await.unwrap();
        let mut ids = old.ids.clone();
        ids.extend(&new.ids);
        ids.sort();
        ids.dedup();
        p.follow_chain(&clean);
        p.sync_to(24).await.unwrap();
        assert_eq!(
            fingerprint(&p, &ids, 24).await,
            fingerprint(&clean, &ids, 24).await
        );
        assert!(
            p.indexer
                .miner_pass_storage()
                .get_pass_by_inscription_id(&old.ids[4])
                .unwrap()
                .is_none()
        );
        let p = p.reopen().await;
        assert_eq!(
            fingerprint(&p, &ids, 24).await,
            fingerprint(&clean, &ids, 24).await
        );
        p.cleanup();
        clean.cleanup();
    }
}
