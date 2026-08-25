use balance_history::{DEFAULT_CACHE_BUDGET_PERCENT, IndexConfig, derive_cache_limits};
use balance_history_snapshot_tool::{ExactHeightSnapshotBuilder, SnapshotCreateOptions};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use usdb_util::{LogConfig, MemoryUsageSnapshot, get_memory_usage_snapshot};

const TOOL_NAME: &str = "balance-history-snapshot-tool";
const DEFAULT_SNAPSHOT_MAX_MEMORY_PERCENT: usize = 80;

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
    /// Calculate cgroup-aware cache limits without opening a snapshot workspace.
    MemoryPlan {
        /// Percentage of the effective memory ceiling assigned to both caches.
        #[arg(long, default_value_t = DEFAULT_CACHE_BUDGET_PERCENT)]
        cache_budget_percent: usize,

        /// Whole-host/cgroup usage percentage that triggers cache shrinking.
        #[arg(long, default_value_t = DEFAULT_SNAPSHOT_MAX_MEMORY_PERCENT)]
        max_memory_percent: usize,
    },

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
        Command::MemoryPlan {
            cache_budget_percent,
            max_memory_percent,
        } => build_memory_plan(
            get_memory_usage_snapshot(),
            cache_budget_percent,
            max_memory_percent,
        )
        .and_then(|plan| print_value(&plan, cli.json)),
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

#[derive(Debug, Serialize)]
struct SnapshotMemoryPlan {
    source: String,
    memory_limit_bytes: u64,
    current_used_bytes: u64,
    cache_budget_percent: usize,
    utxo_cache_bytes: u64,
    balance_cache_bytes: u64,
    total_cache_bytes: u64,
    max_memory_percent: usize,
    pressure_threshold_bytes: u64,
    bytes_between_cache_budget_and_pressure_threshold: u64,
    bytes_outside_cache_budget: u64,
}

fn build_memory_plan(
    memory: MemoryUsageSnapshot,
    cache_budget_percent: usize,
    max_memory_percent: usize,
) -> Result<SnapshotMemoryPlan, String> {
    let limits = derive_cache_limits(memory.limit_bytes, cache_budget_percent)?;
    let index_config = IndexConfig {
        utxo_max_cache_bytes: usize::try_from(limits.utxo_cache_bytes)
            .map_err(|_| "Derived UTXO cache limit exceeds usize".to_string())?,
        balance_max_cache_bytes: usize::try_from(limits.balance_cache_bytes)
            .map_err(|_| "Derived balance cache limit exceeds usize".to_string())?,
        max_memory_percent,
        ..IndexConfig::default()
    };
    index_config.validate()?;
    index_config.validate_memory_budget(memory.limit_bytes)?;

    let pressure_threshold_bytes =
        ((memory.limit_bytes as u128 * max_memory_percent as u128) / 100) as u64;
    Ok(SnapshotMemoryPlan {
        source: memory.source.to_string(),
        memory_limit_bytes: memory.limit_bytes,
        current_used_bytes: memory.used_bytes,
        cache_budget_percent,
        utxo_cache_bytes: limits.utxo_cache_bytes,
        balance_cache_bytes: limits.balance_cache_bytes,
        total_cache_bytes: limits.total_cache_bytes,
        max_memory_percent,
        pressure_threshold_bytes,
        bytes_between_cache_budget_and_pressure_threshold: pressure_threshold_bytes
            .saturating_sub(limits.total_cache_bytes),
        bytes_outside_cache_budget: memory.limit_bytes.saturating_sub(limits.total_cache_bytes),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use usdb_util::MemoryAccountingSource;

    fn memory(limit_gib: u64) -> MemoryUsageSnapshot {
        MemoryUsageSnapshot {
            source: MemoryAccountingSource::Physical,
            limit_bytes: limit_gib * 1024 * 1024 * 1024,
            used_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    #[test]
    fn snapshot_memory_plan_uses_two_thirds_with_one_to_three_split() {
        let plan = build_memory_plan(memory(64), 66, 80).unwrap();
        assert_eq!(plan.total_cache_bytes, plan.memory_limit_bytes * 66 / 100);
        assert_eq!(plan.utxo_cache_bytes, plan.total_cache_bytes / 4);
        assert_eq!(
            plan.balance_cache_bytes,
            plan.total_cache_bytes - plan.utxo_cache_bytes
        );
        assert!(plan.bytes_between_cache_budget_and_pressure_threshold > 0);
    }

    #[test]
    fn snapshot_memory_plan_rejects_cache_too_close_to_pressure_threshold() {
        let error = build_memory_plan(memory(64), 75, 80).unwrap_err();
        assert!(error.contains("memory-pressure threshold"));
    }
}
