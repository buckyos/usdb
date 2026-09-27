//! Isolate the process-global logger while exercising real file output and restart behavior.

use std::path::Path;

pub fn run_log_process(test: &str, root: &Path, mode: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("USDB_TEST_LOG_ROOT", root)
        .env("USDB_TEST_LOG_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Log child failed: mode={mode}, stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn read_logs(root: &Path) -> String {
    std::fs::read_dir(root.join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}
