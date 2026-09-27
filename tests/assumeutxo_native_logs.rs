//! Verify native lifecycle summaries in the production file logger across process restarts.

#[path = "common/logging.rs"]
mod logging;

use super::test_common::{Fixture, Workspace};
use crate::bootstrap::{NativeBootstrapPhase, prepare_native_bootstrap};
use std::fs;
use std::path::PathBuf;

#[test]
fn native_verification_file_logs_survive_restart_and_report_cancellation() {
    // A child has its own global logger, independent of concurrently running DB tests.
    if let Some(root) = std::env::var_os("USDB_TEST_LOG_ROOT") {
        let root = PathBuf::from(root);
        fs::create_dir_all(&root).unwrap();
        let mode = std::env::var("USDB_TEST_LOG_MODE").unwrap();
        let chain = Fixture::load_at(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
            "blocks",
        );
        let cfg = chain.native_config(&root, 103);
        let logger = usdb_util::init_log(
            usdb_util::LogConfig::new("balance-history")
                .with_service_root_dir(cfg.root_dir.clone())
                .with_level("info")
                .enable_console(false),
        )
        .unwrap();
        let result = prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| {
            if mode == "reuse" {
                panic!("A sealed restart must not enter cancellable import or verification work");
            }
            mode == "cancel"
                && fs::read(cfg.root_dir.join("bootstrap-progress.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_some_and(|p| p["verification_stage"] == "utxos_and_balance_aggregation")
        });
        if mode == "cancel" {
            assert!(result.unwrap_err().contains("cancelled during origin scan"));
            assert!(!cfg.db_dir().join("balance_history").exists());
        } else {
            assert_eq!(result.unwrap().unwrap().phase, NativeBootstrapPhase::Sealed);
        }
        logger.shutdown();
        return;
    }

    let work = Workspace::new();
    let test = "assumeutxo::native_log_tests::native_verification_file_logs_survive_restart_and_report_cancellation";
    let success = work.0.join("success");
    logging::run_log_process(test, &success, "success");
    let log_root = success.join("native");
    let before = logging::read_logs(&log_root);
    for message in [
        "Native balance verification started:",
        "Bootstrap origin scan started: table=utxos",
        "Bootstrap origin scan finished: table=utxos",
        "Bootstrap origin scan finished: table=balances",
        "Native balance comparison started:",
        "Native balance comparison finished:",
        "matched=true",
        "Native balance verification finished:",
        "Native bootstrap sealing started:",
        "Native bootstrap published:",
        "origin_commit=",
        "sha256=",
        "scanned_per_second=",
        "comparison_elapsed_seconds=",
    ] {
        assert!(
            before.contains(message),
            "Missing persisted milestone: {message}"
        );
    }
    assert!(!before.contains("Native balance verification failed:"));
    let progress = fs::read(log_root.join("bootstrap-progress.json")).unwrap();
    logging::run_log_process(test, &success, "reuse");
    let after = logging::read_logs(&log_root);
    assert!(after.contains("Native bootstrap reused:"));
    assert_eq!(
        after
            .matches("Native balance verification started:")
            .count(),
        1
    );
    assert_eq!(
        fs::read(log_root.join("bootstrap-progress.json")).unwrap(),
        progress
    );

    let cancel = work.0.join("cancel");
    logging::run_log_process(test, &cancel, "cancel");
    let failed = logging::read_logs(&cancel.join("native"));
    assert!(failed.contains("Native balance verification failed: height=103, stage=utxos_and_balance_aggregation, last_observed_scanned=0"));
    assert!(failed.contains("cancelled during origin scan"));
    assert!(failed.contains("Native bootstrap failed:"));
    assert!(!failed.contains("Native balance verification finished:"));
    assert!(!failed.contains("Native bootstrap published:"));
}
