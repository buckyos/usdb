mod btc;
mod config;
mod db;
mod indexer;
mod output;
mod utxo;

#[macro_use]
extern crate log;

use crate::config::BalanceHistoryConfig;
use crate::db::BalanceHistoryDB;
use crate::indexer::BalanceHistoryIndexer;
use crate::output::IndexOutput;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let (_lock, _guard) = usdb_util::init_process_lock(usdb_util::BALANCE_HISTORY_SERVICE_NAME);

    // Init file logging
    usdb_util::init_log(usdb_util::BALANCE_HISTORY_SERVICE_NAME);

    let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    info!("Using service directory: {:?}", root_dir);

    // Load configuration
    let config = match BalanceHistoryConfig::load(&root_dir) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };
    let config = Arc::new(config);

    // Init console output
    let output = IndexOutput::new(0);
    let output = Arc::new(output);

    // Initialize the database
    output.set_message("Initializing database...");
    let db = match BalanceHistoryDB::new(&root_dir, config.clone()) {
        Ok(database) => database,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            std::process::exit(1);
        }
    };
    let db = Arc::new(db);
    output.set_message("Database initialized.");

    // Start the indexer
    let indexer = match BalanceHistoryIndexer::new(config.clone(), db.clone(), output.clone()) {
        Ok(idx) => idx,
        Err(e) => {
            error!("Failed to initialize indexer: {}", e);
            std::process::exit(1);
        }
    };

    output.set_message("Starting indexer...");
    indexer.run().await.unwrap();
}
