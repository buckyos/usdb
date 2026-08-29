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
fn failed_stateful_subcommand_keeps_stdout_clean_and_flushes_file_log() {
    let root = TestRoot::new("balance-history-command-logging");
    let output = Command::new(env!("CARGO_BIN_EXE_balance-history"))
        .arg("--root-dir")
        .arg(root.path())
        .args(["install-snapshot", "--file", "missing.db"])
        .env("USDB_PROCESS_LOG_LEVEL", "info")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not exist"));

    let log = fs::read_dir(root.path().join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_file())
        .map(fs::read_to_string)
        .unwrap()
        .unwrap();
    assert!(log.contains("Process started"));
    assert!(log.contains("Snapshot file does not exist"));
    assert!(log.contains("file_log="));
    assert!(log.contains("console_log=false"));
}
