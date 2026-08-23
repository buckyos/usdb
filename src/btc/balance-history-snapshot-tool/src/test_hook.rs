#[cfg(debug_assertions)]
const ABORT_AFTER_CHECKPOINT_ENV: &str = "USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT";

/// Abruptly terminates a debug build after one durable checkpoint is on disk.
///
/// This hook exists only to exercise cross-process recovery. Release builds compile the
/// environment lookup and abort path out entirely.
#[cfg(debug_assertions)]
pub(crate) fn abort_after_checkpoint(checkpoint: &str) {
    let configured = std::env::var(ABORT_AFTER_CHECKPOINT_ENV).ok();
    if configured.as_deref() == Some(checkpoint) {
        eprintln!(
            "Aborting at snapshot test checkpoint {checkpoint} because {ABORT_AFTER_CHECKPOINT_ENV} is set"
        );
        std::process::abort();
    }
}

#[cfg(not(debug_assertions))]
pub(crate) fn abort_after_checkpoint(_checkpoint: &str) {}
