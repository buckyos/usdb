#![allow(dead_code)]

mod bench;
mod btc;
mod cache;
mod config;
mod db;
mod index;
mod output;
mod service;
mod status;
mod tool;

#[macro_use]
extern crate log;

use crate::config::BalanceHistoryConfig;
use crate::db::BalanceHistoryDB;
use crate::index::BalanceHistoryIndexer;
use crate::output::IndexOutput;
use crate::service::BalanceHistoryRpcServer;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use usdb_util::LogConfig;

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

    IndexAddress {},

    Snapshot {},

    VerifySnapshot {},

    Verify {},
}

async fn main_run() {
    let (_lock, _guard) = usdb_util::init_process_lock(usdb_util::BALANCE_HISTORY_SERVICE_NAME);

    // Init console output
    let status = status::SyncStatusManager::new();
    let status = Arc::new(status);
    let output = IndexOutput::new(status);
    let output = Arc::new(output);

    // Init file logging
    let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME).enable_console(false);
    usdb_util::init_log(config);

    let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    info!("Using service directory: {}", root_dir.display());
    output.println(&format!("Using service directory: {}", root_dir.display()));

    // Load configuration
    let config = match BalanceHistoryConfig::load(&root_dir) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load config: {}", e);
            println!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };
    let config = Arc::new(config);

    // Initialize the database
    output.println("Initializing database... this may take a while.");
    let db = match BalanceHistoryDB::new(&root_dir, config.clone()) {
        Ok(database) => database,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            output.println(&format!("Failed to initialize database: {}", e));
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
            output.println(&format!("Failed to initialize indexer: {}", e));
            std::process::exit(1);
        }
    };
    output.println("Starting indexer...");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(());

    // Start the RPC server
    let ret = BalanceHistoryRpcServer::start(
        config.clone(),
        output.status().clone(),
        db.clone(),
        shutdown_tx,
    );
    if let Err(e) = &ret {
        error!("Failed to start RPC server: {}", e);
        output.println(&format!("Failed to start RPC server: {}", e));
        std::process::exit(1);
    }
    let rpc_server = ret.unwrap();

    output.println(&format!("RPC server started at {}", rpc_server.get_listen_url()));

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
        _ = shutdown_rx.changed() => {
            info!("Shutdown signal received from RPC, shutting down...");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        result = indexer.run() => {
            output.println("Indexer run loop exited.");
            if let Err(e) = result {
                error!("Indexer encountered an error: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Cleanup on shutdown
    output.println("Shutting down indexer...");
    indexer.shutdown().await;
    output.println("Shutdown indexer complete.");

    db.flush_all().unwrap_or_else(|e| {
        error!("Failed to flush database on shutdown: {}", e);
    });

    rpc_server.close().await;

    println!("Shutdown complete.");

    // Sleep a moment to ensure all logs are flushed
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
}

#[tokio::main]
async fn main() {
    let cli = BalanceHistoryCli::parse();

    match cli.command {
        Some(BalanceHistoryCommands::ClearDb {}) => {
            // Init file logging
            let file_name = format!("{}_clear_db", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig {
                service_name: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
                file_name: Some(file_name),
                console: false,
            };
            usdb_util::init_log(config);

            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            if let Err(e) = crate::tool::clear_db_files(&root_dir) {
                error!("Failed to clear database files: {}", e);
                std::process::exit(1);
            }
            println!("Database files cleared successfully.");
            return;
        }
        Some(BalanceHistoryCommands::IndexAddress {}) => {
            // Init file logging
            let file_name = format!("{}_index_address", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig {
                service_name: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
                file_name: Some(file_name),
                console: false,
            };
            usdb_util::init_log(config);

            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            println!("Indexing addresses in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            let address_index =
                crate::index::AddressIndexer::new(&root_dir, config.clone(), output.clone())
                    .unwrap();
            if let Err(e) = address_index.build_index() {
                error!("Failed to build address index: {}", e);
                output.println(&format!("Failed to build address index: {}", e));
                std::process::exit(1);
            }

            println!("Address index built successfully.");
            return;
        }
        Some(BalanceHistoryCommands::Snapshot {}) => {
            // Init file logging
            let config = LogConfig {
                service_name: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
                file_name: Some(format!(
                    "{}_snapshot",
                    usdb_util::BALANCE_HISTORY_SERVICE_NAME
                )),
                console: false,
            };
            usdb_util::init_log(config);

            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            println!("Generating snapshot in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            let db = match BalanceHistoryDB::new(&root_dir, config.clone()) {
                Ok(database) => database,
                Err(e) => {
                    error!("Failed to initialize database: {}", e);
                    output.println(&format!("Failed to initialize database: {}", e));
                    std::process::exit(1);
                }
            };
            let db = Arc::new(db);

            let snapshot_indexer =
                crate::index::SnapshotIndexer::new(config.clone(), db.clone(), output.clone());
            let target_block_height = 400_000; // Example target height
            if let Err(e) = snapshot_indexer.run(target_block_height) {
                error!("Failed to generate snapshot: {}", e);
                output.println(&format!("Failed to generate snapshot: {}", e));
                std::process::exit(1);
            }

            println!("Snapshot generated successfully.");
            return;
        }
        Some(BalanceHistoryCommands::VerifySnapshot {}) => {
            // Init file logging
            let config = LogConfig {
                service_name: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
                file_name: Some(format!(
                    "{}_verify_snapshot",
                    usdb_util::BALANCE_HISTORY_SERVICE_NAME
                )),
                console: true,
            };
            usdb_util::init_log(config);

            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            println!("Verifying snapshot in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            // Load Address DB
            let db = match crate::db::AddressDB::new(&root_dir) {
                Ok(database) => database,
                Err(e) => {
                    error!("Failed to open address database: {}", e);
                    output.println(&format!("Failed to open address database: {}", e));
                    std::process::exit(1);
                }
            };
            let address_db = Arc::new(db);

            let block_height = 400_000; // Example block height
            let db = match crate::db::SnapshotDB::open_by_height(&root_dir, block_height, false) {
                Ok(database) => database,
                Err(e) => {
                    error!("Failed to open snapshot database: {}", e);
                    output.println(&format!("Failed to open snapshot database: {}", e));
                    std::process::exit(1);
                }
            };
            let snapshot_db = Arc::new(db);

            let electrs_client = match usdb_util::ElectrsClient::new(&config.electrs.rpc_url()) {
                Ok(client) => client,
                Err(e) => {
                    error!("Failed to create electrs client: {}", e);
                    output.println(&format!("Failed to create electrs client: {}", e));
                    std::process::exit(1);
                }
            };
            let electrs_client = Arc::new(electrs_client);

            let verifier = crate::index::SnapshotVerifier::new(
                config.clone(),
                electrs_client,
                address_db,
                snapshot_db,
            );
            if let Err(e) = verifier.verify(1).await {
                error!("Failed to verify snapshot: {}", e);
                output.println(&format!("Failed to verify snapshot: {}", e));
                std::process::exit(1);
            }

            println!("Snapshot verified successfully.");
            return;
        }
        Some(BalanceHistoryCommands::Verify {}) => {
            // Init file logging
            let config = LogConfig {
                service_name: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
                file_name: Some(format!(
                    "{}_verify",
                    usdb_util::BALANCE_HISTORY_SERVICE_NAME
                )),
                console: true,
            };
            usdb_util::init_log(config);

            let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            println!("Verifying balance history in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            // Load balance history DB
            let db = match BalanceHistoryDB::new(&root_dir, config.clone()) {
                Ok(database) => database,
                Err(e) => {
                    error!("Failed to initialize database: {}", e);
                    output.println(&format!("Failed to initialize database: {}", e));
                    std::process::exit(1);
                }
            };
            let db = Arc::new(db);

            let electrs_client = match usdb_util::ElectrsClient::new(&config.electrs.rpc_url()) {
                Ok(client) => client,
                Err(e) => {
                    error!("Failed to create electrs client: {}", e);
                    output.println(&format!("Failed to create electrs client: {}", e));
                    std::process::exit(1);
                }
            };
            let electrs_client = Arc::new(electrs_client);

            let verifier = crate::index::BalanceHistoryVerifier::new(
                config.clone(),
                electrs_client,
                db,
            );

            tokio::task::spawn_blocking(move || {
                if let Err(e) = verifier.verify_latest() {
                    error!("Failed to verify balance history: {}", e);
                    output.println(&format!("Failed to verify balance history: {}", e));
                    std::process::exit(1);
                }
            })
            .await
            .unwrap();

            println!("Snapshot verified successfully.");
            return;
        }
        None => {}
    }

    if cli.daemon {
        // Proceed to daemonize and run the main process
        crate::tool::daemonize_process(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    }

    main_run().await;
    println!("Balance History service exited.");
}
