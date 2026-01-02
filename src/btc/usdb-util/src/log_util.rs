use super::dirs::get_service_dir;
use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, Naming, detailed_format};

pub struct LogConfig {
    pub service_name: String,
    pub file_name: Option<String>,
    pub console: bool,
}

impl LogConfig {
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
            file_name: None,
            console: false,
        }
    }

    pub fn with_file_name(mut self, file_name: &str) -> Self {
        self.file_name = Some(file_name.to_string());
        self
    }

    pub fn enable_console(mut self, enable: bool) -> Self {
        self.console = enable;
        self
    }
}

pub fn init_log(config: LogConfig) {
    let log_dir = get_service_dir(&config.service_name).join("logs");
    std::fs::create_dir_all(&log_dir).expect("Failed to create log directory");

    let file_name = config.file_name.unwrap_or(config.service_name);
    let logger = Logger::try_with_str("info")
        .unwrap()
        .format(detailed_format) // Set detailed log format
        .log_to_file(
            FileSpec::default()
                .directory(log_dir) // Log files directory
                .basename(file_name), // Base name of log files
        )
        // --- Enable log rotation ---
        .rotate(
            Criterion::Size(100_000_000), // Rotate when file size reaches 100 MB
            Naming::Timestamps,           // Use timestamps for new file names
            Cleanup::KeepLogFiles(20),    // Keep only the latest 20 log files
        );

    let logger = if config.console {
        logger.duplicate_to_stderr(flexi_logger::Duplicate::All)
    } else {
        logger
    };

    logger.start().expect("Failed to initialize flexi_logger");
}
