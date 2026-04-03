use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use usdb_util::{BALANCE_HISTORY_SERVICE_HTTP_PORT, BTCConfig, ElectrsConfig, OrdConfig};

fn default_batch_size() -> usize {
    128
}

fn get_cache_size() -> usize {
    let available_memory = usdb_util::get_smart_memory_limit();
    info!("Available memory: {} bytes", available_memory);

    // Leave 8 GB free
    let cache_size = available_memory.saturating_sub(1024 * 1024 * 1024 * 8);
    info!("Calculated cache size: {} bytes", cache_size);

    cache_size as usize
}

// 1/4 of total cache size, at least 1 GB
fn default_utxo_cache_bytes() -> usize {
    let size = get_cache_size() / 4;
    size.max(1024 * 1024 * 1024)
}

// 3/4 of total cache size, at least 3 GB
fn default_balance_cache_bytes() -> usize {
    let size = get_cache_size() * 3 / 4;
    size.max(3 * 1024 * 1024 * 1024)
}

// When memory percent is not specified, default to 90%
// That is when used memory percent is up to 90%, we will start shrinking caches
fn default_max_memory_percent() -> usize {
    90
}

fn default_local_loader_threshold() -> usize {
    500
}

// Keep a hot undo window for common BTC reorg recovery.
fn default_undo_retention_blocks() -> u32 {
    64
}

// Throttle undo cleanup so batch catch-up does not prune on every block.
fn default_undo_cleanup_interval_blocks() -> u32 {
    16
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexConfig {
    /// Threshold of blocks behind to switch to LocalLoader client
    #[serde(default = "default_local_loader_threshold")]
    pub local_loader_threshold: usize,

    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    // UTXO cache size in bytes in memory
    #[serde(default = "default_utxo_cache_bytes")]
    pub utxo_max_cache_bytes: usize,

    // Balance cache size in bytes in memory
    #[serde(default = "default_balance_cache_bytes")]
    pub balance_max_cache_bytes: usize,

    // Maximum percent of system memory to use for caches
    // Value can be 10-100
    #[serde(default = "default_max_memory_percent")]
    pub max_memory_percent: usize,

    #[serde(default = "default_max_sync_block_height")]
    pub max_sync_block_height: u32,

    /// Number of recent committed blocks whose undo journal is retained for rollback.
    #[serde(default = "default_undo_retention_blocks")]
    pub undo_retention_blocks: u32,

    /// Block interval used to trigger low-frequency undo journal pruning.
    #[serde(default = "default_undo_cleanup_interval_blocks")]
    pub undo_cleanup_interval_blocks: u32,
}

// By default, no limit on max sync block height
// But if we need to create snapshot at some specific height, we can set this value
fn default_max_sync_block_height() -> u32 {
    u32::MAX
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            local_loader_threshold: default_local_loader_threshold(),
            batch_size: default_batch_size(),
            utxo_max_cache_bytes: default_utxo_cache_bytes(),
            balance_max_cache_bytes: default_balance_cache_bytes(),
            max_memory_percent: default_max_memory_percent(),
            max_sync_block_height: default_max_sync_block_height(),
            undo_retention_blocks: default_undo_retention_blocks(),
            undo_cleanup_interval_blocks: default_undo_cleanup_interval_blocks(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcServer {
    #[serde(default = "default_rpc_host")]
    pub host: String,

    #[serde(default = "default_rpc_port")]
    pub port: u16,
}

fn default_rpc_host() -> String {
    "127.0.0.1".to_string()
}

fn default_rpc_port() -> u16 {
    BALANCE_HISTORY_SERVICE_HTTP_PORT
}

/// Trust policy applied when installing snapshot sidecars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotTrustMode {
    /// Allow snapshot installs without manifest or detached signature checks.
    Dev,
    /// Require a manifest-backed staged state-ref validation, but not a signature.
    Manifest,
    /// Require both manifest-backed staged validation and a trusted detached signature.
    Signed,
}

fn default_snapshot_trust_mode() -> SnapshotTrustMode {
    SnapshotTrustMode::Dev
}

/// Snapshot signing and trust configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotConfig {
    /// Trust mode enforced by snapshot install.
    #[serde(default = "default_snapshot_trust_mode")]
    pub trust_mode: SnapshotTrustMode,
    /// Optional Ed25519 signing-key file used when creating snapshot manifests.
    #[serde(default)]
    pub signing_key_file: Option<PathBuf>,
    /// Optional trusted public-key set used when verifying detached signatures.
    #[serde(default)]
    pub trusted_keys_file: Option<PathBuf>,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            trust_mode: default_snapshot_trust_mode(),
            signing_key_file: None,
            trusted_keys_file: None,
        }
    }
}

impl Default for RpcServer {
    fn default() -> Self {
        RpcServer {
            host: default_rpc_host(),
            port: default_rpc_port(),
        }
    }
}

fn get_default_root_dir() -> PathBuf {
    let root_dir = usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    root_dir
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceHistoryConfig {
    #[serde(default = "get_default_root_dir")]
    pub root_dir: PathBuf,

    pub btc: BTCConfig,
    pub ordinals: OrdConfig,
    pub electrs: ElectrsConfig,

    pub sync: IndexConfig,
    pub rpc_server: RpcServer,
    #[serde(default)]
    pub snapshot: SnapshotConfig,
}

impl Default for BalanceHistoryConfig {
    fn default() -> Self {
        Self {
            root_dir: get_default_root_dir(),
            btc: BTCConfig::default(),
            ordinals: OrdConfig::default(),
            electrs: ElectrsConfig::default(),
            sync: IndexConfig::default(),
            rpc_server: RpcServer::default(),
            snapshot: SnapshotConfig::default(),
        }
    }
}

impl BalanceHistoryConfig {
    pub fn load(root_dir: &Path) -> Result<Self, String> {
        let path = root_dir.join("config.toml");
        if !path.exists() {
            let mut default_config = BalanceHistoryConfig::default();
            default_config.root_dir = root_dir.to_path_buf();
            info!(
                "Config file {} does not exist. Using default configuration.",
                path.display()
            );
            info!(
                "Default config: {}",
                toml::to_string_pretty(&default_config).unwrap()
            );
            Ok(default_config)
        } else {
            info!("Loading config from {}", path.display());
            let config_data = std::fs::read_to_string(&path).map_err(|e| {
                let msg = format!("Failed to read config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;
            info!("Config data: {}", config_data);

            let config: BalanceHistoryConfig = toml::from_str(&config_data).map_err(|e| {
                let msg = format!("Failed to parse config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;

            Ok(config)
        }
    }

    pub fn db_dir(&self) -> PathBuf {
        self.root_dir.join("db")
    }

    pub fn snapshot_dir(&self) -> PathBuf {
        self.root_dir.join("snapshots")
    }

    /// Resolves a service-local path against `root_dir` when the input is relative.
    pub fn resolve_service_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root_dir.join(path)
        }
    }

    /// Returns the configured snapshot signing-key path, if any.
    pub fn snapshot_signing_key_path(&self) -> Option<PathBuf> {
        self.snapshot
            .signing_key_file
            .as_deref()
            .map(|path| self.resolve_service_path(path))
    }

    /// Returns the configured trusted snapshot key-set path, if any.
    pub fn snapshot_trusted_keys_path(&self) -> Option<PathBuf> {
        self.snapshot
            .trusted_keys_file
            .as_deref()
            .map(|path| self.resolve_service_path(path))
    }
}

pub type BalanceHistoryConfigRef = Arc<BalanceHistoryConfig>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("balance_history_cfg_{}_{}", tag, nanos));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn test_load_missing_config_uses_given_root_dir() {
        let root = temp_root("missing_cfg");
        let cfg = BalanceHistoryConfig::load(&root).unwrap();
        assert_eq!(cfg.root_dir, root);

        std::fs::remove_dir_all(&cfg.root_dir).unwrap();
    }
}
