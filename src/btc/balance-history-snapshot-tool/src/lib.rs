//! Restartable exact-height full-UTXO snapshot builder for balance-history.

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
    CompletedSnapshotRef, SnapshotBuildJob, SnapshotBuildStage, SnapshotBuilderState,
    SnapshotCompleteMarker,
};
pub(crate) use test_hook::abort_after_checkpoint;
pub(crate) use verify::{build_complete_marker, verify_published_artifact, verify_snapshot_files};

#[cfg(test)]
mod test;
