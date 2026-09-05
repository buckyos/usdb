use crate::{
    BuilderPaths, CompletedSnapshotRef, SnapshotBuildJob, SnapshotBuildStage, SnapshotBuilderState,
    load_json, save_json_atomic, unique_run_id,
};
use std::path::PathBuf;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join("usdb")
        .join("balance_history_snapshot_tool")
        .join(format!("{}-{}", name, unique_run_id()))
}

#[test]
fn builder_paths_keep_workspace_shared_and_artifacts_per_height_hash() {
    let root = temp_root("paths");
    let paths = BuilderPaths::new(root.clone());

    assert_eq!(paths.workspace, root.join("workspace"));
    assert_eq!(
        paths.job_file(42),
        root.join("jobs").join("000000000042").join("job.json")
    );
    assert_eq!(
        paths.snapshot_artifact_dir(42, "abc"),
        root.join("snapshots").join("000000000042").join("abc")
    );
    assert_eq!(
        paths.core_artifact_dir(42, "abc"),
        root.join("snapshots")
            .join("000000000042")
            .join("abc")
            .join("core")
    );
    assert_eq!(
        paths.registry_artifact_dir(42, "abc"),
        root.join("snapshots")
            .join("000000000042")
            .join("abc")
            .join("script-registry")
    );
}

#[test]
fn atomic_state_round_trip_preserves_active_and_completed_identity() {
    let root = temp_root("state_round_trip");
    let paths = BuilderPaths::new(root);
    paths.create_dirs().unwrap();

    let completed = CompletedSnapshotRef {
        height: 100,
        btc_block_hash: "11".repeat(32),
        snapshot_id: "snapshot-100".to_string(),
        artifact_dir: "snapshots/000000000100/hash".to_string(),
    };
    let mut state = SnapshotBuilderState::new("regtest".to_string());
    state.latest_completed = Some(completed.clone());
    state.active_job_height = Some(101);
    save_json_atomic(&paths.state_file, &state).unwrap();

    let loaded: SnapshotBuilderState = load_json(&paths.state_file).unwrap().unwrap();
    assert_eq!(loaded, state);

    let mut job = SnapshotBuildJob::new(101, Some(completed), None);
    job.set_core_stage(SnapshotBuildStage::Syncing);
    save_json_atomic(&paths.job_file(101), &job).unwrap();
    let loaded_job: SnapshotBuildJob = load_json(&paths.job_file(101)).unwrap().unwrap();
    assert_eq!(loaded_job, job);
}
