//! Kill separate test-binary processes at real RocksDB/bootstrap boundaries, then reopen them.

use super::test_common::{Fixture, Workspace};
use super::*;
use crate::bootstrap::{NativeBootstrapPhase, prepare_native_bootstrap};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WORKER: &str = "assumeutxo::process_tests::native_bootstrap_process_worker";

struct Evidence {
    workspace: Workspace,
    phase: &'static str,
}

impl Evidence {
    fn new(phase: &'static str) -> Self {
        Self {
            workspace: Workspace::new(),
            phase,
        }
    }
}

impl std::ops::Deref for Evidence {
    type Target = Workspace;
    fn deref(&self) -> &Workspace {
        &self.workspace
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        // Preserve small diagnostics before Workspace removes the isolated databases.
        let base = std::env::var_os("USDB_TEST_ARTIFACT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("usdb-bootstrap-process-evidence"));
        let output = base.join(self.0.file_name().unwrap());
        let saved = (|| -> std::io::Result<()> {
            fs::create_dir_all(&output)?;
            for name in [
                "worker.log",
                "boundaries.log",
                "reached.json",
                "result.json",
            ] {
                if self.0.join(name).is_file() {
                    fs::copy(self.0.join(name), output.join(name))?;
                }
            }
            fs::write(
                output.join("summary.json"),
                serde_json::to_vec(&serde_json::json!({
                    "phase":self.phase, "status":if std::thread::panicking() {"failed"} else {"ok"}
                }))?,
            )?;
            Ok(())
        })();
        if let Err(error) = saved {
            eprintln!("P1 evidence export failed: {error}");
        }
    }
}

fn fixture() -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        "blocks",
    )
}

struct Worker(Child);

impl Worker {
    fn start(root: &Path, phase: &str) -> Self {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("worker.log"))
            .unwrap();
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", WORKER, "--nocapture"])
                .env("USDB_TEST_BOOTSTRAP_PROCESS_ROOT", root)
                .env("USDB_TEST_BOOTSTRAP_PAUSE", phase)
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        )
    }

    fn wait_for(&mut self, root: &Path, paused: bool) {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(
                    !paused && status.success(),
                    "worker exited {status}: {}",
                    fs::read_to_string(root.join("worker.log")).unwrap()
                );
                return;
            }
            if paused && root.join("reached.json").exists() {
                let value: serde_json::Value =
                    serde_json::from_slice(&fs::read(root.join("reached.json")).unwrap()).unwrap();
                assert_eq!(value["pid"], self.0.id());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "worker did not reach expected boundary: {}",
                root.display()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "child process entry; invoked only by the process recovery acceptance"]
fn native_bootstrap_process_worker() {
    let root = PathBuf::from(std::env::var("USDB_TEST_BOOTSTRAP_PROCESS_ROOT").unwrap());
    let config: crate::BalanceHistoryConfig =
        serde_json::from_slice(&fs::read(root.join("config.json")).unwrap()).unwrap();
    let cfg = Arc::new(config);
    let chain = fixture();
    let state = prepare_native_bootstrap(cfg.clone(), chain.client_with_stable_tip(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(state.phase, NativeBootstrapPhase::Sealed);
    let db = BalanceHistoryDB::open_read_only(cfg).unwrap();
    db.validate_native_bootstrap_service().unwrap();
    let commit = db.get_block_commit(103).unwrap().unwrap();
    let mut value = serde_json::json!({"state":state, "commit":{
        "height":commit.block_height, "hash":commit.btc_block_hash,
        "delta_root":format::hex(&commit.balance_delta_root), "commit":format::hex(&commit.block_commit)},
        "floors":db.get_query_retention_floors().unwrap(), "height":db.get_btc_block_height().unwrap()});
    // Duration is observational, not part of the persisted logical identity.
    value["state"]["source_verification"]
        .as_object_mut()
        .unwrap()
        .remove("elapsed_seconds");
    fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn configure(work: &Workspace) -> Arc<crate::BalanceHistoryConfig> {
    let cfg = fixture().native_config(&work.0, 103);
    fs::write(
        work.0.join("config.json"),
        serde_json::to_vec(&*cfg).unwrap(),
    )
    .unwrap();
    cfg
}

#[test]
fn native_bootstrap_process_recovers_import_replay_verify_and_publication() {
    use std::os::unix::process::ExitStatusExt;
    let reference = Evidence::new("reference");
    configure(&reference);
    Worker::start(&reference.0, "").wait_for(&reference.0, false);
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(reference.0.join("result.json")).unwrap()).unwrap();
    for phase in [
        "import_committed",
        "replay_committed",
        "verifying",
        "sealed_before_publish",
    ] {
        let work = Evidence::new(phase);
        let cfg = configure(&work);
        let scratch_parent = cfg.root_dir.join("bootstrap-staging/db");
        // Repeat the verification crash to catch unbounded abandoned workspace growth.
        for _ in 0..if phase == "verifying" { 2 } else { 1 } {
            let _ = fs::remove_file(work.0.join("reached.json"));
            let mut worker = Worker::start(&work.0, phase);
            worker.wait_for(&work.0, true);
            assert!(!cfg.db_dir().join("balance_history").exists());
            worker.0.kill().unwrap();
            assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
            let mut staging = (*cfg).clone();
            staging.root_dir = cfg.root_dir.join("bootstrap-staging");
            let staged = BalanceHistoryDB::open_read_only(Arc::new(staging)).unwrap();
            let saved = staged.get_native_bootstrap_state().unwrap().unwrap();
            assert!(saved.imported_coins > 0);
            if phase == "sealed_before_publish" {
                assert_eq!(saved.phase, NativeBootstrapPhase::Sealed);
                fs::remove_file(&cfg.bootstrap.as_ref().unwrap().snapshot_file).unwrap();
            } else {
                assert!(staged.validate_native_bootstrap_service().is_err());
            }
            if phase == "verifying" {
                assert_eq!(
                    fs::read_dir(&scratch_parent)
                        .unwrap()
                        .filter(|e| e
                            .as_ref()
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with("native-verify-"))
                        .count(),
                    1
                );
            }
        }
        Worker::start(&work.0, "").wait_for(&work.0, false);
        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(work.0.join("result.json")).unwrap()).unwrap();
        assert_eq!(actual, expected, "changed state after {phase}");
        assert!(!fs::read_dir(&scratch_parent).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("native-verify-")
        }));
        // Published state is reusable even without the import source and cannot revisit a fault point.
        let _ = fs::remove_file(&cfg.bootstrap.as_ref().unwrap().snapshot_file);
        let before = fs::read(work.0.join("boundaries.log")).unwrap();
        Worker::start(&work.0, "import_committed").wait_for(&work.0, false);
        assert_eq!(fs::read(work.0.join("boundaries.log")).unwrap(), before);
    }
}
