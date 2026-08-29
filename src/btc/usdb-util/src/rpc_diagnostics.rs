use crate::{
    CONSENSUS_RPC_ERR_ACTIVATION_RECORD_CONFLICT, CONSENSUS_RPC_ERR_ACTIVATION_RECORD_NOT_FOUND,
    CONSENSUS_RPC_ERR_ACTIVE_VERSION_SET_MISMATCH, CONSENSUS_RPC_ERR_BLOCK_HASH_MISMATCH,
    CONSENSUS_RPC_ERR_COMMIT_PROTOCOL_VERSION_MISMATCH, CONSENSUS_RPC_ERR_FORMULA_VERSION_MISMATCH,
    CONSENSUS_RPC_ERR_HEIGHT_NOT_SYNCED, CONSENSUS_RPC_ERR_HISTORY_NOT_AVAILABLE,
    CONSENSUS_RPC_ERR_LOCAL_STATE_COMMIT_MISMATCH, CONSENSUS_RPC_ERR_NO_RECORD,
    CONSENSUS_RPC_ERR_SNAPSHOT_ID_MISMATCH, CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY,
    CONSENSUS_RPC_ERR_STATE_NOT_RETAINED, CONSENSUS_RPC_ERR_SYSTEM_STATE_ID_MISMATCH,
    CONSENSUS_RPC_ERR_VERSION_MISMATCH, CONSENSUS_RPC_ERR_VERSION_NOT_SUPPORTED,
    CONSENSUS_RPC_ERR_VIEW_VERSION_MISMATCH,
};
use jsonrpc_core::futures_util::{FutureExt, future::Either};
use jsonrpc_core::{Call, Error, ErrorCode, Metadata, Middleware, Output};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Environment variable controlling the slow JSON-RPC threshold in milliseconds.
pub const SLOW_RPC_THRESHOLD_MS_ENV: &str = "USDB_SLOW_RPC_THRESHOLD_MS";
/// Default slow JSON-RPC threshold used by BTC-side services.
pub const DEFAULT_SLOW_RPC_THRESHOLD_MS: u64 = 500;

/// Operational classification of an outgoing JSON-RPC failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcFailureClass {
    /// Malformed requests, unknown methods, or invalid parameters controlled by the caller.
    Client,
    /// A valid request rejected by an explicit consensus or business-state contract.
    Expected,
    /// Storage, invariant, serialization, or otherwise unexpected server failure.
    Internal,
}

impl RpcFailureClass {
    /// Stable label emitted in structured operational logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Expected => "expected",
            Self::Internal => "internal",
        }
    }
}

/// Classifies an outgoing JSON-RPC error without inspecting free-form messages.
///
/// Unknown service error codes deliberately remain internal. A service must opt
/// each expected business code into `expected_server_error_codes`; shared
/// consensus-state codes are recognized automatically.
pub fn classify_rpc_error(
    error: &Error,
    expected_server_error_codes: &HashSet<i64>,
) -> RpcFailureClass {
    match error.code {
        ErrorCode::ParseError
        | ErrorCode::InvalidRequest
        | ErrorCode::MethodNotFound
        | ErrorCode::InvalidParams => RpcFailureClass::Client,
        ErrorCode::InternalError => RpcFailureClass::Internal,
        ErrorCode::ServerError(code)
            if is_consensus_rpc_error_code(code) || expected_server_error_codes.contains(&code) =>
        {
            RpcFailureClass::Expected
        }
        ErrorCode::ServerError(_) => RpcFailureClass::Internal,
    }
}

/// JSON-RPC middleware that classifies every outgoing error and records slow calls.
///
/// Request parameters and structured error data are intentionally omitted from
/// logs. This keeps credentials and large cursor/context payloads out of process
/// logs while preserving service, method, code, class, message, and latency.
#[derive(Clone, Debug)]
pub struct RpcDiagnosticsMiddleware {
    service: &'static str,
    expected_server_error_codes: Arc<HashSet<i64>>,
    slow_threshold: Duration,
}

impl RpcDiagnosticsMiddleware {
    /// Creates middleware with the environment-configured slow-call threshold.
    pub fn new(
        service: &'static str,
        expected_server_error_codes: impl IntoIterator<Item = i64>,
    ) -> Self {
        let slow_threshold_ms = resolve_slow_rpc_threshold_ms();
        Self {
            service,
            expected_server_error_codes: Arc::new(
                expected_server_error_codes.into_iter().collect(),
            ),
            slow_threshold: Duration::from_millis(slow_threshold_ms),
        }
    }

    fn observe(&self, method: &str, elapsed: Duration, output: Option<&Output>) {
        let elapsed_ms = elapsed.as_millis();
        let slow = elapsed >= self.slow_threshold;
        let Some(Output::Failure(failure)) = output else {
            if slow {
                warn!(
                    "Slow RPC completed: service={}, method={}, outcome=success, elapsed_ms={}, threshold_ms={}",
                    self.service,
                    method,
                    elapsed_ms,
                    self.slow_threshold.as_millis()
                );
            }
            return;
        };

        let class = classify_rpc_error(&failure.error, &self.expected_server_error_codes);
        match class {
            RpcFailureClass::Internal => error!(
                "RPC failed: service={}, method={}, error_class={}, code={}, elapsed_ms={}, slow={}, message={}",
                self.service,
                method,
                class.as_str(),
                failure.error.code.code(),
                elapsed_ms,
                slow,
                failure.error.message
            ),
            RpcFailureClass::Client | RpcFailureClass::Expected if slow => warn!(
                "Slow RPC rejected: service={}, method={}, error_class={}, code={}, elapsed_ms={}, threshold_ms={}, message={}",
                self.service,
                method,
                class.as_str(),
                failure.error.code.code(),
                elapsed_ms,
                self.slow_threshold.as_millis(),
                failure.error.message
            ),
            RpcFailureClass::Client | RpcFailureClass::Expected => debug!(
                "RPC rejected: service={}, method={}, error_class={}, code={}, elapsed_ms={}, message={}",
                self.service,
                method,
                class.as_str(),
                failure.error.code.code(),
                elapsed_ms,
                failure.error.message
            ),
        }
    }
}

impl<M: Metadata> Middleware<M> for RpcDiagnosticsMiddleware {
    type Future = jsonrpc_core::middleware::NoopFuture;
    type CallFuture = Pin<Box<dyn Future<Output = Option<Output>> + Send>>;

    fn on_call<F, X>(&self, call: Call, meta: M, next: F) -> Either<Self::CallFuture, X>
    where
        F: Fn(Call, M) -> X + Send + Sync,
        X: Future<Output = Option<Output>> + Send + 'static,
    {
        let method = match &call {
            Call::MethodCall(call) => call.method.clone(),
            Call::Notification(call) => call.method.clone(),
            Call::Invalid { .. } => "<invalid>".to_string(),
        };
        let started_at = Instant::now();
        let middleware = self.clone();
        let future = next(call, meta);
        Either::Left(Box::pin(future.map(move |output| {
            middleware.observe(&method, started_at.elapsed(), output.as_ref());
            output
        })))
    }
}

fn resolve_slow_rpc_threshold_ms() -> u64 {
    let Some(value) = std::env::var(SLOW_RPC_THRESHOLD_MS_ENV).ok() else {
        return DEFAULT_SLOW_RPC_THRESHOLD_MS;
    };
    match value.parse::<u64>() {
        Ok(value) if value > 0 => value,
        _ => {
            warn!(
                "Invalid {}={:?}; using default {} ms",
                SLOW_RPC_THRESHOLD_MS_ENV, value, DEFAULT_SLOW_RPC_THRESHOLD_MS
            );
            DEFAULT_SLOW_RPC_THRESHOLD_MS
        }
    }
}

fn is_consensus_rpc_error_code(code: i64) -> bool {
    matches!(
        code,
        CONSENSUS_RPC_ERR_HEIGHT_NOT_SYNCED
            | CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY
            | CONSENSUS_RPC_ERR_SNAPSHOT_ID_MISMATCH
            | CONSENSUS_RPC_ERR_BLOCK_HASH_MISMATCH
            | CONSENSUS_RPC_ERR_VERSION_MISMATCH
            | CONSENSUS_RPC_ERR_LOCAL_STATE_COMMIT_MISMATCH
            | CONSENSUS_RPC_ERR_SYSTEM_STATE_ID_MISMATCH
            | CONSENSUS_RPC_ERR_NO_RECORD
            | CONSENSUS_RPC_ERR_STATE_NOT_RETAINED
            | CONSENSUS_RPC_ERR_HISTORY_NOT_AVAILABLE
            | CONSENSUS_RPC_ERR_VIEW_VERSION_MISMATCH
            | CONSENSUS_RPC_ERR_FORMULA_VERSION_MISMATCH
            | CONSENSUS_RPC_ERR_ACTIVATION_RECORD_NOT_FOUND
            | CONSENSUS_RPC_ERR_ACTIVATION_RECORD_CONFLICT
            | CONSENSUS_RPC_ERR_VERSION_NOT_SUPPORTED
            | CONSENSUS_RPC_ERR_ACTIVE_VERSION_SET_MISMATCH
            | CONSENSUS_RPC_ERR_COMMIT_PROTOCOL_VERSION_MISMATCH
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: ErrorCode) -> Error {
        Error {
            code,
            message: "test".to_string(),
            data: None,
        }
    }

    #[test]
    fn classifies_client_expected_and_internal_errors_without_message_matching() {
        let expected = HashSet::from([-32_010]);
        assert_eq!(
            classify_rpc_error(&error(ErrorCode::InvalidParams), &expected),
            RpcFailureClass::Client
        );
        assert_eq!(
            classify_rpc_error(
                &error(ErrorCode::ServerError(CONSENSUS_RPC_ERR_SNAPSHOT_NOT_READY)),
                &expected
            ),
            RpcFailureClass::Expected
        );
        assert_eq!(
            classify_rpc_error(&error(ErrorCode::ServerError(-32_010)), &expected),
            RpcFailureClass::Expected
        );
        assert_eq!(
            classify_rpc_error(&error(ErrorCode::InternalError), &expected),
            RpcFailureClass::Internal
        );
        assert_eq!(
            classify_rpc_error(&error(ErrorCode::ServerError(-32_999)), &expected),
            RpcFailureClass::Internal
        );
    }
}
