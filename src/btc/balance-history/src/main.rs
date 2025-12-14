mod btc;
mod config;
mod db;
mod indexer;
mod output;
mod utxo;
mod tool;
mod rpc;
mod balance;

#[macro_use]
extern crate log;

use crate::config::BalanceHistoryConfig;
use crate::db::BalanceHistoryDB;
use crate::indexer::BalanceHistoryIndexer;
use crate::output::IndexOutput;
use clap::{Parser, Subcommand};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "balance-history")]
#[command(author = "buckyos")]
#[command(version = "0.1.0")]
#[command(about = "Bitcoin Balance History Indexer", long_about = None)]
#[command(long_about = None)]
struct BalanceHistoryCli {
    #[command(subcommand)]
    command: Option<BalanceHistoryCommands>,

    /// Run the service in daemon mode
    #[arg(short, long)]
    daemon: bool,
}

#[derive(Subcommand, Debug, Clone, Copy)]
#[command(rename_all = "kebab-case")]
enum BalanceHistoryCommands {
    /// Delete the database files, DANGEROUS: This will remove all indexed data!
    /// Use with caution.
    // #[command(alias = "c")]
    ClearDb {},
}

async fn main_run() {
    
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
    let output = IndexOutput::new();
    let output = Arc::new(output);

    // Initialize the database
    output.println("Initializing database...");
    let db = match BalanceHistoryDB::new(&root_dir, config.clone()) {
        Ok(database) => database,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            std::process::exit(1);
        }
    };
    let db = Arc::new(db);
    output.println("Database initialized.");

    // Start the indexer
    let indexer = match BalanceHistoryIndexer::new(config.clone(), db.clone(), output.clone()) {
        Ok(idx) => idx,
        Err(e) => {
            error!("Failed to initialize indexer: {}", e);
            std::process::exit(1);
        }
    };
    output.println("Starting indexer...");

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

    tokio::select! {
        _ = sigint => {
            info!("Received Ctrl+C, shutting down...");
            output.println("Shutting down...");
        }
        _ = sigterm => {
            info!("Received SIGTERM, shutting down...");
            output.println("Shutting down...");
        }
        result = indexer.run() => {
            if let Err(e) = result {
                error!("Indexer encountered an error: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Cleanup on shutdown
    indexer.shutdown().await;

    db.flush_all().unwrap_or_else(|e| {
        error!("Failed to flush database on shutdown: {}", e);
    });

    println!("Shutdown complete.");

    // Sleep a moment to ensure all logs are flushed
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
}

#[tokio::main]
async fn main() {
    let cli = BalanceHistoryCli::parse();

    match cli.command {
        Some(BalanceHistoryCommands::ClearDb {}) => {
            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            if let Err(e) = crate::tool::clear_db_files(&root_dir) {
                error!("Failed to clear database files: {}", e);
                std::process::exit(1);
            }
            println!("Database files cleared successfully.");
            return;
        }
        None => {}
    }

    if cli.daemon {
        // Proceed to daemonize and run the main process
        crate::tool::daemonize_process(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    }

    main_run().await;
}