use crate::crypto::{
    require_signing_key_trusted, sign_manifest, verify_balance_history_manifest_signature,
    verify_manifest_signature,
};
use crate::data::{IndexerDiskLayout, validate_indexer_data};
use crate::rpc::{
    CheckpointRpcClient, extract_indexer_state_identity, require_consensus_ready,
    validate_paired_state_refs,
};
use crate::{
    BalanceHistorySnapshotBinding, CHECKPOINT_MANIFEST_FILE, CHECKPOINT_SIGNATURE_FILE,
    CHECKPOINT_SIGNATURE_SCHEME, CheckpointFileEntry, INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION,
    INDEXER_CHECKPOINT_MANIFEST_VERSION, IndexerCheckpointManifest,
};
use balance_history::{SnapshotManifest, SnapshotSigningKeyFile};
use named_lock::{NamedLock, NamedLockGuard};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Inputs required to export one immutable checkpoint from a running indexer.
#[derive(Clone, Debug)]
pub struct ExportCheckpointOptions {
    /// Running indexer service root.
    pub indexer_root: PathBuf,
    /// Local indexer RPC endpoint used to seal state and request a clean stop.
    pub indexer_rpc_url: String,
    /// Exact target height; export refuses a service at any other durable height.
    pub checkpoint_height: u32,
    /// Network bundle identity recorded in the artifact.
    pub network_bundle_id: String,
    /// USDB chain ID recorded in the artifact.
    pub chain_id: u64,
    /// Per-network BTC index origin.
    pub index_origin_height: u32,
    /// Signed balance-history snapshot manifest at the same height.
    pub balance_history_manifest: PathBuf,
    /// Trusted public-key catalog used to verify the upstream snapshot first.
    pub trusted_keys: PathBuf,
    /// Private signing-key file for this checkpoint manifest.
    pub signing_key: PathBuf,
    /// Parent directory under which an immutable artifact directory is published.
    pub output_root: PathBuf,
    /// Maximum time to wait for the graceful service stop and process lock.
    pub stop_timeout: Duration,
}

/// Result of a completed checkpoint export.
#[derive(Clone, Debug, Serialize)]
pub struct ExportCheckpointReport {
    /// Immutable operation identity.
    pub operation_id: String,
    /// Published artifact directory.
    pub artifact_dir: PathBuf,
    /// Signed manifest path.
    pub manifest_path: PathBuf,
    /// Exact checkpoint height.
    pub checkpoint_height: u32,
    /// Number of files committed below `data/`.
    pub file_count: usize,
}

pub(crate) struct HeldProcessLock {
    _lock: NamedLock,
    _guard: NamedLockGuard,
}

/// Seals state through RPC, gracefully stops indexer, and publishes a signed immutable directory.
pub async fn export_checkpoint(
    options: ExportCheckpointOptions,
) -> Result<ExportCheckpointReport, String> {
    let layout = IndexerDiskLayout::load(&options.indexer_root)?;
    if layout.genesis_block_height != options.index_origin_height {
        return Err(format!(
            "Requested index origin {} does not match indexer config {}",
            options.index_origin_height, layout.genesis_block_height
        ));
    }
    let bh_manifest = load_and_verify_balance_history_manifest(
        &options.balance_history_manifest,
        &options.trusted_keys,
        true,
    )?;
    if bh_manifest.state_ref.block_height != options.checkpoint_height {
        return Err(format!(
            "Balance-history snapshot height mismatch: expected {}, got {}",
            options.checkpoint_height, bh_manifest.state_ref.block_height
        ));
    }
    let signing_key = SnapshotSigningKeyFile::load(&options.signing_key)?;
    require_signing_key_trusted(&signing_key, &options.trusted_keys)?;
    std::fs::create_dir_all(&options.output_root).map_err(|error| {
        format!(
            "Failed to create checkpoint output root {}: {error}",
            options.output_root.display()
        )
    })?;
    let source_data = std::fs::canonicalize(&layout.data_dir).map_err(|error| {
        format!(
            "Failed to canonicalize indexer data directory {}: {error}",
            layout.data_dir.display()
        )
    })?;
    let output_root = std::fs::canonicalize(&options.output_root).map_err(|error| {
        format!(
            "Failed to canonicalize checkpoint output root {}: {error}",
            options.output_root.display()
        )
    })?;
    if output_root.starts_with(&source_data) {
        return Err(format!(
            "Checkpoint output root {} must not be inside indexer data directory {}",
            output_root.display(),
            source_data.display()
        ));
    }

    let client = CheckpointRpcClient::new(&options.indexer_rpc_url)?;
    let readiness = client.indexer_readiness().await?;
    require_consensus_ready(&readiness, options.checkpoint_height, "usdb-indexer")?;
    let indexer_state_ref = client.indexer_state_ref(options.checkpoint_height).await?;
    let state_identity = extract_indexer_state_identity(&indexer_state_ref)?;
    validate_paired_state_refs(&state_identity, &bh_manifest.state_ref)?;

    client.stop_indexer().await?;
    let _process_lock = wait_for_process_lock("usdb-indexer", options.stop_timeout).await?;

    let bh_manifest_sha256 = sha256_file(&options.balance_history_manifest)?;
    let bh_binding = BalanceHistorySnapshotBinding {
        manifest_file_name: file_name(&options.balance_history_manifest)?,
        manifest_sha256: bh_manifest_sha256,
        snapshot_file_name: bh_manifest.file_name.clone(),
        snapshot_file_sha256: bh_manifest.file_sha256.clone(),
        state_ref: bh_manifest.state_ref.clone(),
        balance_query_floor: bh_manifest.balance_query_floor,
        history_query_floor: bh_manifest.history_query_floor,
    };
    let temp_dir = options.output_root.join(format!(
        ".checkpoint-export-{}-{}",
        std::process::id(),
        unix_timestamp()
    ));
    if temp_dir.exists() {
        return Err(format!(
            "Checkpoint temporary directory already exists: {}",
            temp_dir.display()
        ));
    }
    let temp_data = temp_dir.join("data");
    copy_directory(&layout.data_dir, &temp_data)?;
    let files = inventory_files(&temp_data)?;
    let operation_id = build_operation_id(
        &options.network_bundle_id,
        options.chain_id,
        &bh_binding,
        &state_identity,
        &files,
    )?;
    let artifact_dir_name = format!(
        "usdb-indexer-checkpoint-{}-{}",
        options.checkpoint_height,
        &operation_id[..16]
    );
    let final_dir = options.output_root.join(&artifact_dir_name);
    if final_dir.exists() {
        return Err(format!(
            "Checkpoint artifact already exists: {}",
            final_dir.display()
        ));
    }

    let manifest = IndexerCheckpointManifest {
        manifest_version: INDEXER_CHECKPOINT_MANIFEST_VERSION.to_string(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        data_schema_version: INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION.to_string(),
        operation_id: operation_id.clone(),
        network_bundle_id: options.network_bundle_id,
        chain_id: options.chain_id,
        btc_network: layout.bitcoin_network.to_string(),
        index_origin_height: options.index_origin_height,
        checkpoint_height: options.checkpoint_height,
        artifact_dir_name: artifact_dir_name.clone(),
        files,
        balance_history: bh_binding,
        indexer_state_ref,
        state_identity,
        signature_scheme: CHECKPOINT_SIGNATURE_SCHEME.to_string(),
        signing_key_id: signing_key.key_id.clone(),
        generated_at: unix_timestamp(),
    };

    let staged_layout = IndexerDiskLayout {
        data_dir: temp_data,
        bitcoin_network: layout.bitcoin_network,
        genesis_block_height: layout.genesis_block_height,
    };
    validate_indexer_data(&staged_layout, &manifest)?;
    let manifest_path = temp_dir.join(CHECKPOINT_MANIFEST_FILE);
    save_json_atomic(&manifest_path, &manifest)?;
    let signature_path = temp_dir.join(CHECKPOINT_SIGNATURE_FILE);
    sign_manifest(&manifest, &signing_key, &signature_path)?;
    sync_tree(&temp_dir)?;
    std::fs::rename(&temp_dir, &final_dir).map_err(|error| {
        format!(
            "Failed to publish checkpoint {} to {}: {error}",
            temp_dir.display(),
            final_dir.display()
        )
    })?;
    sync_dir(&options.output_root)?;

    let published_manifest = final_dir.join(CHECKPOINT_MANIFEST_FILE);
    load_and_verify_checkpoint(&published_manifest, &options.trusted_keys, true)?;
    Ok(ExportCheckpointReport {
        operation_id,
        artifact_dir: final_dir,
        manifest_path: published_manifest,
        checkpoint_height: options.checkpoint_height,
        file_count: manifest.files.len(),
    })
}

/// Loads one checkpoint manifest, verifies its signature, and optionally hashes every data file.
pub fn load_and_verify_checkpoint(
    manifest_path: &Path,
    trusted_keys_path: &Path,
    verify_files: bool,
) -> Result<IndexerCheckpointManifest, String> {
    if file_name(manifest_path)? != CHECKPOINT_MANIFEST_FILE {
        return Err(format!(
            "Checkpoint manifest must be named {CHECKPOINT_MANIFEST_FILE}"
        ));
    }
    let data = std::fs::read(manifest_path).map_err(|error| {
        format!(
            "Failed to read checkpoint manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: IndexerCheckpointManifest = serde_json::from_slice(&data).map_err(|error| {
        format!(
            "Failed to parse checkpoint manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if manifest.manifest_version != INDEXER_CHECKPOINT_MANIFEST_VERSION {
        return Err(format!(
            "Unsupported checkpoint manifest version {}",
            manifest.manifest_version
        ));
    }
    if manifest.tool_version.trim().is_empty() {
        return Err("Checkpoint manifest has an empty tool_version".into());
    }
    if manifest.data_schema_version != INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION {
        return Err(format!(
            "Unsupported indexer checkpoint data schema {}",
            manifest.data_schema_version
        ));
    }
    if manifest.checkpoint_height < manifest.index_origin_height {
        return Err(format!(
            "Checkpoint height {} precedes index origin {}",
            manifest.checkpoint_height, manifest.index_origin_height
        ));
    }
    if manifest.state_identity.block_height != manifest.checkpoint_height {
        return Err("Checkpoint state identity height does not match checkpoint height".into());
    }
    let artifact_dir = manifest_path
        .parent()
        .ok_or_else(|| "Checkpoint manifest has no parent directory".to_string())?;
    if file_name(artifact_dir)? != manifest.artifact_dir_name {
        return Err(format!(
            "Checkpoint artifact directory mismatch: manifest={}, actual={}",
            manifest.artifact_dir_name,
            artifact_dir.display()
        ));
    }
    verify_manifest_signature(
        &manifest,
        &artifact_dir.join(CHECKPOINT_SIGNATURE_FILE),
        trusted_keys_path,
    )?;
    if verify_files {
        let actual = inventory_files(&artifact_dir.join("data"))?;
        if actual != manifest.files {
            return Err("Checkpoint data file inventory does not match signed manifest".into());
        }
        let bitcoin_network = bitcoincore_rpc::bitcoin::Network::from_str(&manifest.btc_network)
            .map_err(|error| {
                format!(
                    "Checkpoint manifest has an invalid Bitcoin network {}: {error}",
                    manifest.btc_network
                )
            })?;
        validate_indexer_data(
            &IndexerDiskLayout {
                data_dir: artifact_dir.join("data"),
                bitcoin_network,
                genesis_block_height: manifest.index_origin_height,
            },
            &manifest,
        )?;
    }
    let rebuilt_operation_id = build_operation_id(
        &manifest.network_bundle_id,
        manifest.chain_id,
        &manifest.balance_history,
        &manifest.state_identity,
        &manifest.files,
    )?;
    if rebuilt_operation_id != manifest.operation_id {
        return Err(format!(
            "Checkpoint operation ID mismatch: expected {}, got {}",
            manifest.operation_id, rebuilt_operation_id
        ));
    }
    Ok(manifest)
}

/// Loads and verifies a signed balance-history manifest and, when requested, its snapshot file.
pub fn load_and_verify_balance_history_manifest(
    manifest_path: &Path,
    trusted_keys_path: &Path,
    verify_snapshot_file: bool,
) -> Result<SnapshotManifest, String> {
    let manifest = SnapshotManifest::load(manifest_path)?;
    verify_balance_history_manifest_signature(&manifest, manifest_path, trusted_keys_path)?;
    if verify_snapshot_file {
        let snapshot_path = manifest_path
            .parent()
            .ok_or_else(|| "Balance-history manifest has no parent directory".to_string())?
            .join(&manifest.file_name);
        let actual = sha256_file(&snapshot_path)?;
        if actual != manifest.file_sha256 {
            return Err(format!(
                "Balance-history snapshot hash mismatch: expected {}, got {}",
                manifest.file_sha256, actual
            ));
        }
    }
    Ok(manifest)
}

/// Verifies both signed artifacts and every field committed by their pair binding.
pub fn load_and_verify_checkpoint_pair(
    checkpoint_manifest_path: &Path,
    balance_history_manifest_path: &Path,
    trusted_keys_path: &Path,
) -> Result<IndexerCheckpointManifest, String> {
    let checkpoint = load_and_verify_checkpoint(checkpoint_manifest_path, trusted_keys_path, true)?;
    let balance_history = load_and_verify_balance_history_manifest(
        balance_history_manifest_path,
        trusted_keys_path,
        true,
    )?;
    let binding = &checkpoint.balance_history;
    if file_name(balance_history_manifest_path)? != binding.manifest_file_name
        || sha256_file(balance_history_manifest_path)? != binding.manifest_sha256
        || balance_history.file_name != binding.snapshot_file_name
        || balance_history.file_sha256 != binding.snapshot_file_sha256
        || balance_history.state_ref != binding.state_ref
        || balance_history.balance_query_floor != binding.balance_query_floor
        || balance_history.history_query_floor != binding.history_query_floor
    {
        return Err("Balance-history artifact does not match signed checkpoint binding".into());
    }
    Ok(checkpoint)
}

pub(crate) async fn wait_for_process_lock(
    service_name: &str,
    timeout: Duration,
) -> Result<HeldProcessLock, String> {
    let lock_name = format!("{service_name}_lock");
    let lock = NamedLock::create(&lock_name)
        .map_err(|error| format!("Failed to create process lock {lock_name}: {error}"))?;
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(guard) => {
                return Ok(HeldProcessLock {
                    _lock: lock,
                    _guard: guard,
                });
            }
            Err(_) if started.elapsed() < timeout => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => {
                return Err(format!(
                    "Timed out waiting for {service_name} process lock after {:.1}s: {error}",
                    timeout.as_secs_f64()
                ));
            }
        }
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Failed to open {} for SHA-256: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Failed to hash {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn inventory_files(root: &Path) -> Result<Vec<CheckpointFileEntry>, String> {
    if !root.is_dir() {
        return Err(format!(
            "Checkpoint data directory is missing: {}",
            root.display()
        ));
    }
    let mut paths = Vec::new();
    collect_files(root, root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|relative| {
            let absolute = root.join(&relative);
            let metadata = std::fs::metadata(&absolute).map_err(|error| {
                format!(
                    "Failed to stat checkpoint file {}: {error}",
                    absolute.display()
                )
            })?;
            Ok(CheckpointFileEntry {
                path: relative_path_string(&relative)?,
                size: metadata.len(),
                sha256: sha256_file(&absolute)?,
            })
        })
        .collect()
}

fn collect_files(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = std::fs::read_dir(current)
        .map_err(|error| format!("Failed to read directory {}: {error}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to enumerate {}: {error}", current.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "Checkpoint data must not contain symlinks: {}",
                entry.path().display()
            ));
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), output)?;
        } else if file_type.is_file() {
            output.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| format!("Failed to make checkpoint path relative: {error}"))?
                    .to_path_buf(),
            );
        } else {
            return Err(format!(
                "Checkpoint data contains an unsupported filesystem entry: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn copy_directory(source: &Path, target: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "Source data directory is missing: {}",
            source.display()
        ));
    }
    std::fs::create_dir_all(target).map_err(|error| {
        format!(
            "Failed to create staging directory {}: {error}",
            target.display()
        )
    })?;
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| {
            format!(
                "Failed to read source directory {}: {error}",
                source.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Failed to enumerate {}: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Failed to inspect {}: {error}", source_path.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "Refusing to copy symlink into checkpoint: {}",
                source_path.display()
            ));
        }
        if file_type.is_dir() {
            copy_directory(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &target_path).map_err(|error| {
                format!(
                    "Failed to copy checkpoint file {} to {}: {error}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
            OpenOptions::new()
                .read(true)
                .open(&target_path)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    format!(
                        "Failed to sync checkpoint file {}: {error}",
                        target_path.display()
                    )
                })?;
        }
    }
    sync_dir(target)
}

pub(crate) fn build_operation_id(
    network_bundle_id: &str,
    chain_id: u64,
    balance_history: &BalanceHistorySnapshotBinding,
    state_identity: &crate::IndexerCheckpointStateIdentity,
    files: &[CheckpointFileEntry],
) -> Result<String, String> {
    let canonical = serde_json::to_vec(&(
        "usdb-paired-checkpoint-operation:v1",
        network_bundle_id,
        chain_id,
        balance_history,
        state_identity,
        files,
    ))
    .map_err(|error| format!("Failed to serialize checkpoint operation identity: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

pub(crate) fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("JSON path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create JSON parent {}: {error}", parent.display()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let data = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Failed to serialize JSON {}: {error}", path.display()))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| {
            format!(
                "Failed to create temporary JSON {}: {error}",
                temp.display()
            )
        })?;
    file.write_all(&data)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            format!(
                "Failed to persist temporary JSON {}: {error}",
                temp.display()
            )
        })?;
    std::fs::rename(&temp, path).map_err(|error| {
        format!(
            "Failed to atomically publish JSON {} to {}: {error}",
            temp.display(),
            path.display()
        )
    })?;
    sync_dir(parent)
}

pub(crate) fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Failed to sync directory {}: {error}", path.display()))
}

fn sync_tree(root: &Path) -> Result<(), String> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let current = directories[index].clone();
        index += 1;
        for entry in std::fs::read_dir(&current)
            .map_err(|error| format!("Failed to read {} for sync: {error}", current.display()))?
        {
            let entry = entry.map_err(|error| {
                format!(
                    "Failed to enumerate {} for sync: {error}",
                    current.display()
                )
            })?;
            if entry
                .file_type()
                .map_err(|error| format!("Failed to inspect {}: {error}", entry.path().display()))?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        sync_dir(&directory)?;
    }
    Ok(())
}

fn relative_path_string(path: &Path) -> Result<String, String> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "Unsafe checkpoint relative path: {}",
            path.display()
        ));
    }
    Ok(path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

pub(crate) fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("Path has no UTF-8 file name: {}", path.display()))
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
