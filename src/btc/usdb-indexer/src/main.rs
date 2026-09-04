#![allow(dead_code)]

mod balance;
mod btc;
mod config;
mod constants;
mod index;
mod inscription;
mod output;
mod service;
mod status;
mod storage;

#[macro_use]
extern crate log;

use clap::Parser;
use config::ConfigManager;
use index::InscriptionIndexer;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "usdb-indexer")]
#[command(author = "buckyos")]
#[command(version = "0.1.0")]
#[command(about = "USDB Indexer", long_about = None)]
struct UsdbIndexerCli {
    /// Override service root directory (default: ~/.usdb/usdb-indexer)
    #[arg(long)]
    root_dir: Option<PathBuf>,

    /// Skip process lock acquisition (for isolated integration tests)
    #[arg(long, default_value_t = false)]
    skip_process_lock: bool,
}

#[tokio::main]
async fn main() {
    let cli = UsdbIndexerCli::parse();

    // Acquire application lock to prevent multiple instances unless explicitly disabled.
    let _lock_guard = if cli.skip_process_lock {
        None
    } else {
        Some(usdb_util::init_process_lock(
            usdb_util::USDB_INDEXER_SERVICE_NAME,
        ))
    };

    let root_dir = cli
        .root_dir
        .unwrap_or_else(|| usdb_util::get_service_dir(usdb_util::USDB_INDEXER_SERVICE_NAME));
    let log_config = usdb_util::current_process_log_config!(usdb_util::USDB_INDEXER_SERVICE_NAME)
        .with_service_root_dir(root_dir.clone())
        .enable_console(false);
    let log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize usdb-indexer logging: {error}");
        std::process::exit(1);
    });
    info!(
        "USDB indexer startup options: root_dir={}, skip_process_lock={}",
        root_dir.display(),
        cli.skip_process_lock
    );

    let output = output::IndexOutput::new();
    let output = Arc::new(output);

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
    indexer
        .init()
        .await
        .map_err(|e| {
            error!("Failed to restore indexer runtime state: {}", e);
            println!("Failed to restore indexer runtime state: {}", e);
            std::process::exit(1);
        })
        .unwrap();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(());

    let rpc_server = if config.config().usdb.rpc_server_enabled {
        // jsonrpc-http-server owns a synchronous Tokio runtime internally. Its
        // startup and error-path teardown must run outside this async runtime.
        let rpc_config = config.clone();
        let rpc_status_manager = status_manager.clone();
        let rpc_indexer = indexer.clone();
        let rpc_shutdown_tx = shutdown_tx.clone();
        match tokio::task::spawn_blocking(move || {
            service::UsdbIndexerRpcServer::start(
                rpc_config,
                rpc_status_manager,
                rpc_indexer,
                rpc_shutdown_tx,
            )
        })
        .await
        {
            Ok(Ok(server)) => Some(server),
            Ok(Err(e)) => {
                error!("Failed to start usdb-indexer RPC server: {}", e);
                println!("Failed to start usdb-indexer RPC server: {}", e);
                std::process::exit(1);
            }
            Err(e) => {
                error!("USDB indexer RPC server startup task failed: {}", e);
                println!("USDB indexer RPC server startup task failed: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        info!("USDB indexer RPC server is disabled by config");
        None
    };

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
            status_manager.set_shutdown_requested(true);
            indexer.stop();
        }
        _ = sigterm => {
            output.println("Received SIGTERM, shutting down...");
            status_manager.set_shutdown_requested(true);
            indexer.stop();
        }
        _ = shutdown_rx.changed() => {
            output.println("Received RPC stop signal, shutting down...");
            status_manager.set_shutdown_requested(true);
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

    if let Some(server) = &rpc_server {
        server.close().await;
    }

    output.println("Indexer has shut down gracefully.");
    log_handle.shutdown();
}
