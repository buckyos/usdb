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

impl VerificationStage {
    /// Stable stage names shared with the operator's progress records.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::OriginUtxos => "origin_utxos",
            Self::OriginBalances => "origin_balances",
            Self::UtxosAndBalanceAggregation => "utxos_and_balance_aggregation",
            Self::CompareBalances => "compare_balances",
        }
    }
}

/// Preserve lifecycle summaries in the configured file logger and existing container output.
pub(crate) fn log_bootstrap_milestone(message: std::fmt::Arguments<'_>) {
    log::info!("{message}");
    eprintln!("{message}");
}

/// Bound file-log progress independently of the ten-second monitor and stderr observations.
pub(crate) struct VerificationLogThrottle {
    previous: Instant,
    scanned: u64,
}

impl VerificationLogThrottle {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            previous: now,
            scanned: 0,
        }
    }

    /// Emit only when actual work advances and at least one minute has passed.
    pub(crate) fn due(&mut self, scanned: u64, now: Instant) -> bool {
        if scanned <= self.scanned
            || now.saturating_duration_since(self.previous) < Duration::from_secs(60)
        {
            return false;
        }
        self.previous = now;
        self.scanned = scanned;
        true
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_progress_is_throttled_without_manufacturing_work_or_catching_up_logs() {
        let start = Instant::now();
        let mut throttle = VerificationLogThrottle::new(start);
        assert!(!throttle.due(1, start + Duration::from_secs(10)));
        assert!(!throttle.due(2, start + Duration::from_secs(59)));
        assert!(throttle.due(3, start + Duration::from_secs(60)));
        assert!(!throttle.due(4, start + Duration::from_secs(61)));
        assert!(!throttle.due(3, start + Duration::from_secs(3600)));
        assert!(throttle.due(5, start + Duration::from_secs(3601)));
        assert!(!throttle.due(6, start + Duration::from_secs(3602)));
        // Each new stage starts its own counter and interval.
        let mut comparison = VerificationLogThrottle::new(start + Duration::from_secs(3602));
        assert!(!comparison.due(1, start + Duration::from_secs(3603)));
        assert!(comparison.due(1, start + Duration::from_secs(3662)));
    }
}
