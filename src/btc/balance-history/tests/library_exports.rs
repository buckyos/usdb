use balance_history::db::BalanceHistoryDBMode;
use balance_history::status::{SyncPhase, SyncStatusManager};

#[test]
fn integration_tests_can_import_core_modules() {
    let _db_mode = BalanceHistoryDBMode::BestEffort;

    let status = SyncStatusManager::new();
    let current = status.get_status();

    assert_eq!(current.phase, SyncPhase::Initializing);
    assert_eq!(current.current, 0);
    assert_eq!(current.total, 0);
}
