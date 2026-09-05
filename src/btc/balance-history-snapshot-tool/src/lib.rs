//! Restartable exact-height core and script-registry artifact builder for balance-history.

#![warn(missing_docs)]

mod builder;
mod state;
mod test_hook;
mod verify;

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
