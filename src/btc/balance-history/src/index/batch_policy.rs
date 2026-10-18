//! In-process batch sizing; never changes canonical validation or durable commit boundaries.

use std::time::Duration;

use crate::config::BatchMode;

const FAST_BATCH: Duration = Duration::from_secs(10);
const SLOW_BATCH: Duration = Duration::from_secs(30);
const GROW_AFTER: u8 = 4;

/// Keep pressure history across sync iterations, but reset it on process restart.
#[derive(Debug)]
pub(super) struct BatchPolicy {
    mode: BatchMode,
    maximum: usize,
    minimum: usize,
    current: usize,
    healthy_batches: u8,
}

impl BatchPolicy {
    pub(super) fn new(mode: BatchMode, maximum: usize) -> Self {
        Self {
            mode,
            maximum,
            minimum: maximum.min(16),
            current: if mode == BatchMode::Fixed {
                maximum
            } else {
                maximum.min(32)
            },
            healthy_batches: 0,
        }
    }

    pub(super) fn size(&self) -> usize {
        self.current
    }

    /// Observe one attempted batch. Only full, fast, retry-free commits justify growth;
    /// unrelated failures reset the streak without being misclassified as RPC pressure.
    pub(super) fn observe(
        &mut self,
        completed: bool,
        blocks: usize,
        elapsed: Duration,
        rpc_pressure: bool,
    ) -> &'static str {
        if self.mode == BatchMode::Fixed {
            return "fixed";
        }
        if rpc_pressure || (completed && elapsed >= SLOW_BATCH) {
            self.healthy_batches = 0;
            self.current = (self.current / 2).max(self.minimum);
            return if rpc_pressure {
                "rpc_pressure"
            } else {
                "slow_batch"
            };
        }
        if !completed || blocks < self.current || elapsed > FAST_BATCH {
            self.healthy_batches = 0;
            return "hold";
        }
        self.healthy_batches += 1;
        if self.healthy_batches >= GROW_AFTER {
            self.current = self.current.saturating_mul(2).min(self.maximum);
            self.healthy_batches = 0;
            return "healthy_batches";
        }
        "hold"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(policy: &mut BatchPolicy) {
        policy.observe(true, policy.size(), FAST_BATCH, false);
    }

    #[test]
    fn growth_requires_sustained_full_commits_and_respects_cap() {
        let mut p = BatchPolicy::new(BatchMode::Adaptive, 128);
        assert_eq!(p.size(), 32);
        for target in [64, 128, 128] {
            let previous = p.size();
            for _ in 0..3 {
                fast(&mut p);
                assert_eq!(p.size(), previous);
            }
            fast(&mut p);
            assert_eq!(p.size(), target);
        }
    }

    #[test]
    fn pressure_shrinks_even_recovered_reads_and_recovery_needs_new_streak() {
        let mut p = BatchPolicy::new(BatchMode::Adaptive, 128);
        for _ in 0..4 {
            fast(&mut p);
        }
        assert_eq!(p.observe(true, 64, FAST_BATCH, true), "rpc_pressure");
        assert_eq!(p.size(), 32);
        p.observe(false, 32, FAST_BATCH, true);
        assert_eq!(p.size(), 16);
        p.observe(true, 16, SLOW_BATCH, false);
        assert_eq!(p.size(), 16);
        for _ in 0..3 {
            fast(&mut p);
        }
        assert_eq!(p.size(), 16);
        fast(&mut p);
        assert_eq!(p.size(), 32);
        assert_eq!(p.observe(true, 32, SLOW_BATCH, false), "slow_batch");
        assert_eq!(p.size(), 16);
    }

    #[test]
    fn tails_failures_and_midrange_duration_do_not_promote() {
        for (completed, blocks, elapsed) in [
            (true, 1, FAST_BATCH),
            (false, 32, SLOW_BATCH),
            (true, 32, Duration::from_secs(11)),
        ] {
            let mut p = BatchPolicy::new(BatchMode::Adaptive, 128);
            for _ in 0..3 {
                fast(&mut p);
            }
            p.observe(completed, blocks, elapsed, false);
            fast(&mut p);
            assert_eq!(p.size(), 32);
        }
    }

    #[test]
    fn explicit_caps_and_fixed_mode_are_preserved() {
        for maximum in [1, 8, 16, 24, 48, 128] {
            let mut p = BatchPolicy::new(BatchMode::Adaptive, maximum);
            for _ in 0..16 {
                fast(&mut p);
            }
            assert_eq!(p.size(), maximum);
            for _ in 0..16 {
                p.observe(false, maximum, SLOW_BATCH, true);
            }
            assert_eq!(p.size(), maximum.min(16));
            let mut fixed = BatchPolicy::new(BatchMode::Fixed, maximum);
            fixed.observe(false, maximum, SLOW_BATCH, true);
            for _ in 0..16 {
                fast(&mut fixed);
            }
            assert_eq!(fixed.size(), maximum);
        }
    }
}
