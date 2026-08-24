#[cfg(debug_assertions)]
const ABORT_AFTER_CHECKPOINT_ENV: &str = "USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT";
#[cfg(debug_assertions)]
const FAIL_AT_CHECKPOINT_ENV: &str = "USDB_BH_SNAPSHOT_TEST_FAIL_AT_CHECKPOINT";

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

/// Returns a deterministic error at one debug-only test checkpoint.
///
/// Unlike `abort_after_checkpoint`, this exercises ordinary error unwinding while preserving
/// the durable job state that a later process must resume. Release builds omit the environment
/// lookup and always continue.
#[cfg(debug_assertions)]
pub(crate) fn fail_at_checkpoint(checkpoint: &str) -> Result<(), String> {
    let configured = std::env::var(FAIL_AT_CHECKPOINT_ENV).ok();
    if configured.as_deref() == Some(checkpoint) {
        return Err(format!(
            "Injected snapshot test failure at checkpoint {checkpoint} because {FAIL_AT_CHECKPOINT_ENV} is set"
        ));
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
pub(crate) fn fail_at_checkpoint(_checkpoint: &str) -> Result<(), String> {
    Ok(())
}
