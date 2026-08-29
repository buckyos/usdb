use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use usdb_indexer_checkpoint_tool::{
    ExportCheckpointOptions, InstallPairOptions, export_checkpoint, install_pair,
    load_and_verify_checkpoint_pair, verify_recovery,
};

const TOOL_NAME: &str = "usdb-indexer-checkpoint-tool";

#[derive(Debug, Parser)]
#[command(name = TOOL_NAME)]
#[command(about = "Export, sign, install, and verify paired USDB BTC checkpoints")]
struct Cli {
    /// Print command results as JSON.
    #[arg(long, default_value_t = false)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[command(rename_all = "kebab-case")]
enum Command {
    /// Seal state through RPC, gracefully stop indexer, and export an immutable checkpoint.
    Export {
        #[arg(long)]
        indexer_root: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:28020")]
        indexer_rpc_url: String,
        #[arg(long)]
        height: u32,
        #[arg(long)]
        network_bundle_id: String,
        #[arg(long)]
        chain_id: u64,
        #[arg(long)]
        index_origin_height: u32,
        #[arg(long)]
        balance_history_manifest: PathBuf,
        #[arg(long)]
        trusted_keys: PathBuf,
        #[arg(long)]
        signing_key: PathBuf,
        #[arg(long)]
        output_root: PathBuf,
        #[arg(long, default_value_t = 120)]
        stop_timeout_secs: u64,
    },

    /// Verify checkpoint signature, file inventory, upstream signature, and pair binding.
    Verify {
        #[arg(long)]
        checkpoint_manifest: PathBuf,
        #[arg(long)]
        balance_history_manifest: PathBuf,
        #[arg(long)]
        trusted_keys: PathBuf,
    },

    /// Install both artifacts into staging, atomically publish each, and resume after failures.
    InstallPair {
        #[arg(long)]
        checkpoint_manifest: PathBuf,
        #[arg(long)]
        balance_history_manifest: PathBuf,
        #[arg(long)]
        trusted_keys: PathBuf,
        #[arg(long)]
        indexer_root: PathBuf,
        #[arg(long)]
        balance_history_root: PathBuf,
        #[arg(long)]
        network_bundle_id: String,
        #[arg(long)]
        chain_id: u64,
        #[arg(long)]
        index_origin_height: u32,
        #[arg(long, default_value_t = 10)]
        lock_timeout_secs: u64,
    },

    /// Recompute both state refs from restarted services and write the final recovery marker.
    VerifyRecovery {
        #[arg(long)]
        checkpoint_manifest: PathBuf,
        #[arg(long)]
        trusted_keys: PathBuf,
        #[arg(long)]
        indexer_root: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:28020")]
        indexer_rpc_url: String,
        #[arg(long, default_value = "http://127.0.0.1:28010")]
        balance_history_rpc_url: String,
        #[arg(long, default_value_t = 300)]
        readiness_timeout_secs: u64,
    },
}

#[derive(Serialize)]
struct VerifyReport {
    operation_id: String,
    checkpoint_height: u32,
    network_bundle_id: String,
    chain_id: u64,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let log_root = usdb_util::get_service_dir(TOOL_NAME);
    let log_config = usdb_util::current_process_log_config!(TOOL_NAME)
        .with_service_root_dir(log_root)
        .enable_console(false);
    let log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize checkpoint tool logging: {error}");
        std::process::exit(1);
    });

    let result = match cli.command {
        Command::Export {
            indexer_root,
            indexer_rpc_url,
            height,
            network_bundle_id,
            chain_id,
            index_origin_height,
            balance_history_manifest,
            trusted_keys,
            signing_key,
            output_root,
            stop_timeout_secs,
        } => export_checkpoint(ExportCheckpointOptions {
            indexer_root,
            indexer_rpc_url,
            checkpoint_height: height,
            network_bundle_id,
            chain_id,
            index_origin_height,
            balance_history_manifest,
            trusted_keys,
            signing_key,
            output_root,
            stop_timeout: Duration::from_secs(stop_timeout_secs.max(1)),
        })
        .await
        .and_then(|report| print_value(&report, cli.json)),
        Command::Verify {
            checkpoint_manifest,
            balance_history_manifest,
            trusted_keys,
        } => load_and_verify_checkpoint_pair(
            &checkpoint_manifest,
            &balance_history_manifest,
            &trusted_keys,
        )
        .and_then(|manifest| {
            print_value(
                &VerifyReport {
                    operation_id: manifest.operation_id,
                    checkpoint_height: manifest.checkpoint_height,
                    network_bundle_id: manifest.network_bundle_id,
                    chain_id: manifest.chain_id,
                },
                cli.json,
            )
        }),
        Command::InstallPair {
            checkpoint_manifest,
            balance_history_manifest,
            trusted_keys,
            indexer_root,
            balance_history_root,
            network_bundle_id,
            chain_id,
            index_origin_height,
            lock_timeout_secs,
        } => install_pair(InstallPairOptions {
            checkpoint_manifest,
            balance_history_manifest,
            trusted_keys,
            indexer_root,
            balance_history_root,
            network_bundle_id,
            chain_id,
            index_origin_height,
            lock_timeout: Duration::from_secs(lock_timeout_secs.max(1)),
        })
        .await
        .and_then(|report| print_value(&report, cli.json)),
        Command::VerifyRecovery {
            checkpoint_manifest,
            trusted_keys,
            indexer_root,
            indexer_rpc_url,
            balance_history_rpc_url,
            readiness_timeout_secs,
        } => verify_recovery(
            &checkpoint_manifest,
            &trusted_keys,
            &indexer_root,
            &indexer_rpc_url,
            &balance_history_rpc_url,
            Duration::from_secs(readiness_timeout_secs.max(1)),
        )
        .await
        .and_then(|marker| print_value(&marker, cli.json)),
    };

    let code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("Checkpoint tool command failed: {error}");
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    };
    log_handle.shutdown();
    code
}

fn print_value<T: Serialize>(value: &T, json: bool) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value)
                .map_err(|error| format!("Failed to serialize command result: {error}"))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string(value)
                .map_err(|error| format!("Failed to serialize command result: {error}"))?
        );
    }
    Ok(())
}
