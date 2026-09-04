use crate::artifact::{
    copy_directory, file_name, inventory_files, load_and_verify_balance_history_manifest,
    load_and_verify_checkpoint, save_json_atomic, sha256_file, sync_dir, unix_timestamp,
    wait_for_process_lock,
};
use crate::data::{IndexerDiskLayout, validate_indexer_data};
use crate::rpc::wait_for_restarted_state_refs;
use crate::{
    IndexerCheckpointManifest, PAIRED_INSTALL_JOURNAL_VERSION, PairedCheckpointRecoveryMarker,
    PairedInstallJournal, PairedInstallStage, RECOVERY_MARKER_VERSION,
};
use balance_history::{
    BalanceHistoryConfig, BalanceHistoryDB, BalanceHistoryDBMode, IndexOutput, SnapshotData,
    SnapshotInstaller, SnapshotManifest, SnapshotVerificationState, SyncStatusManager,
    build_historical_state_ref_at_height,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const INSTALL_JOURNAL_FILE: &str = "paired-checkpoint-install.journal.json";
const INSTALL_COMPLETE_MARKER_FILE: &str = "paired-checkpoint-install.done.json";
const RECOVERY_MARKER_FILE: &str = "paired-checkpoint-recovery.done.json";
const FAULT_ENV: &str = "USDB_CHECKPOINT_FAIL_AFTER";

/// Inputs for a restartable offline installation of both checkpoint artifacts.
#[derive(Clone, Debug)]
pub struct InstallPairOptions {
    /// Signed indexer checkpoint manifest.
    pub checkpoint_manifest: PathBuf,
    /// Signed balance-history snapshot manifest bound by the checkpoint.
    pub balance_history_manifest: PathBuf,
    /// Trusted public-key catalog for both artifact manifests.
    pub trusted_keys: PathBuf,
    /// Empty or previously checkpoint-installed indexer service root.
    pub indexer_root: PathBuf,
    /// Empty or previously snapshot-installed balance-history service root.
    pub balance_history_root: PathBuf,
    /// Expected network bundle identity from deployment configuration.
    pub network_bundle_id: String,
    /// Expected USDB chain ID from deployment configuration.
    pub chain_id: u64,
    /// Expected per-network index origin from deployment configuration.
    pub index_origin_height: u32,
    /// Maximum time to wait for both service process locks.
    pub lock_timeout: Duration,
}

/// Completed or resumed paired-install result.
#[derive(Clone, Debug, Serialize)]
pub struct InstallPairReport {
    /// Immutable paired operation identity.
    pub operation_id: String,
    /// Exact installed checkpoint height.
    pub checkpoint_height: u32,
    /// Final durable install stage.
    pub stage: PairedInstallStage,
    /// Durable journal path.
    pub journal_path: PathBuf,
    /// Offline completion marker path.
    pub marker_path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
struct OfflineInstallMarker {
    version: u32,
    operation_id: String,
    checkpoint_height: u32,
    checkpoint_manifest_sha256: String,
    balance_history_manifest_sha256: String,
    completed_at: u64,
}

/// Installs indexer first and balance-history second while both process locks are held.
///
/// If the process exits between atomic publishes, rerunning the same command recognizes and
/// verifies the already-published side before continuing with the other side.
pub async fn install_pair(options: InstallPairOptions) -> Result<InstallPairReport, String> {
    install_pair_with_lock_names(options, "usdb-indexer", "balance-history").await
}

async fn install_pair_with_lock_names(
    options: InstallPairOptions,
    indexer_lock_name: &str,
    balance_history_lock_name: &str,
) -> Result<InstallPairReport, String> {
    let checkpoint =
        load_and_verify_checkpoint(&options.checkpoint_manifest, &options.trusted_keys, true)?;
    validate_deployment_binding(&options, &checkpoint)?;
    let balance_history = load_and_verify_balance_history_manifest(
        &options.balance_history_manifest,
        &options.trusted_keys,
        true,
    )?;
    validate_upstream_binding(
        &checkpoint,
        &balance_history,
        &options.balance_history_manifest,
    )?;

    std::fs::create_dir_all(&options.indexer_root).map_err(|error| {
        format!(
            "Failed to create indexer root {}: {error}",
            options.indexer_root.display()
        )
    })?;
    std::fs::create_dir_all(&options.balance_history_root).map_err(|error| {
        format!(
            "Failed to create balance-history root {}: {error}",
            options.balance_history_root.display()
        )
    })?;
    let _indexer_lock = wait_for_process_lock(indexer_lock_name, options.lock_timeout).await?;
    let _balance_history_lock =
        wait_for_process_lock(balance_history_lock_name, options.lock_timeout).await?;

    let checkpoint_manifest_sha256 = sha256_file(&options.checkpoint_manifest)?;
    let bootstrap_dir = options.indexer_root.join("bootstrap");
    std::fs::create_dir_all(&bootstrap_dir).map_err(|error| {
        format!(
            "Failed to create paired checkpoint bootstrap directory {}: {error}",
            bootstrap_dir.display()
        )
    })?;
    let journal_path = bootstrap_dir.join(INSTALL_JOURNAL_FILE);
    let mut journal = load_or_create_journal(
        &journal_path,
        &options,
        &checkpoint,
        &checkpoint_manifest_sha256,
    )?;

    if journal.stage == PairedInstallStage::Prepared {
        validate_indexer_target(&options.indexer_root, &checkpoint)?;
        validate_balance_history_target(&options.balance_history_root, &balance_history)?;
        journal.stage = PairedInstallStage::TargetsValidated;
        journal.updated_at = unix_timestamp();
        save_json_atomic(&journal_path, &journal)?;
    }

    if journal.stage == PairedInstallStage::TargetsValidated {
        publish_indexer_data(&options, &checkpoint)?;
        maybe_fail("indexer_published")?;
        journal.stage = PairedInstallStage::IndexerPublished;
        journal.updated_at = unix_timestamp();
        save_json_atomic(&journal_path, &journal)?;
    } else {
        verify_installed_indexer(&options.indexer_root, &checkpoint)?;
    }

    if journal.stage == PairedInstallStage::IndexerPublished {
        journal.stage = PairedInstallStage::BalanceHistoryInstalling;
        journal.updated_at = unix_timestamp();
        save_json_atomic(&journal_path, &journal)?;
        maybe_fail("balance_history_installing")?;
    }

    if journal.stage == PairedInstallStage::BalanceHistoryInstalling {
        install_or_recover_balance_history(&options, &balance_history)?;
        maybe_fail("balance_history_published")?;
        journal.stage = PairedInstallStage::BalanceHistoryPublished;
        journal.updated_at = unix_timestamp();
        save_json_atomic(&journal_path, &journal)?;
    } else if matches!(
        journal.stage,
        PairedInstallStage::BalanceHistoryPublished | PairedInstallStage::Complete
    ) {
        verify_installed_balance_history(&options.balance_history_root, &balance_history)?;
    }

    verify_installed_indexer(&options.indexer_root, &checkpoint)?;
    verify_installed_balance_history(&options.balance_history_root, &balance_history)?;
    journal.stage = PairedInstallStage::Complete;
    journal.updated_at = unix_timestamp();
    save_json_atomic(&journal_path, &journal)?;

    let marker_path = bootstrap_dir.join(INSTALL_COMPLETE_MARKER_FILE);
    save_json_atomic(
        &marker_path,
        &OfflineInstallMarker {
            version: 1,
            operation_id: checkpoint.operation_id.clone(),
            checkpoint_height: checkpoint.checkpoint_height,
            checkpoint_manifest_sha256,
            balance_history_manifest_sha256: checkpoint.balance_history.manifest_sha256.clone(),
            completed_at: unix_timestamp(),
        },
    )?;
    Ok(InstallPairReport {
        operation_id: checkpoint.operation_id,
        checkpoint_height: checkpoint.checkpoint_height,
        stage: journal.stage,
        journal_path,
        marker_path,
    })
}

#[cfg(test)]
pub(crate) async fn install_pair_for_test(
    options: InstallPairOptions,
    lock_suffix: &str,
) -> Result<InstallPairReport, String> {
    install_pair_with_lock_names(
        options,
        &format!("usdb-indexer-checkpoint-test-{lock_suffix}"),
        &format!("balance-history-checkpoint-test-{lock_suffix}"),
    )
    .await
}

/// Verifies both restarted services and writes the final consensus recovery marker.
pub async fn verify_recovery(
    checkpoint_manifest: &Path,
    trusted_keys: &Path,
    indexer_root: &Path,
    indexer_rpc_url: &str,
    balance_history_rpc_url: &str,
    readiness_timeout: Duration,
) -> Result<PairedCheckpointRecoveryMarker, String> {
    let manifest = load_and_verify_checkpoint(checkpoint_manifest, trusted_keys, false)?;
    let identity = wait_for_restarted_state_refs(
        &manifest,
        indexer_rpc_url,
        balance_history_rpc_url,
        readiness_timeout,
    )
    .await?;
    let marker = PairedCheckpointRecoveryMarker {
        version: RECOVERY_MARKER_VERSION,
        operation_id: manifest.operation_id,
        checkpoint_height: manifest.checkpoint_height,
        snapshot_id: identity.snapshot_id,
        local_state_commit: identity.local_state_commit,
        system_state_id: identity.system_state_id,
        verified_at: unix_timestamp(),
    };
    let marker_path = indexer_root.join("bootstrap").join(RECOVERY_MARKER_FILE);
    save_json_atomic(&marker_path, &marker)?;
    Ok(marker)
}

fn validate_deployment_binding(
    options: &InstallPairOptions,
    checkpoint: &IndexerCheckpointManifest,
) -> Result<(), String> {
    if checkpoint.network_bundle_id != options.network_bundle_id {
        return Err(format!(
            "Checkpoint network bundle mismatch: expected {}, got {}",
            options.network_bundle_id, checkpoint.network_bundle_id
        ));
    }
    if checkpoint.chain_id != options.chain_id {
        return Err(format!(
            "Checkpoint chain ID mismatch: expected {}, got {}",
            options.chain_id, checkpoint.chain_id
        ));
    }
    if checkpoint.index_origin_height != options.index_origin_height {
        return Err(format!(
            "Checkpoint index origin mismatch: expected {}, got {}",
            options.index_origin_height, checkpoint.index_origin_height
        ));
    }
    Ok(())
}

fn validate_upstream_binding(
    checkpoint: &IndexerCheckpointManifest,
    balance_history: &SnapshotManifest,
    manifest_path: &Path,
) -> Result<(), String> {
    let actual_manifest_sha256 = sha256_file(manifest_path)?;
    let expected = &checkpoint.balance_history;
    if file_name(manifest_path)? != expected.manifest_file_name
        || actual_manifest_sha256 != expected.manifest_sha256
        || balance_history.file_name != expected.snapshot_file_name
        || balance_history.file_sha256 != expected.snapshot_file_sha256
        || balance_history.state_ref != expected.state_ref
        || balance_history.balance_query_floor != expected.balance_query_floor
        || balance_history.history_query_floor != expected.history_query_floor
    {
        return Err("Balance-history artifact does not match signed paired binding".into());
    }
    Ok(())
}

fn load_or_create_journal(
    path: &Path,
    options: &InstallPairOptions,
    checkpoint: &IndexerCheckpointManifest,
    checkpoint_manifest_sha256: &str,
) -> Result<PairedInstallJournal, String> {
    if path.exists() {
        let data = std::fs::read(path).map_err(|error| {
            format!("Failed to read install journal {}: {error}", path.display())
        })?;
        let journal: PairedInstallJournal = serde_json::from_slice(&data).map_err(|error| {
            format!(
                "Failed to parse install journal {}: {error}",
                path.display()
            )
        })?;
        if journal.version != PAIRED_INSTALL_JOURNAL_VERSION
            || journal.operation_id != checkpoint.operation_id
            || journal.checkpoint_manifest_sha256 != checkpoint_manifest_sha256
            || journal.indexer_root != options.indexer_root
            || journal.balance_history_root != options.balance_history_root
        {
            return Err(format!(
                "Existing paired install journal does not match requested operation: {}",
                path.display()
            ));
        }
        return Ok(journal);
    }
    let journal = PairedInstallJournal {
        version: PAIRED_INSTALL_JOURNAL_VERSION,
        operation_id: checkpoint.operation_id.clone(),
        checkpoint_manifest_sha256: checkpoint_manifest_sha256.to_string(),
        indexer_root: options.indexer_root.clone(),
        balance_history_root: options.balance_history_root.clone(),
        stage: PairedInstallStage::Prepared,
        updated_at: unix_timestamp(),
    };
    save_json_atomic(path, &journal)?;
    Ok(journal)
}

pub(crate) fn publish_indexer_data(
    options: &InstallPairOptions,
    checkpoint: &IndexerCheckpointManifest,
) -> Result<(), String> {
    let layout = IndexerDiskLayout::load(&options.indexer_root)?;
    if layout.data_dir.exists() && directory_has_entries(&layout.data_dir)? {
        return verify_installed_indexer(&options.indexer_root, checkpoint);
    }
    if layout.data_dir.exists() {
        std::fs::remove_dir(&layout.data_dir).map_err(|error| {
            format!(
                "Failed to remove empty indexer data directory {}: {error}",
                layout.data_dir.display()
            )
        })?;
    }
    let data_parent = layout
        .data_dir
        .parent()
        .ok_or_else(|| "Indexer data directory has no parent".to_string())?;
    std::fs::create_dir_all(data_parent).map_err(|error| {
        format!(
            "Failed to create indexer data parent {}: {error}",
            data_parent.display()
        )
    })?;
    let staging = data_parent.join(format!(
        ".paired-checkpoint-{}.staging",
        &checkpoint.operation_id[..16]
    ));
    remove_managed_directory(&staging, "checkpoint staging")?;
    let artifact_data = options
        .checkpoint_manifest
        .parent()
        .ok_or_else(|| "Checkpoint manifest has no artifact directory".to_string())?
        .join("data");
    copy_directory(&artifact_data, &staging)?;
    if inventory_files(&staging)? != checkpoint.files {
        return Err("Staged indexer file inventory does not match checkpoint manifest".into());
    }
    let staged_layout = IndexerDiskLayout {
        data_dir: staging.clone(),
        bitcoin_network: layout.bitcoin_network,
        genesis_block_height: layout.genesis_block_height,
    };
    validate_indexer_data(&staged_layout, checkpoint)?;
    maybe_fail("indexer_staged")?;
    std::fs::rename(&staging, &layout.data_dir).map_err(|error| {
        format!(
            "Failed to atomically publish indexer checkpoint {} to {}: {error}",
            staging.display(),
            layout.data_dir.display()
        )
    })?;
    sync_dir(data_parent)
}

fn verify_installed_indexer(
    indexer_root: &Path,
    checkpoint: &IndexerCheckpointManifest,
) -> Result<(), String> {
    let layout = IndexerDiskLayout::load(indexer_root)?;
    if inventory_files(&layout.data_dir)? != checkpoint.files {
        return Err(format!(
            "Installed indexer data under {} does not match checkpoint file inventory",
            layout.data_dir.display()
        ));
    }
    validate_indexer_data(&layout, checkpoint).map(|_| ())
}

fn validate_indexer_target(
    indexer_root: &Path,
    checkpoint: &IndexerCheckpointManifest,
) -> Result<(), String> {
    let layout = IndexerDiskLayout::load(indexer_root)?;
    if !layout.data_dir.exists() || !directory_has_entries(&layout.data_dir)? {
        return Ok(());
    }
    verify_installed_indexer(indexer_root, checkpoint).map_err(|error| {
        format!("Indexer target contains data that does not match paired checkpoint: {error}")
    })
}

fn validate_balance_history_target(root: &Path, manifest: &SnapshotManifest) -> Result<(), String> {
    let config = Arc::new(BalanceHistoryConfig::load(root)?);
    let live_db = config.db_dir().join("balance_history");
    if !live_db.exists() || !directory_has_entries(&live_db)? {
        if has_balance_history_managed_directories(&config)? {
            return Err(format!(
                "Balance-history target contains unclaimed snapshot install remnants under {}",
                config.root_dir.display()
            ));
        }
        return Ok(());
    }
    verify_installed_balance_history(root, manifest).map_err(|error| {
        format!("Balance-history target contains data that does not match paired snapshot: {error}")
    })
}

fn install_or_recover_balance_history(
    options: &InstallPairOptions,
    manifest: &SnapshotManifest,
) -> Result<(), String> {
    let config = Arc::new(BalanceHistoryConfig::load(&options.balance_history_root)?);
    let live_db = config.db_dir().join("balance_history");
    if live_db.exists() && directory_has_entries(&live_db)? {
        if verify_installed_balance_history(&options.balance_history_root, manifest).is_ok() {
            return Ok(());
        }
        cleanup_balance_history_managed_directories(&config, true)?;
    } else {
        cleanup_balance_history_managed_directories(&config, false)?;
    }
    let status = Arc::new(SyncStatusManager::new());
    let output = Arc::new(IndexOutput::new(status));
    let db = Arc::new(BalanceHistoryDB::open(
        config.clone(),
        BalanceHistoryDBMode::BestEffort,
    )?);
    let snapshot_file = options
        .balance_history_manifest
        .parent()
        .ok_or_else(|| "Balance-history manifest has no parent directory".to_string())?
        .join(&manifest.file_name);
    SnapshotInstaller::new(config.clone(), db, output).install(SnapshotData {
        file: snapshot_file,
        manifest_file: Some(options.balance_history_manifest.clone()),
    })?;
    verify_installed_balance_history(&options.balance_history_root, manifest)?;
    cleanup_balance_history_managed_directories(&config, false)
}

fn has_balance_history_managed_directories(config: &BalanceHistoryConfig) -> Result<bool, String> {
    let entries = std::fs::read_dir(&config.root_dir).map_err(|error| {
        format!(
            "Failed to inspect balance-history root {}: {error}",
            config.root_dir.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to inspect balance-history root {}: {error}",
                config.root_dir.display()
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("snapshot_install_staging_")
            || name.starts_with("db_backup_snapshot_install_")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn cleanup_balance_history_managed_directories(
    config: &BalanceHistoryConfig,
    remove_live_db: bool,
) -> Result<(), String> {
    if remove_live_db {
        remove_managed_directory(&config.db_dir(), "interrupted balance-history live DB")?;
    }
    let entries = std::fs::read_dir(&config.root_dir).map_err(|error| {
        format!(
            "Failed to inspect balance-history root {}: {error}",
            config.root_dir.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to inspect balance-history root {}: {error}",
                config.root_dir.display()
            )
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("snapshot_install_staging_")
            || name.starts_with("db_backup_snapshot_install_")
        {
            remove_managed_directory(&entry.path(), "balance-history snapshot install remnant")?;
        }
    }
    Ok(())
}

fn remove_managed_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Failed to inspect {label} {}: {error}",
                path.display()
            ));
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_dir() {
        return Err(format!(
            "Refusing to remove {label} because it is not a real directory: {}",
            path.display()
        ));
    }
    std::fs::remove_dir_all(path)
        .map_err(|error| format!("Failed to remove {label} {}: {error}", path.display()))
}

fn verify_installed_balance_history(
    root: &Path,
    manifest: &SnapshotManifest,
) -> Result<(), String> {
    let config = Arc::new(BalanceHistoryConfig::load(root)?);
    let db = BalanceHistoryDB::open_read_only(config.clone())?;
    if db.get_btc_block_height()? != manifest.state_ref.block_height {
        return Err(format!(
            "Installed balance-history height mismatch: expected {}, got {}",
            manifest.state_ref.block_height,
            db.get_btc_block_height()?
        ));
    }
    let actual =
        build_historical_state_ref_at_height(&config, &db, manifest.state_ref.block_height)?
            .ok_or_else(|| "Installed balance-history state-ref is unavailable".to_string())?;
    if actual != manifest.state_ref {
        return Err(format!(
            "Installed balance-history state-ref mismatch: expected={:?}, actual={:?}",
            manifest.state_ref, actual
        ));
    }
    let provenance = db
        .get_snapshot_install_provenance()?
        .ok_or_else(|| "Installed balance-history has no snapshot provenance".to_string())?;
    if provenance.verification_state != SnapshotVerificationState::SignatureVerified
        || !provenance.signature_verified
        || provenance.snapshot_id.as_deref() != Some(manifest.state_ref.snapshot_id.as_str())
    {
        return Err(format!(
            "Installed balance-history snapshot provenance is not trusted: {provenance:?}"
        ));
    }
    Ok(())
}

fn directory_has_entries(path: &Path) -> Result<bool, String> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|error| format!("Failed to inspect directory {}: {error}", path.display()))?;
    Ok(entries
        .next()
        .transpose()
        .map_err(|error| format!("Failed to inspect directory {}: {error}", path.display()))?
        .is_some())
}

fn maybe_fail(stage: &str) -> Result<(), String> {
    if std::env::var(FAULT_ENV).ok().as_deref() == Some(stage) {
        return Err(format!("Injected paired checkpoint failure after {stage}"));
    }
    Ok(())
}

#[cfg(test)]
mod managed_directory_tests {
    use super::remove_managed_directory;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "usdb_checkpoint_managed_dir_{tag}_{}_{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn removes_real_managed_directory() {
        let root = temp_root("real");
        let managed = root.join("snapshot_install_staging_test");
        std::fs::create_dir(&managed).unwrap();
        std::fs::write(managed.join("partial"), b"data").unwrap();

        remove_managed_directory(&managed, "test managed directory").unwrap();

        assert!(!managed.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_managed_directory_without_removing_target() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, b"unchanged").unwrap();
        let managed = root.join("snapshot_install_staging_test");
        symlink(&outside, &managed).unwrap();

        let error = remove_managed_directory(&managed, "test managed directory").unwrap_err();

        assert!(error.contains("not a real directory"));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
        std::fs::remove_file(managed).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
