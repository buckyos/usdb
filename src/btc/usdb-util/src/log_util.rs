use super::dirs::get_service_dir;
use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, LoggerHandle, Naming, detailed_format};
use std::backtrace::Backtrace;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Environment variable used to override the process log specification.
pub const PROCESS_LOG_LEVEL_ENV: &str = "USDB_PROCESS_LOG_LEVEL";
/// Environment variable used to override the per-file rotation size in bytes.
pub const PROCESS_LOG_MAX_FILE_BYTES_ENV: &str = "USDB_PROCESS_LOG_MAX_FILE_BYTES";
/// Environment variable used to override the number of rotated files retained.
pub const PROCESS_LOG_KEEP_FILES_ENV: &str = "USDB_PROCESS_LOG_KEEP_FILES";

/// Default process log specification.
pub const DEFAULT_PROCESS_LOG_LEVEL: &str = "info";
/// Default size threshold for one process log file: 100 MB.
pub const DEFAULT_PROCESS_LOG_MAX_FILE_BYTES: u64 = 100_000_000;
/// Default number of rotated process log files retained per basename.
pub const DEFAULT_PROCESS_LOG_KEEP_FILES: usize = 20;

/// One sampled observation in a consecutive failure sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureLogEvent {
    /// One-based consecutive failure attempt.
    pub attempt: u64,
    /// Time elapsed since the first failure in this sequence.
    pub elapsed: Duration,
    /// Whether the caller should emit a repeated failure log for this attempt.
    pub should_log: bool,
}

/// Summary returned when a previously failing operation succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureRecoveryEvent {
    /// Number of consecutive failures before recovery.
    pub failed_attempts: u64,
    /// Total time from the first failure until recovery.
    pub elapsed: Duration,
}

/// Tracks consecutive failures and samples repeated log records.
///
/// Attempts 1-3, powers of two, and every `report_every` attempt are reported.
/// This keeps long dependency outages observable without writing one identical
/// error per retry. A zero `report_every` disables the periodic rule while
/// retaining the initial and power-of-two samples.
#[derive(Debug)]
pub struct ConsecutiveFailureTracker {
    report_every: u64,
    failed_attempts: u64,
    first_failure_at: Option<Instant>,
}

impl ConsecutiveFailureTracker {
    /// Creates an empty consecutive-failure tracker.
    pub fn new(report_every: u64) -> Self {
        Self {
            report_every,
            failed_attempts: 0,
            first_failure_at: None,
        }
    }

    /// Records one failed attempt and returns its sampling decision.
    pub fn record_failure(&mut self) -> FailureLogEvent {
        self.record_failure_at(Instant::now())
    }

    /// Clears a failure sequence and returns its recovery summary, if any.
    pub fn record_success(&mut self) -> Option<FailureRecoveryEvent> {
        self.record_success_at(Instant::now())
    }

    fn record_failure_at(&mut self, now: Instant) -> FailureLogEvent {
        let first_failure_at = *self.first_failure_at.get_or_insert(now);
        self.failed_attempts = self.failed_attempts.saturating_add(1);
        let attempt = self.failed_attempts;
        let periodic = self.report_every > 0 && attempt.is_multiple_of(self.report_every);

        FailureLogEvent {
            attempt,
            elapsed: now.saturating_duration_since(first_failure_at),
            should_log: attempt <= 3 || attempt.is_power_of_two() || periodic,
        }
    }

    fn record_success_at(&mut self, now: Instant) -> Option<FailureRecoveryEvent> {
        let first_failure_at = self.first_failure_at.take()?;
        let failed_attempts = std::mem::take(&mut self.failed_attempts);
        Some(FailureRecoveryEvent {
            failed_attempts,
            elapsed: now.saturating_duration_since(first_failure_at),
        })
    }
}

/// Configuration used to initialize logging for one USDB process.
///
/// Explicit builder values take precedence over `USDB_PROCESS_LOG_*`
/// environment variables. The environment variables in turn take precedence
/// over the shared defaults.
#[derive(Debug, Clone)]
pub struct LogConfig {
    service_name: String,
    file: bool,
    file_name: Option<String>,
    console: bool,
    service_root_dir: Option<PathBuf>,
    level: Option<String>,
    max_file_bytes: Option<u64>,
    keep_files: Option<usize>,
    binary_name: Option<String>,
    binary_version: Option<String>,
    build_revision: Option<String>,
}

impl LogConfig {
    /// Creates process logging configuration with file output enabled.
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
            file_name: None,
            console: false,
            file: true,
            service_root_dir: None,
            level: None,
            max_file_bytes: None,
            keep_files: None,
            binary_name: None,
            binary_version: None,
            build_revision: None,
        }
    }

    /// Enables or disables file output.
    pub fn enable_file(mut self, enable: bool) -> Self {
        self.file = enable;
        self
    }

    /// Overrides the basename used for this process's log files.
    pub fn with_file_name(mut self, file_name: &str) -> Self {
        self.file_name = Some(file_name.to_string());
        self
    }

    /// Enables or disables duplication to standard error.
    pub fn enable_console(mut self, enable: bool) -> Self {
        self.console = enable;
        self
    }

    /// Uses the supplied service root, with logs stored under its `logs` child.
    pub fn with_service_root_dir(mut self, service_root_dir: PathBuf) -> Self {
        self.service_root_dir = Some(service_root_dir);
        self
    }

    /// Overrides the flexi_logger log specification.
    pub fn with_level(mut self, level: &str) -> Self {
        self.level = Some(level.to_string());
        self
    }

    /// Overrides the per-file rotation threshold in bytes.
    pub fn with_max_file_bytes(mut self, max_file_bytes: u64) -> Self {
        self.max_file_bytes = Some(max_file_bytes);
        self
    }

    /// Overrides the number of rotated files retained for this basename.
    pub fn with_keep_files(mut self, keep_files: usize) -> Self {
        self.keep_files = Some(keep_files);
        self
    }

    /// Attaches immutable build identity to the startup record.
    pub fn with_process_identity(
        mut self,
        binary_name: &str,
        binary_version: &str,
        build_revision: Option<&str>,
    ) -> Self {
        self.binary_name = Some(binary_name.to_string());
        self.binary_version = Some(binary_version.to_string());
        self.build_revision = build_revision.map(str::to_string);
        self
    }

    fn resolve(self) -> Result<ResolvedLogConfig, LogInitError> {
        self.resolve_with(|name| std::env::var(name).ok())
    }

    fn resolve_with<F>(self, get_env: F) -> Result<ResolvedLogConfig, LogInitError>
    where
        F: Fn(&str) -> Option<String>,
    {
        if !self.file && !self.console {
            return Err(LogInitError::new(
                "at least one logging destination must be enabled",
            ));
        }

        let level = self
            .level
            .or_else(|| get_env(PROCESS_LOG_LEVEL_ENV))
            .unwrap_or_else(|| DEFAULT_PROCESS_LOG_LEVEL.to_string());
        Logger::try_with_str(&level).map_err(|error| {
            LogInitError::new(format!(
                "invalid {PROCESS_LOG_LEVEL_ENV} log specification {level:?}: {error}"
            ))
        })?;

        let max_file_bytes = resolve_positive_number(
            self.max_file_bytes,
            get_env(PROCESS_LOG_MAX_FILE_BYTES_ENV),
            DEFAULT_PROCESS_LOG_MAX_FILE_BYTES,
            PROCESS_LOG_MAX_FILE_BYTES_ENV,
        )?;
        let keep_files = resolve_positive_number(
            self.keep_files,
            get_env(PROCESS_LOG_KEEP_FILES_ENV),
            DEFAULT_PROCESS_LOG_KEEP_FILES,
            PROCESS_LOG_KEEP_FILES_ENV,
        )?;

        let service_root_dir = self
            .service_root_dir
            .unwrap_or_else(|| get_service_dir(&self.service_name));
        let file_name = self.file_name.unwrap_or_else(|| self.service_name.clone());
        let binary_name = self.binary_name.unwrap_or_else(current_binary_name);

        Ok(ResolvedLogConfig {
            service_name: self.service_name,
            file: self.file,
            file_name,
            console: self.console,
            service_root_dir,
            level,
            max_file_bytes,
            keep_files,
            binary_name,
            binary_version: self.binary_version.unwrap_or_else(|| "unknown".to_string()),
            build_revision: self.build_revision.unwrap_or_else(|| "unknown".to_string()),
        })
    }
}

/// Builds a [`LogConfig`] with identity from the calling Cargo binary.
///
/// Release builds should set `USDB_BUILD_REVISION` at compile time. GitHub
/// builds fall back to the compile-time `GITHUB_SHA` value.
#[macro_export]
macro_rules! current_process_log_config {
    ($service_name:expr) => {
        $crate::LogConfig::new($service_name).with_process_identity(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            option_env!("USDB_BUILD_REVISION").or(option_env!("GITHUB_SHA")),
        )
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLogConfig {
    service_name: String,
    file: bool,
    file_name: String,
    console: bool,
    service_root_dir: PathBuf,
    level: String,
    max_file_bytes: u64,
    keep_files: usize,
    binary_name: String,
    binary_version: String,
    build_revision: String,
}

/// Error returned when process logging cannot be configured or started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogInitError {
    message: String,
}

impl LogInitError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for LogInitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LogInitError {}

/// Handle used to flush or shut down the initialized process logger.
pub struct ProcessLogger {
    handle: LoggerHandle,
}

impl ProcessLogger {
    /// Flushes all logger writers immediately.
    pub fn flush(&self) {
        self.handle.flush();
    }

    /// Flushes and shuts down all logger writers and cleanup workers.
    pub fn shutdown(&self) {
        self.handle.flush();
        self.handle.shutdown();
    }
}

/// Initializes the global process logger and installs the shared panic hook.
///
/// The caller must retain the returned handle for the process lifetime and call
/// [`ProcessLogger::shutdown`] during graceful shutdown. Initialization errors
/// are returned so binaries can report them on stderr before exiting.
pub fn init_log(config: LogConfig) -> Result<ProcessLogger, LogInitError> {
    let config = config.resolve()?;
    let log_dir = config.service_root_dir.join("logs");
    if config.file {
        std::fs::create_dir_all(&log_dir).map_err(|error| {
            LogInitError::new(format!(
                "failed to create log directory {}: {error}",
                log_dir.display()
            ))
        })?;
    }

    let logger = Logger::try_with_str(&config.level)
        .map_err(|error| LogInitError::new(format!("failed to parse log level: {error}")))?
        .format(detailed_format);
    let logger = if config.file {
        logger
            .log_to_file(
                FileSpec::default()
                    .directory(&log_dir)
                    .basename(&config.file_name),
            )
            .rotate(
                Criterion::Size(config.max_file_bytes),
                Naming::Timestamps,
                Cleanup::KeepLogFiles(config.keep_files),
            )
    } else {
        logger
    };
    let logger = if config.file && config.console {
        logger.duplicate_to_stderr(flexi_logger::Duplicate::All)
    } else {
        logger
    };

    let handle = logger
        .start()
        .map_err(|error| LogInitError::new(format!("failed to start process logger: {error}")))?;
    install_panic_hook(config.service_name.clone(), handle.clone());
    log_startup(&config, &log_dir);

    Ok(ProcessLogger { handle })
}

fn resolve_positive_number<T>(
    explicit: Option<T>,
    environment: Option<String>,
    default: T,
    environment_name: &str,
) -> Result<T, LogInitError>
where
    T: Copy + Display + std::str::FromStr + PartialEq + Default,
    <T as std::str::FromStr>::Err: Display,
{
    let value = if let Some(value) = explicit {
        value
    } else if let Some(value) = environment {
        value.parse::<T>().map_err(|error| {
            LogInitError::new(format!(
                "invalid {environment_name} value {value:?}: {error}"
            ))
        })?
    } else {
        default
    };
    if value == T::default() {
        return Err(LogInitError::new(format!(
            "{environment_name} must be greater than zero"
        )));
    }
    Ok(value)
}

fn current_binary_name() -> String {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn log_startup(config: &ResolvedLogConfig, log_dir: &Path) {
    let log_destination = if config.file {
        format!("{}/{}_rCURRENT.log", log_dir.display(), config.file_name)
    } else {
        "disabled".to_string()
    };
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    log::info!(
        target: "usdb::startup",
        "Process started: service={}, binary={}, version={}, build_revision={}, pid={}, executable={}, service_root={}, file_log={}, console_log={}, log_level={}, max_file_bytes={}, keep_files={}",
        config.service_name,
        config.binary_name,
        config.binary_version,
        config.build_revision,
        std::process::id(),
        executable,
        config.service_root_dir.display(),
        log_destination,
        config.console,
        config.level,
        config.max_file_bytes,
        config.keep_files,
    );
}

fn install_panic_hook(service_name: String, handle: LoggerHandle) {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let location = panic_info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_string());
        let message = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let backtrace = Backtrace::force_capture();

        log::error!(
            target: "usdb::panic",
            "Process panicked: service={}, thread={}, location={}, message={}, backtrace={}",
            service_name,
            thread_name,
            location,
            message,
            backtrace,
        );
        handle.flush();
        previous_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve_with_environment(
        config: LogConfig,
        environment: &[(&str, &str)],
    ) -> Result<ResolvedLogConfig, LogInitError> {
        let environment = environment
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>();
        config.resolve_with(|name| environment.get(name).cloned())
    }

    #[test]
    fn defaults_preserve_existing_file_logging_policy() {
        let root = PathBuf::from("/tmp/usdb-log-defaults");
        let resolved = resolve_with_environment(
            LogConfig::new("test-service").with_service_root_dir(root.clone()),
            &[],
        )
        .unwrap();

        assert!(resolved.file);
        assert!(!resolved.console);
        assert_eq!(resolved.service_root_dir, root);
        assert_eq!(resolved.file_name, "test-service");
        assert_eq!(resolved.level, DEFAULT_PROCESS_LOG_LEVEL);
        assert_eq!(resolved.max_file_bytes, DEFAULT_PROCESS_LOG_MAX_FILE_BYTES);
        assert_eq!(resolved.keep_files, DEFAULT_PROCESS_LOG_KEEP_FILES);
    }

    #[test]
    fn environment_overrides_shared_defaults() {
        let resolved = resolve_with_environment(
            LogConfig::new("test-service"),
            &[
                (PROCESS_LOG_LEVEL_ENV, "warn,test_module=debug"),
                (PROCESS_LOG_MAX_FILE_BYTES_ENV, "4096"),
                (PROCESS_LOG_KEEP_FILES_ENV, "7"),
            ],
        )
        .unwrap();

        assert_eq!(resolved.level, "warn,test_module=debug");
        assert_eq!(resolved.max_file_bytes, 4096);
        assert_eq!(resolved.keep_files, 7);
    }

    #[test]
    fn explicit_values_take_precedence_over_environment() {
        let resolved = resolve_with_environment(
            LogConfig::new("test-service")
                .with_level("debug")
                .with_max_file_bytes(8192)
                .with_keep_files(9),
            &[
                (PROCESS_LOG_LEVEL_ENV, "error"),
                (PROCESS_LOG_MAX_FILE_BYTES_ENV, "1024"),
                (PROCESS_LOG_KEEP_FILES_ENV, "3"),
            ],
        )
        .unwrap();

        assert_eq!(resolved.level, "debug");
        assert_eq!(resolved.max_file_bytes, 8192);
        assert_eq!(resolved.keep_files, 9);
    }

    #[test]
    fn invalid_environment_is_rejected() {
        let invalid_level = resolve_with_environment(
            LogConfig::new("test-service"),
            &[(PROCESS_LOG_LEVEL_ENV, "not a valid spec")],
        )
        .unwrap_err();
        assert!(invalid_level.to_string().contains(PROCESS_LOG_LEVEL_ENV));

        let zero_rotation = resolve_with_environment(
            LogConfig::new("test-service"),
            &[(PROCESS_LOG_MAX_FILE_BYTES_ENV, "0")],
        )
        .unwrap_err();
        assert!(
            zero_rotation
                .to_string()
                .contains("must be greater than zero")
        );
    }

    #[test]
    fn at_least_one_destination_is_required() {
        let error = resolve_with_environment(
            LogConfig::new("test-service")
                .enable_file(false)
                .enable_console(false),
            &[],
        )
        .unwrap_err();

        assert!(error.to_string().contains("logging destination"));
    }

    #[test]
    fn failure_tracker_samples_initial_power_of_two_and_periodic_attempts() {
        let start = Instant::now();
        let mut tracker = ConsecutiveFailureTracker::new(10);
        let mut sampled = Vec::new();

        for attempt in 1..=20u64 {
            let event = tracker.record_failure_at(start + Duration::from_secs(attempt));
            assert_eq!(event.attempt, attempt);
            if event.should_log {
                sampled.push(attempt);
            }
        }

        assert_eq!(sampled, vec![1, 2, 3, 4, 8, 10, 16, 20]);
    }

    #[test]
    fn failure_tracker_reports_recovery_and_resets_sequence() {
        let start = Instant::now();
        let mut tracker = ConsecutiveFailureTracker::new(60);
        tracker.record_failure_at(start);
        tracker.record_failure_at(start + Duration::from_secs(2));

        let recovery = tracker
            .record_success_at(start + Duration::from_secs(5))
            .unwrap();
        assert_eq!(recovery.failed_attempts, 2);
        assert_eq!(recovery.elapsed, Duration::from_secs(5));
        assert_eq!(
            tracker.record_success_at(start + Duration::from_secs(6)),
            None
        );

        let next = tracker.record_failure_at(start + Duration::from_secs(7));
        assert_eq!(next.attempt, 1);
        assert_eq!(next.elapsed, Duration::ZERO);
    }
}
