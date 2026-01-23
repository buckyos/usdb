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

    let status_manager = status::StatusManager::new(config.clone(), output.clone()).map_err(|e| {
        error!("Failed to initialize status manager: {}", e);
        println!("Failed to initialize status manager: {}", e);
        std::process::exit(1);
    }).unwrap();

    status_manager.run_monitor();

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");
}
