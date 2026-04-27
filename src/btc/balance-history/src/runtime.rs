use crate::config::BalanceHistoryConfig;
use crate::index::BalanceHistoryIndexer;
use crate::output::IndexOutput;
use crate::service::BalanceHistoryRpcServer;
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::LogConfig;

/// Runs the balance-history indexing service until an external shutdown signal
/// or an indexer error stops the process.
pub async fn run_service(
    root_dir: PathBuf,
    max_block_height: Option<u32>,
    skip_process_lock: bool,
) {
    let _lock_guard = if skip_process_lock {
        None
    } else {
        Some(usdb_util::init_process_lock(
            usdb_util::BALANCE_HISTORY_SERVICE_NAME,
        ))
    };

    std::fs::create_dir_all(&root_dir).unwrap_or_else(|e| {
        println!(
            "Failed to create balance-history root directory {}: {}",
            root_dir.display(),
            e
        );
        std::process::exit(1);
    });

    let status = crate::status::SyncStatusManager::new();
    let status = Arc::new(status);
    let output = IndexOutput::new(status);
    let output = Arc::new(output);

    let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
        .with_service_root_dir(root_dir.clone())
        .enable_console(false);
    usdb_util::init_log(config);

    output.println(&format!("Using service directory: {}", root_dir.display()));

    let mut config = match BalanceHistoryConfig::load(&root_dir) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("Failed to load config: {}", e);
            output.eprintln(&format!("Failed to load config: {}", e));
            std::process::exit(1);
        }
    };

    if let Some(max_height) = max_block_height {
        config.sync.max_sync_block_height = max_height;
        output.println(&format!(
            "Indexing balance history up to block height: {}",
            max_height
        ));
    } else {
        output.println("Indexing balance history up to the latest block height.");
    }

    let config = Arc::new(config);

    let indexer = match BalanceHistoryIndexer::new(config.clone(), output.clone()) {
        Ok(idx) => idx,
        Err(e) => {
            output.eprintln(&format!("Failed to initialize indexer: {}", e));
            std::process::exit(1);
        }
    };
    output.println("Starting indexer...");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(());

    let ret = BalanceHistoryRpcServer::start(
        config.clone(),
        output.status().clone(),
        indexer.db().clone(),
        shutdown_tx,
    );
    if let Err(e) = &ret {
        output.eprintln(&format!("Failed to start RPC server: {}", e));
        std::process::exit(1);
    }
    let rpc_server = ret.unwrap();

    output.println(&format!(
        "RPC server started at {}",
        rpc_server.get_listen_url()
    ));

    use tokio::signal;
    let sigint = signal::ctrl_c();

    #[cfg(unix)]
    let sigterm = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending();

    tokio::select! {
        _ = sigint => {
            output.status().set_shutdown_requested(true);
            output.println("Received Ctrl+C, shutting down...");
        }
        _ = sigterm => {
            output.status().set_shutdown_requested(true);
            output.println("Received SIGTERM, shutting down...");
        }
        _ = shutdown_rx.changed() => {
            output.status().set_shutdown_requested(true);
            output.println("Shutdown signal received from RPC, shutting down...");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        result = indexer.run() => {
            output.println("Indexer run loop exited.");
            if let Err(e) = result {
                output.eprintln(&format!("Indexer encountered an error: {}", e));
                std::process::exit(1);
            }
        }
    }

    output.println("Shutting down indexer...");
    indexer.shutdown().await;
    output.println("Shutdown indexer complete.");

    indexer.db().flush_all().unwrap_or_else(|e| {
        error!("Failed to flush database on shutdown: {}", e);
    });

    rpc_server.close().await;

    println!("Shutdown complete.");

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
}
