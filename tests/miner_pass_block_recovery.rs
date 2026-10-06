//! Publication failures must gate new blocks until all derived caches can be restored.
use super::*;

#[tokio::test]
async fn failed_tracker_reload_blocks_retry_before_any_new_events() {
    let height = 330;
    let root = test_root_dir("indexer_behavior", "publication_recovery_gate");
    write_test_config(&root, height);
    let fixture = build_indexer_fixture_with_hint_provider_at_root(
        root,
        Arc::new(MockInscriptionSource::default()),
        Arc::new(MockBlockHintProvider::default().with_block(height, build_test_block(vec![]))),
        Arc::new(MockTransferTracker::default().with_reload_failures(3)),
        vec![
            MockResponse::Immediate(Ok(vec![])),
            MockResponse::Immediate(Ok(vec![])),
        ],
        Arc::new(MockBalanceProvider::default()),
    );
    let db = rusqlite::Connection::open(
        ConfigManager::load(Some(fixture.root_dir.clone()))
            .unwrap()
            .data_dir()
            .join(crate::constants::MINER_PASS_DB_FILE),
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER fail_progress BEFORE INSERT ON state WHEN NEW.name='btc_synced_block_height' BEGIN SELECT RAISE(ABORT, 'injected publication failure'); END;").unwrap();
    let error = fixture
        .indexer
        .sync_blocks_without_reconcile_for_test(height..=height)
        .await
        .unwrap_err();
    assert!(
        error.contains("Injected mock transfer reload failure"),
        "{error}"
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    assert_eq!(
        fixture
            .storage
            .get_committed_synced_btc_block_height()
            .unwrap(),
        None
    );
    db.execute_batch("DROP TRIGGER fail_progress").unwrap();
    // The second failed reload occurs before planner/tracker execution for the retry.
    assert!(
        fixture
            .indexer
            .sync_blocks_without_reconcile_for_test(height..=height)
            .await
            .unwrap_err()
            .contains("new blocks remain gated")
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    // The outer sync loop must also retry recovery before any idle/upstream early return.
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("new blocks remain gated")
    );
    assert_eq!(
        *fixture
            .status
            .block_processing_pending_height
            .lock()
            .unwrap(),
        Some(height)
    );
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
    fixture
        .indexer
        .sync_blocks_without_reconcile_for_test(height..=height)
        .await
        .unwrap();
    assert_eq!(fixture.transfer_tracker.reload_call_count(), 4);
    assert_eq!(fixture.transfer_tracker.commit_call_count(), 2);
    assert_eq!(
        fixture
            .storage
            .get_committed_synced_btc_block_height()
            .unwrap(),
        Some(height)
    );
    drop(db);
    cleanup_temp_dir(&fixture.root_dir);
}
