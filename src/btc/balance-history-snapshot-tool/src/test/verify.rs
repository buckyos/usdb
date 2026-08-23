use crate::{
    build_complete_marker, save_json_atomic, unique_run_id, verify_published_artifact,
    verify_snapshot_files,
};
use balance_history::{
    BlockCommitEntry, HistoricalSnapshotStateRef, SnapshotDB, SnapshotHash, SnapshotManifest,
    SnapshotMeta, manifest_path_for_snapshot_file,
};
use bitcoincore_rpc::bitcoin::BlockHash;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use std::path::PathBuf;
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, ConsensusSnapshotIdentity,
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
    let db_path = artifact_dir.join("snapshot_10.db");
    let commit = BlockCommitEntry {
        block_height: 10,
        btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
        balance_delta_root: [11u8; 32],
        block_commit: [12u8; 32],
    };
    let mut db = SnapshotDB::open(&db_path).unwrap();
    db.put_block_commit_entries(std::slice::from_ref(&commit))
        .unwrap();
    let mut meta = SnapshotMeta::new(10);
    meta.block_commit_count = 1;
    db.update_meta(&meta).unwrap();
    let db_path = db.finalize_for_distribution().unwrap();

    let block_hash = format!("{:x}", commit.btc_block_hash);
    let state_ref = HistoricalSnapshotStateRef {
        block_height: 10,
        stable_block_hash: block_hash.clone(),
        latest_block_commit: "0c".repeat(32),
        consensus_identity: ConsensusSnapshotIdentity {
            source_chain: "btc".to_string(),
            network: "regtest".to_string(),
            stable_height: 10,
            stable_block_hash: block_hash.clone(),
            stable_lag: 5,
            balance_history_api_version: "1.0.0".to_string(),
            balance_history_semantics_version: "1".to_string(),
        },
        snapshot_id: "snapshot-10".to_string(),
        snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    };
    let manifest = SnapshotManifest::build(
        "snapshot_10.db".to_string(),
        SnapshotHash::calc_hash(&db_path).unwrap(),
        state_ref,
        None,
    );
    let manifest_path = manifest_path_for_snapshot_file(&db_path);
    manifest.save(&manifest_path).unwrap();

    let verified =
        verify_snapshot_files(&db_path, &manifest_path, "regtest", 10, Some(&block_hash)).unwrap();
    let marker = build_complete_marker(&verified, "regtest").unwrap();
    save_json_atomic(&artifact_dir.join("complete.json"), &marker).unwrap();

    let reloaded =
        verify_published_artifact(&artifact_dir, "regtest", 10, Some(&block_hash)).unwrap();
    assert_eq!(reloaded, marker);
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
        snapshot_id: "snapshot-10".to_string(),
        snapshot_file: "../snapshot.db".to_string(),
        manifest_file: "snapshot.db.manifest.json".to_string(),
        signature_file: None,
        file_sha256: "22".repeat(32),
        balance_history_count: 0,
        utxo_count: 0,
        block_commit_count: 1,
        script_registry_count: 0,
        completed_at: 1,
    };
    save_json_atomic(&artifact_dir.join("complete.json"), &marker).unwrap();

    let error = verify_published_artifact(&artifact_dir, "regtest", 10, None).unwrap_err();
    assert!(error.contains("unsafe artifact file name"));
}
