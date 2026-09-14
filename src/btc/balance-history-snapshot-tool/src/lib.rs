//! Restartable exact-height core and script-registry artifact builder for balance-history.

#![warn(missing_docs)]

mod builder;
mod state;
mod test_hook;
mod verify;

/// Held producer lock for baseline synchronization and export.
pub struct BaselineWorkspaceLock {
    _guard: BuilderLock,
}

/// Acquire the existing producer workspace lock for managed baseline synchronization.
pub fn lock_baseline_workspace(root: &std::path::Path) -> Result<BaselineWorkspaceLock, String> {
    Ok(BaselineWorkspaceLock {
        _guard: BuilderLock::acquire(root)?,
    })
}

/// Exercise durable baseline checkpoints in debug binaries; production builds are inert.
pub fn baseline_test_checkpoint(label: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if std::env::var("USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT")
        .ok()
        .as_deref()
        == Some(label)
    {
        // Exit without running destructors, but avoid a multi-gigabyte core dump in CI.
        std::process::exit(86);
    }
    fail_at_checkpoint(label)
}

pub use builder::*;
pub(crate) use state::{
    BUILDER_STATE_VERSION, BuilderLock, BuilderPaths, COMPLETE_MARKER_VERSION, JOB_STATE_VERSION,
    load_json, save_json_atomic, unique_run_id, unix_timestamp,
};
pub use state::{
    CompletedSnapshotRef, ScriptRegistryCompleteMarker, SnapshotBuildJob, SnapshotBuildStage,
    SnapshotBuilderState, SnapshotCompleteMarker, SnapshotComponent, SnapshotComponentBuildState,
    SnapshotVerificationPhase, SnapshotVerificationProgress,
};
pub(crate) use test_hook::{abort_after_checkpoint, fail_at_checkpoint};
pub(crate) use verify::{
    build_complete_marker, build_registry_complete_marker, verify_published_artifact,
    verify_published_artifact_marker, verify_published_registry, verify_published_registry_marker,
    verify_registry_files_with_progress, verify_snapshot_files_with_progress,
};
#[cfg(test)]
pub(crate) use verify::{verify_registry_files, verify_snapshot_files};

#[cfg(test)]
mod test;
