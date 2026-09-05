use crate::{
    COMPLETE_MARKER_VERSION, ScriptRegistryCompleteMarker, SnapshotCompleteMarker,
    SnapshotVerificationPhase, load_json, unix_timestamp,
};
use balance_history::{
    BalanceHistoryDBIdentity, CoreSnapshotDb, CoreSnapshotManifest, ScriptRegistryManifest,
    ScriptRegistrySnapshotDb, SnapshotHash, signature_path_for_manifest_file,
};
use log::info;
use std::path::{Path, PathBuf};
use std::time::Instant;

const SNAPSHOT_VERIFICATION_CACHE_SIZE_KIB: u32 = 512 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct VerifiedSnapshot {
    pub db_path: PathBuf,
    pub manifest_path: PathBuf,
    pub signature_path: Option<PathBuf>,
    pub manifest: CoreSnapshotManifest,
    pub balance_history_count: u64,
    pub utxo_count: u64,
    pub block_commit_count: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct VerifiedScriptRegistry {
    pub db_path: PathBuf,
    pub manifest_path: PathBuf,
    pub signature_path: Option<PathBuf>,
    pub manifest: ScriptRegistryManifest,
    pub entry_count: u64,
}

pub(crate) fn verify_snapshot_files(
    db_path: &Path,
    manifest_path: &Path,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
) -> Result<VerifiedSnapshot, String> {
    verify_snapshot_files_with_progress(
        db_path,
        manifest_path,
        expected_network,
        expected_height,
        expected_block_hash,
        |_| Ok(()),
    )
}

pub(crate) fn verify_snapshot_files_with_progress<F>(
    db_path: &Path,
    manifest_path: &Path,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
    mut on_phase: F,
) -> Result<VerifiedSnapshot, String>
where
    F: FnMut(SnapshotVerificationPhase) -> Result<(), String>,
{
    require_file(db_path, "Core snapshot DB")?;
    require_file(manifest_path, "Core snapshot manifest")?;
    let manifest = CoreSnapshotManifest::load(manifest_path)?;
    verify_file_name(db_path, &manifest.file_name, "Core snapshot")?;
    verify_common_identity(
        &manifest.db_identity,
        &manifest.state_ref.consensus_identity.network,
        manifest.state_ref.block_height,
        &manifest.state_ref.stable_block_hash,
        expected_network,
        expected_height,
        expected_block_hash,
        "Core snapshot",
    )?;

    let started = begin_phase(SnapshotVerificationPhase::FileHash, db_path, &mut on_phase)?;
    verify_file_hash(db_path, &manifest.file_sha256, "Core snapshot")?;
    finish_phase(SnapshotVerificationPhase::FileHash, db_path, started);

    let db = CoreSnapshotDb::open_for_verification(db_path, SNAPSHOT_VERIFICATION_CACHE_SIZE_KIB)?;
    let started = begin_phase(
        SnapshotVerificationPhase::IntegrityCheck,
        db_path,
        &mut on_phase,
    )?;
    db.verify_integrity()?;
    finish_phase(SnapshotVerificationPhase::IntegrityCheck, db_path, started);
    let started = begin_phase(SnapshotVerificationPhase::Schema, db_path, &mut on_phase)?;
    db.verify_schema()?;
    finish_phase(SnapshotVerificationPhase::Schema, db_path, started);

    let meta = db.read_meta()?;
    if meta.block_height != expected_height
        || meta.db_identity != manifest.db_identity
        || meta.core_snapshot_id != manifest.core_snapshot_id
        || meta.generated_at != manifest.generated_at
    {
        return Err(format!(
            "Core snapshot metadata identity mismatch: height={} db_identity={:?} core_snapshot_id={} generated_at={}",
            meta.block_height, meta.db_identity, meta.core_snapshot_id, meta.generated_at
        ));
    }

    let started = begin_phase(
        SnapshotVerificationPhase::BalanceHistoryCount,
        db_path,
        &mut on_phase,
    )?;
    let balance_history_count = db.balance_history_count()?;
    finish_phase(
        SnapshotVerificationPhase::BalanceHistoryCount,
        db_path,
        started,
    );
    let started = begin_phase(SnapshotVerificationPhase::UtxoCount, db_path, &mut on_phase)?;
    let utxo_count = db.utxo_count()?;
    finish_phase(SnapshotVerificationPhase::UtxoCount, db_path, started);
    let started = begin_phase(
        SnapshotVerificationPhase::BlockCommitCount,
        db_path,
        &mut on_phase,
    )?;
    let block_commit_count = db.block_commit_count()?;
    finish_phase(
        SnapshotVerificationPhase::BlockCommitCount,
        db_path,
        started,
    );
    let actual_counts = (balance_history_count, utxo_count, block_commit_count);
    let metadata_counts = (
        meta.balance_history_count,
        meta.utxo_count,
        meta.block_commit_count,
    );
    if actual_counts != metadata_counts {
        return Err(format!(
            "Core snapshot metadata count mismatch: metadata={metadata_counts:?}, actual={actual_counts:?}"
        ));
    }

    let started = begin_phase(
        SnapshotVerificationPhase::CommitIdentity,
        db_path,
        &mut on_phase,
    )?;
    let latest_commit = db.latest_block_commit()?.ok_or_else(|| {
        format!(
            "Core snapshot at height {} does not contain a block commit",
            expected_height
        )
    })?;
    if latest_commit.block_height != expected_height
        || format!("{:x}", latest_commit.btc_block_hash) != manifest.state_ref.stable_block_hash
        || encode_hex(&latest_commit.block_commit) != manifest.state_ref.latest_block_commit
    {
        return Err("Core snapshot latest block commitment does not match manifest".to_string());
    }
    let signature_path = verify_signature_layout(
        manifest_path,
        manifest.signing_key_id.is_some(),
        "core snapshot",
    )?;
    finish_phase(SnapshotVerificationPhase::CommitIdentity, db_path, started);

    Ok(VerifiedSnapshot {
        db_path: db_path.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        signature_path,
        manifest,
        balance_history_count,
        utxo_count,
        block_commit_count,
    })
}

pub(crate) fn verify_registry_files(
    db_path: &Path,
    manifest_path: &Path,
    core_manifest: &CoreSnapshotManifest,
) -> Result<VerifiedScriptRegistry, String> {
    verify_registry_files_with_progress(db_path, manifest_path, core_manifest, |_| Ok(()))
}

pub(crate) fn verify_registry_files_with_progress<F>(
    db_path: &Path,
    manifest_path: &Path,
    core_manifest: &CoreSnapshotManifest,
    mut on_phase: F,
) -> Result<VerifiedScriptRegistry, String>
where
    F: FnMut(SnapshotVerificationPhase) -> Result<(), String>,
{
    require_file(db_path, "Script-registry DB")?;
    require_file(manifest_path, "Script-registry manifest")?;
    core_manifest.validate()?;
    let manifest = ScriptRegistryManifest::load(manifest_path)?;
    manifest.validate_against_core(core_manifest)?;
    verify_file_name(db_path, &manifest.file_name, "Script registry")?;

    let started = begin_phase(SnapshotVerificationPhase::FileHash, db_path, &mut on_phase)?;
    verify_file_hash(db_path, &manifest.file_sha256, "Script registry")?;
    finish_phase(SnapshotVerificationPhase::FileHash, db_path, started);
    let db = ScriptRegistrySnapshotDb::open_for_verification(
        db_path,
        SNAPSHOT_VERIFICATION_CACHE_SIZE_KIB,
    )?;
    let started = begin_phase(
        SnapshotVerificationPhase::IntegrityCheck,
        db_path,
        &mut on_phase,
    )?;
    db.verify_integrity()?;
    finish_phase(SnapshotVerificationPhase::IntegrityCheck, db_path, started);
    let started = begin_phase(SnapshotVerificationPhase::Schema, db_path, &mut on_phase)?;
    db.verify_schema()?;
    finish_phase(SnapshotVerificationPhase::Schema, db_path, started);
    let meta = db.read_meta()?;
    if meta.base != manifest.base || meta.generated_at != manifest.generated_at {
        return Err(
            "Script-registry DB base identity or generation time does not match manifest"
                .to_string(),
        );
    }
    let started = begin_phase(
        SnapshotVerificationPhase::RegistryCount,
        db_path,
        &mut on_phase,
    )?;
    let entry_count = db.entry_count()?;
    finish_phase(SnapshotVerificationPhase::RegistryCount, db_path, started);
    if entry_count != meta.entry_count || entry_count != manifest.entry_count {
        return Err(format!(
            "Script-registry count mismatch: manifest={}, metadata={}, actual={}",
            manifest.entry_count, meta.entry_count, entry_count
        ));
    }
    let started = begin_phase(
        SnapshotVerificationPhase::CommitIdentity,
        db_path,
        &mut on_phase,
    )?;
    let signature_path = verify_signature_layout(
        manifest_path,
        manifest.signing_key_id.is_some(),
        "script registry",
    )?;
    finish_phase(SnapshotVerificationPhase::CommitIdentity, db_path, started);
    Ok(VerifiedScriptRegistry {
        db_path: db_path.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        signature_path,
        manifest,
        entry_count,
    })
}

fn begin_phase<F>(
    phase: SnapshotVerificationPhase,
    db_path: &Path,
    on_phase: &mut F,
) -> Result<Instant, String>
where
    F: FnMut(SnapshotVerificationPhase) -> Result<(), String>,
{
    on_phase(phase)?;
    info!(
        "Starting snapshot verification phase {:?} for {}",
        phase,
        db_path.display()
    );
    Ok(Instant::now())
}

fn finish_phase(phase: SnapshotVerificationPhase, db_path: &Path, started: Instant) {
    info!(
        "Completed snapshot verification phase {:?} for {} in {:.1}s",
        phase,
        db_path.display(),
        started.elapsed().as_secs_f64()
    );
}

pub(crate) fn build_complete_marker(
    verified: &VerifiedSnapshot,
    network: &str,
) -> Result<SnapshotCompleteMarker, String> {
    Ok(SnapshotCompleteMarker {
        version: COMPLETE_MARKER_VERSION,
        height: verified.manifest.state_ref.block_height,
        network: network.to_string(),
        btc_block_hash: verified.manifest.state_ref.stable_block_hash.clone(),
        snapshot_id: verified.manifest.core_snapshot_id.clone(),
        core_artifact_id: verified.manifest.core_artifact_id.clone(),
        snapshot_file: file_name(&verified.db_path)?,
        manifest_file: file_name(&verified.manifest_path)?,
        signature_file: optional_file_name(verified.signature_path.as_ref())?,
        file_sha256: verified.manifest.file_sha256.clone(),
        balance_history_count: verified.balance_history_count,
        utxo_count: verified.utxo_count,
        block_commit_count: verified.block_commit_count,
        completed_at: unix_timestamp(),
    })
}

pub(crate) fn build_registry_complete_marker(
    verified: &VerifiedScriptRegistry,
) -> Result<ScriptRegistryCompleteMarker, String> {
    Ok(ScriptRegistryCompleteMarker {
        version: COMPLETE_MARKER_VERSION,
        height: verified.manifest.base.base_height,
        network: verified.manifest.base.btc_network.clone(),
        btc_block_hash: verified.manifest.base.base_block_hash.clone(),
        core_snapshot_id: verified.manifest.base.core_snapshot_id.clone(),
        registry_artifact_id: verified.manifest.registry_artifact_id.clone(),
        registry_file: file_name(&verified.db_path)?,
        manifest_file: file_name(&verified.manifest_path)?,
        signature_file: optional_file_name(verified.signature_path.as_ref())?,
        file_sha256: verified.manifest.file_sha256.clone(),
        entry_count: verified.entry_count,
        completed_at: unix_timestamp(),
    })
}

pub(crate) fn verify_published_artifact(
    artifact_dir: &Path,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
) -> Result<SnapshotCompleteMarker, String> {
    let marker = verify_published_artifact_marker(
        artifact_dir,
        expected_network,
        expected_height,
        expected_block_hash,
    )?;
    let verified = verify_snapshot_files(
        &safe_artifact_file(artifact_dir, &marker.snapshot_file)?,
        &safe_artifact_file(artifact_dir, &marker.manifest_file)?,
        expected_network,
        expected_height,
        Some(&marker.btc_block_hash),
    )?;
    let rebuilt = build_complete_marker(&verified, expected_network)?;
    if !same_core_marker_identity(&marker, &rebuilt) {
        return Err(format!(
            "Core completion marker does not match verified artifact {}",
            artifact_dir.display()
        ));
    }
    Ok(marker)
}

pub(crate) fn verify_published_artifact_marker(
    artifact_dir: &Path,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
) -> Result<SnapshotCompleteMarker, String> {
    let marker_path = artifact_dir.join("complete.json");
    let marker: SnapshotCompleteMarker = load_json(&marker_path)?.ok_or_else(|| {
        format!(
            "Core artifact is missing completion marker {}",
            marker_path.display()
        )
    })?;
    verify_marker_header(
        marker.version,
        marker.height,
        &marker.network,
        &marker.btc_block_hash,
        expected_network,
        expected_height,
        expected_block_hash,
        "Core",
    )?;
    let db_path = safe_artifact_file(artifact_dir, &marker.snapshot_file)?;
    let manifest_path = safe_artifact_file(artifact_dir, &marker.manifest_file)?;
    require_file(&db_path, "Core snapshot DB")?;
    require_file(&manifest_path, "Core snapshot manifest")?;
    let manifest = CoreSnapshotManifest::load(&manifest_path)?;
    if manifest.file_name != marker.snapshot_file
        || manifest.file_sha256 != marker.file_sha256
        || manifest.core_snapshot_id != marker.snapshot_id
        || manifest.core_artifact_id != marker.core_artifact_id
        || manifest.state_ref.block_height != marker.height
        || manifest.state_ref.consensus_identity.network != marker.network
        || manifest.state_ref.stable_block_hash != marker.btc_block_hash
    {
        return Err("Core completion marker does not match artifact manifest".to_string());
    }
    verify_marker_signature_layout(
        artifact_dir,
        &manifest_path,
        marker.signature_file.as_deref(),
        manifest.signing_key_id.is_some(),
    )?;
    Ok(marker)
}

pub(crate) fn verify_published_registry(
    artifact_dir: &Path,
    core_manifest: &CoreSnapshotManifest,
) -> Result<ScriptRegistryCompleteMarker, String> {
    let marker = verify_published_registry_marker(artifact_dir, core_manifest)?;
    let verified = verify_registry_files(
        &safe_artifact_file(artifact_dir, &marker.registry_file)?,
        &safe_artifact_file(artifact_dir, &marker.manifest_file)?,
        core_manifest,
    )?;
    let rebuilt = build_registry_complete_marker(&verified)?;
    if !same_registry_marker_identity(&marker, &rebuilt) {
        return Err(format!(
            "Registry completion marker does not match verified artifact {}",
            artifact_dir.display()
        ));
    }
    Ok(marker)
}

pub(crate) fn verify_published_registry_marker(
    artifact_dir: &Path,
    core_manifest: &CoreSnapshotManifest,
) -> Result<ScriptRegistryCompleteMarker, String> {
    let marker_path = artifact_dir.join("complete.json");
    let marker: ScriptRegistryCompleteMarker = load_json(&marker_path)?.ok_or_else(|| {
        format!(
            "Script-registry artifact is missing completion marker {}",
            marker_path.display()
        )
    })?;
    verify_marker_header(
        marker.version,
        marker.height,
        &marker.network,
        &marker.btc_block_hash,
        &core_manifest.db_identity.btc_network,
        core_manifest.state_ref.block_height,
        Some(&core_manifest.state_ref.stable_block_hash),
        "Script-registry",
    )?;
    if marker.core_snapshot_id != core_manifest.core_snapshot_id {
        return Err("Registry completion marker targets a different core snapshot".to_string());
    }
    let db_path = safe_artifact_file(artifact_dir, &marker.registry_file)?;
    let manifest_path = safe_artifact_file(artifact_dir, &marker.manifest_file)?;
    require_file(&db_path, "Script-registry DB")?;
    require_file(&manifest_path, "Script-registry manifest")?;
    let manifest = ScriptRegistryManifest::load(&manifest_path)?;
    manifest.validate_against_core(core_manifest)?;
    if manifest.file_name != marker.registry_file
        || manifest.file_sha256 != marker.file_sha256
        || manifest.registry_artifact_id != marker.registry_artifact_id
        || manifest.entry_count != marker.entry_count
    {
        return Err("Registry completion marker does not match artifact manifest".to_string());
    }
    verify_marker_signature_layout(
        artifact_dir,
        &manifest_path,
        marker.signature_file.as_deref(),
        manifest.signing_key_id.is_some(),
    )?;
    Ok(marker)
}

fn verify_file_hash(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let actual = SnapshotHash::calc_hash(path)?;
    if actual != expected {
        return Err(format!(
            "{label} file hash mismatch: manifest={expected}, actual={actual}"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_common_identity(
    db_identity: &BalanceHistoryDBIdentity,
    network: &str,
    height: u32,
    block_hash: &str,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
    label: &str,
) -> Result<(), String> {
    if network != expected_network || height != expected_height {
        return Err(format!(
            "{label} identity mismatch: expected network={expected_network} height={expected_height}, got network={network} height={height}"
        ));
    }
    let expected_identity = BalanceHistoryDBIdentity::for_network_name(expected_network)?;
    if db_identity != &expected_identity {
        return Err(format!(
            "{label} DB identity mismatch: expected {expected_identity:?}, got {db_identity:?}"
        ));
    }
    if let Some(expected_block_hash) = expected_block_hash
        && block_hash != expected_block_hash
    {
        return Err(format!(
            "{label} BTC block hash mismatch: expected {expected_block_hash}, got {block_hash}"
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_marker_header(
    version: u32,
    height: u32,
    network: &str,
    block_hash: &str,
    expected_network: &str,
    expected_height: u32,
    expected_block_hash: Option<&str>,
    label: &str,
) -> Result<(), String> {
    if version != COMPLETE_MARKER_VERSION {
        return Err(format!(
            "Unsupported {label} completion marker version {version}"
        ));
    }
    if height != expected_height || network != expected_network {
        return Err(format!(
            "{label} completion marker identity mismatch: expected network={expected_network} height={expected_height}, got network={network} height={height}"
        ));
    }
    if let Some(expected) = expected_block_hash
        && block_hash != expected
    {
        return Err(format!(
            "{label} completion marker block hash mismatch: expected {expected}, got {block_hash}"
        ));
    }
    Ok(())
}

fn verify_file_name(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    let actual = file_name(path)?;
    if actual != expected {
        return Err(format!(
            "{label} manifest file name mismatch: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

fn verify_signature_layout(
    manifest_path: &Path,
    signed: bool,
    label: &str,
) -> Result<Option<PathBuf>, String> {
    let signature_path = signature_path_for_manifest_file(manifest_path);
    match (signed, signature_path.is_file()) {
        (true, true) => Ok(Some(signature_path)),
        (true, false) => Err(format!(
            "Signed {label} is missing signature file {}",
            signature_path.display()
        )),
        (false, false) => Ok(None),
        (false, true) => Err(format!(
            "Unsigned {label} has a stale signature file {}",
            signature_path.display()
        )),
    }
}

fn verify_marker_signature_layout(
    artifact_dir: &Path,
    manifest_path: &Path,
    marker_signature_file: Option<&str>,
    signed: bool,
) -> Result<(), String> {
    let expected = signature_path_for_manifest_file(manifest_path);
    let marker_path = marker_signature_file
        .map(|name| safe_artifact_file(artifact_dir, name))
        .transpose()?;
    match (signed, marker_path) {
        (true, Some(path)) if path == expected && path.is_file() => Ok(()),
        (false, None) if !expected.exists() => Ok(()),
        _ => Err("Completion marker signature layout does not match manifest".to_string()),
    }
}

fn same_core_marker_identity(a: &SnapshotCompleteMarker, b: &SnapshotCompleteMarker) -> bool {
    a.height == b.height
        && a.network == b.network
        && a.btc_block_hash == b.btc_block_hash
        && a.snapshot_id == b.snapshot_id
        && a.core_artifact_id == b.core_artifact_id
        && a.snapshot_file == b.snapshot_file
        && a.manifest_file == b.manifest_file
        && a.signature_file == b.signature_file
        && a.file_sha256 == b.file_sha256
        && a.balance_history_count == b.balance_history_count
        && a.utxo_count == b.utxo_count
        && a.block_commit_count == b.block_commit_count
}

fn same_registry_marker_identity(
    a: &ScriptRegistryCompleteMarker,
    b: &ScriptRegistryCompleteMarker,
) -> bool {
    a.height == b.height
        && a.network == b.network
        && a.btc_block_hash == b.btc_block_hash
        && a.core_snapshot_id == b.core_snapshot_id
        && a.registry_artifact_id == b.registry_artifact_id
        && a.registry_file == b.registry_file
        && a.manifest_file == b.manifest_file
        && a.signature_file == b.signature_file
        && a.file_sha256 == b.file_sha256
        && a.entry_count == b.entry_count
}

fn require_file(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{label} does not exist: {}", path.display()));
    }
    Ok(())
}

fn optional_file_name(path: Option<&PathBuf>) -> Result<Option<String>, String> {
    path.map(|path| file_name(path)).transpose()
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("Invalid artifact file name: {}", path.display()))
}

fn safe_artifact_file(artifact_dir: &Path, file_name: &str) -> Result<PathBuf, String> {
    let path = Path::new(file_name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "Completion marker contains unsafe artifact file name {file_name}"
        ));
    }
    Ok(artifact_dir.join(path))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}
