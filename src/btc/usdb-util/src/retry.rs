//! Scheduling helpers for service availability failures, independent of consensus policy.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::{Duration, Instant};

/// Return exponential outage backoff with 20% jitter, capped at fifteen seconds.
/// `attempt` starts at one and should reset after a successful observation or commit.
pub fn service_retry_delay(attempt: u64) -> Duration {
    jittered_delay(attempt, RandomState::new().hash_one(attempt) % 401)
}

fn jittered_delay(attempt: u64, jitter: u64) -> Duration {
    let base_ms = (1_000u64 << attempt.saturating_sub(1).min(4)).min(15_000);
    Duration::from_millis((base_ms * (800 + jitter) / 1_000).min(15_000))
}

/// Wait on a blocking worker, checking cancellation at least every hundred milliseconds.
/// Returns true when cancellation interrupted the wait.
pub fn sleep_until_cancelled(duration: Duration, mut cancelled: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if cancelled() {
            return true;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outage_delay_grows_and_stays_bounded() {
        let delays: Vec<_> = (1..=6)
            .map(|n| jittered_delay(n, 200).as_millis())
            .collect();
        assert_eq!(delays, [1_000, 2_000, 4_000, 8_000, 15_000, 15_000]);
        assert_eq!(jittered_delay(u64::MAX, 400), Duration::from_secs(15));
        assert_eq!(jittered_delay(1, 0), Duration::from_millis(800));
        assert_eq!(jittered_delay(1, 400), Duration::from_millis(1_200));
    }

    #[test]
    fn cancelled_wait_returns_without_sleeping() {
        assert!(sleep_until_cancelled(Duration::from_secs(15), || true));
        assert!(!sleep_until_cancelled(Duration::ZERO, || false));
    }
}
