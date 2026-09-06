//! Acceptance of atomic anchor publication and legacy recovery using real SQLite/RocksDB.
//! Included in the indexer binary's tests to reuse its existing injected upstream fixture.

use super::{
    Arc, IndexerFixture, MinerPassStorage, MockBalanceHistoryCommitProvider, MockBalanceProvider,
    MockBlockHintProvider, MockInscriptionSource, MockStatus, MockTransferTracker, Ordering,
    PassBlockCommitEntry, PathBuf, build_indexer_fixture_with_runtime_deps_at_root,
    build_test_block, cleanup_temp_dir, regtest_stable_lag, snapshot_from_commit, test_root_dir,
    write_test_config,
};
use crate::storage::MinePassStorageSavePointGuard;

#[path = "common/indexer_snapshot_anchor.rs"]
mod common;
use common::indexer_fixture;

#[test]
fn failed_history_batch_rolls_back_rows_and_coverage_together() {
    let root = test_root_dir("anchor_acceptance", "batch_failure");
    std::fs::create_dir_all(&root).unwrap();
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    storage.update_synced_btc_block_height(101).unwrap();
    let first = snapshot_from_commit(&MockBalanceHistoryCommitProvider::default_commit(100));
    let mut second = snapshot_from_commit(&MockBalanceHistoryCommitProvider::default_commit(101));
    second.stable_block_hash = None;
    assert!(
        storage
            .upsert_balance_history_snapshot_history_entries(&[first, second])
            .is_err()
    );
    assert!(
        storage
            .get_balance_history_snapshot_anchor_at_height(100)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(100)
    );
    drop(storage);
    cleanup_temp_dir(&root);
}

#[test]
fn startup_recomputes_coverage_from_rows_instead_of_trusting_an_old_cursor() {
    let root = test_root_dir("anchor_acceptance", "old_cursor");
    std::fs::create_dir_all(&root).unwrap();
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    for height in 100..=102 {
        storage
            .upsert_balance_history_block_anchor(
                &MockBalanceHistoryCommitProvider::default_commit(height),
                regtest_stable_lag(),
            )
            .unwrap();
    }
    storage.update_synced_btc_block_height(102).unwrap();
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(102)
    );
    drop(storage);
    let old_writer =
        rusqlite::Connection::open(root.join(crate::constants::MINER_PASS_DB_FILE)).unwrap();
    old_writer
        .execute(
            "DELETE FROM balance_history_snapshot_history WHERE block_height = 101",
            [],
        )
        .unwrap();
    drop(old_writer);
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    let progress = storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.ready_height, Some(100));
    assert_eq!(progress.pending_from, Some(101));
    drop(storage);
    cleanup_temp_dir(&root);
}

#[test]
#[ignore = "Crash fixture invoked only by the parent acceptance test with a temporary database"]
fn crash_child() {
    let root = PathBuf::from(
        std::env::var_os("USDB_TEST_ANCHOR_CRASH_ROOT").expect("parent fixture root"),
    );
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
    storage
        .upsert_balance_history_block_anchor(
            &MockBalanceHistoryCommitProvider::default_commit(101),
            regtest_stable_lag(),
        )
        .unwrap();
    storage
        .upsert_active_balance_snapshot(101, 1234, 1)
        .unwrap();
    storage.update_synced_btc_block_height(101).unwrap();
    if std::env::var("USDB_TEST_ANCHOR_CRASH_COMMITTED").unwrap() == "yes" {
        guard.commit().unwrap();
    }
    // Deliberately bypass every Rust destructor, as an interrupted process would.
    std::process::exit(73);
}

#[test]
fn process_exit_before_and_after_commit_preserves_atomic_anchor_publication() {
    for committed in [false, true] {
        let root = test_root_dir("anchor_acceptance", "process_exit");
        std::fs::create_dir_all(&root).unwrap();
        let storage = MinerPassStorage::new(&root).unwrap();
        storage.reconcile_snapshot_history_coverage(100).unwrap();
        storage
            .upsert_balance_history_block_anchor(
                &MockBalanceHistoryCommitProvider::default_commit(100),
                regtest_stable_lag(),
            )
            .unwrap();
        storage.update_synced_btc_block_height(100).unwrap();
        drop(storage);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "index::test::indexer_behavior::snapshot_anchor_acceptance::crash_child",
                "--ignored",
            ])
            .env("USDB_TEST_ANCHOR_CRASH_ROOT", &root)
            .env(
                "USDB_TEST_ANCHOR_CRASH_COMMITTED",
                if committed { "yes" } else { "no" },
            )
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(73),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let storage = MinerPassStorage::new(&root).unwrap();
        storage.reconcile_snapshot_history_coverage(100).unwrap();
        let progress = storage
            .get_committed_snapshot_history_progress(100)
            .unwrap();
        assert_eq!(
            progress.synced_height,
            Some(if committed { 101 } else { 100 })
        );
        assert_eq!(progress.ready_height, progress.synced_height);
        assert_eq!(
            storage
                .get_balance_history_snapshot_anchor_at_height(101)
                .unwrap()
                .is_some(),
            committed
        );
        assert_eq!(
            storage.get_active_balance_snapshot(101).unwrap().is_some(),
            committed
        );
        drop(storage);
        cleanup_temp_dir(&root);
    }
}

#[tokio::test]
async fn backfill_shutdown_and_wrong_height_never_publish_history_readiness() {
    let root = test_root_dir("anchor_acceptance", "shutdown_and_wrong_height");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    provider.set_state_ref(
        100,
        MockBalanceHistoryCommitProvider::default_state_ref(101),
    );
    let fixture = indexer_fixture(root.clone(), 100, 101, provider.clone());
    fixture.storage.update_synced_btc_block_height(101).unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(101, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(
            &MockBalanceHistoryCommitProvider::default_commit(101),
        ))
        .unwrap();
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("backfill anchor mismatch")
    );
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(100)
    );
    provider.state_ref_calls.lock().unwrap().clear();
    fixture.indexer.stop();
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("interrupted by shutdown")
    );
    assert!(provider.state_ref_calls.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(100)
    );
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[tokio::test]
async fn normal_sync_commits_all_anchors_without_post_scan_history_requests() {
    let root = test_root_dir("anchor_acceptance", "normal_sync");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let fixture = indexer_fixture(root.clone(), 100, 103, provider.clone());
    assert_eq!(fixture.indexer.sync_once_for_test().await.unwrap(), 103);
    assert!(provider.state_ref_calls.lock().unwrap().is_empty());
    for height in 100..=103 {
        let anchor = fixture
            .storage
            .get_balance_history_snapshot_anchor_at_height(height)
            .unwrap()
            .unwrap();
        let commit = fixture
            .storage
            .get_pass_block_commit(height)
            .unwrap()
            .unwrap();
        assert_eq!(
            anchor.latest_block_commit,
            commit.balance_history_block_commit
        );
        assert_eq!(
            anchor.stable_block_hash,
            MockBalanceHistoryCommitProvider::default_commit(height).btc_block_hash
        );
    }
    let progress = fixture
        .storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.synced_height, Some(103));
    assert_eq!(progress.ready_height, Some(103));
    assert_eq!(progress.pending_from, None);
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[tokio::test]
async fn failure_after_anchor_write_rolls_back_height_anchor_and_local_commit() {
    let root = test_root_dir("anchor_acceptance", "height_write_failure");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let fixture = indexer_fixture(root.clone(), 100, 101, provider);
    let injector =
        rusqlite::Connection::open(root.join("data").join(crate::constants::MINER_PASS_DB_FILE))
            .unwrap();
    injector
        .execute_batch(
            "CREATE TRIGGER fail_height BEFORE UPDATE ON state
        WHEN NEW.name='btc_synced_block_height' AND NEW.value=101
        BEGIN SELECT RAISE(ABORT, 'injected height persistence failure'); END;",
        )
        .unwrap();
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("injected height persistence failure")
    );
    let progress = fixture
        .storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.synced_height, Some(100));
    assert_eq!(progress.ready_height, Some(100));
    assert!(
        fixture
            .storage
            .get_balance_history_snapshot_anchor_at_height(101)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .storage
            .get_pass_block_commit(101)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .storage
            .get_active_balance_snapshot(101)
            .unwrap()
            .is_none()
    );
    injector.execute_batch("DROP TRIGGER fail_height").unwrap();
    drop(injector);
    assert_eq!(fixture.indexer.sync_once_for_test().await.unwrap(), 101);
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(101)
    );
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[tokio::test]
async fn failed_block_keeps_height_and_anchor_at_the_previous_commit_then_retries() {
    let root = test_root_dir("anchor_acceptance", "failed_block");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    provider.set_block_commit(101, None);
    let fixture = indexer_fixture(root.clone(), 100, 102, provider.clone());
    assert!(fixture.indexer.sync_once_for_test().await.is_err());
    let progress = fixture
        .storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.synced_height, Some(100));
    assert_eq!(progress.ready_height, Some(100));
    assert!(
        fixture
            .storage
            .get_balance_history_snapshot_anchor_at_height(101)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .storage
            .get_pass_block_commit(101)
            .unwrap()
            .is_none()
    );
    provider.set_block_commit(
        101,
        Some(MockBalanceHistoryCommitProvider::default_commit(101)),
    );
    assert_eq!(fixture.indexer.sync_once_for_test().await.unwrap(), 102);
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(102)
    );
    assert!(provider.state_ref_calls.lock().unwrap().is_empty());
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[tokio::test]
async fn legacy_backfill_failure_and_restart_resume_committed_batches_without_new_blocks() {
    let root = test_root_dir("anchor_acceptance", "legacy_restart");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    provider.fail_state_ref_at.store(165, Ordering::SeqCst);
    let fixture = indexer_fixture(root.clone(), 100, 170, provider.clone());
    fixture.storage.update_synced_btc_block_height(170).unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(170, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(
            &MockBalanceHistoryCommitProvider::default_commit(170),
        ))
        .unwrap();
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(100)
    );
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("Injected history RPC failure")
    );
    let progress = fixture
        .storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.synced_height, Some(170));
    assert_eq!(progress.ready_height, Some(163));
    assert_eq!(progress.pending_from, Some(164));
    drop(fixture);

    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let fixture = indexer_fixture(root.clone(), 100, 170, provider.clone());
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(164)
    );
    assert_eq!(fixture.indexer.sync_once_for_test().await.unwrap(), 170);
    assert_eq!(
        *provider.state_ref_calls.lock().unwrap(),
        (164..170).collect::<Vec<_>>()
    );
    let progress = fixture
        .storage
        .get_committed_snapshot_history_progress(100)
        .unwrap();
    assert_eq!(progress.ready_height, Some(170));
    assert_eq!(progress.pending_from, None);
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[tokio::test]
async fn legacy_backfill_rejects_an_anchor_from_another_chain() {
    let root = test_root_dir("anchor_acceptance", "legacy_mismatch");
    let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
    let fixture = indexer_fixture(root.clone(), 100, 101, provider.clone());
    fixture.storage.update_synced_btc_block_height(101).unwrap();
    fixture
        .storage
        .upsert_active_balance_snapshot(101, 0, 0)
        .unwrap();
    fixture
        .storage
        .upsert_balance_history_snapshot_anchor(&snapshot_from_commit(
            &MockBalanceHistoryCommitProvider::default_commit(101),
        ))
        .unwrap();
    fixture
        .storage
        .upsert_pass_block_commit(&PassBlockCommitEntry {
            block_height: 100,
            balance_history_block_height: 100,
            balance_history_block_commit: "ff".repeat(32),
            mutation_root: "aa".repeat(32),
            block_commit: "bb".repeat(32),
            commit_protocol_version: "1.0.0".into(),
            commit_hash_algo: "sha256".into(),
        })
        .unwrap();
    assert!(
        fixture
            .indexer
            .sync_once_for_test()
            .await
            .unwrap_err()
            .contains("backfill anchor mismatch")
    );
    assert_eq!(
        fixture
            .storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(100)
    );
    assert!(
        fixture
            .storage
            .get_balance_history_snapshot_anchor_at_height(100)
            .unwrap()
            .is_none()
    );
    drop(fixture);
    cleanup_temp_dir(&root);
}

#[test]
fn committed_reader_never_publishes_anchor_or_height_from_an_open_savepoint() {
    let root = test_root_dir("anchor_acceptance", "read_visibility");
    std::fs::create_dir_all(&root).unwrap();
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    for commit in [false, true] {
        let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
        storage
            .upsert_balance_history_block_anchor(
                &MockBalanceHistoryCommitProvider::default_commit(100),
                regtest_stable_lag(),
            )
            .unwrap();
        storage.update_synced_btc_block_height(100).unwrap();
        assert_eq!(
            storage.get_committed_synced_btc_block_height().unwrap(),
            None
        );
        assert_eq!(
            storage
                .get_committed_snapshot_history_progress(100)
                .unwrap()
                .ready_height,
            None
        );
        if commit {
            guard.commit().unwrap();
        } else {
            drop(guard);
        }
        assert_eq!(
            storage.get_committed_synced_btc_block_height().unwrap(),
            commit.then_some(100)
        );
        assert_eq!(
            storage
                .get_committed_snapshot_history_progress(100)
                .unwrap()
                .ready_height,
            commit.then_some(100)
        );
    }
    drop(storage);
    cleanup_temp_dir(&root);
}

#[test]
fn coverage_handles_sparse_history_reorg_and_maximum_height() {
    let root = test_root_dir("anchor_acceptance", "coverage_reorg");
    std::fs::create_dir_all(&root).unwrap();
    let storage = MinerPassStorage::new(&root).unwrap();
    storage.reconcile_snapshot_history_coverage(100).unwrap();
    storage.update_synced_btc_block_height(103).unwrap();
    for h in [103, 100, 102] {
        storage
            .upsert_balance_history_block_anchor(
                &MockBalanceHistoryCommitProvider::default_commit(h),
                regtest_stable_lag(),
            )
            .unwrap();
    }
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .pending_from,
        Some(101)
    );
    storage
        .upsert_balance_history_block_anchor(
            &MockBalanceHistoryCommitProvider::default_commit(101),
            regtest_stable_lag(),
        )
        .unwrap();
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(103)
    );
    storage.rollback_to_block_height(101, None).unwrap();
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(101)
    );
    assert!(
        storage
            .get_balance_history_snapshot_anchor_at_height(102)
            .unwrap()
            .is_none()
    );
    let mut replacement = MockBalanceHistoryCommitProvider::default_commit(102);
    replacement.btc_block_hash = "ff".repeat(32);
    replacement.block_commit = "ee".repeat(32);
    let guard = MinePassStorageSavePointGuard::new(&storage).unwrap();
    storage
        .upsert_balance_history_block_anchor(&replacement, regtest_stable_lag())
        .unwrap();
    storage.update_synced_btc_block_height(102).unwrap();
    guard.commit().unwrap();
    assert_eq!(
        storage
            .get_committed_snapshot_history_progress(100)
            .unwrap()
            .ready_height,
        Some(102)
    );
    assert_eq!(
        storage
            .get_balance_history_snapshot_anchor_at_height(102)
            .unwrap()
            .unwrap()
            .latest_block_commit,
        replacement.block_commit
    );
    storage
        .reconcile_snapshot_history_coverage(u32::MAX)
        .unwrap();
    storage
        .upsert_balance_history_block_anchor(
            &MockBalanceHistoryCommitProvider::default_commit(u32::MAX),
            regtest_stable_lag(),
        )
        .unwrap();
    storage.update_synced_btc_block_height(u32::MAX).unwrap();
    let progress = storage
        .get_committed_snapshot_history_progress(u32::MAX)
        .unwrap();
    assert_eq!(progress.ready_height, Some(u32::MAX));
    assert_eq!(progress.pending_from, None);
    drop(storage);
    cleanup_temp_dir(&root);
}
