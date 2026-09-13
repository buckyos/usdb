//! Regression for a polled snapshot overtaken by the block stream during a reorg.

use std::sync::Arc;

use super::{
    MockBalanceHistoryCommitProvider, MockBalanceProvider, MockBlockHintProvider,
    MockInscriptionSource, MockStatus, MockTransferTracker,
    build_indexer_fixture_with_runtime_deps_at_root, build_test_block, cleanup_temp_dir,
    mock_balance_history_commit, snapshot_from_commit, test_root_dir, write_test_config,
};

#[tokio::test]
async fn stale_polled_snapshot_cannot_replace_committed_block_anchor() {
    for hash_only in [false, true] {
        let root = test_root_dir("stale_anchor", if hash_only { "hash" } else { "branch" });
        write_test_config(&root, 100);
        let current = mock_balance_history_commit(100, "a", "b", "c");
        let mut stale = snapshot_from_commit(&current);
        stale.stable_block_hash = Some("d".repeat(64));
        if !hash_only {
            stale.latest_block_commit = Some("e".repeat(64));
        }
        let status = Arc::new(MockStatus::new(100));
        status.set_snapshot(stale);
        let provider = Arc::new(MockBalanceHistoryCommitProvider::default());
        provider.set_block_commit(100, Some(current.clone()));
        let fixture = build_indexer_fixture_with_runtime_deps_at_root(
            root.clone(),
            Arc::new(MockInscriptionSource::default()),
            Arc::new(MockBlockHintProvider::default().with_block(100, build_test_block(vec![]))),
            Arc::new(MockTransferTracker::default()),
            vec![],
            Arc::new(MockBalanceProvider::default()),
            status.clone(),
            provider,
        );

        // The polled head is stale, while per-block RPC already supplies the replacement chain.
        let error = fixture.indexer.sync_once_for_test().await.unwrap_err();
        assert!(
            error.contains("does not match committed block anchor"),
            "{error}"
        );
        assert_eq!(
            fixture.storage.get_synced_btc_block_height().unwrap(),
            Some(100)
        );
        let committed = fixture
            .storage
            .get_balance_history_snapshot_anchor_at_height(100)
            .unwrap()
            .unwrap();
        assert_eq!(committed.stable_block_hash, current.btc_block_hash);
        assert_eq!(committed.latest_block_commit, current.block_commit);
        assert!(
            fixture
                .storage
                .get_balance_history_snapshot_anchor()
                .unwrap()
                .is_none()
        );
        let pass_commit = fixture.storage.get_pass_block_commit(100).unwrap().unwrap();
        assert_eq!(
            pass_commit.balance_history_block_commit,
            current.block_commit
        );

        // A refreshed poll adopts the existing durable block without replay or anchor corruption.
        status.set_snapshot(snapshot_from_commit(&current));
        assert_eq!(fixture.indexer.sync_once_for_test().await.unwrap(), 100);
        let adopted = fixture
            .storage
            .get_balance_history_snapshot_anchor()
            .unwrap()
            .unwrap();
        assert_eq!(adopted.stable_block_hash, current.btc_block_hash);
        assert_eq!(adopted.latest_block_commit, current.block_commit);
        assert_eq!(
            fixture
                .storage
                .get_pass_block_commit(100)
                .unwrap()
                .unwrap()
                .block_commit,
            pass_commit.block_commit
        );
        assert_eq!(fixture.transfer_tracker.commit_call_count(), 1);
        cleanup_temp_dir(&root);
    }
}
