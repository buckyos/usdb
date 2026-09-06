//! Shared read-only identity checks for offline split-artifact audits.

use crate::db::{CoreSnapshotDb, CoreSnapshotMeta, ScriptRegistrySnapshotDb};
use crate::snapshot_contract::{CoreSnapshotManifest, ScriptRegistryManifest};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Concrete core artifact identity recorded in an audit report.
#[derive(Clone, Debug, Serialize)]
pub struct AuditCoreArtifact {
    /// Canonical immutable database path.
    pub file: PathBuf,
    /// Canonical manifest path.
    pub manifest_file: PathBuf,
    /// Validated manifest, including the declared file hash and state identity.
    pub manifest: CoreSnapshotManifest,
}

/// Concrete registry artifact identity recorded in reports and restart checkpoints.
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AuditRegistryArtifact {
    /// Canonical immutable database path.
    pub file: PathBuf,
    /// Canonical manifest path.
    pub manifest_file: PathBuf,
    /// Validated manifest anchored to the paired core.
    pub manifest: ScriptRegistryManifest,
}

/// Identity-checked artifacts; hash verification is explicit and never implied by metadata checks.
#[derive(Clone, Debug, Serialize)]
pub struct SplitSnapshotAudit {
    /// Required consensus-state artifact.
    pub core: AuditCoreArtifact,
    /// Optional auxiliary artifact, required when auditing scripts.
    pub script_registry: Option<AuditRegistryArtifact>,
    /// Whether this audit recomputed every supplied database hash.
    pub file_hashes_verified: bool,
    /// Core metadata counts are checked against scan counts by the full comparator.
    #[serde(skip)]
    pub core_meta: CoreSnapshotMeta,
}

impl SplitSnapshotAudit {
    /// Checks schemas, manifest identities and database anchors without a full table scan.
    /// Set `verify_file_hashes` to bind current file bytes to the declared artifact hashes.
    pub fn open(
        core_file: &Path,
        core_manifest: Option<&Path>,
        registry_file: Option<&Path>,
        registry_manifest: Option<&Path>,
        verify_file_hashes: bool,
    ) -> Result<Self, String> {
        let result = Self::open_inner(
            core_file,
            core_manifest,
            registry_file,
            registry_manifest,
            verify_file_hashes,
        );
        if let Err(error) = &result {
            log::error!(
                "Split audit input validation failed: core={}, registry={registry_file:?}, error={error}",
                core_file.display()
            );
        }
        result
    }

    fn open_inner(
        core_file: &Path,
        core_manifest: Option<&Path>,
        registry_file: Option<&Path>,
        registry_manifest: Option<&Path>,
        verify_file_hashes: bool,
    ) -> Result<Self, String> {
        if registry_file.is_none() && registry_manifest.is_some() {
            return Err("A registry manifest requires a registry database".to_string());
        }
        let core_file = canonical_file(core_file)?;
        immutable_audit_uri(&core_file)?;
        let core_manifest = canonical_file(
            &core_manifest
                .map(PathBuf::from)
                .unwrap_or_else(|| core_file.with_extension("manifest.json")),
        )?;
        let manifest = CoreSnapshotManifest::load(&core_manifest)?;
        check_basename(&core_file, &manifest.file_name)?;
        let db = CoreSnapshotDb::open_for_verification(&core_file, 8192)?;
        db.verify_schema()?;
        let meta = db.read_meta()?;
        if meta.block_height != manifest.state_ref.block_height
            || meta.db_identity != manifest.db_identity
            || meta.core_snapshot_id != manifest.core_snapshot_id
            || meta.generated_at != manifest.generated_at
        {
            return Err(
                "Core audit database metadata does not match manifest identity".to_string(),
            );
        }
        let latest = db
            .latest_block_commit()?
            .ok_or("Core audit database has no block commitment")?;
        if latest.block_height != meta.block_height
            || latest.btc_block_hash.to_string() != manifest.state_ref.stable_block_hash
            || hex_bytes(&latest.block_commit) != manifest.state_ref.latest_block_commit
        {
            return Err(
                "Core audit database latest commitment does not match manifest".to_string(),
            );
        }
        if verify_file_hashes {
            verify_hash(&core_file, &manifest.file_sha256)?;
        }
        let registry = registry_file
            .map(|file| -> Result<_, String> {
                let file = canonical_file(file)?;
                immutable_audit_uri(&file)?;
                let manifest_file = canonical_file(
                    &registry_manifest
                        .map(PathBuf::from)
                        .unwrap_or_else(|| file.with_extension("manifest.json")),
                )?;
                let registry = ScriptRegistryManifest::load(&manifest_file)?;
                registry.validate_against_core(&manifest)?;
                check_basename(&file, &registry.file_name)?;
                let db = ScriptRegistrySnapshotDb::open_for_verification(&file, 8192)?;
                db.verify_schema()?;
                let meta = db.read_meta()?;
                if meta.base != registry.base
                    || meta.entry_count != registry.entry_count
                    || meta.generated_at != registry.generated_at
                {
                    return Err(
                        "Registry audit database metadata does not match manifest identity/count"
                            .to_string(),
                    );
                }
                if verify_file_hashes {
                    verify_hash(&file, &registry.file_sha256)?;
                }
                Ok(AuditRegistryArtifact {
                    file,
                    manifest_file,
                    manifest: registry,
                })
            })
            .transpose()?;
        Ok(Self {
            core: AuditCoreArtifact {
                file: core_file,
                manifest_file: core_manifest,
                manifest,
            },
            script_registry: registry,
            file_hashes_verified: verify_file_hashes,
            core_meta: meta,
        })
    }
}

/// Encodes an immutable SQLite URI and rejects nonempty WAL files rather than ignoring their data.
pub fn immutable_audit_uri(path: &Path) -> Result<String, String> {
    let path = canonical_file(path)?;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    match std::fs::metadata(PathBuf::from(wal)) {
        Ok(meta) if meta.len() != 0 => {
            return Err(format!(
                "Audit input has a nonempty WAL; finalize/freeze it first: {}",
                path.display()
            ));
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => {
            return Err(format!(
                "Failed to inspect audit WAL for {}: {error}",
                path.display()
            ));
        }
    }
    let mut uri = url::Url::from_file_path(&path)
        .map_err(|_| format!("Invalid audit path: {}", path.display()))?;
    uri.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");
    Ok(uri.to_string())
}

/// Opens an immutable artifact with bounded SQLite cache and no sidecar writes.
pub fn open_immutable_audit_db(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        immutable_audit_uri(path)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Failed to open audit database {}: {error}", path.display()))?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA cache_size=-8192;")
        .map_err(|error| {
            format!(
                "Failed to configure audit database {}: {error}",
                path.display()
            )
        })?;
    Ok(conn)
}

fn canonical_file(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("Failed to resolve audit file {}: {error}", path.display()))?;
    if !path.is_file() {
        return Err(format!(
            "Audit input is not a regular file: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn check_basename(path: &Path, expected: &str) -> Result<(), String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(expected) {
        return Err(format!(
            "Audit manifest file_name mismatch: file={}, expected={expected}",
            path.display()
        ));
    }
    Ok(())
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    eprintln!("[snapshot-audit] hash started: file={}", path.display());
    let mut file = std::fs::File::open(path).map_err(|error| {
        format!(
            "Failed to open audit hash input {}: {error}",
            path.display()
        )
    })?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut processed = 0u64;
    let mut heartbeat = std::time::Instant::now();
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Failed to hash audit file {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        processed += count as u64;
        if heartbeat.elapsed().as_secs() >= 5 {
            eprintln!(
                "[snapshot-audit] hash progress: file={}, bytes={processed}",
                path.display()
            );
            heartbeat = std::time::Instant::now();
        }
    }
    if hex_bytes(&hash.finalize()) != expected {
        return Err(format!("Audit file SHA256 mismatch: {}", path.display()));
    }
    eprintln!(
        "[snapshot-audit] hash completed: file={}, bytes={processed}",
        path.display()
    );
    Ok(())
}
