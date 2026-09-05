use crate::{
    build_complete_marker, build_registry_complete_marker, save_json_atomic, unique_run_id,
    verify_published_artifact, verify_published_artifact_marker, verify_published_registry,
    verify_registry_files, verify_snapshot_files,
};
use balance_history::{
    BalanceHistoryDBIdentity, BlockCommitEntry, CoreSnapshotDb, CoreSnapshotManifest,
    CoreSnapshotMeta, HistoricalSnapshotStateRef, ScriptRegistryBaseIdentity,
    ScriptRegistryManifest, ScriptRegistrySnapshotDb, ScriptRegistrySnapshotMeta, SnapshotHash,
    manifest_path_for_snapshot_file,
};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, Network};
use std::path::PathBuf;
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, ConsensusSnapshotIdentity,
    build_consensus_snapshot_id,
};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join("usdb")
        .join("balance_history_snapshot_tool")
        .join(format!("{}-{}", name, unique_run_id()))
}

#[test]
fn completed_artifact_round_trip_checks_db_manifest_and_marker() {
    let artifact_dir = temp_root("verify_round_trip");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let db_path = artifact_dir.join("balance_history_core_10.db");
    let commit = BlockCommitEntry {
        block_height: 10,
        btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
        balance_delta_root: [11u8; 32],
        block_commit: [12u8; 32],
    };
    let mut db = CoreSnapshotDb::create(&db_path).unwrap();
    db.put_block_commit_entries(std::slice::from_ref(&commit))
        .unwrap();
    let db_identity = BalanceHistoryDBIdentity::for_network(Network::Regtest);
    let block_hash = format!("{:x}", commit.btc_block_hash);
    let mut state_ref = HistoricalSnapshotStateRef {
        block_height: 10,
        stable_block_hash: block_hash.clone(),
        latest_block_commit: "0c".repeat(32),
        consensus_identity: ConsensusSnapshotIdentity {
            source_chain: "btc".to_string(),
            network: "regtest".to_string(),
            stable_height: 10,
            stable_block_hash: block_hash.clone(),
            stable_lag: 10,
            balance_history_api_version: "1.0.0".to_string(),
            balance_history_semantics_version: "1".to_string(),
        },
        snapshot_id: String::new(),
        snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    };
    state_ref.snapshot_id = build_consensus_snapshot_id(&state_ref.consensus_identity);
    db.write_meta(&CoreSnapshotMeta {
        block_height: 10,
        balance_history_count: 0,
        utxo_count: 0,
        block_commit_count: 1,
        generated_at: 1,
        db_identity: db_identity.clone(),
        core_snapshot_id: state_ref.snapshot_id.clone(),
    })
    .unwrap();
    let db_path = db.finalize_for_distribution().unwrap();

    let mut manifest = CoreSnapshotManifest::build(
        "balance_history_core_10.db".to_string(),
        SnapshotHash::calc_hash(&db_path).unwrap(),
        state_ref,
        db_identity,
        None,
        1,
    )
    .unwrap();
    let manifest_path = manifest_path_for_snapshot_file(&db_path);
    manifest.save(&manifest_path).unwrap();

    let verified =
        verify_snapshot_files(&db_path, &manifest_path, "regtest", 10, Some(&block_hash)).unwrap();
    assert!(!db_path.with_extension("db-wal").exists());
    assert!(!db_path.with_extension("db-shm").exists());
    let marker = build_complete_marker(&verified, "regtest").unwrap();
    save_json_atomic(&artifact_dir.join("complete.json"), &marker).unwrap();

    let marker_only =
        verify_published_artifact_marker(&artifact_dir, "regtest", 10, Some(&block_hash)).unwrap();
    assert_eq!(marker_only, marker);
    let reloaded =
        verify_published_artifact(&artifact_dir, "regtest", 10, Some(&block_hash)).unwrap();
    assert_eq!(reloaded, marker);

    manifest.db_identity.data_model_version = "balance-history-data-model:tampered-v0".to_string();
    manifest.core_artifact_id = manifest.calculate_artifact_id().unwrap();
    manifest.save(&manifest_path).unwrap();
    let error = verify_snapshot_files(&db_path, &manifest_path, "regtest", 10, Some(&block_hash))
        .unwrap_err();
    assert!(error.contains("Core snapshot DB identity mismatch"));
}

#[test]
fn registry_artifact_round_trip_is_bound_to_core_manifest() {
    let root = temp_root("registry_verify_round_trip");
    let core_dir = root.join("core");
    let registry_dir = root.join("script-registry");
    std::fs::create_dir_all(&core_dir).unwrap();
    std::fs::create_dir_all(&registry_dir).unwrap();
    let core_db_path = core_dir.join("balance_history_core_10.db");
    let commit = BlockCommitEntry {
        block_height: 10,
        btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
        balance_delta_root: [11u8; 32],
        block_commit: [12u8; 32],
    };
    let block_hash = format!("{:x}", commit.btc_block_hash);
    let identity = BalanceHistoryDBIdentity::for_network(Network::Regtest);
    let consensus_identity = ConsensusSnapshotIdentity {
        source_chain: "btc".to_string(),
        network: "regtest".to_string(),
        stable_height: 10,
        stable_block_hash: block_hash.clone(),
        stable_lag: 10,
        balance_history_api_version: "1.0.0".to_string(),
        balance_history_semantics_version: "1".to_string(),
    };
    let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
    let state_ref = HistoricalSnapshotStateRef {
        block_height: 10,
        stable_block_hash: block_hash.clone(),
        latest_block_commit: "0c".repeat(32),
        consensus_identity,
        snapshot_id: snapshot_id.clone(),
        snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    };
    let mut core_db = CoreSnapshotDb::create(&core_db_path).unwrap();
    core_db
        .put_block_commit_entries(std::slice::from_ref(&commit))
        .unwrap();
    core_db
        .write_meta(&CoreSnapshotMeta {
            block_height: 10,
            balance_history_count: 0,
            utxo_count: 0,
            block_commit_count: 1,
            generated_at: 1,
            db_identity: identity.clone(),
            core_snapshot_id: snapshot_id.clone(),
        })
        .unwrap();
    core_db.finalize_for_distribution().unwrap();
    let core_manifest = CoreSnapshotManifest::build(
        "balance_history_core_10.db".to_string(),
        SnapshotHash::calc_hash(&core_db_path).unwrap(),
        state_ref,
        identity.clone(),
        None,
        1,
    )
    .unwrap();
    core_manifest
        .save(&manifest_path_for_snapshot_file(&core_db_path))
        .unwrap();

    let registry_db_path = registry_dir.join("script_registry_10.db");
    let registry_db = ScriptRegistrySnapshotDb::create(&registry_db_path).unwrap();
    let base = ScriptRegistryBaseIdentity {
        btc_network: "regtest".to_string(),
        btc_genesis_hash: identity.btc_genesis_hash,
        base_height: 10,
        base_block_hash: block_hash,
        core_snapshot_id: snapshot_id,
    };
    registry_db
        .write_meta(&ScriptRegistrySnapshotMeta {
            base: base.clone(),
            entry_count: 0,
            generated_at: 2,
        })
        .unwrap();
    registry_db.finalize_for_distribution().unwrap();
    let registry_manifest = ScriptRegistryManifest::build(
        "script_registry_10.db".to_string(),
        SnapshotHash::calc_hash(&registry_db_path).unwrap(),
        base,
        0,
        None,
        2,
    )
    .unwrap();
    let registry_manifest_path = manifest_path_for_snapshot_file(&registry_db_path);
    registry_manifest.save(&registry_manifest_path).unwrap();
    let verified =
        verify_registry_files(&registry_db_path, &registry_manifest_path, &core_manifest).unwrap();
    let marker = build_registry_complete_marker(&verified).unwrap();
    save_json_atomic(&registry_dir.join("complete.json"), &marker).unwrap();
    assert_eq!(
        verify_published_registry(&registry_dir, &core_manifest).unwrap(),
        marker
    );
}

#[test]
fn old_job_schema_is_rejected_instead_of_implicitly_migrated() {
    let legacy_job = r#"{
        "version": 1,
        "target_height": 10,
        "base_checkpoint": null,
        "stage": "verifying",
        "expected_block_hash": null,
        "btc_block_hash": null,
        "snapshot_id": null,
        "temp_dir": "tmp/job",
        "artifact_dir": null,
        "attempt": 1,
        "started_at": 1,
        "updated_at": 2,
        "completed_at": null
    }"#;

    assert!(serde_json::from_str::<crate::SnapshotBuildJob>(legacy_job).is_err());
}

#[test]
fn v2_job_rejects_removed_aggregate_component_fields() {
    let mut value = serde_json::to_value(crate::SnapshotBuildJob::new(10, None, None)).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("stage".to_string(), serde_json::json!("verifying"));

    assert!(serde_json::from_value::<crate::SnapshotBuildJob>(value).is_err());
}

#[test]
fn completed_artifact_rejects_unsafe_marker_file_path() {
    let artifact_dir = temp_root("verify_unsafe_path");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let marker = crate::SnapshotCompleteMarker {
        version: crate::COMPLETE_MARKER_VERSION,
        height: 10,
        network: "regtest".to_string(),
        btc_block_hash: "11".repeat(32),
        snapshot_id: "33".repeat(32),
        core_artifact_id: "44".repeat(32),
        snapshot_file: "../snapshot.db".to_string(),
        manifest_file: "snapshot.db.manifest.json".to_string(),
        signature_file: None,
        file_sha256: "22".repeat(32),
        balance_history_count: 0,
        utxo_count: 0,
        block_commit_count: 1,
        completed_at: 1,
    };
    save_json_atomic(&artifact_dir.join("complete.json"), &marker).unwrap();

    let error = verify_published_artifact(&artifact_dir, "regtest", 10, None).unwrap_err();
    assert!(error.contains("unsafe artifact file name"));
}
