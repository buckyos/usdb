use balance_history_snapshot_tool::{ExactHeightSnapshotBuilder, SnapshotCreateOptions};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use usdb_util::LogConfig;

const TOOL_NAME: &str = "balance-history-snapshot-tool";

#[derive(Parser, Debug)]
#[command(name = TOOL_NAME)]
#[command(about = "Build restartable full-UTXO balance-history snapshots at exact BTC heights")]
struct Cli {
    /// Builder root containing the shared workspace, jobs, and immutable snapshots.
    #[arg(long)]
    root_dir: Option<PathBuf>,

    /// Print command results as JSON.
    #[arg(long, default_value_t = false)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Synchronize the shared workspace to one exact height and publish a full checkpoint.
    Create {
        #[arg(long)]
        height: u32,

        /// Optional canonical BTC block hash that the target height must resolve to.
        #[arg(long)]
        expected_block_hash: Option<String>,

        /// Balance-history config copied into a new workspace on first use.
        #[arg(long)]
        config: Option<PathBuf>,

        /// Seconds between retries while the target waits to enter the stable range.
        #[arg(long, default_value_t = 5)]
        poll_interval_secs: u64,
    },

    /// Show builder state and an optional per-height job.
    Status {
        #[arg(long)]
        height: Option<u32>,
    },

    /// List all persisted jobs in ascending height order.
    List,

    /// Reopen and verify one completed immutable artifact.
    Verify {
        #[arg(long)]
        height: u32,

        /// Required when more than one same-height branch artifact exists.
        #[arg(long)]
        block_hash: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let root_dir = cli
        .root_dir
        .clone()
        .unwrap_or_else(|| usdb_util::get_service_dir("balance-history-snapshot-builder"));
    let log_config = LogConfig::new(TOOL_NAME)
        .with_service_root_dir(root_dir.clone())
        .enable_console(!cli.json);
    usdb_util::init_log(log_config);

    let builder = ExactHeightSnapshotBuilder::new(root_dir);
    let result = match cli.command {
        Command::Create {
            height,
            expected_block_hash,
            config,
            poll_interval_secs,
        } => builder
            .create(SnapshotCreateOptions {
                target_height: height,
                expected_block_hash,
                poll_interval: Duration::from_secs(poll_interval_secs.max(1)),
                config_file: config,
            })
            .and_then(|report| print_value(&report, cli.json)),
        Command::Status { height } => builder
            .status(height)
            .and_then(|status| print_value(&status, cli.json)),
        Command::List => builder
            .list_jobs()
            .and_then(|jobs| print_value(&jobs, cli.json)),
        Command::Verify { height, block_hash } => builder
            .verify(height, block_hash.as_deref())
            .and_then(|marker| print_value(&marker, cli.json)),
    };

    if let Err(error) = result {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}

fn print_value<T: Serialize + std::fmt::Debug>(value: &T, json: bool) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value)
                .map_err(|e| format!("Failed to serialize command result: {}", e))?
        );
    } else {
        println!("{:#?}", value);
    }
    Ok(())
}
