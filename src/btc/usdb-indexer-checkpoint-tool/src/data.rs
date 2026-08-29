use crate::{IndexerCheckpointManifest, IndexerCheckpointStateIdentity};
use bitcoincore_rpc::bitcoin::Network;
use rocksdb::{DB, Options};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use rust_rocksdb::{self as rocksdb};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use usdb_util::{
    BTCConfig, LocalStateActiveBalanceSnapshot, LocalStateCommitIdentity,
    LocalStatePassCommitIdentity, SystemStateIdentity, VersionFamily, build_local_state_commit,
    build_system_state_id, embedded_btc_activation_registry_catalog,
};

const MINER_PASS_DB_FILE: &str = "miner_pass.db";
const ENERGY_DB_DIR: &str = "energy";
const PASS_ENERGY_CF: &str = "pass_energy";
const META_CF: &str = "meta";
const ENERGY_SYNCED_HEIGHT_KEY: &[u8] = b"synced_block_height";
const ENERGY_PENDING_HEIGHT_KEY: &[u8] = b"pending_block_height";
const PASS_SYNCED_HEIGHT_KEY: &str = "btc_synced_block_height";
const PASS_RECOVERY_PENDING_KEY: &str = "upstream_reorg_recovery_pending_height";

#[derive(Debug, Deserialize)]
struct DiskUsdbConfig {
    genesis_block_height: u32,
}

#[derive(Debug, Deserialize)]
struct DiskIndexerConfig {
    #[serde(default)]
    isolate: Option<String>,
    bitcoin: BTCConfig,
    usdb: DiskUsdbConfig,
}

/// Resolved immutable data location and consensus inputs from an indexer service root.
#[derive(Clone, Debug)]
pub struct IndexerDiskLayout {
    /// Actual data directory after applying the optional isolate namespace.
    pub data_dir: PathBuf,
    /// Bitcoin network selected by the indexer configuration.
    pub bitcoin_network: Network,
    /// First BTC height interpreted by the USDB indexer.
    pub genesis_block_height: u32,
}

impl IndexerDiskLayout {
    /// Loads the indexer configuration without creating or mutating runtime directories.
    pub fn load(root_dir: &Path) -> Result<Self, String> {
        let config_path = root_dir.join("config.json");
        let data = std::fs::read(&config_path).map_err(|error| {
            format!(
                "Failed to read indexer config {}: {error}",
                config_path.display()
            )
        })?;
        let config: DiskIndexerConfig = serde_json::from_slice(&data).map_err(|error| {
            format!(
                "Failed to parse indexer config {}: {error}",
                config_path.display()
            )
        })?;
        let data_dir = match &config.isolate {
            Some(isolate) => root_dir.join(isolate).join("data"),
            None => root_dir.join("data"),
        };
        Ok(Self {
            data_dir,
            bitcoin_network: config.bitcoin.network(),
            genesis_block_height: config.usdb.genesis_block_height,
        })
    }
}

#[derive(Debug)]
struct PassCommitRow {
    block_height: u32,
    block_commit: String,
    commit_protocol_version: String,
    commit_hash_algo: String,
}

#[derive(Debug)]
struct SnapshotAnchorRow {
    stable_block_hash: String,
    latest_block_commit: String,
    stable_lag: u32,
}

/// Opens a frozen indexer data directory read-only and recomputes its checkpoint identity.
pub fn validate_indexer_data(
    layout: &IndexerDiskLayout,
    manifest: &IndexerCheckpointManifest,
) -> Result<IndexerCheckpointStateIdentity, String> {
    if manifest.checkpoint_height != manifest.state_identity.block_height {
        return Err("Checkpoint height and normalized state identity disagree".into());
    }
    if layout.bitcoin_network.to_string() != manifest.btc_network {
        return Err(format!(
            "Indexer Bitcoin network mismatch: config={}, manifest={}",
            layout.bitcoin_network, manifest.btc_network
        ));
    }
    if layout.genesis_block_height != manifest.index_origin_height {
        return Err(format!(
            "Indexer origin mismatch: config={}, manifest={}",
            layout.genesis_block_height, manifest.index_origin_height
        ));
    }

    let db_path = layout.data_dir.join(MINER_PASS_DB_FILE);
    if !db_path.is_file() {
        return Err(format!(
            "Indexer pass database is missing: {}",
            db_path.display()
        ));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&db_path, flags).map_err(|error| {
        format!(
            "Failed to open indexer pass database read-only {}: {error}",
            db_path.display()
        )
    })?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check;", [], |row| row.get(0))
        .map_err(|error| format!("Failed to run indexer SQLite integrity check: {error}"))?;
    if integrity != "ok" {
        return Err(format!(
            "Indexer SQLite integrity check failed: {integrity}"
        ));
    }

    let height = manifest.checkpoint_height;
    let pass_height =
        numeric_state(&conn, PASS_SYNCED_HEIGHT_KEY)?.and_then(|value| u32::try_from(value).ok());
    if pass_height != Some(height) {
        return Err(format!(
            "Indexer pass synced height mismatch: expected {height}, got {pass_height:?}"
        ));
    }
    if numeric_state(&conn, PASS_RECOVERY_PENDING_KEY)?.is_some() {
        return Err("Indexer checkpoint cannot contain pending upstream reorg recovery".into());
    }

    let anchor = load_snapshot_anchor(&conn, height)?;
    if anchor.stable_block_hash != manifest.state_identity.stable_block_hash
        || anchor.latest_block_commit != manifest.state_identity.latest_block_commit
        || anchor.stable_lag
            != manifest
                .balance_history
                .state_ref
                .consensus_identity
                .stable_lag
    {
        return Err(format!(
            "Indexer stored upstream anchor does not match paired manifest at height {height}"
        ));
    }

    let latest_pass_commit =
        load_latest_pass_commit(&conn, height)?.map(|row| LocalStatePassCommitIdentity {
            block_height: row.block_height,
            block_commit: row.block_commit,
            commit_protocol_version: row.commit_protocol_version,
            commit_hash_algo: row.commit_hash_algo,
        });
    let active_balance = load_active_balance_snapshot(&conn, height, layout.genesis_block_height)?;

    validate_energy_store(&layout.data_dir.join(ENERGY_DB_DIR), height)?;

    let catalog = embedded_btc_activation_registry_catalog(layout.bitcoin_network)
        .map_err(|error| format!("Failed to load embedded BTC activation catalog: {error}"))?;
    let registry = catalog
        .registry_by_id(&manifest.state_identity.activation_registry_id)
        .map_err(|error| format!("Checkpoint activation registry is unavailable: {error}"))?;
    let active_versions = registry
        .lookup_active_version_set(height)
        .map_err(|error| format!("Failed to resolve checkpoint active versions: {error}"))?;
    active_versions
        .validate_btc_indexer_v1()
        .map_err(|error| format!("Unsupported checkpoint active versions: {error}"))?;
    let active_version_set_id = active_versions.active_version_set_id();
    let commit_protocol_version = active_versions
        .require_string(VersionFamily::CommitProtocolVersion)
        .map_err(|error| format!("Checkpoint commit protocol is unavailable: {error}"))?
        .to_string();

    let local_identity = LocalStateCommitIdentity {
        commit_protocol_version,
        upstream_snapshot_id: manifest.state_identity.snapshot_id.clone(),
        active_version_set_id: active_version_set_id.clone(),
        local_synced_block_height: height,
        latest_pass_block_commit: latest_pass_commit,
        latest_active_balance_snapshot: active_balance,
    };
    let local_state_commit = build_local_state_commit(&local_identity);
    let system_state_id = build_system_state_id(&SystemStateIdentity {
        upstream_snapshot_id: manifest.state_identity.snapshot_id.clone(),
        local_state_commit: local_state_commit.clone(),
    });
    let rebuilt = IndexerCheckpointStateIdentity {
        block_height: height,
        stable_block_hash: anchor.stable_block_hash,
        latest_block_commit: anchor.latest_block_commit,
        snapshot_id: manifest.state_identity.snapshot_id.clone(),
        activation_registry_id: manifest.state_identity.activation_registry_id.clone(),
        active_version_set_id,
        local_state_commit,
        system_state_id,
    };
    if rebuilt != manifest.state_identity {
        return Err(format!(
            "Offline indexer state-ref recomputation mismatch: expected={:?}, actual={:?}",
            manifest.state_identity, rebuilt
        ));
    }
    Ok(rebuilt)
}

fn numeric_state(conn: &Connection, name: &str) -> Result<Option<i64>, String> {
    conn.query_row("SELECT value FROM state WHERE name = ?1", [name], |row| {
        row.get(0)
    })
    .optional()
    .map_err(|error| format!("Failed to read indexer numeric state {name}: {error}"))
}

fn load_snapshot_anchor(conn: &Connection, height: u32) -> Result<SnapshotAnchorRow, String> {
    conn.query_row(
        "SELECT stable_block_hash, latest_block_commit, stable_lag \
         FROM balance_history_snapshot_history WHERE block_height = ?1",
        [i64::from(height)],
        |row| {
            let stable_lag: i64 = row.get(2)?;
            Ok((row.get(0)?, row.get(1)?, stable_lag))
        },
    )
    .optional()
    .map_err(|error| format!("Failed to read indexer upstream anchor at height {height}: {error}"))?
    .ok_or_else(|| format!("Indexer upstream anchor is missing at height {height}"))
    .and_then(|(stable_block_hash, latest_block_commit, stable_lag)| {
        Ok(SnapshotAnchorRow {
            stable_block_hash,
            latest_block_commit,
            stable_lag: u32::try_from(stable_lag)
                .map_err(|_| format!("Invalid stored stable lag {stable_lag}"))?,
        })
    })
}

fn load_latest_pass_commit(
    conn: &Connection,
    height: u32,
) -> Result<Option<PassCommitRow>, String> {
    conn.query_row(
        "SELECT block_height, block_commit, commit_protocol_version, commit_hash_algo \
         FROM pass_block_commits WHERE block_height <= ?1 \
         ORDER BY block_height DESC LIMIT 1",
        [i64::from(height)],
        |row| {
            let block_height: i64 = row.get(0)?;
            Ok((block_height, row.get(1)?, row.get(2)?, row.get(3)?))
        },
    )
    .optional()
    .map_err(|error| format!("Failed to read latest indexer pass commit: {error}"))?
    .map(
        |(block_height, block_commit, commit_protocol_version, commit_hash_algo)| {
            Ok(PassCommitRow {
                block_height: u32::try_from(block_height)
                    .map_err(|_| format!("Invalid pass commit height {block_height}"))?,
                block_commit,
                commit_protocol_version,
                commit_hash_algo,
            })
        },
    )
    .transpose()
}

fn load_active_balance_snapshot(
    conn: &Connection,
    height: u32,
    genesis_height: u32,
) -> Result<Option<LocalStateActiveBalanceSnapshot>, String> {
    let latest = conn
        .query_row(
            "SELECT block_height, total_balance, active_address_count \
             FROM active_balance_snapshots ORDER BY block_height DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("Failed to read indexer active balance snapshot: {error}"))?;

    if height < genesis_height {
        if latest.is_some() {
            return Err(format!(
                "Indexer contains an active balance snapshot before origin {genesis_height}"
            ));
        }
        return Ok(None);
    }
    let (snapshot_height, total_balance, active_address_count) = latest
        .ok_or_else(|| format!("Indexer active balance snapshot is missing at height {height}"))?;
    if snapshot_height != i64::from(height) {
        return Err(format!(
            "Indexer active balance snapshot height mismatch: expected {height}, got {snapshot_height}"
        ));
    }
    Ok(Some(LocalStateActiveBalanceSnapshot {
        block_height: height,
        total_balance: u64::try_from(total_balance)
            .map_err(|_| format!("Invalid active total balance {total_balance}"))?,
        active_address_count: u32::try_from(active_address_count)
            .map_err(|_| format!("Invalid active address count {active_address_count}"))?,
    }))
}

fn validate_energy_store(path: &Path, expected_height: u32) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!(
            "Indexer energy database is missing: {}",
            path.display()
        ));
    }
    let options = Options::default();
    let db = DB::open_cf_for_read_only(&options, path, [PASS_ENERGY_CF, META_CF], false).map_err(
        |error| {
            format!(
                "Failed to open indexer energy database read-only {}: {error}",
                path.display()
            )
        },
    )?;
    let meta = db
        .cf_handle(META_CF)
        .ok_or_else(|| "Indexer energy metadata column family is missing".to_string())?;
    let synced_height = read_u32_meta(&db, &meta, ENERGY_SYNCED_HEIGHT_KEY)?;
    if synced_height != Some(expected_height) {
        return Err(format!(
            "Indexer energy synced height mismatch: expected {expected_height}, got {synced_height:?}"
        ));
    }
    if read_u32_meta(&db, &meta, ENERGY_PENDING_HEIGHT_KEY)?.is_some() {
        return Err("Indexer checkpoint cannot contain a pending energy block".into());
    }
    Ok(())
}

fn read_u32_meta(
    db: &DB,
    cf: &impl rocksdb::AsColumnFamilyRef,
    key: &[u8],
) -> Result<Option<u32>, String> {
    let Some(bytes) = db
        .get_cf(cf, key)
        .map_err(|error| format!("Failed to read indexer energy metadata: {error}"))?
    else {
        return Ok(None);
    };
    let bytes: [u8; 4] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("Invalid indexer energy metadata length for key {key:?}"))?;
    Ok(Some(u32::from_be_bytes(bytes)))
}
