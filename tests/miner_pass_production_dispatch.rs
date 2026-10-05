//! The actual non-test binary must reject the executors accepted only by cfg(test) pipelines.
#[path = "common/miner_pass_conformance.rs"]
mod conformance;

use conformance::{CONFORMANCE_SCHEMA, CONFORMANCE_SCOPE, CONFORMANCE_STATE, conformance_catalog};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use usdb_util::{BtcActivationRegistryCatalog, VersionFamily};

#[test]
fn ordinary_binary_rejects_conformance_schema_and_state_before_opening_stores() {
    for (family, version) in [
        (VersionFamily::InscriptionSchemaVersion, CONFORMANCE_SCHEMA),
        (VersionFamily::PassStateMachineVersion, CONFORMANCE_STATE),
    ] {
        let root = std::env::temp_dir().join(format!(
            "usdb-production-dispatch-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let catalog = conformance_catalog(&[(family, 10, version)]);
        let id = BtcActivationRegistryCatalog::from_json(&catalog)
            .unwrap()
            .current_registry_id()
            .to_owned();
        std::fs::write(root.join("catalog.json"), catalog).unwrap();
        let config = serde_json::json!({
            "isolate":null,
            "bitcoin":{"network":"regtest","rpc_url":"http://127.0.0.1:1","auth":"None"},
            "ordinals":{"rpc_url":"http://127.0.0.1:1"},
            "balance_history":{"rpc_url":"http://127.0.0.1:1"},
            "usdb":{"genesis_block_height":10,"rules_scope":CONFORMANCE_SCOPE,
                "activation_registry_id":id,"activation_registry_catalog_file":"catalog.json",
                "inscription_source":"bitcoind","rpc_server_enabled":false}
        });
        std::fs::write(root.join("config.json"), config.to_string()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_usdb-indexer"))
            .args(["--skip-process-lock", "--root-dir"])
            .arg(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let mut timed_out = false;
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                timed_out = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let pass_created = root.join("data/miner_pass.db").exists();
        let energy_created = root.join("data/energy").exists();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            !timed_out,
            "Production binary did not reject {version}: {stdout}\n{stderr}"
        );
        assert!(!output.status.success(), "{version}: {stdout}\n{stderr}");
        assert!(
            stdout.contains("Unsupported MinerPass rule combination") && stdout.contains(version),
            "{stdout}\n{stderr}"
        );
        assert!(
            !pass_created && !energy_created,
            "Unsupported rules opened stores"
        );
    }
}
