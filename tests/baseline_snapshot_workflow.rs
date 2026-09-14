#[path = "common/baseline_release.rs"]
mod common;

use common::{Fixture, fixture};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn command(root: &Path, fixture: &Fixture, action: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_balance-history-snapshot-tool"));
    cmd.current_dir(root)
        .arg("--root-dir")
        .arg(root.join("builder"))
        .arg("--json")
        .args([
            "baseline",
            action,
            "--height",
            "103",
            "--expected-block-hash",
            &fixture.hash,
        ]);
    cmd
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn abrupt_process_exit_resumes_atomic_rows_and_publication() {
    for stage in [
        "utxos",
        "balances",
        "script_registry",
        "data_finished",
        "verified",
        "published",
    ] {
        let temp = TempDir::new().unwrap();
        let fixture = fixture(temp.path());
        let output = command(temp.path(), &fixture, "create")
            .args(["--network", "regtest", "--batch-size", "7"])
            .arg("--source-root")
            .arg(&fixture.source)
            .arg("--genesis-block")
            .arg(&fixture.block)
            .arg("--signing-key")
            .arg(&fixture.signing)
            .arg("--trusted-keys")
            .arg(&fixture.trust)
            .env(
                "USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT",
                format!("baseline_{stage}"),
            )
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(86),
            "{stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if stage == "utxos" {
            let status = success(command(temp.path(), &fixture, "status").output().unwrap());
            assert_eq!(status["checkpoint"], serde_json::json!(["utxos", 7]));
        }
        let resumed = success(
            command(temp.path(), &fixture, "resume")
                .arg("--signing-key")
                .arg(&fixture.signing)
                .arg("--trusted-keys")
                .arg(&fixture.trust)
                .output()
                .unwrap(),
        );
        assert_eq!(resumed["stage"], "complete");
        success(
            command(temp.path(), &fixture, "verify")
                .arg("--trusted-keys")
                .arg(&fixture.trust)
                .output()
                .unwrap(),
        );
    }
}

#[test]
fn real_wrapper_conversion_finalization_and_simulated_publication() {
    let temp = TempDir::new().unwrap();
    let fixture = fixture(temp.path());
    let inputs = serde_json::json!({
        "source":fixture.source,"block":fixture.block,"hash":fixture.hash,"key_root":fixture.key_root,
        "signing":fixture.signing,"trust":fixture.trust,"core":fixture.core,"registry":fixture.registry,
    });
    let file = temp.path().join("fixture.json");
    std::fs::write(&file, serde_json::to_vec(&inputs).unwrap()).unwrap();
    let output = Command::new("python3")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/baseline_snapshot_workflow.py"),
        )
        .arg(temp.path())
        .arg(env!("CARGO_BIN_EXE_balance-history-snapshot-tool"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
