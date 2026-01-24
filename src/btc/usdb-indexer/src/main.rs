#![allow(dead_code)]

mod btc;
mod config;
mod constants;
mod index;
mod output;
mod status;
mod storage;
mod util;

#[macro_use]
extern crate log;

use config::ConfigManager;
use index::InscriptionIndexer;
use std::sync::Arc;
use usdb_util::LogConfig;

#[tokio::main]
async fn main() {
    // Acquire application lock to prevent multiple instances
    let (_lock, _guard) = usdb_util::init_process_lock(usdb_util::USDB_INDEXER_SERVICE_NAME);

    // Init file logging
    let config = LogConfig::new(usdb_util::USDB_INDEXER_SERVICE_NAME).enable_console(false);
    usdb_util::init_log(config);

    let output = output::IndexOutput::new();
    let output = Arc::new(output);

    let root_dir = usdb_util::get_service_dir(usdb_util::USDB_INDEXER_SERVICE_NAME);
    output.println(&format!("Using service directory: {}", root_dir.display()));

    // Load configuration
    let config = match ConfigManager::load(Some(root_dir)) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load config: {}", e);
            println!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };
    let config = Arc::new(config);

    let status_manager = status::StatusManager::new(config.clone(), output.clone())
        .map_err(|e| {
            error!("Failed to initialize status manager: {}", e);
            println!("Failed to initialize status manager: {}", e);
            std::process::exit(1);
        })
        .unwrap();
    let status_manager = Arc::new(status_manager);

    status_manager.run_monitor();

    let indexer = InscriptionIndexer::new(config.clone(), status_manager.clone())
        .map_err(|e| {
            error!("Failed to initialize indexer: {}", e);
            println!("Failed to initialize indexer: {}", e);
            std::process::exit(1);
        })
        .unwrap();
    let indexer = Arc::new(indexer);

    // Create a Future to wait for Ctrl+C (SIGINT) signal
    use tokio::signal;
    let sigint = signal::ctrl_c();

    // Create a Future to wait for SIGTERM signal (sent by kill command by default)
    #[cfg(unix)]
    let sigterm = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM signal handler")
            .recv()
            .await;
    };

    // On non-Unix systems, we only rely on Ctrl+C
    #[cfg(not(unix))]
    let sigterm = std::future::pending();

    output.println("Starting USDB Indexer...");
    tokio::select! {
        _ = sigint => {
            output.println("Received Ctrl+C, shutting down...");
            indexer.stop();
        }
        _ = sigterm => {
            output.println("Received SIGTERM, shutting down...");
            indexer.stop();
        }
        ret = indexer.run() => {
            output.println("Indexer run loop exited.");
            if let Err(e) = ret {
                error!("Indexer encountered an error: {}", e);
                // output.eprintln(&format!("Indexer encountered an error: {}", e));
                std::process::exit(1);
            }
        }
    }

    output.println("Indexer has shut down gracefully.");

    // Sleep a moment to ensure all logs are flushed
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    std::process::exit(0);
}
