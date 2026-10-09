use std::future::Future;

/// Register Unix stop handlers immediately, before asynchronous initialization or DB recovery.
///
/// Call inside a Tokio runtime and retain the returned future across startup phases.
/// A signal received before polling stays queued; callers decide when it is safe to
/// stop and must finish their own database writes and worker cleanup.
/// Non-Unix platforms retain Tokio's Ctrl+C handling when the future is polled.
pub fn shutdown_signal(
    service: &'static str,
) -> Result<impl Future<Output = &'static str> + Send, String> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let register = |kind, name| {
            signal(kind).map_err(|error| {
                let msg = format!(
                    "Failed to register shutdown signal: service={service}, signal={name}, error={error}"
                );
                error!("{msg}");
                msg
            })
        };
        let mut terminate = register(SignalKind::terminate(), "SIGTERM")?;
        let mut interrupt = register(SignalKind::interrupt(), "SIGINT")?;
        Ok(async move {
            tokio::select! {
                _ = terminate.recv() => "SIGTERM",
                _ = interrupt.recv() => "SIGINT",
            }
        })
    }
    #[cfg(not(unix))]
    {
        Ok(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                error!("Shutdown signal failed: service={service}, signal=Ctrl+C, error={error}");
            }
            "Ctrl+C"
        })
    }
}

#[cfg(all(test, unix))]
#[path = "../../../../tests/shutdown_signal.rs"]
mod tests;
