//! Bounded verification observations driven by actual scan work, never timer-only liveness.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Each stage has its own counter; changing stages is not a reset of bootstrap state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationStage {
    OriginUtxos,
    OriginBalances,
    UtxosAndBalanceAggregation,
    CompareBalances,
}

/// Observational counters only. A completed scan is not proof of successful verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VerificationProgress {
    pub stage: VerificationStage,
    pub scanned: u64,
    pub total: Option<u64>,
}

/// Persist transitions/completed scans immediately; throttle other advancing counters to ten seconds.
pub(crate) struct VerificationJournal {
    path: PathBuf,
    height: u32,
    started: Instant,
    previous: Option<(VerificationProgress, Instant)>,
}

impl VerificationJournal {
    pub(crate) fn new(root: &Path, height: u32, started: Instant) -> Self {
        Self {
            path: root.join("bootstrap-progress.json"),
            height,
            started,
            previous: None,
        }
    }

    /// `now` is monotonic; the atomic report file retains the actual wall-clock update time.
    pub(crate) fn observe(
        &mut self,
        value: VerificationProgress,
        now: Instant,
    ) -> Result<(), String> {
        if let Some((previous, written)) = self.previous {
            if value == previous {
                return Ok(());
            }
            if value.stage == previous.stage
                && now.saturating_duration_since(written) < Duration::from_secs(10)
                && value.total != Some(value.scanned)
            {
                return Ok(());
            }
        }
        crate::assumeutxo::write_report(
            &self.path,
            &serde_json::json!({
                "phase": "verifying", "height": self.height,
                "verification_stage": value.stage,
                "verification_scanned": value.scanned,
                "verification_total": value.total,
                "elapsed_seconds": now.saturating_duration_since(self.started).as_secs_f64(),
            }),
        )?;
        self.previous = Some((value, now));
        Ok(())
    }
}
