//! Bounded retries for individual read-only RPCs, below parsing and canonical checks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bitcoincore_rpc::{Error, jsonrpc};

/// Cumulative read counters shared by all clones of one RPC client.
#[derive(Clone, Copy, Debug, Default)]
pub struct RpcReadStats {
    /// Actual application-level RPC attempts (transport reconnects are not counted).
    pub attempts: u64,
    /// Attempts after a transient failure of the same read.
    pub retries: u64,
    /// Reads whose transient retry allowance was exhausted.
    pub exhausted: u64,
}

#[derive(Default)]
struct Counters {
    attempts: AtomicU64,
    retries: AtomicU64,
    exhausted: AtomicU64,
}

/// Retain typed classification before the existing client API converts errors to strings.
pub(super) struct ReadError {
    pub message: String,
    pub transient: bool,
}

impl From<Error> for ReadError {
    fn from(error: Error) -> Self {
        Self {
            transient: is_transient(&error),
            message: error.to_string(),
        }
    }
}

// Only known availability failures qualify. Authentication, malformed responses and all
// other Core error codes must fail immediately, regardless of their error message text.
fn is_transient(error: &Error) -> bool {
    use jsonrpc::simple_http::Error as HttpError;
    use std::io::ErrorKind;

    match error {
        Error::JsonRpc(jsonrpc::Error::Rpc(error)) => error.code == -28,
        Error::JsonRpc(jsonrpc::Error::Transport(error)) => {
            match error.downcast_ref::<HttpError>() {
                Some(HttpError::SocketError(error)) => matches!(
                    error.kind(),
                    ErrorKind::TimedOut
                        | ErrorKind::WouldBlock
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::ConnectionRefused
                        | ErrorKind::NotConnected
                        | ErrorKind::BrokenPipe
                        | ErrorKind::UnexpectedEof
                        | ErrorKind::Interrupted
                ),
                Some(HttpError::IncompleteResponse { .. }) => true,
                Some(HttpError::HttpErrorCode(429 | 502 | 503 | 504)) => true,
                _ => false,
            }
        }
        _ => false,
    }
}

#[derive(Clone)]
pub(super) struct ReadRetry {
    cancelled: Arc<AtomicBool>,
    gate: Arc<Mutex<()>>,
    runtime: bool,
    budget: Option<Duration>,
    unavailable_until: Arc<Mutex<Option<Instant>>>,
    counters: Arc<Counters>,
}

impl ReadRetry {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            gate: Arc::new(Mutex::new(())),
            runtime: false,
            budget: None,
            unavailable_until: Arc::new(Mutex::new(None)),
            counters: Arc::new(Counters::default()),
        }
    }

    pub fn runtime(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            runtime: true,
            budget: Some(Duration::from_secs(60)),
            ..Self::new(cancelled)
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn stats(&self) -> RpcReadStats {
        RpcReadStats {
            attempts: self.counters.attempts.load(Ordering::Relaxed),
            retries: self.counters.retries.load(Ordering::Relaxed),
            exhausted: self.counters.exhausted.load(Ordering::Relaxed),
        }
    }

    fn budget_expired(&self, started: Instant) -> bool {
        self.budget
            .is_some_and(|budget| started.elapsed() >= budget)
    }

    // Runtime workers share one retry owner. Queued Rayon workers must not each spend
    // several timeouts on the same outage. The short cooldown hands control to outer backoff.
    fn lock_runtime(
        &self,
        operation: &str,
        started: Instant,
    ) -> Result<MutexGuard<'_, ()>, String> {
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(format!("Runtime BTC RPC cancelled: operation={operation}"));
            }
            if self.budget_expired(started) {
                return Err(format!(
                    "Runtime BTC RPC retry budget exhausted while queued: operation={operation}"
                ));
            }
            match self.gate.try_lock() {
                Ok(guard) => {
                    if self
                        .unavailable_until
                        .lock()
                        .unwrap()
                        .is_some_and(|until| Instant::now() < until)
                    {
                        return Err(format!(
                            "Runtime BTC RPC temporarily unavailable after exhausted read: operation={operation}"
                        ));
                    }
                    return Ok(guard);
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(error) => return Err(format!("Runtime BTC RPC gate failed: {error}")),
            }
        }
    }

    // Core's HTTP transport serializes calls on one socket. Check cancellation AFTER
    // acquiring our gate, so queued replay workers exit after the in-flight request
    // finishes rather than each entering the transport for another timeout.
    // Bootstrap backoff never holds this gate; runtime uses a separate retry owner.
    fn lock(&self, operation: &str) -> Result<MutexGuard<'_, ()>, String> {
        let guard = self
            .gate
            .lock()
            .map_err(|_| "Native bootstrap RPC gate poisoned".to_string())?;
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(format!(
                "Native bootstrap RPC cancelled: operation={operation}"
            ));
        }
        Ok(guard)
    }

    pub fn run<T>(
        &self,
        operation: &str,
        call: impl FnMut() -> Result<T, ReadError>,
    ) -> Result<T, String> {
        self.run_with_wait(operation, call, |delay| {
            let started = Instant::now();
            while started.elapsed() < delay {
                if self.cancelled.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(
                    delay
                        .saturating_sub(started.elapsed())
                        .min(Duration::from_millis(100)),
                );
            }
        })
    }

    // Inject only the wait for deterministic tests; production always checks cancellation
    // during backoff. Never retry a database operation or an entire replay/verification pass.
    fn run_with_wait<T>(
        &self,
        operation: &str,
        mut call: impl FnMut() -> Result<T, ReadError>,
        mut wait: impl FnMut(Duration),
    ) -> Result<T, String> {
        let started = Instant::now();
        let label = if self.runtime {
            "Runtime BTC"
        } else {
            "Native bootstrap"
        };
        let max_attempts = if self.runtime { 3 } else { 6 };
        let _runtime_guard = if self.runtime {
            Some(self.lock_runtime(operation, started)?)
        } else {
            None
        };
        for attempt in 1..=max_attempts {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(format!(
                    "{label} RPC cancelled: operation={operation}, attempts={}",
                    attempt - 1
                ));
            }
            // This bounds admission and backoff, including queue time. An already running
            // synchronous transport call still completes under its existing socket timeout.
            if self.budget_expired(started) {
                self.counters.exhausted.fetch_add(1, Ordering::Relaxed);
                *self.unavailable_until.lock().unwrap() =
                    Some(Instant::now() + Duration::from_secs(1));
                return Err(format!(
                    "{label} RPC retry budget exhausted: operation={operation}, attempts={}",
                    attempt - 1
                ));
            }
            let result = {
                let _bootstrap_guard = if self.runtime {
                    None
                } else {
                    Some(self.lock(operation)?)
                };
                self.counters.attempts.fetch_add(1, Ordering::Relaxed);
                if attempt > 1 {
                    self.counters.retries.fetch_add(1, Ordering::Relaxed);
                }
                call()
            };
            match result {
                Ok(value) => {
                    if attempt > 1 {
                        let msg = format!(
                            "{label} RPC recovered: operation={operation}, attempts={attempt}, elapsed_ms={}",
                            started.elapsed().as_millis()
                        );
                        info!("{msg}");
                        if !self.runtime {
                            eprintln!("{msg}");
                        }
                    }
                    return Ok(value);
                }
                Err(error) if error.transient && attempt < max_attempts => {
                    let delay = Duration::from_secs(if self.runtime {
                        1 << (attempt - 1)
                    } else {
                        (1 << attempt).min(20)
                    });
                    let delay = self.budget.map_or(delay, |budget| {
                        delay.min(budget.saturating_sub(started.elapsed()))
                    });
                    let msg = format!(
                        "{label} RPC retry: operation={operation}, attempt={attempt}/{max_attempts}, retry_in_secs={}, retry_in_ms={}, elapsed_ms={}, error={}",
                        delay.as_secs(),
                        delay.as_millis(),
                        started.elapsed().as_millis(),
                        error.message
                    );
                    warn!("{msg}");
                    if !self.runtime {
                        eprintln!("{msg}");
                    }
                    wait(delay);
                }
                Err(error) => {
                    if error.transient {
                        self.counters.exhausted.fetch_add(1, Ordering::Relaxed);
                        if self.runtime {
                            *self.unavailable_until.lock().unwrap() =
                                Some(Instant::now() + Duration::from_secs(1));
                        }
                    }
                    return Err(format!(
                        "{label} RPC {}: operation={operation}, attempts={attempt}, elapsed_ms={}, error={}",
                        if error.transient {
                            "retries exhausted"
                        } else {
                            "failed without retry"
                        },
                        started.elapsed().as_millis(),
                        error.message
                    ));
                }
            }
        }
        unreachable!("The last attempt always returns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpc::simple_http::Error as HttpError;
    use std::io::{Error as IoError, ErrorKind};

    fn transport(error: HttpError) -> Error {
        Error::JsonRpc(jsonrpc::Error::Transport(Box::new(error)))
    }

    fn warmup() -> ReadError {
        Error::JsonRpc(jsonrpc::Error::Rpc(jsonrpc::error::RpcError {
            code: -28,
            message: "Loading block index".into(),
            data: None,
        }))
        .into()
    }

    #[test]
    fn runtime_retries_only_the_failed_read_and_shares_outage_cooldown() {
        let retry = ReadRetry::runtime(Arc::new(AtomicBool::new(false)));
        let mut calls = 0;
        let mut delays = Vec::new();
        assert_eq!(
            retry
                .run_with_wait(
                    "getblockhash",
                    || {
                        calls += 1;
                        if calls < 3 { Err(warmup()) } else { Ok(42) }
                    },
                    |delay| delays.push(delay.as_secs())
                )
                .unwrap(),
            42
        );
        assert_eq!(delays, [1, 2]);
        assert_eq!(retry.stats().attempts, 3);
        assert_eq!(retry.stats().retries, 2);
        assert_eq!(retry.stats().exhausted, 0);
        assert!(
            retry
                .run_with_wait::<()>("getblock", || Err(warmup()), |_| {})
                .unwrap_err()
                .contains("attempts=3")
        );
        assert_eq!(retry.stats().exhausted, 1);
        let queued = retry.clone();
        assert!(
            queued
                .run::<()>("getblock", || panic!(
                    "Queued workers must not each exhaust retries"
                ))
                .unwrap_err()
                .contains("temporarily unavailable")
        );
    }

    #[test]
    fn runtime_budget_and_cancellation_prevent_another_transport_attempt() {
        let mut retry = ReadRetry::runtime(Arc::new(AtomicBool::new(false)));
        retry.budget = Some(Duration::ZERO);
        assert!(
            retry
                .run::<()>("getblock", || panic!("Budget is exhausted"))
                .unwrap_err()
                .contains("budget exhausted")
        );
        let retry = ReadRetry::runtime(Arc::new(AtomicBool::new(false)));
        let mut calls = 0;
        assert!(
            retry
                .run_with_wait::<()>(
                    "getblock",
                    || {
                        calls += 1;
                        Err(warmup())
                    },
                    |_| retry.cancel()
                )
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn transient_classification_is_typed_and_conservative() {
        for kind in [
            ErrorKind::TimedOut,
            ErrorKind::WouldBlock,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionRefused,
            ErrorKind::UnexpectedEof,
        ] {
            assert!(is_transient(&transport(HttpError::SocketError(
                IoError::from(kind)
            ))));
        }
        for code in [429, 502, 503, 504] {
            assert!(is_transient(&transport(HttpError::HttpErrorCode(code))));
        }
        for code in [400, 401, 403, 404, 500] {
            assert!(!is_transient(&transport(HttpError::HttpErrorCode(code))));
        }
        assert!(warmup().transient);
        for code in [-5, -8, -32601, -32602, -32603] {
            assert!(!is_transient(&Error::JsonRpc(jsonrpc::Error::Rpc(
                jsonrpc::error::RpcError {
                    code,
                    message: "timeout; Resource temporarily unavailable".into(),
                    data: None,
                }
            ))));
        }
        assert!(!is_transient(&transport(HttpError::SocketError(
            IoError::from(ErrorKind::PermissionDenied)
        ))));
        assert!(!is_transient(&transport(HttpError::HttpResponseTooShort {
            actual: 4,
            needed: 12
        })));
        assert!(!is_transient(&Error::JsonRpc(
            jsonrpc::Error::NonceMismatch
        )));
        assert!(!is_transient(&Error::JsonRpc(jsonrpc::Error::Json(
            serde_json::from_str::<u64>("bad").unwrap_err()
        ))));
    }

    #[test]
    fn retry_recovers_without_repeating_successful_work() {
        let retry = ReadRetry::new(Arc::new(AtomicBool::new(false)));
        let mut attempts = 0;
        let mut delays = Vec::new();
        let result = retry.run_with_wait(
            "getblockhash height=103",
            || {
                attempts += 1;
                if attempts < 3 { Err(warmup()) } else { Ok(103) }
            },
            |delay| {
                assert!(
                    retry.gate.try_lock().is_ok(),
                    "Backoff must not block other reads"
                );
                delays.push(delay.as_secs());
            },
        );
        assert_eq!(result.unwrap(), 103);
        assert_eq!(attempts, 3);
        assert_eq!(delays, [2, 4]);
    }

    #[test]
    fn retry_exhaustion_is_bounded_and_permanent_errors_fail_immediately() {
        let retry = ReadRetry::new(Arc::new(AtomicBool::new(false)));
        let mut attempts = 0;
        let mut delays = Vec::new();
        let result = retry.run_with_wait::<()>(
            "getblockcount",
            || {
                attempts += 1;
                Err(warmup())
            },
            |delay| delays.push(delay.as_secs()),
        );
        assert!(result.unwrap_err().contains("retries exhausted"));
        assert_eq!(attempts, 6);
        assert_eq!(delays, [2, 4, 8, 16, 20]);
        let result = retry.run_with_wait::<()>(
            "getblockcount",
            || {
                Err(ReadError {
                    message: "Invalid cookie".into(),
                    transient: false,
                })
            },
            |_| panic!("Permanent errors must not wait"),
        );
        assert!(result.unwrap_err().contains("attempts=1"));
    }

    #[test]
    fn cancellation_stops_backoff_before_another_request() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let retry = ReadRetry::new(cancelled.clone());
        let mut attempts = 0;
        let result = retry.run_with_wait::<()>(
            "getblockhash height=103",
            || {
                attempts += 1;
                Err(warmup())
            },
            |_| cancelled.store(true, Ordering::Relaxed),
        );
        assert!(result.unwrap_err().contains("cancelled"));
        assert_eq!(attempts, 1);
        assert!(
            retry
                .run::<()>("getblockcount", || panic!("Already cancelled"))
                .is_err()
        );
    }

    #[test]
    fn queued_reads_check_cancellation_before_entering_the_transport() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let retry = ReadRetry::new(cancelled.clone());
        let in_flight = retry.gate.lock().unwrap();
        let queued = retry.clone();
        let (started, waiting) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            queued.lock("getblockhash height=103").unwrap_err()
        });
        waiting.recv().unwrap();
        cancelled.store(true, Ordering::Relaxed);
        drop(in_flight);
        assert!(worker.join().unwrap().contains("RPC cancelled"));
    }
}
