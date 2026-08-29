mod checkpoint;
mod electrs_audit;
mod model;
mod snapshot;

use checkpoint::{
    atomic_write_json, build_run_id, load_or_create_checkpoint, now_unix, save_checkpoint,
    sort_results,
};
use clap::Parser;
use electrs_audit::{ElectrsAuditSettings, audit_samples, preflight, validate_server_limit};
use model::{AUDIT_REPORT_VERSION, AuditReport, AuditSummary, RunIdentity};
use snapshot::{Blacklist, SnapshotStore};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const TOOL_NAME: &str = "balance-history-electrs-audit";

#[derive(Debug, Parser)]
#[command(name = TOOL_NAME)]
#[command(about = "Deterministically audit a balance-history SQLite snapshot against electrs")]
struct Cli {
    /// Immutable balance-history SQLite snapshot.
    #[arg(long)]
    snapshot_db: PathBuf,

    /// Snapshot manifest sidecar. Defaults to <snapshot>.manifest.json.
    #[arg(long)]
    manifest: Option<PathBuf>,

    /// Electrs TCP/SSL endpoint.
    #[arg(long, default_value = "tcp://127.0.0.1:50001")]
    electrs_url: String,

    /// Runtime electrs config used to verify index_lookup_limit is active.
    #[arg(long)]
    electrs_config: Option<PathBuf>,

    /// Confirm electrs was restarted after enabling the limit in --electrs-config.
    #[arg(long, default_value_t = false)]
    confirm_electrs_restarted_with_config: bool,

    /// Explicitly permit a remote/uninspectable electrs endpoint without server-limit proof.
    #[arg(long, default_value_t = false)]
    allow_unverified_electrs_limit: bool,

    /// Deterministic sampling seed.
    #[arg(long, default_value = "usdb-balance-history-mainnet-audit-v1")]
    seed: String,

    /// Total number of deterministic samples.
    #[arg(long, default_value_t = 32)]
    sample_count: usize,

    /// Percentage sampled from script_registry entries absent from terminal balances.
    #[arg(long, default_value_t = 25)]
    zero_sample_percent: u8,

    /// Optional script-hash/address blacklist, one item per line with # comments.
    #[arg(long)]
    blacklist: Option<PathBuf>,

    /// Client-side history bound. The verified server limit must be no larger.
    #[arg(long, default_value_t = 20_000)]
    max_history_entries: usize,

    /// Number of independent electrs worker connections.
    #[arg(long, default_value_t = 4)]
    concurrency: usize,

    /// Maximum weighted shared transaction cache size in MiB.
    #[arg(long, default_value_t = 256)]
    transaction_cache_mib: u64,

    /// Per-electrs-request timeout in seconds (1-255).
    #[arg(long, default_value_t = 30)]
    timeout_secs: u8,

    /// Recompute the complete snapshot file SHA256 before sampling.
    #[arg(long, default_value_t = false)]
    verify_file_hash: bool,

    /// Permit too-popular/BIP30-ambiguous samples without failing the command.
    #[arg(long, default_value_t = false)]
    allow_skipped: bool,

    /// Exact final JSON report path. Overrides automatic naming.
    #[arg(long, conflicts_with = "output_dir")]
    output: Option<PathBuf>,

    /// Directory for the automatically named report. Defaults to the current directory.
    #[arg(long, conflicts_with = "output")]
    output_dir: Option<PathBuf>,

    /// Restart checkpoint path. Defaults to a sidecar next to the resolved report path.
    #[arg(long)]
    checkpoint: Option<PathBuf>,

    /// Print the full final report to stdout.
    #[arg(long, default_value_t = false)]
    json: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{TOOL_NAME}: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    if cli.sample_count > 10_000 {
        return Err("sample_count must not exceed 10000".to_string());
    }
    let snapshot = SnapshotStore::open(
        &cli.snapshot_db,
        cli.manifest.as_deref(),
        cli.verify_file_hash,
    )?;
    let blacklist = Blacklist::load(cli.blacklist.as_deref(), snapshot.network())?;
    let plan = snapshot.sample(
        &cli.seed,
        cli.sample_count,
        cli.zero_sample_percent,
        &blacklist,
    )?;
    let server_limit = validate_server_limit(
        cli.electrs_config.as_deref(),
        cli.max_history_entries,
        cli.confirm_electrs_restarted_with_config,
        cli.allow_unverified_electrs_limit,
    )?;
    let settings = ElectrsAuditSettings {
        url: cli.electrs_url.clone(),
        target_height: snapshot.summary.height,
        target_block_hash: snapshot.summary.block_hash.clone(),
        timeout_secs: cli.timeout_secs,
        max_history_entries: cli.max_history_entries,
        concurrency: cli.concurrency,
        transaction_cache_bytes: cli
            .transaction_cache_mib
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "transaction_cache_mib overflows u64 bytes".to_string())?,
    };
    let electrs = preflight(&settings, server_limit)?;
    let run_identity = RunIdentity {
        snapshot_file: snapshot.summary.file.clone(),
        declared_snapshot_sha256: snapshot.summary.declared_file_sha256.clone(),
        snapshot_height: snapshot.summary.height,
        snapshot_block_hash: snapshot.summary.block_hash.clone(),
        electrs_url: cli.electrs_url.clone(),
        seed: cli.seed,
        sample_count: cli.sample_count,
        zero_sample_percent: cli.zero_sample_percent,
        max_history_entries: cli.max_history_entries,
        blacklist_id: blacklist.id,
    };
    let run_id = build_run_id(&run_identity)?;
    let output_path = resolve_output_path(
        cli.output.as_deref(),
        cli.output_dir.as_deref(),
        &run_identity,
        &run_id,
    );
    let checkpoint_path = cli
        .checkpoint
        .unwrap_or_else(|| checkpoint_path_for_output(&output_path));
    let mut checkpoint = load_or_create_checkpoint(&checkpoint_path, &run_id)?;

    let planned_ids = plan
        .samples
        .iter()
        .map(|sample| sample.sample_id.clone())
        .collect::<HashSet<_>>();
    let mut completed_ids = HashSet::new();
    for result in &checkpoint.results {
        if !planned_ids.contains(&result.sample_id) {
            return Err(format!(
                "Checkpoint contains sample not present in deterministic plan: {}",
                result.sample_id
            ));
        }
        if !completed_ids.insert(result.sample_id.clone()) {
            return Err(format!(
                "Checkpoint contains duplicate sample result: {}",
                result.sample_id
            ));
        }
    }
    let pending = plan
        .samples
        .into_iter()
        .filter(|sample| !completed_ids.contains(&sample.sample_id))
        .collect::<Vec<_>>();
    eprintln!(
        "[{TOOL_NAME}] snapshot_height={} samples={} resumed={} pending={} electrs_tip={} server_limit={:?}",
        snapshot.summary.height,
        cli.sample_count,
        checkpoint.results.len(),
        pending.len(),
        electrs.tip_height,
        electrs.configured_index_lookup_limit
    );

    let execution = audit_samples(pending, &settings, |result| {
        eprintln!(
            "[{TOOL_NAME}] sample={} script_hash={} status={:?} expected={} computed={:?} elapsed_ms={}",
            result.sample_id,
            result.script_hash,
            result.status,
            result.expected_balance,
            result.computed_balance,
            result.elapsed_ms
        );
        checkpoint.results.push(result);
        sort_results(&mut checkpoint.results);
        save_checkpoint(&checkpoint_path, &checkpoint)
    })?;
    sort_results(&mut checkpoint.results);

    let mut summary =
        AuditSummary::from_results(cli.sample_count, &checkpoint.results, cli.allow_skipped);
    summary.blacklisted_candidates_replaced = plan.stats.blacklisted_candidates_replaced;
    summary.duplicate_candidates_replaced = plan.stats.duplicate_candidates_replaced;
    summary.history_requests_this_process = execution.history_requests;
    summary.transaction_requests_this_process = execution.transaction_requests;
    summary.transaction_cache_hits_this_process = execution.transaction_cache_hits;
    summary.transaction_cache_entries_at_completion = execution.transaction_cache_entries;
    summary.transaction_cache_weighted_bytes_at_completion =
        execution.transaction_cache_weighted_bytes;
    let report = AuditReport {
        report_version: AUDIT_REPORT_VERSION.to_string(),
        run_id,
        started_at_unix: checkpoint.started_at_unix,
        completed_at_unix: now_unix(),
        run_identity,
        snapshot: snapshot.summary,
        electrs,
        summary: summary.clone(),
        results: checkpoint.results,
    };
    atomic_write_json(&output_path, &report)?;
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("Failed to serialize final report: {error}"))?
        );
    } else {
        println!(
            "report={} matched={} mismatched={} skipped={} errors={} complete={} ok={}",
            output_path.display(),
            summary.matched,
            summary.mismatched,
            summary.skipped_too_popular + summary.skipped_bip30_ambiguous,
            summary.errors,
            summary.complete,
            summary.ok
        );
    }
    if !summary.ok {
        return Err(format!(
            "Audit did not pass; inspect report {}",
            output_path.display()
        ));
    }
    Ok(())
}

fn resolve_output_path(
    explicit_output: Option<&Path>,
    output_dir: Option<&Path>,
    identity: &RunIdentity,
    run_id: &str,
) -> PathBuf {
    if let Some(output) = explicit_output {
        return output.to_path_buf();
    }
    let output_dir = output_dir.unwrap_or_else(|| Path::new("."));
    let seed = seed_file_component(&identity.seed);
    let short_run_id = run_id.chars().take(12).collect::<String>();
    output_dir.join(format!(
        "{TOOL_NAME}-h{}-n{}-{seed}-{short_run_id}.json",
        identity.snapshot_height, identity.sample_count
    ))
}

fn seed_file_component(seed: &str) -> String {
    let mut component = String::new();
    let mut separator_pending = false;
    for character in seed.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            if separator_pending && !component.is_empty() {
                component.push('-');
            }
            separator_pending = false;
            if component.len() < 48 {
                component.push(character);
            }
        } else {
            separator_pending = true;
        }
        if component.len() >= 48 {
            break;
        }
    }
    while component.ends_with(['-', '_']) {
        component.pop();
    }
    if component.is_empty() {
        "seed".to_string()
    } else {
        component
    }
}

fn checkpoint_path_for_output(output: &Path) -> PathBuf {
    let file_name = output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("audit-report.json");
    output.with_file_name(format!("{file_name}.checkpoint.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(seed: &str, sample_count: usize) -> RunIdentity {
        RunIdentity {
            snapshot_file: "/snapshot.db".to_string(),
            declared_snapshot_sha256: "11".repeat(32),
            snapshot_height: 963_800,
            snapshot_block_hash: "22".repeat(32),
            electrs_url: "tcp://127.0.0.1:50001".to_string(),
            seed: seed.to_string(),
            sample_count,
            zero_sample_percent: 25,
            max_history_entries: 20_000,
            blacklist_id: "33".repeat(32),
        }
    }

    #[test]
    fn automatic_output_path_is_bound_to_count_seed_and_run_id() {
        let path = resolve_output_path(
            None,
            Some(Path::new("/audit")),
            &identity("usdb mainnet/audit v1", 1_000),
            "abcdef1234567890",
        );
        assert_eq!(
            path,
            Path::new(
                "/audit/balance-history-electrs-audit-h963800-n1000-usdb-mainnet-audit-v1-abcdef123456.json"
            )
        );
    }

    #[test]
    fn explicit_output_remains_an_exact_override() {
        let output = Path::new("/audit/fixed.json");
        assert_eq!(
            resolve_output_path(
                Some(output),
                None,
                &identity("seed", 32),
                "abcdef1234567890",
            ),
            output
        );
    }

    #[test]
    fn unusable_seed_still_has_a_safe_file_component() {
        assert_eq!(seed_file_component("///\u{6d4b}\u{8bd5}///"), "seed");
    }
}
