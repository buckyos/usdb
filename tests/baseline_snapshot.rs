//! Normalize real-Core full/native sources, then verify semantics and signed corruption cases.

use super::*;
use crate::assumeutxo::test_common::{Fixture, Workspace};
use crate::bootstrap::prepare_native_bootstrap;
use crate::{
    BalanceHistoryConfig, BalanceHistoryDB, BalanceHistoryDBMode, ScriptRegistryEntry,
    SnapshotSigningKeyFile,
};
use base64::Engine;
use bitcoincore_rpc::bitcoin::{ScriptBuf, consensus};
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use usdb_util::ToBtcScriptHash;

fn chain() -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        "blocks",
    )
}

fn options(work: &Path, source: PathBuf, name: &str, fixture: &Fixture) -> BaselineExportOptions {
    let signing = work.join("signing-key.json");
    let trusted = work.join("trusted-keys.json");
    let key = SnapshotSigningKeyFile {
        key_id: "baseline-test".to_owned(),
        secret_key_base64: base64::engine::general_purpose::STANDARD.encode([7; 32]),
    };
    fs::write(&signing, serde_json::to_vec(&key).unwrap()).unwrap();
    fs::write(&trusted, serde_json::to_vec(&serde_json::json!({"keys":[{"key_id":key.key_id,
        "public_key_base64":base64::engine::general_purpose::STANDARD.encode(key.to_signing_key().unwrap().verifying_key().to_bytes())}]})).unwrap()).unwrap();
    let height = fixture.blocks.len() as u32 - 1;
    let block = &fixture.blocks[height as usize];
    let raw = work.join("genesis.block");
    fs::write(&raw, consensus::serialize(block)).unwrap();
    BaselineExportOptions {
        source_root: source,
        network: Network::Regtest,
        height,
        block_hash: block.block_hash(),
        genesis_block_file: raw,
        output_dir: work.join(name),
        signing_key_file: signing,
        trusted_keys_file: trusted,
    }
}

fn manifest(options: &BaselineExportOptions) -> PathBuf {
    options.output_dir.join(format!(
        "balance_history_baseline_{}.manifest.json",
        options.height
    ))
}

fn source_config(root: &Path) -> Arc<BalanceHistoryConfig> {
    let mut cfg = BalanceHistoryConfig {
        root_dir: root.to_path_buf(),
        ..Default::default()
    };
    cfg.btc.network = Network::Regtest;
    Arc::new(cfg)
}

#[test]
fn full_and_native_exports_have_equal_normalized_state_and_precise_registry() {
    let work = Workspace::new();
    let fixture = chain();
    let full = work.0.join("full");
    fixture.reference(&full, &work.0.join("reference.db"));
    let unrelated = ScriptBuf::from_bytes(vec![0x51, 0x75, 0x75, 0x51]);
    {
        let db =
            BalanceHistoryDB::open(source_config(&full), BalanceHistoryDBMode::Normal).unwrap();
        db.put_script_registry_entries(&[ScriptRegistryEntry {
            script_hash: unrelated.to_btc_script_hash(),
            script_pubkey: unrelated.clone(),
        }])
        .unwrap();
    }
    let native = fixture.native_config(&work.0, 103);
    prepare_native_bootstrap(native.clone(), fixture.client_with_stable_tip(), &|| false).unwrap();
    let a = options(&work.0, full, "full-export", &fixture);
    let b = options(&work.0, native.root_dir.clone(), "native-export", &fixture);
    let full = export_baseline_snapshot(&a).unwrap();
    let native = export_baseline_snapshot(&b).unwrap();
    assert_eq!(full.state, native.state);
    assert_eq!(full.logical_sha256, native.logical_sha256);
    assert!(matches!(full.source, BaselineSource::FullReplay { .. }));
    assert!(matches!(native.source, BaselineSource::Assumeutxo { .. }));
    assert_eq!(full.state.identity.balance_query_floor, 103);
    assert_eq!(full.state.identity.history_query_floor, 104);
    assert_eq!(full.state.tables["block_commits"].rows, 1);
    let independent = std::process::Command::new("python3")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../tests/common/baseline_snapshot_golden.py"),
        )
        .arg(a.output_dir.join(&full.file_name))
        .output()
        .unwrap();
    assert!(
        independent.status.success(),
        "{}",
        String::from_utf8_lossy(&independent.stderr)
    );
    let independent: serde_json::Value = serde_json::from_slice(&independent.stdout).unwrap();
    assert_eq!(
        independent["state"],
        serde_json::to_value(&full.state).unwrap()
    );
    assert_eq!(independent["logical_sha256"], full.logical_sha256);
    let db = Connection::open(a.output_dir.join(&full.file_name)).unwrap();
    let live: BTreeSet<Vec<u8>> = db
        .prepare("SELECT DISTINCT script_hash FROM utxos")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let spent_in_genesis = ScriptBuf::from_bytes(vec![0x51, 0x61]).to_btc_script_hash();
    assert!(!live.contains(spent_in_genesis.as_ref() as &[u8]));
    let mut expected = live;
    for tx in &fixture.blocks[103].txdata {
        for output in &tx.output {
            expected.insert((output.script_pubkey.to_btc_script_hash().as_ref() as &[u8]).to_vec());
        }
    }
    let actual: BTreeSet<Vec<u8>> = db
        .prepare("SELECT script_hash FROM script_registry")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(actual, expected);
    assert!(actual.contains(spent_in_genesis.as_ref() as &[u8]));
    assert!(!actual.contains(unrelated.to_btc_script_hash().as_ref() as &[u8]));
    let zero: i64 = db
        .query_row("SELECT count(*) FROM utxos WHERE value=0", [], |r| r.get(0))
        .unwrap();
    assert!(zero > 0);
    drop(db);
    verify_baseline_snapshot(&manifest(&b), &b.trusted_keys_file).unwrap();
    assert!(
        export_baseline_snapshot(&a)
            .unwrap_err()
            .contains("new directory")
    );
}

// Re-sign modified bytes as a trusted producer to exercise semantic validation,
// independently of the already-tested transfer hash/signature checks.
fn resign(options: &BaselineExportOptions) {
    let path = manifest(options);
    let mut m: BaselineManifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let db = options.output_dir.join(&m.file_name);
    m.file_sha256 = storage::file_hash(&db).unwrap();
    m.file_size = fs::metadata(db).unwrap().len();
    fs::write(&path, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    crate::index::sign_snapshot_artifact_manifest(
        &SnapshotSigningKeyFile::load(&options.signing_key_file).unwrap(),
        &path,
        &m.signature_payload().unwrap(),
    )
    .unwrap();
}

#[test]
fn signed_file_checks_reject_semantic_corruption_and_unknown_signer() {
    let work = Workspace::new();
    let fixture = chain();
    let source = work.0.join("full");
    fixture.reference(&source, &work.0.join("reference.db"));
    let good = options(&work.0, source, "good", &fixture);
    let exported = export_baseline_snapshot(&good).unwrap();
    for (name, sql, expected) in [
        (
            "balance",
            "UPDATE balances SET balance=balance+1 WHERE script_hash=(SELECT script_hash FROM balances LIMIT 1)",
            "per-script balance",
        ),
        (
            "same_total",
            "UPDATE balances SET balance=balance+1 WHERE script_hash=(SELECT script_hash FROM balances WHERE balance>1 ORDER BY script_hash LIMIT 1); UPDATE balances SET balance=balance-1 WHERE script_hash=(SELECT script_hash FROM balances WHERE balance>1 ORDER BY script_hash DESC LIMIT 1)",
            "per-script balance",
        ),
        (
            "missing_script",
            "DELETE FROM script_registry WHERE script_hash=(SELECT script_hash FROM script_registry LIMIT 1)",
            "mapping is missing",
        ),
        (
            "wrong_script",
            "UPDATE script_registry SET script_pubkey=x'51' WHERE script_hash=(SELECT script_hash FROM script_registry LIMIT 1)",
            "script/hash mismatch",
        ),
        (
            "extra_script",
            "INSERT INTO script_registry VALUES (zeroblob(32),x'51')",
            "unrelated historical",
        ),
        (
            "commit",
            "UPDATE block_commits SET block_height=102",
            "original genesis commit",
        ),
        (
            "schema",
            "CREATE TABLE unexpected (value TEXT)",
            "schema mismatch",
        ),
        (
            "schema_prefix",
            "CREATE TABLE sqliteXunexpected (value TEXT)",
            "schema mismatch",
        ),
    ] {
        let bad = BaselineExportOptions {
            output_dir: work.0.join(name),
            ..options(&work.0, good.source_root.clone(), name, &fixture)
        };
        fs::create_dir(&bad.output_dir).unwrap();
        for entry in fs::read_dir(&good.output_dir).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), bad.output_dir.join(entry.file_name())).unwrap();
        }
        let db = Connection::open(bad.output_dir.join(&exported.file_name)).unwrap();
        db.execute_batch(sql).unwrap();
        db.close().unwrap();
        resign(&bad);
        let error = verify_baseline_snapshot(&manifest(&bad), &bad.trusted_keys_file).unwrap_err();
        assert!(error.contains(expected), "{name}: {error}");
    }
    let path = manifest(&good);
    let original = fs::read(&path).unwrap();
    let mut unsigned: BaselineManifest = serde_json::from_slice(&original).unwrap();
    unsigned.generated_at += 1;
    fs::write(&path, serde_json::to_vec_pretty(&unsigned).unwrap()).unwrap();
    assert!(
        verify_baseline_snapshot(&path, &good.trusted_keys_file)
            .unwrap_err()
            .contains("signature verification failed")
    );
    fs::write(&path, original).unwrap();
    let sidecar = good.output_dir.join(format!("{}-wal", exported.file_name));
    fs::write(&sidecar, b"unsigned WAL").unwrap();
    assert!(
        verify_baseline_snapshot(&path, &good.trusted_keys_file)
            .unwrap_err()
            .contains("sidecar is not allowed")
    );
    fs::remove_file(sidecar).unwrap();
    fs::write(&good.trusted_keys_file, r#"{"keys":[]}"#).unwrap();
    assert!(
        verify_baseline_snapshot(&manifest(&good), &good.trusted_keys_file)
            .unwrap_err()
            .contains("not trusted")
    );
}

#[test]
fn exporter_rejects_wrong_height_block_and_unsealed_source() {
    let work = Workspace::new();
    let fixture = chain();
    let source = work.0.join("full");
    fixture.reference(&source, &work.0.join("reference.db"));
    let mut args = options(&work.0, source, "rejected", &fixture);
    args.height = 102;
    args.block_hash = fixture.blocks[102].block_hash();
    fs::write(
        &args.genesis_block_file,
        consensus::serialize(&fixture.blocks[102]),
    )
    .unwrap();
    assert!(
        export_baseline_snapshot(&args)
            .unwrap_err()
            .contains("exact genesis height")
    );
    assert!(!args.output_dir.exists());
    args.height = 103;
    assert!(
        export_baseline_snapshot(&args)
            .unwrap_err()
            .contains("different BTC block hash")
    );
    let cfg = fixture.native_config(&work.0, 103);
    let db = BalanceHistoryDB::open(cfg.clone(), BalanceHistoryDBMode::Normal).unwrap();
    db.begin_native_bootstrap(&cfg.bootstrap.as_ref().unwrap().identity)
        .unwrap();
    db.put_btc_block_height(103).unwrap();
    drop(db);
    let incomplete = options(&work.0, cfg.root_dir.clone(), "incomplete", &fixture);
    assert!(
        export_baseline_snapshot(&incomplete)
            .unwrap_err()
            .contains("sealed native")
    );
    assert!(!incomplete.output_dir.exists());
    assert!(BaselineIdentity::new(Network::Regtest, u32::MAX, args.block_hash).is_err());
}
