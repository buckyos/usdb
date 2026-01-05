#![allow(dead_code)]

mod btc;
mod config;
mod index;
mod constants;
mod util;
mod storage;

#[macro_use]
extern crate log;

use std::process::exit;
use config::ConfigManager;
use named_lock::{NamedLock};

#[tokio::main]
async fn main() {
    // Acquire application lock to prevent multiple instances
    let guard = match NamedLock::create("usdb_indexer_lock"){
        Ok(guard) => guard,
        Err(e) => {
            println!("Failed to acquire application lock: {}", e);
            exit(1);
        }
    };
    let _guard = match guard.lock() {
        Ok(_lock_guard) => { _lock_guard },
        Err(e) => {
            println!("Another instance is already running: {}", e);
            exit(1);
        }
    };

    // Init config on the default path $HOME/.usdb/config.json
    let config = match ConfigManager::new(None) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to initialize config: {}", e);
            exit(1);
        }
    };


}
