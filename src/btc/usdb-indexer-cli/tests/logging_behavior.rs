use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestHome {
    path: PathBuf,
}

impl TestHome {
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

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn closed_local_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    format!("http://operator:topsecret@{address}")
}

#[test]
fn help_does_not_initialize_runtime_logging() {
    let output = Command::new(env!("CARGO_BIN_EXE_usdb-indexer-cli"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
}

#[test]
fn rpc_failure_uses_stderr_without_polluting_stdout_or_creating_log_files() {
    let home = TestHome::new("usdb-indexer-cli-logging");
    let output = Command::new(env!("CARGO_BIN_EXE_usdb-indexer-cli"))
        .args(["--url", &closed_local_endpoint(), "rpc-info"])
        .env("HOME", home.path())
        .env("USDB_PROCESS_LOG_LEVEL", "error")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("USDB indexer CLI command failed"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("topsecret"));
    assert!(!home.path().join(".usdb").exists());
}
