//! Offline validation CLI; it never starts or reconfigures a Bitcoin or balance-history service.

use std::path::PathBuf;
use std::sync::Arc;

use balance_history::assumeutxo::*;
use bitcoincore_rpc::Auth;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Resumable AssumeUTXO import/replay validation prototype")]
struct Cli {
    /// Absolute dedicated output workspace (required except for scan).
    #[arg(long, global = true)]
    root_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Verify the entire source file and Core logical UTXO commitment without creating a database.
    Scan {
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Import or resume a staged baseline; publish only after verification succeeds.
    Import {
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        reference_core: PathBuf,
        #[arg(long)]
        reference_sha256: String,
        #[arg(long, default_value_t = 65536)]
        batch_size: usize,
    },
    /// Replay through a fixed height using an existing RPC node that retains the required blocks.
    Replay {
        #[arg(long)]
        height: u32,
        #[arg(long)]
        rpc_url: String,
        #[arg(long)]
        cookie_file: PathBuf,
        #[arg(long, default_value_t = 20)]
        batch_size: u32,
    },
    /// Compare all UTXOs, nonzero balances and baseline-to-target commits with the reference core.
    Compare,
    /// Audit commits, state refs, query floors and deterministic script samples using read-only databases.
    Audit {
        #[arg(long, default_value_t = 4)]
        samples_per_prefix: u32,
    },
    /// Read atomic progress reports, including while another command is running.
    Status,
}

fn identity(path: &PathBuf) -> Result<SnapshotIdentity, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("Read identity: {e}"))?)
        .map_err(|e| e.to_string())
}

fn run(cli: Cli) -> Result<(), String> {
    if let Command::Scan {
        snapshot,
        identity: file,
        output,
    } = cli.command
    {
        let result = scan_snapshot(&snapshot, &identity(&file)?, 65536, |_, _| Ok(()))?;
        write_report(&output, &result)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let root = cli
        .root_dir
        .ok_or("--root-dir is required for this command")?;
    if matches!(cli.command, Command::Status) {
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot_status(&root)?).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    let _lock =
        AssumeUtxoWorkspaceLock::acquire(&root, matches!(cli.command, Command::Import { .. }))?;
    match cli.command {
        Command::Import {
            snapshot,
            identity: file,
            reference_core,
            reference_sha256,
            batch_size,
        } => {
            let options = AssumeUtxoImportOptions {
                snapshot,
                identity: identity(&file)?,
                reference_core,
                reference_sha256,
            };
            import_snapshot(&root, &options, batch_size)?;
        }
        Command::Replay {
            height,
            rpc_url,
            cookie_file,
            batch_size,
        } => {
            let url = url::Url::parse(&rpc_url).map_err(|e| format!("Invalid RPC URL: {e}"))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err("Use --cookie-file instead of credentials in the RPC URL".to_string());
            }
            let auth = Auth::CookieFile(cookie_file.clone());
            auth.clone().get_user_pass().map_err(|e| {
                format!(
                    "Cannot read RPC cookie {}: {e}; check --cookie-file and whether the Bitcoin node is running",
                    cookie_file.display()
                )
            })?;
            let rpc = usdb_util::BTCRpcClient::new(rpc_url, auth)
                .map_err(|e| format!("Create replay RPC client: {e}"))?;
            replay_snapshot(&root, Arc::new(Box::new(rpc)), height, batch_size)?;
        }
        Command::Compare => {
            let result = compare_snapshot(&root)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            if !result.equal {
                return Err("Full-state comparison failed; see comparison-result.json".to_string());
            }
        }
        Command::Audit { samples_per_prefix } => {
            let result = audit_snapshot(&root, samples_per_prefix)?;
            println!(
                "P5 audit passed: height={}, commits={}, balance_samples={}, report={}",
                result["target_height"],
                result["commits_checked"],
                result["balance_samples"],
                root.join("p5-semantics-result.json").display()
            );
        }
        Command::Scan { .. } | Command::Status => unreachable!(),
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("AssumeUTXO validation failed: {error}");
        std::process::exit(1);
    }
}
