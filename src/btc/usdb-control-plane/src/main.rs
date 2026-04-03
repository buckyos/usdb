mod config;
mod models;
mod rpc_client;
mod server;

#[macro_use]
extern crate log;

use clap::Parser;
use config::ControlPlaneConfig;
use std::path::PathBuf;
use usdb_util::{LogConfig, USDB_CONTROL_PLANE_SERVICE_NAME};

#[derive(Parser, Debug)]
#[command(name = "usdb-control-plane")]
#[command(author = "buckyos")]
#[command(version = "0.1.0")]
#[command(about = "USDB control plane for local console and service overview")]
struct ControlPlaneCli {
    /// Override service root directory (default: ~/.usdb/usdb-control-plane)
    #[arg(long)]
    root_dir: Option<PathBuf>,

    /// Skip process lock acquisition (for isolated integration tests)
    #[arg(long, default_value_t = false)]
    skip_process_lock: bool,
}

#[tokio::main]
async fn main() {
    let cli = ControlPlaneCli::parse();

    let _lock_guard = if cli.skip_process_lock {
        None
    } else {
        Some(usdb_util::init_process_lock(
            USDB_CONTROL_PLANE_SERVICE_NAME,
        ))
    };

    let root_dir = cli
        .root_dir
        .unwrap_or_else(|| usdb_util::get_service_dir(USDB_CONTROL_PLANE_SERVICE_NAME));
    std::fs::create_dir_all(&root_dir).unwrap_or_else(|e| {
        eprintln!(
            "Failed to create control-plane root directory {}: {}",
            root_dir.display(),
            e
        );
        std::process::exit(1);
    });

    let log_config = LogConfig::new(USDB_CONTROL_PLANE_SERVICE_NAME)
        .with_service_root_dir(root_dir.clone())
        .enable_console(false);
    usdb_util::init_log(log_config);

    let config = match ControlPlaneConfig::load(&root_dir) {
        Ok(config) => config,
        Err(e) => {
            error!("Failed to load control-plane config: {}", e);
            eprintln!("Failed to load control-plane config: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = server::run_server(config).await {
        error!("USDB control plane exited with error: {}", e);
        eprintln!("USDB control plane exited with error: {}", e);
        std::process::exit(1);
    }
}
