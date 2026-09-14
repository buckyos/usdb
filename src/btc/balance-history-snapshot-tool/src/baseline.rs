//! Unified baseline producer commands sharing the existing tool, locks and signing keys.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use balance_history::baseline_snapshot::{
    BaselineIdentity, BaselineJobInput, BaselineJobSource, baseline_job_status,
    create_or_resume_baseline, verify_baseline_job,
};
use balance_history::{
    BalanceHistoryConfig, BalanceHistoryIndexer, IndexOutput, SyncStatusManager,
};
use bitcoincore_rpc::bitcoin::{BlockHash, Network, consensus};
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub(crate) enum BaselineCommand {
    /// Create or resume a unified snapshot from a source DB, signed split pair, or builder config.
    Create(CreateArgs),
    /// Resume a frozen job; data batches are reused and incomplete verification scans restart.
    Resume(ResumeArgs),
    /// Recheck a completed signed artifact against its frozen job identity.
    Verify(VerifyArgs),
    /// Print durable job progress without opening producer sources.
    Status(Target),
}

#[derive(Args, Debug)]
pub(crate) struct Target {
    #[arg(long)]
    height: u32,
    /// Canonical hash fixed when creating the job.
    #[arg(long, alias = "block-hash")]
    expected_block_hash: BlockHash,
}

#[derive(Args, Debug)]
pub(crate) struct CreateArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long, default_value = "bitcoin")]
    network: Network,
    /// Stopped exact-height BH root; mutually exclusive with split input and managed sync.
    #[arg(long, conflicts_with_all = ["core_manifest", "config"])]
    source_root: Option<PathBuf>,
    /// Signed original core manifest.
    #[arg(long, requires = "registry_manifest", conflicts_with = "config")]
    core_manifest: Option<PathBuf>,
    /// Signed original registry manifest paired with the core.
    #[arg(long, requires = "core_manifest")]
    registry_manifest: Option<PathBuf>,
    /// BH configuration for a dedicated managed workspace, synced exactly to business genesis.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Raw binary G block; managed sync fetches it through the configured Bitcoin RPC.
    #[arg(long, conflicts_with = "config")]
    genesis_block: Option<PathBuf>,
    #[arg(long)]
    signing_key: PathBuf,
    #[arg(long)]
    trusted_keys: PathBuf,
    #[arg(long, default_value_t = 20_000)]
    batch_size: usize,
}

#[derive(Args, Debug)]
pub(crate) struct ResumeArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long)]
    signing_key: PathBuf,
    #[arg(long)]
    trusted_keys: PathBuf,
    #[arg(long, default_value_t = 20_000)]
    batch_size: usize,
}

#[derive(Args, Debug)]
pub(crate) struct VerifyArgs {
    #[command(flatten)]
    target: Target,
    #[arg(long)]
    trusted_keys: PathBuf,
}

fn job_dir(root: &Path, target: &Target) -> PathBuf {
    root.join("jobs").join(format!(
        "{:012}-{}",
        target.height, target.expected_block_hash
    ))
}

fn observer(stage: &str) -> Result<(), String> {
    let label = format!("baseline_{stage}");
    balance_history_snapshot_tool::baseline_test_checkpoint(&label)
}

fn selected_status(
    root: &Path,
    target: &Target,
) -> Result<balance_history::baseline_snapshot::BaselineJobReport, String> {
    let report = baseline_job_status(&job_dir(root, target))?;
    if report.identity.height != target.height
        || report.identity.block_hash != target.expected_block_hash
    {
        return Err("Baseline job differs from selected genesis".into());
    }
    Ok(report)
}

pub(crate) fn run(root: &Path, command: BaselineCommand) -> Result<serde_json::Value, String> {
    let _guard = if matches!(
        &command,
        BaselineCommand::Create(_) | BaselineCommand::Resume(_)
    ) {
        Some(balance_history_snapshot_tool::lock_baseline_workspace(
            root,
        )?)
    } else {
        None
    };
    match command {
        BaselineCommand::Status(target) => {
            Ok(serde_json::to_value(selected_status(root, &target)?).map_err(|e| e.to_string())?)
        }
        BaselineCommand::Verify(args) => {
            let report = baseline_job_status(&job_dir(root, &args.target))?;
            let manifest = verify_baseline_job(&job_dir(root, &args.target), &args.trusted_keys)?;
            if manifest.state.identity != report.identity
                || Some(&manifest.logical_sha256) != report.logical_sha256.as_ref()
                || manifest.state.identity.height != args.target.height
                || manifest.state.identity.block_hash != args.target.expected_block_hash
            {
                return Err("Baseline artifact differs from the selected job".into());
            }
            Ok(serde_json::to_value(manifest).map_err(|e| e.to_string())?)
        }
        BaselineCommand::Resume(args) => {
            selected_status(root, &args.target)?;
            let root = job_dir(root, &args.target);
            Ok(serde_json::to_value(create_or_resume_baseline(
                &root,
                None,
                &args.signing_key,
                &args.trusted_keys,
                args.batch_size,
                &observer,
            )?)
            .map_err(|e| e.to_string())?)
        }
        BaselineCommand::Create(args) => {
            let identity = BaselineIdentity::new(
                args.network,
                args.target.height,
                args.target.expected_block_hash,
            )?;
            if !(1..=20_000).contains(&args.batch_size) {
                return Err("Baseline batch size must be in 1..=20000".into());
            }
            let key = balance_history::SnapshotSigningKeyFile::load(&args.signing_key)?;
            if balance_history::SnapshotTrustedKeySet::load(&args.trusted_keys)?
                .find_verifying_key(&key.key_id)?
                != Some(key.to_signing_key()?.verifying_key())
            {
                return Err("Baseline signer differs from selected trusted-key catalog".into());
            }
            let (source, block) = if let Some(path) = args.config {
                managed_source(root, &args.target, args.network, &path)?
            } else {
                let source = if let Some(root) = args.source_root {
                    BaselineJobSource::Rocksdb { root }
                } else if let (Some(core_manifest), Some(registry_manifest)) =
                    (args.core_manifest, args.registry_manifest)
                {
                    BaselineJobSource::LegacySplit {
                        core_manifest,
                        registry_manifest,
                    }
                } else {
                    return Err("Baseline create requires --source-root, --core-manifest with --registry-manifest, or --config".into());
                };
                (
                    source,
                    args.genesis_block
                        .ok_or("Offline baseline sources require --genesis-block")?,
                )
            };
            let input = BaselineJobInput {
                identity,
                source,
                genesis_block_file: block,
            };
            Ok(serde_json::to_value(create_or_resume_baseline(
                &job_dir(root, &args.target),
                Some(input),
                &args.signing_key,
                &args.trusted_keys,
                args.batch_size,
                &observer,
            )?)
            .map_err(|e| e.to_string())?)
        }
    }
}

// Managed sync is a producer workspace, not a running node or an installation.
// Existing native bootstrap and full replay retain their own durable sync recovery.
fn managed_source(
    root: &Path,
    target: &Target,
    network: Network,
    source: &Path,
) -> Result<(BaselineJobSource, PathBuf), String> {
    let workspace = root.join("workspaces").join(format!(
        "{:012}-{}",
        target.height, target.expected_block_hash
    ));
    std::fs::create_dir_all(&workspace).map_err(|e| e.to_string())?;
    let _lock = balance_history_snapshot_tool::lock_baseline_workspace(&workspace)?;
    let bytes = std::fs::read_to_string(source)
        .map_err(|e| format!("Failed to read builder configuration: {e}"))?;
    let mut config: BalanceHistoryConfig =
        toml::from_str(&bytes).map_err(|e| format!("Invalid builder configuration: {e}"))?;
    if config.btc.network() != network {
        return Err("Builder network differs from requested baseline".into());
    }
    config.root_dir = workspace.clone();
    config.sync.max_sync_block_height = target.height;
    if let Some(native) = &config.bootstrap
        && (native.identity.origin_height != target.height
            || native.identity.origin_block_hash != target.expected_block_hash)
    {
        return Err("Native builder origin differs from requested baseline".into());
    }
    config.validate()?;
    let frozen = workspace.join("producer-config.toml");
    let encoded = toml::to_string(&config).map_err(|e| e.to_string())?;
    if frozen.exists() {
        if std::fs::read_to_string(&frozen).map_err(|e| e.to_string())? != encoded {
            return Err(
                "Managed baseline configuration changed; keep the original source configuration"
                    .into(),
            );
        }
    } else {
        freeze_file(&frozen, encoded.as_bytes())?;
    }
    // A job already past sync must not reopen RocksDB: even housekeeping can change
    // the source file fingerprint used to protect a resumable export.
    let job = job_dir(root, target);
    let block_file = workspace.join("genesis.block");
    if !job.join("job.json").exists() {
        let config = Arc::new(config);
        let client = balance_history::btc::create_canonical_btc_client(
            balance_history::btc::create_btc_rpc_client(&config)?,
            &config,
        )?;
        balance_history::bootstrap::prepare_native_bootstrap(
            config.clone(),
            client.clone(),
            &|| false,
        )?;
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        let indexer = BalanceHistoryIndexer::new(config.clone(), output)?;
        if indexer.db().get_btc_block_height()? > target.height {
            return Err("Managed workspace is above baseline genesis".into());
        }
        indexer.sync_to_height(target.height, Duration::from_secs(5))?;
        let block = client.get_block_by_height(target.height)?;
        if block.block_hash() != target.expected_block_hash {
            return Err("Bitcoin RPC returned a different baseline block hash".into());
        }
        freeze_file(&block_file, &consensus::serialize(&block))?;
        drop(indexer);
    }
    Ok((BaselineJobSource::Rocksdb { root: workspace }, block_file))
}

fn freeze_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
    std::fs::File::open(path.parent().ok_or("Producer file has no parent")?)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| e.to_string())
}
