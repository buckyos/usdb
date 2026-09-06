#[path = "common/split_snapshot_audit.rs"]
mod common;

use balance_history::snapshot_audit::SplitSnapshotAudit;
use balance_history::{
    LegacySnapshotIntegrityCheck, LegacyStateCompareOptions, compare_legacy_split_snapshot,
};
use common::{Fixture, fixture};
use rusqlite::Connection;
use tempfile::TempDir;

fn inputs(value: &Fixture, hash: bool) -> Result<SplitSnapshotAudit, String> {
    SplitSnapshotAudit::open(&value.core, None, Some(&value.registry), None, hash)
}

fn options(value: &Fixture) -> LegacyStateCompareOptions {
    LegacyStateCompareOptions {
        snapshot_db: value.legacy.clone(),
        target_height: 10,
        include_script_registry: true,
        parallelism: 2,
        max_examples: 4,
        integrity_check: LegacySnapshotIntegrityCheck::Quick,
    }
}

#[test]
fn all_tables_match_without_creating_sqlite_sidecars() {
    let temp = TempDir::new().unwrap();
    let value = fixture(temp.path());
    let names = || {
        let mut paths = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    };
    let before = names();
    let report =
        compare_legacy_split_snapshot(&options(&value), inputs(&value, true).unwrap(), None)
            .unwrap();
    assert!(report.ok);
    assert_eq!(report.tables.len(), 4);
    assert_eq!(
        report
            .tables
            .iter()
            .map(|table| table.current_rows)
            .collect::<Vec<_>>(),
        [8, 8, 2, 12]
    );
    assert!(report.current.file_hashes_verified);
    assert_eq!(before, names());
}

#[test]
fn mutations_and_count_mismatches_fail_the_full_scan() {
    for sql in [
        "UPDATE balance_history SET balance=balance+1 WHERE script_hash=(SELECT script_hash FROM balance_history LIMIT 1)",
        "UPDATE utxos SET value=value+1 WHERE outpoint=(SELECT outpoint FROM utxos LIMIT 1)",
        "UPDATE block_commits SET btc_block_hash=zeroblob(32) WHERE block_height=9",
        "UPDATE meta SET utxo_count=utxo_count+1",
    ] {
        let temp = TempDir::new().unwrap();
        let value = fixture(temp.path());
        Connection::open(&value.core)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        let report =
            compare_legacy_split_snapshot(&options(&value), inputs(&value, false).unwrap(), None)
                .unwrap();
        assert!(!report.ok, "{sql}");
        assert!(report.unexpected_difference_rows > 0);
        assert!(
            inputs(&value, true)
                .unwrap_err()
                .contains("SHA256 mismatch")
        );
    }
}

#[test]
fn registry_difference_is_detected_and_missing_registry_is_rejected() {
    let temp = TempDir::new().unwrap();
    let value = fixture(temp.path());
    Connection::open(&value.registry).unwrap().execute_batch("UPDATE script_registry SET script_pubkey=x'51' WHERE script_hash=(SELECT script_hash FROM script_registry LIMIT 1)").unwrap();
    let report =
        compare_legacy_split_snapshot(&options(&value), inputs(&value, false).unwrap(), None)
            .unwrap();
    assert!(!report.ok);
    assert!(
        report
            .tables
            .last()
            .unwrap()
            .unexpected_by_kind
            .contains_key("script_registry_value_mismatch")
    );
    let core_only = SplitSnapshotAudit::open(&value.core, None, None, None, false).unwrap();
    assert!(
        compare_legacy_split_snapshot(&options(&value), core_only, None)
            .unwrap_err()
            .contains("requires --script-registry-db")
    );
}

#[test]
fn incorrect_registry_anchors_are_rejected_before_scanning() {
    for (field, replacement) in [
        ("btc_network", "bitcoin"),
        ("base_height", "11"),
        ("base_block_hash", "wrong"),
        ("core_snapshot_id", "wrong"),
    ] {
        let temp = TempDir::new().unwrap();
        let value = fixture(temp.path());
        Connection::open(&value.registry)
            .unwrap()
            .execute(&format!("UPDATE meta SET {field}=?1"), [replacement])
            .unwrap();
        assert!(inputs(&value, false).is_err(), "{field}");
    }
}

#[test]
fn nonempty_wal_is_never_silently_ignored() {
    let temp = TempDir::new().unwrap();
    let value = fixture(temp.path());
    std::fs::write(value.registry.with_extension("db-wal"), b"pending").unwrap();
    assert!(inputs(&value, false).unwrap_err().contains("nonempty WAL"));
}

#[test]
fn cli_writes_full_report_and_rejects_ambiguous_sources() {
    let temp = TempDir::new().unwrap();
    let value = fixture(temp.path());
    let output = temp.path().join("report.json");
    let binary = env!("CARGO_BIN_EXE_balance-history-snapshot-tool");
    let run = std::process::Command::new(binary)
        .arg("--root-dir")
        .arg(temp.path())
        .args([
            "--json",
            "compare-legacy",
            "--height",
            "10",
            "--snapshot-db",
        ])
        .arg(&value.legacy)
        .arg("--core-snapshot-db")
        .arg(&value.core)
        .arg("--include-script-registry")
        .arg("--script-registry-db")
        .arg(&value.registry)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["tables"].as_array().unwrap().len(), 4);
    let ambiguous = std::process::Command::new(binary)
        .args(["compare-legacy", "--height", "10", "--snapshot-db"])
        .arg(&value.legacy)
        .arg("--core-snapshot-db")
        .arg(&value.core)
        .arg("--balance-history-root")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
}

#[test]
fn mismatched_legacy_target_and_core_only_mode_are_checked() {
    let temp = TempDir::new().unwrap();
    let value = fixture(temp.path());
    let mut options = options(&value);
    options.include_script_registry = false;
    let core_only = || SplitSnapshotAudit::open(&value.core, None, None, None, false).unwrap();
    let report = compare_legacy_split_snapshot(&options, core_only(), None).unwrap();
    assert!(report.ok);
    assert_eq!(report.tables.len(), 3);
    Connection::open(&value.legacy)
        .unwrap()
        .execute_batch("UPDATE block_commits SET btc_block_hash=zeroblob(32) WHERE block_height=10")
        .unwrap();
    assert!(
        compare_legacy_split_snapshot(&options, core_only(), None)
            .unwrap_err()
            .contains("BTC block hash mismatch")
    );
}
