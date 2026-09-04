use crate::config::BalanceHistoryConfig;
use crate::index::BalanceHistoryIndexer;
use crate::output::IndexOutput;
use crate::service::BalanceHistoryRpcServer;
use std::path::PathBuf;
use std::sync::Arc;

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

    let log_config =
        usdb_util::current_process_log_config!(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
            .with_service_root_dir(root_dir.clone())
            .enable_console(false);
    let log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize balance-history logging: {error}");
        std::process::exit(1);
    });
    info!(
        "Balance-history startup options: root_dir={}, max_block_height={:?}, skip_process_lock={}",
        root_dir.display(),
        max_block_height,
        skip_process_lock
    );

    let status = crate::status::SyncStatusManager::new();
    let status = Arc::new(status);
    let output = IndexOutput::new(status);
    let output = Arc::new(output);

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

    let memory = usdb_util::get_memory_usage_snapshot();
    if let Err(error) = config.validate_memory_budget(memory.limit_bytes) {
        error!("Invalid balance-history memory configuration: {}", error);
        output.eprintln(&format!(
            "Invalid balance-history memory configuration: {}",
            error
        ));
        std::process::exit(1);
    }
    let cache_budget = config.sync.cache_budget_bytes().unwrap();
    info!(
        "Balance-history memory plan: source={}, limit_bytes={}, used_bytes={}, utxo_cache_bytes={}, balance_cache_bytes={}, total_cache_bytes={}, pressure_threshold_percent={}",
        memory.source,
        memory.limit_bytes,
        memory.used_bytes,
        config.sync.utxo_max_cache_bytes,
        config.sync.balance_max_cache_bytes,
        cache_budget,
        config.sync.max_memory_percent
    );
    output.println(&format!(
        "Memory plan: {} byte cache budget within {} byte {} limit",
        cache_budget, memory.limit_bytes, memory.source
    ));

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

    // jsonrpc-http-server owns a synchronous Tokio runtime internally. Start it
    // outside this async runtime so an initialization error can drop that
    // runtime safely and preserve the original error.
    let rpc_config = config.clone();
    let rpc_status = output.status().clone();
    let rpc_db = indexer.db().clone();
    let ret = tokio::task::spawn_blocking(move || {
        BalanceHistoryRpcServer::start(rpc_config, rpc_status, rpc_db, shutdown_tx)
    })
    .await
    .unwrap_or_else(|error| Err(format!("RPC server startup task failed: {error}")));
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

    output.println("Shutdown complete.");
    log_handle.shutdown();
}
