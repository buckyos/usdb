use axum::Router;
use std::future::{Future, IntoFuture};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::Instant;

/// Stop accepting connections, drain active requests, and report long waits without aborting writes.
pub async fn serve_until_shutdown(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = &'static str> + Send + 'static,
) -> std::io::Result<()> {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let reason = shutdown.await;
            info!("Control-plane shutdown started: signal={reason}, phase=draining_http");
            let _ = started_tx.send(Instant::now());
        })
        .into_future();
    tokio::pin!(server);
    let started = tokio::select! {
        biased;
        started = started_rx => started.unwrap_or_else(|_| Instant::now()),
        result = &mut server => return result,
    };
    let period = Duration::from_secs(15);
    let mut progress = tokio::time::interval_at(started + period, period);
    loop {
        tokio::select! {
            result = &mut server => {
                info!("Control-plane shutdown finished: phase=draining_http, elapsed_ms={}, success={}",
                    started.elapsed().as_millis(), result.is_ok());
                return result;
            }
            _ = progress.tick() => {
                info!("Control-plane shutdown progress: phase=draining_http, elapsed_secs={}, waiting_for=active_requests",
                    started.elapsed().as_secs());
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/control_plane_shutdown.rs"]
mod tests;
