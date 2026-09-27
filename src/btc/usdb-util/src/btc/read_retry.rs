//! Opt-in retries for individual read-only RPCs during native bootstrap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bitcoincore_rpc::{Error, jsonrpc};

const MAX_ATTEMPTS: u32 = 6;

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
}

impl ReadRetry {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            gate: Arc::new(Mutex::new(())),
        }
    }

    // Core's HTTP transport serializes calls on one socket. Check cancellation AFTER
    // acquiring our gate, so queued replay workers exit after the in-flight request
    // finishes rather than each entering the transport for another timeout.
    // Backoff never holds this gate.
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
        for attempt in 1..=MAX_ATTEMPTS {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(format!(
                    "Native bootstrap RPC cancelled: operation={operation}, attempts={}",
                    attempt - 1
                ));
            }
            let result = {
                let _guard = self.lock(operation)?;
                call()
            };
            match result {
                Ok(value) => {
                    if attempt > 1 {
                        let msg = format!(
                            "Native bootstrap RPC recovered: operation={operation}, attempts={attempt}, elapsed_ms={}",
                            started.elapsed().as_millis()
                        );
                        info!("{msg}");
                        eprintln!("{msg}");
                    }
                    return Ok(value);
                }
                Err(error) if error.transient && attempt < MAX_ATTEMPTS => {
                    let delay = Duration::from_secs((1 << attempt).min(20));
                    let msg = format!(
                        "Native bootstrap RPC retry: operation={operation}, attempt={attempt}/{MAX_ATTEMPTS}, retry_in_secs={}, elapsed_ms={}, error={}",
                        delay.as_secs(),
                        started.elapsed().as_millis(),
                        error.message
                    );
                    warn!("{msg}");
                    eprintln!("{msg}");
                    wait(delay);
                }
                Err(error) => {
                    return Err(format!(
                        "Native bootstrap RPC {}: operation={operation}, attempts={attempt}, elapsed_ms={}, error={}",
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
