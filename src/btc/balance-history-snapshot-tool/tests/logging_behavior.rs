use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("usdb-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn json_result_stays_on_stdout_and_persistent_log_is_flushed() {
    let root = TestRoot::new("snapshot-tool-logging");
    let output = Command::new(env!("CARGO_BIN_EXE_balance-history-snapshot-tool"))
        .arg("--root-dir")
        .arg(root.path())
        .args(["--json", "memory-plan"])
        .env("USDB_PROCESS_LOG_LEVEL", "info")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).unwrap();
    assert!(output.stderr.is_empty());

    let log_dir = root.path().join("logs");
    let log_files = fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    assert_eq!(log_files.len(), 1);

    let log = fs::read_to_string(&log_files[0]).unwrap();
    assert!(log.contains("Process started"));
    assert!(log.contains("binary=balance-history-snapshot-tool"));
    assert!(log.contains("console_log=false"));
}

#[test]
fn json_failure_uses_stderr_and_is_persisted_before_exit() {
    let root = TestRoot::new("snapshot-tool-error-logging");
    let output = Command::new(env!("CARGO_BIN_EXE_balance-history-snapshot-tool"))
        .arg("--root-dir")
        .arg(root.path())
        .args([
            "--json",
            "memory-plan",
            "--cache-budget-percent",
            "90",
            "--max-memory-percent",
            "80",
        ])
        .env("USDB_PROCESS_LOG_LEVEL", "info")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());

    let log = fs::read_dir(root.path().join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_file())
        .map(fs::read_to_string)
        .unwrap()
        .unwrap();
    assert!(log.contains("Snapshot tool command failed"));
}
