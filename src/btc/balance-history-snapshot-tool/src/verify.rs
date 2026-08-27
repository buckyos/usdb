use crate::{
    COMPLETE_MARKER_VERSION, SnapshotCompleteMarker, SnapshotVerificationPhase, load_json,
    unix_timestamp,
};
use balance_history::{
    BalanceHistoryDBIdentity, SnapshotDB, SnapshotHash, SnapshotManifest,
    signature_path_for_manifest_file,
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
    pub manifest: SnapshotManifest,
    pub balance_history_count: u64,
    pub utxo_count: u64,
    pub block_commit_count: u64,
    pub script_registry_count: u64,
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
    if !db_path.is_file() {
        return Err(format!(
            "Snapshot DB file does not exist: {}",
            db_path.display()
        ));
    }
    if !manifest_path.is_file() {
        return Err(format!(
            "Snapshot manifest file does not exist: {}",
            manifest_path.display()
        ));
    }

    let manifest = SnapshotManifest::load(manifest_path)?;
    let actual_name = db_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("Invalid snapshot DB file name: {}", db_path.display()))?;
    if manifest.file_name != actual_name {
        return Err(format!(
            "Snapshot manifest file name mismatch: expected {}, got {}",
            manifest.file_name, actual_name
        ));
    }

    let started = begin_phase(SnapshotVerificationPhase::FileHash, db_path, &mut on_phase)?;
    let actual_hash = SnapshotHash::calc_hash(db_path)?;
    finish_phase(SnapshotVerificationPhase::FileHash, db_path, started);
    if !actual_hash.eq_ignore_ascii_case(&manifest.file_sha256) {
        return Err(format!(
            "Snapshot file hash mismatch: manifest={}, actual={}",
            manifest.file_sha256, actual_hash
        ));
    }
    if manifest.state_ref.block_height != expected_height {
        return Err(format!(
            "Snapshot height mismatch: expected {}, got {}",
            expected_height, manifest.state_ref.block_height
        ));
    }
    if manifest.state_ref.consensus_identity.network != expected_network {
        return Err(format!(
            "Snapshot network mismatch: expected {}, got {}",
            expected_network, manifest.state_ref.consensus_identity.network
        ));
    }
    let expected_identity = BalanceHistoryDBIdentity::for_network_name(expected_network)?;
    if manifest.db_identity != expected_identity {
        return Err(format!(
            "Snapshot manifest DB identity mismatch: expected {:?}, got {:?}",
            expected_identity, manifest.db_identity
        ));
    }
    if let Some(expected_block_hash) = expected_block_hash
        && !manifest
            .state_ref
            .stable_block_hash
            .eq_ignore_ascii_case(expected_block_hash)
    {
        return Err(format!(
            "Snapshot BTC block hash mismatch: expected {}, got {}",
            expected_block_hash, manifest.state_ref.stable_block_hash
        ));
    }

    let snapshot_db =
        SnapshotDB::open_read_only_for_verification(db_path, SNAPSHOT_VERIFICATION_CACHE_SIZE_KIB)?;
    let started = begin_phase(
        SnapshotVerificationPhase::IntegrityCheck,
        db_path,
        &mut on_phase,
    )?;
    snapshot_db.verify_integrity()?;
    finish_phase(SnapshotVerificationPhase::IntegrityCheck, db_path, started);
    let meta = snapshot_db.get_meta()?;
    if meta.block_height != expected_height {
        return Err(format!(
            "Snapshot metadata height mismatch: expected {}, got {}",
            expected_height, meta.block_height
        ));
    }
    if meta.db_identity != manifest.db_identity {
        return Err(format!(
            "Snapshot metadata DB identity mismatch: manifest {:?}, DB {:?}",
            manifest.db_identity, meta.db_identity
        ));
    }

    let started = begin_phase(
        SnapshotVerificationPhase::BalanceHistoryCount,
        db_path,
        &mut on_phase,
    )?;
    let balance_history_count = snapshot_db.stat_balance_history_entries_count()?;
    finish_phase(
        SnapshotVerificationPhase::BalanceHistoryCount,
        db_path,
        started,
    );

    let started = begin_phase(SnapshotVerificationPhase::UtxoCount, db_path, &mut on_phase)?;
    let utxo_count = snapshot_db.stat_utxo_entries_count()?;
    finish_phase(SnapshotVerificationPhase::UtxoCount, db_path, started);

    let started = begin_phase(
        SnapshotVerificationPhase::BlockCommitCount,
        db_path,
        &mut on_phase,
    )?;
    let block_commit_count = snapshot_db.stat_block_commit_entries_count()?;
    finish_phase(
        SnapshotVerificationPhase::BlockCommitCount,
        db_path,
        started,
    );

    let started = begin_phase(
        SnapshotVerificationPhase::ScriptRegistryCount,
        db_path,
        &mut on_phase,
    )?;
    let script_registry_count = snapshot_db.stat_script_registry_entries_count()?;
    finish_phase(
        SnapshotVerificationPhase::ScriptRegistryCount,
        db_path,
        started,
    );
    let actual_counts = (
        balance_history_count,
        utxo_count,
        block_commit_count,
        script_registry_count,
    );
    let metadata_counts = (
        meta.balance_history_count,
        meta.utxo_count,
        meta.block_commit_count,
        meta.script_registry_count,
    );
    if actual_counts != metadata_counts {
        return Err(format!(
            "Snapshot metadata count mismatch: metadata={:?}, actual={:?}",
            metadata_counts, actual_counts
        ));
    }

    let started = begin_phase(
        SnapshotVerificationPhase::CommitIdentity,
        db_path,
        &mut on_phase,
    )?;
    let commits = if expected_height == 0 {
        snapshot_db.get_block_commit_entries(1, None)?
    } else {
        snapshot_db.get_block_commit_entries(1, Some(expected_height - 1))?
    };
    let latest_commit = commits.first().ok_or_else(|| {
        format!(
            "Snapshot at height {} does not contain a block commit",
            expected_height
        )
    })?;
    if latest_commit.block_height != expected_height {
        return Err(format!(
            "Snapshot latest block commit height mismatch: expected {}, got {}",
            expected_height, latest_commit.block_height
        ));
    }
    let latest_block_hash = format!("{:x}", latest_commit.btc_block_hash);
    if !latest_block_hash.eq_ignore_ascii_case(&manifest.state_ref.stable_block_hash) {
        return Err(format!(
            "Snapshot latest block commit hash mismatch: manifest={}, DB={}",
            manifest.state_ref.stable_block_hash, latest_block_hash
        ));
    }
    let latest_commit_hex = encode_hex(&latest_commit.block_commit);
    if latest_commit_hex != manifest.state_ref.latest_block_commit {
        return Err(format!(
            "Snapshot latest block commit value mismatch: manifest={}, DB={}",
            manifest.state_ref.latest_block_commit, latest_commit_hex
        ));
    }

    let expected_signature_path = signature_path_for_manifest_file(manifest_path);
    let signature_path = if manifest.signing_key_id.is_some() {
        if !expected_signature_path.is_file() {
            return Err(format!(
                "Signed snapshot is missing signature file {}",
                expected_signature_path.display()
            ));
        }
        Some(expected_signature_path)
    } else {
        if expected_signature_path.exists() {
            return Err(format!(
                "Unsigned snapshot has a stale signature file {}",
                expected_signature_path.display()
            ));
        }
        None
    };
    finish_phase(SnapshotVerificationPhase::CommitIdentity, db_path, started);

    Ok(VerifiedSnapshot {
        db_path: db_path.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        signature_path,
        manifest,
        balance_history_count,
        utxo_count,
        block_commit_count,
        script_registry_count,
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
        snapshot_id: verified.manifest.state_ref.snapshot_id.clone(),
        snapshot_file: file_name(&verified.db_path)?,
        manifest_file: file_name(&verified.manifest_path)?,
        signature_file: verified
            .signature_path
            .as_ref()
            .map(|path| file_name(path))
            .transpose()?,
        file_sha256: verified.manifest.file_sha256.clone(),
        balance_history_count: verified.balance_history_count,
        utxo_count: verified.utxo_count,
        block_commit_count: verified.block_commit_count,
        script_registry_count: verified.script_registry_count,
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
    let db_path = safe_artifact_file(artifact_dir, &marker.snapshot_file)?;
    let manifest_path = safe_artifact_file(artifact_dir, &marker.manifest_file)?;
    let verified = verify_snapshot_files(
        &db_path,
        &manifest_path,
        expected_network,
        expected_height,
        Some(&marker.btc_block_hash),
    )?;
    let rebuilt = build_complete_marker(&verified, expected_network)?;
    if marker.snapshot_id != rebuilt.snapshot_id
        || marker.file_sha256 != rebuilt.file_sha256
        || marker.balance_history_count != rebuilt.balance_history_count
        || marker.utxo_count != rebuilt.utxo_count
        || marker.block_commit_count != rebuilt.block_commit_count
        || marker.script_registry_count != rebuilt.script_registry_count
        || marker.signature_file != rebuilt.signature_file
    {
        return Err(format!(
            "Snapshot completion marker does not match verified artifact {}",
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
            "Snapshot artifact is missing completion marker {}",
            marker_path.display()
        )
    })?;
    if marker.version != COMPLETE_MARKER_VERSION {
        return Err(format!(
            "Unsupported snapshot completion marker version {}",
            marker.version
        ));
    }
    if marker.height != expected_height || marker.network != expected_network {
        return Err(format!(
            "Snapshot completion marker identity mismatch: expected network={} height={}, got network={} height={}",
            expected_network, expected_height, marker.network, marker.height
        ));
    }
    if let Some(expected_block_hash) = expected_block_hash
        && !marker
            .btc_block_hash
            .eq_ignore_ascii_case(expected_block_hash)
    {
        return Err(format!(
            "Snapshot completion marker block hash mismatch: expected {}, got {}",
            expected_block_hash, marker.btc_block_hash
        ));
    }

    let db_path = safe_artifact_file(artifact_dir, &marker.snapshot_file)?;
    let manifest_path = safe_artifact_file(artifact_dir, &marker.manifest_file)?;
    if !db_path.is_file() {
        return Err(format!(
            "Snapshot completion marker references missing DB file {}",
            db_path.display()
        ));
    }
    let marker_signature_path = marker
        .signature_file
        .as_deref()
        .map(|signature_file| safe_artifact_file(artifact_dir, signature_file))
        .transpose()?;
    let manifest = SnapshotManifest::load(&manifest_path)?;
    let db_file_name = file_name(&db_path)?;
    if manifest.file_name != db_file_name
        || manifest.file_sha256 != marker.file_sha256
        || manifest.state_ref.block_height != marker.height
        || manifest.state_ref.consensus_identity.network != marker.network
        || !manifest
            .state_ref
            .stable_block_hash
            .eq_ignore_ascii_case(&marker.btc_block_hash)
        || manifest.state_ref.snapshot_id != marker.snapshot_id
    {
        return Err(format!(
            "Snapshot completion marker does not match artifact manifest {}",
            artifact_dir.display()
        ));
    }
    let expected_signature_path = signature_path_for_manifest_file(&manifest_path);
    match (
        manifest.signing_key_id.is_some(),
        marker_signature_path.as_ref(),
    ) {
        (true, Some(signature_path)) if *signature_path == expected_signature_path => {}
        (false, None) if !expected_signature_path.exists() => {}
        _ => {
            return Err(format!(
                "Snapshot completion marker signature layout does not match manifest {}",
                artifact_dir.display()
            ));
        }
    }
    if let Some(signature_path) = marker_signature_path
        && !signature_path.is_file()
    {
        return Err(format!(
            "Snapshot completion marker references missing signature file {}",
            signature_path.display()
        ));
    }
    Ok(marker)
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
            "Snapshot completion marker contains unsafe artifact file name {}",
            file_name
        ));
    }
    Ok(artifact_dir.join(path))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}
