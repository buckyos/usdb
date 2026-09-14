//! Bounded readers for exact-height RocksDB and authenticated legacy split files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use bitcoincore_rpc::bitcoin::hashes::Hash;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use usdb_util::BtcScriptHash;

use super::job::BaselineJobSource;
use super::{BaselineIdentity, BaselineSource, Result, storage};
use crate::index::{signature_path_for_manifest_file, verify_snapshot_artifact_manifest_signature};
use crate::snapshot_audit::{SplitSnapshotAudit, open_immutable_audit_db};
use crate::{
    BalanceHistoryConfig, BalanceHistoryDB, BlockCommitEntry, CoreSnapshotManifest,
    ScriptRegistryManifest,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Stamp {
    length: u64,
    modified_ns: u128,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
}

pub(super) fn stamp(path: &Path) -> Result<Stamp> {
    let value = fs::symlink_metadata(path)?;
    if !value.is_file() {
        return Err(format!("Source must be a regular file: {}", path.display()).into());
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Stamp {
        length: value.len(),
        modified_ns: value.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
        #[cfg(unix)]
        device: value.dev(),
        #[cfg(unix)]
        inode: value.ino(),
        #[cfg(unix)]
        changed_seconds: value.ctime(),
        #[cfg(unix)]
        changed_nanos: value.ctime_nsec(),
    })
}

pub(super) enum Reader {
    Rocks(BalanceHistoryDB),
    Split {
        core: Box<Connection>,
        registry: Box<Connection>,
    },
}

impl BaselineJobSource {
    pub(super) fn normalize(&self) -> Result<Self> {
        Ok(match self {
            Self::Rocksdb { root } => Self::Rocksdb {
                root: root.canonicalize()?,
            },
            Self::LegacySplit {
                core_manifest,
                registry_manifest,
            } => Self::LegacySplit {
                core_manifest: core_manifest.canonicalize()?,
                registry_manifest: registry_manifest.canonicalize()?,
            },
        })
    }

    pub(super) fn stamps(&self) -> Result<BTreeMap<PathBuf, Stamp>> {
        let paths = match self {
            Self::Rocksdb { root } => fs::read_dir(root.join("db/balance_history"))?
                .map(|entry| Ok(entry?.path()))
                .collect::<Result<Vec<_>>>()?,
            Self::LegacySplit {
                core_manifest,
                registry_manifest,
            } => {
                let core = CoreSnapshotManifest::load(core_manifest)?;
                let registry = ScriptRegistryManifest::load(registry_manifest)?;
                let core_file = adjacent(core_manifest, &core.file_name)?;
                let registry_file = adjacent(registry_manifest, &registry.file_name)?;
                for file in [&core_file, &registry_file] {
                    reject_sidecars(file)?;
                }
                vec![
                    core_manifest.clone(),
                    registry_manifest.clone(),
                    signature_path_for_manifest_file(core_manifest),
                    signature_path_for_manifest_file(registry_manifest),
                    core_file,
                    registry_file,
                ]
            }
        };
        paths
            .into_iter()
            .map(|path| Ok((path.clone(), stamp(&path)?)))
            .collect()
    }

    pub(super) fn open(
        &self,
        identity: &BaselineIdentity,
        trust: &Path,
    ) -> Result<(Reader, BaselineSource, BlockCommitEntry)> {
        match self {
            Self::Rocksdb { root } => {
                let mut config = BalanceHistoryConfig {
                    root_dir: root.clone(),
                    ..Default::default()
                };
                config.btc.network = identity.network;
                let db = BalanceHistoryDB::open_read_only(Arc::new(config))?;
                let source = db.baseline_source(identity)?;
                let commit = db
                    .get_block_commit(identity.height)?
                    .ok_or("Missing source genesis commit")?;
                if commit.btc_block_hash != identity.block_hash {
                    return Err("Source genesis block hash mismatch".into());
                }
                Ok((Reader::Rocks(db), source, commit))
            }
            Self::LegacySplit {
                core_manifest,
                registry_manifest,
            } => {
                let core = CoreSnapshotManifest::load(core_manifest)?;
                let registry = ScriptRegistryManifest::load(registry_manifest)?;
                registry.validate_against_core(&core)?;
                for (path, scheme, signer, payload) in [
                    (
                        core_manifest,
                        &core.signature_scheme,
                        &core.signing_key_id,
                        core.signature_payload()?,
                    ),
                    (
                        registry_manifest,
                        &registry.signature_scheme,
                        &registry.signing_key_id,
                        registry.signature_payload()?,
                    ),
                ] {
                    verify_snapshot_artifact_manifest_signature(
                        scheme.as_deref(),
                        signer.as_deref(),
                        path,
                        &payload,
                        trust,
                    )?;
                }
                if core.state_ref.block_height != identity.height
                    || core.state_ref.stable_block_hash != identity.block_hash.to_string()
                    || core.db_identity
                        != crate::BalanceHistoryDBIdentity::for_network(identity.network)
                {
                    return Err(
                        "Legacy split source does not match requested genesis/network".into(),
                    );
                }
                let core_file = adjacent(core_manifest, &core.file_name)?;
                let registry_file = adjacent(registry_manifest, &registry.file_name)?;
                // Recheck all signed input bytes on each data-stage resume. A saved cursor
                // never substitutes for authenticating an immutable source.
                for (path, expected) in [
                    (&core_file, &core.file_sha256),
                    (&registry_file, &registry.file_sha256),
                ] {
                    if storage::file_hash(path)? != *expected {
                        return Err("Legacy split source file hash mismatch".into());
                    }
                }
                SplitSnapshotAudit::open(
                    &core_file,
                    Some(core_manifest),
                    Some(&registry_file),
                    Some(registry_manifest),
                    false,
                )?;
                let db = crate::CoreSnapshotDb::open_for_verification(&core_file, 8192)?;
                let commit = db
                    .latest_block_commit()?
                    .ok_or("Missing legacy genesis commit")?;
                Ok((
                    Reader::Split {
                        core: Box::new(open_immutable_audit_db(&core_file)?),
                        registry: Box::new(open_immutable_audit_db(&registry_file)?),
                    },
                    BaselineSource::LegacySplit {
                        core: Box::new(core),
                        registry: Box::new(registry),
                    },
                    commit,
                ))
            }
        }
    }
}

fn adjacent(manifest: &Path, name: &str) -> Result<PathBuf> {
    if Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name) {
        return Err("Legacy artifact filename is not a basename".into());
    }
    Ok(manifest
        .parent()
        .ok_or("Manifest has no parent")?
        .join(name))
}

fn reject_sidecars(file: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = file.as_os_str().to_os_string();
        name.push(suffix);
        if fs::symlink_metadata(PathBuf::from(name)).is_ok() {
            return Err("Legacy input must not contain SQLite sidecars".into());
        }
    }
    Ok(())
}

impl Reader {
    pub(super) fn rows(
        &self,
        balances: bool,
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        match self {
            Self::Rocks(db) => db.baseline_rows(balances, after, limit),
            Self::Split { core, .. } => {
                let (column, table, fields) = if balances {
                    (
                        "script_hash",
                        "balance_history",
                        "script_hash,height,balance",
                    )
                } else {
                    ("outpoint", "utxos", "outpoint,script_hash,value")
                };
                // No nullable-OR predicate: keep every resumed page an indexed seek.
                let sql = if after.is_some() {
                    format!(
                        "SELECT {fields} FROM {table} WHERE {column} < ?1 ORDER BY {column} DESC LIMIT ?2"
                    )
                } else {
                    format!("SELECT {fields} FROM {table} ORDER BY {column} DESC LIMIT ?1")
                };
                let mut stmt = core.prepare(&sql)?;
                let mut rows = if let Some(key) = after {
                    stmt.query(params![
                        if balances { &key[..32] } else { key },
                        limit as i64
                    ])?
                } else {
                    stmt.query([limit as i64])?
                };
                let mut result = Vec::new();
                while let Some(row) = rows.next()? {
                    let mut key: Vec<u8> = row.get(0)?;
                    let mut value;
                    if balances {
                        key.extend(row.get::<_, u32>(1)?.to_be_bytes());
                        value = vec![0; 8];
                    } else {
                        value = row.get::<_, Vec<u8>>(1)?;
                    }
                    value.extend(u64::try_from(row.get::<_, i64>(2)?)?.to_be_bytes());
                    result.push((key, value));
                }
                Ok(result)
            }
        }
    }

    pub(super) fn script(&self, hash: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(match self {
            Self::Rocks(db) => db
                .get_script_registry_entry(&BtcScriptHash::from_slice(hash)?)?
                .map(|script| script.into_bytes()),
            Self::Split { registry, .. } => registry
                .query_row(
                    "SELECT script_pubkey FROM script_registry WHERE script_hash=?1",
                    [hash],
                    |row| row.get(0),
                )
                .optional()?,
        })
    }
}
