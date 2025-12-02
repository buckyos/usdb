use super::dirs::get_service_dir;
use flexi_logger::{detailed_format, Cleanup, Criterion, FileSpec, Logger, Naming};

pub fn init_log(service_name: &str) {
    let log_dir = get_service_dir(service_name).join("logs");
    std::fs::create_dir_all(&log_dir).expect("Failed to create log directory");

    Logger::try_with_str("info")
        .unwrap()
        .format(detailed_format) // Set detailed log format
        .log_to_file(
            FileSpec::default()
                .directory(log_dir) // Log files directory
                .basename(service_name), // Base name of log files (e.g., app_log.log)
        )
        // --- Enable log rotation ---
        .rotate(
            Criterion::Size(100_000_000), // Rotate when file size reaches 100 MB
            Naming::Timestamps,           // Use timestamps for new file names
            Cleanup::KeepLogFiles(20),    // Keep only the latest 20 log files
        )
        .start()
        .expect("Failed to initialize flexi_logger");
}
