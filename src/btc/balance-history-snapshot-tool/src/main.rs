use balance_history::{
    BalanceHistoryConfig, BalanceHistoryDB, DEFAULT_CACHE_BUDGET_PERCENT, IndexConfig,
    LegacySnapshotIntegrityCheck, LegacyStateCompareOptions, LegacyStateCompareProgressRef,
    derive_cache_limits,
};
use balance_history_snapshot_tool::{
    ExactHeightSnapshotBuilder, SnapshotCreateOptions, SnapshotResumeVerifyOptions,
};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
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

    /// Resume verification and publication of an already-generated temporary artifact.
    ResumeVerify {
        #[arg(long)]
        height: u32,

        /// Optional canonical BTC block hash that must match the sealed job.
        #[arg(long)]
        expected_block_hash: Option<String>,
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

    /// Compare a legacy v2 SQLite snapshot with a rebuilt RocksDB at the same exact height.
    CompareLegacy {
        /// Service root containing the rebuilt balance-history RocksDB and config.toml.
        #[arg(long)]
        balance_history_root: PathBuf,

        /// Immutable legacy SQLite snapshot file.
        #[arg(long)]
        snapshot_db: PathBuf,

        /// Exact height at which both stores must be frozen.
        #[arg(long)]
        height: u32,

        /// Also compare the auxiliary script registry. This is the longest phase on mainnet.
        #[arg(long, default_value_t = false)]
        include_script_registry: bool,

        /// Number of independent key shards compared concurrently.
        #[arg(long, default_value_t = 4)]
        parallelism: usize,

        /// Maximum expected and unexpected examples retained per table.
        #[arg(long, default_value_t = 32)]
        max_examples: usize,

        /// SQLite integrity check to run before semantic comparison.
        #[arg(long, value_enum, default_value_t = IntegrityCheckArg::Quick)]
        integrity_check: IntegrityCheckArg,

        /// Optional JSON report output path.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum IntegrityCheckArg {
    Off,
    Quick,
    Full,
}

impl From<IntegrityCheckArg> for LegacySnapshotIntegrityCheck {
    fn from(value: IntegrityCheckArg) -> Self {
        match value {
            IntegrityCheckArg::Off => Self::Off,
            IntegrityCheckArg::Quick => Self::Quick,
            IntegrityCheckArg::Full => Self::Full,
        }
    }
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
        Command::ResumeVerify {
            height,
            expected_block_hash,
        } => builder
            .resume_verify(SnapshotResumeVerifyOptions {
                target_height: height,
                expected_block_hash,
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
        Command::CompareLegacy {
            balance_history_root,
            snapshot_db,
            height,
            include_script_registry,
            parallelism,
            max_examples,
            integrity_check,
            output,
        } => compare_legacy(CompareLegacyArgs {
            balance_history_root,
            snapshot_db,
            height,
            include_script_registry,
            parallelism,
            max_examples,
            integrity_check: integrity_check.into(),
            output,
            json: cli.json,
        }),
    };

    if let Err(error) = result {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}

struct CompareLegacyArgs {
    balance_history_root: PathBuf,
    snapshot_db: PathBuf,
    height: u32,
    include_script_registry: bool,
    parallelism: usize,
    max_examples: usize,
    integrity_check: LegacySnapshotIntegrityCheck,
    output: Option<PathBuf>,
    json: bool,
}

fn compare_legacy(args: CompareLegacyArgs) -> Result<(), String> {
    let config = Arc::new(BalanceHistoryConfig::load(&args.balance_history_root)?);
    eprintln!(
        "[compare-legacy] opening RocksDB read-only at {} (stop balance-history before the full comparison)",
        config.db_dir().display()
    );
    let db = BalanceHistoryDB::open_read_only(config)?;
    eprintln!("[compare-legacy] RocksDB opened; validating frozen height and legacy snapshot");
    let progress: LegacyStateCompareProgressRef = Arc::new(|event| {
        eprintln!(
            "[compare-legacy] table={} shards={}/{} legacy_rows={} current_rows={}",
            event.table,
            event.completed_shards,
            event.total_shards,
            event.legacy_rows,
            event.current_rows
        );
    });
    let report = db.compare_legacy_snapshot(
        &LegacyStateCompareOptions {
            snapshot_db: args.snapshot_db,
            target_height: args.height,
            include_script_registry: args.include_script_registry,
            parallelism: args.parallelism,
            max_examples: args.max_examples,
            integrity_check: args.integrity_check,
        },
        Some(progress),
    )?;
    if let Some(output) = args.output {
        if let Some(parent) = output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create comparison report directory {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }
        std::fs::write(
            &output,
            serde_json::to_vec_pretty(&report)
                .map_err(|e| format!("Failed to serialize comparison report: {}", e))?,
        )
        .map_err(|e| {
            format!(
                "Failed to write comparison report {}: {}",
                output.display(),
                e
            )
        })?;
    }
    print_value(&report, args.json)?;
    if !report.ok {
        return Err(format!(
            "Legacy state comparison found {} unexpected difference rows",
            report.unexpected_difference_rows
        ));
    }
    Ok(())
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
