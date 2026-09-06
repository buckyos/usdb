//! Fixtures shared by storage publication and indexer recovery acceptance tests.

use super::{
    Arc, IndexerFixture, MockBalanceHistoryCommitProvider, MockBalanceProvider,
    MockBlockHintProvider, MockInscriptionSource, MockStatus, MockTransferTracker, PathBuf,
    build_indexer_fixture_with_runtime_deps_at_root, build_test_block, snapshot_from_commit,
    write_test_config,
};

/// Open an isolated indexer with real stores and deterministic upstream blocks.
pub fn indexer_fixture(
    root: PathBuf,
    origin: u32,
    tip: u32,
    provider: Arc<MockBalanceHistoryCommitProvider>,
) -> IndexerFixture {
    if !root.join("config.json").exists() {
        write_test_config(&root, origin);
    }
    let status = Arc::new(MockStatus::new(tip));
    status.set_snapshot(snapshot_from_commit(
        &MockBalanceHistoryCommitProvider::default_commit(tip),
    ));
    let mut hints = MockBlockHintProvider::default();
    for height in origin..=tip {
        hints = hints.with_block(height, build_test_block(vec![]));
    }
    build_indexer_fixture_with_runtime_deps_at_root(
        root,
        Arc::new(MockInscriptionSource::default()),
        Arc::new(hints),
        Arc::new(MockTransferTracker::default()),
        vec![],
        Arc::new(MockBalanceProvider::default()),
        status,
        provider,
    )
}
