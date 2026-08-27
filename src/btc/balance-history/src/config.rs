use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use usdb_util::{BALANCE_HISTORY_SERVICE_HTTP_PORT, BTCAuth, BTCConfig, ElectrsConfig, OrdConfig};

const MIN_CACHE_BYTES: usize = 1024 * 1024;
const MIN_MEMORY_PRESSURE_HEADROOM_PERCENT: usize = 10;

/// Default fraction of the effective memory limit reserved for both in-memory caches.
pub const DEFAULT_CACHE_BUDGET_PERCENT: usize = 66;
const DEFAULT_UTXO_CACHE_SHARE_PERCENT: usize = 25;

/// Concrete UTXO and address-balance cache limits derived from one memory ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DerivedCacheLimits {
    /// Effective host or cgroup memory ceiling used for the calculation.
    pub memory_limit_bytes: u64,
    /// Percentage of the effective ceiling assigned to both caches.
    pub cache_budget_percent: usize,
    /// Combined cache budget.
    pub total_cache_bytes: u64,
    /// UTXO cache budget.
    pub utxo_cache_bytes: u64,
    /// Address-balance cache budget.
    pub balance_cache_bytes: u64,
}

/// Derives the concrete 1:3 UTXO/balance split used by balance-history.
pub fn derive_cache_limits(
    memory_limit_bytes: u64,
    cache_budget_percent: usize,
) -> Result<DerivedCacheLimits, String> {
    if memory_limit_bytes == 0 {
        return Err("Effective memory limit must be greater than zero".to_string());
    }
    if !(1..=100).contains(&cache_budget_percent) {
        return Err("Cache budget percent must be in the range 1..=100".to_string());
    }

    let total_cache_bytes = ((memory_limit_bytes as u128 * cache_budget_percent as u128) / 100)
        .try_into()
        .map_err(|_| "Derived cache budget exceeds u64".to_string())?;
    let utxo_cache_bytes =
        ((total_cache_bytes as u128 * DEFAULT_UTXO_CACHE_SHARE_PERCENT as u128) / 100) as u64;

    Ok(DerivedCacheLimits {
        memory_limit_bytes,
        cache_budget_percent,
        total_cache_bytes,
        utxo_cache_bytes,
        balance_cache_bytes: total_cache_bytes - utxo_cache_bytes,
    })
}

fn default_batch_size() -> usize {
    128
}

fn get_cache_size() -> usize {
    let available_memory = usdb_util::get_smart_memory_limit();
    info!("Available memory: {} bytes", available_memory);

    // A proportional budget leaves room for RocksDB, batch allocations, the OS, and allocator
    // overhead on both small containers and dedicated snapshot hosts.
    let cache_size = derive_cache_limits(available_memory, DEFAULT_CACHE_BUDGET_PERCENT)
        .map(|limits| limits.total_cache_bytes)
        .unwrap_or(0);
    info!("Calculated cache size: {} bytes", cache_size);

    usize::try_from(cache_size).unwrap_or(usize::MAX)
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

    // Whole-host/cgroup pressure threshold that triggers cache shrinking.
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

impl IndexConfig {
    /// Returns the combined upper bound of the UTXO and balance caches.
    pub fn cache_budget_bytes(&self) -> Result<usize, String> {
        self.utxo_max_cache_bytes
            .checked_add(self.balance_max_cache_bytes)
            .ok_or_else(|| "Combined balance-history cache budget overflows usize".to_string())
    }

    /// Validates cache limits before cache capacities are constructed.
    pub fn validate(&self) -> Result<(), String> {
        if self.batch_size == 0 || self.batch_size > u32::MAX as usize {
            return Err(format!(
                "sync.batch_size must be in the range 1..={}, got {}",
                u32::MAX,
                self.batch_size
            ));
        }
        if self.local_loader_threshold > u32::MAX as usize {
            return Err(format!(
                "sync.local_loader_threshold must not exceed {}, got {}",
                u32::MAX,
                self.local_loader_threshold
            ));
        }
        if self.undo_retention_blocks == 0 {
            return Err("sync.undo_retention_blocks must be greater than 0".to_string());
        }
        if self.undo_cleanup_interval_blocks == 0 {
            return Err("sync.undo_cleanup_interval_blocks must be greater than 0".to_string());
        }
        if self.utxo_max_cache_bytes < MIN_CACHE_BYTES {
            return Err(format!(
                "sync.utxo_max_cache_bytes must be at least {} bytes",
                MIN_CACHE_BYTES
            ));
        }
        if self.balance_max_cache_bytes < MIN_CACHE_BYTES {
            return Err(format!(
                "sync.balance_max_cache_bytes must be at least {} bytes",
                MIN_CACHE_BYTES
            ));
        }
        if !(20..=95).contains(&self.max_memory_percent) {
            return Err("sync.max_memory_percent must be in the range 20..=95".to_string());
        }
        self.cache_budget_bytes()?;
        Ok(())
    }

    /// Ensures cache capacity leaves room before the whole-process pressure threshold.
    pub fn validate_memory_budget(&self, memory_limit_bytes: u64) -> Result<(), String> {
        if memory_limit_bytes == 0 {
            return Ok(());
        }
        let cache_budget = self.cache_budget_bytes()? as u64;
        let maximum_cache_percent = self
            .max_memory_percent
            .saturating_sub(MIN_MEMORY_PRESSURE_HEADROOM_PERCENT);
        let maximum_cache_budget =
            ((memory_limit_bytes as u128 * maximum_cache_percent as u128) / 100) as u64;
        if cache_budget > maximum_cache_budget {
            return Err(format!(
                "Combined cache budget {} bytes exceeds {} bytes ({}% of effective memory limit {}); leave at least {} percentage points before the {}% memory-pressure threshold",
                cache_budget,
                maximum_cache_budget,
                maximum_cache_percent,
                memory_limit_bytes,
                MIN_MEMORY_PRESSURE_HEADROOM_PERCENT,
                self.max_memory_percent
            ));
        }
        Ok(())
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

impl RpcServer {
    fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("rpc_server.port must be greater than 0".to_string());
        }
        format!("{}:{}", self.host, self.port)
            .parse::<std::net::SocketAddr>()
            .map_err(|e| format!("Invalid rpc_server address: {}", e))?;
        Ok(())
    }
}

impl SnapshotConfig {
    fn validate(&self) -> Result<(), String> {
        for (name, path) in [
            ("snapshot.signing_key_file", self.signing_key_file.as_ref()),
            (
                "snapshot.trusted_keys_file",
                self.trusted_keys_file.as_ref(),
            ),
        ] {
            if path.is_some_and(|path| path.as_os_str().is_empty()) {
                return Err(format!("{} must not be empty", name));
            }
        }
        if self.trust_mode == SnapshotTrustMode::Signed
            && self.signing_key_file.is_none()
            && self.trusted_keys_file.is_none()
        {
            return Err(
                "snapshot.trust_mode=signed requires signing_key_file or trusted_keys_file"
                    .to_string(),
            );
        }
        Ok(())
    }
}

fn get_default_root_dir() -> PathBuf {
    usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
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
            let default_config = BalanceHistoryConfig {
                root_dir: root_dir.to_path_buf(),
                ..BalanceHistoryConfig::default()
            };
            info!(
                "Config file {} does not exist. Using default configuration.",
                path.display()
            );
            default_config.log_safe_summary("default");
            if let Err(error) = default_config.validate() {
                log::error!("Invalid default balance-history configuration: {}", error);
                return Err(error);
            }
            Ok(default_config)
        } else {
            info!("Loading config from {}", path.display());
            let config_data = std::fs::read_to_string(&path).map_err(|e| {
                let msg = format!("Failed to read config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;
            let mut config: BalanceHistoryConfig = toml::from_str(&config_data).map_err(|e| {
                let msg = format!("Failed to parse config file {}: {}", path.display(), e);
                log::error!("{}", msg);
                msg
            })?;

            if config.root_dir != root_dir {
                warn!(
                    "Ignoring config root_dir {} because the service root is fixed by startup argument {}",
                    config.root_dir.display(),
                    root_dir.display()
                );
                config.root_dir = root_dir.to_path_buf();
            }

            if let Err(error) = config.validate() {
                log::error!(
                    "Invalid balance-history configuration from {}: {}",
                    path.display(),
                    error
                );
                return Err(error);
            }
            config.log_safe_summary("file");
            Ok(config)
        }
    }

    /// Validates static configuration values independently of host memory.
    pub fn validate(&self) -> Result<(), String> {
        if self.root_dir.as_os_str().is_empty() || self.root_dir == Path::new("/") {
            return Err(format!(
                "root_dir must identify a dedicated service directory, got {}",
                self.root_dir.display()
            ));
        }
        self.sync.validate()?;
        self.rpc_server.validate()?;
        self.snapshot.validate()?;
        Ok(())
    }

    /// Ensures configured cache capacities leave memory for the database and runtime.
    pub fn validate_memory_budget(&self, memory_limit_bytes: u64) -> Result<(), String> {
        self.sync.validate_memory_budget(memory_limit_bytes)
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

    fn log_safe_summary(&self, source: &str) {
        let auth_mode = match self.btc.auth.as_ref() {
            None => "default_cookie",
            Some(BTCAuth::None) => "none",
            Some(BTCAuth::UserPass(_, _)) => "user_pass",
            Some(BTCAuth::CookieFile(_)) => "cookie_file",
        };
        info!(
            "Loaded balance-history {} config: root_dir={}, btc_network={}, btc_auth_mode={}, batch_size={}, undo_retention_blocks={}, undo_cleanup_interval_blocks={}, rpc_addr={}:{}, snapshot_trust_mode={:?}",
            source,
            self.root_dir.display(),
            self.btc.network(),
            auth_mode,
            self.sync.batch_size,
            self.sync.undo_retention_blocks,
            self.sync.undo_cleanup_interval_blocks,
            self.rpc_server.host,
            self.rpc_server.port,
            self.snapshot.trust_mode
        );
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

    #[test]
    fn test_load_preserves_explicit_cache_limits() {
        let root = temp_root("explicit_cache");
        let mut expected = BalanceHistoryConfig {
            root_dir: root.clone(),
            ..BalanceHistoryConfig::default()
        };
        expected.sync.utxo_max_cache_bytes = 4 * 1024 * 1024;
        expected.sync.balance_max_cache_bytes = 12 * 1024 * 1024;
        expected.sync.max_memory_percent = 85;
        std::fs::write(
            root.join("config.toml"),
            toml::to_string_pretty(&expected).unwrap(),
        )
        .unwrap();

        let loaded = BalanceHistoryConfig::load(&root).unwrap();
        assert_eq!(loaded.sync.utxo_max_cache_bytes, 4 * 1024 * 1024);
        assert_eq!(loaded.sync.balance_max_cache_bytes, 12 * 1024 * 1024);
        assert_eq!(loaded.sync.max_memory_percent, 85);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_load_uses_startup_root_instead_of_config_redirect() {
        let root = temp_root("root_authority");
        let redirected = root.join("redirected");
        let config = BalanceHistoryConfig {
            root_dir: redirected,
            ..BalanceHistoryConfig::default()
        };
        std::fs::write(
            root.join("config.toml"),
            toml::to_string_pretty(&config).unwrap(),
        )
        .unwrap();

        let loaded = BalanceHistoryConfig::load(&root).unwrap();
        assert_eq!(loaded.root_dir, root);

        std::fs::remove_dir_all(&loaded.root_dir).unwrap();
    }

    #[test]
    fn test_rejects_invalid_cache_configuration() {
        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("utxo_max_cache_bytes")
        );

        config.sync.utxo_max_cache_bytes = MIN_CACHE_BYTES;
        config.sync.max_memory_percent = 101;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("max_memory_percent")
        );
    }

    #[test]
    fn test_rejects_unsafe_sync_and_rpc_configuration() {
        let mut config = BalanceHistoryConfig::default();
        config.sync.batch_size = 0;
        assert!(config.validate().unwrap_err().contains("batch_size"));

        config.sync.batch_size = default_batch_size();
        config.sync.undo_retention_blocks = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("undo_retention_blocks")
        );

        config.sync.undo_retention_blocks = default_undo_retention_blocks();
        config.sync.undo_cleanup_interval_blocks = 0;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("undo_cleanup_interval_blocks")
        );

        config.sync.undo_cleanup_interval_blocks = default_undo_cleanup_interval_blocks();
        config.rpc_server.port = 0;
        assert!(config.validate().unwrap_err().contains("rpc_server.port"));

        config.rpc_server.port = default_rpc_port();
        config.rpc_server.host = "not a host".to_string();
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("rpc_server address")
        );
    }

    #[test]
    fn test_signed_snapshot_configuration_requires_a_key_role() {
        let mut config = BalanceHistoryConfig::default();
        config.snapshot.trust_mode = SnapshotTrustMode::Signed;
        assert!(
            config
                .validate()
                .unwrap_err()
                .contains("signing_key_file or trusted_keys_file")
        );

        config.snapshot.signing_key_file = Some(PathBuf::from("signing-key.json"));
        config.validate().unwrap();

        config.snapshot.signing_key_file = None;
        config.snapshot.trusted_keys_file = Some(PathBuf::from("trusted-keys.json"));
        config.validate().unwrap();
    }

    #[test]
    fn test_rejects_cache_budget_at_memory_limit() {
        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = 4 * 1024 * 1024;
        config.sync.balance_max_cache_bytes = 12 * 1024 * 1024;
        config.sync.max_memory_percent = 85;

        assert!(
            config
                .validate_memory_budget(16 * 1024 * 1024)
                .unwrap_err()
                .contains("memory-pressure threshold")
        );
        config.validate_memory_budget(22 * 1024 * 1024).unwrap();
    }

    #[test]
    fn two_thirds_cache_plan_preserves_runtime_headroom() {
        let memory_limit = 64 * 1024 * 1024 * 1024;
        let limits = derive_cache_limits(memory_limit, DEFAULT_CACHE_BUDGET_PERCENT).unwrap();
        assert_eq!(
            limits.total_cache_bytes,
            memory_limit * DEFAULT_CACHE_BUDGET_PERCENT as u64 / 100
        );
        assert_eq!(limits.utxo_cache_bytes, limits.total_cache_bytes / 4);
        assert_eq!(
            limits.balance_cache_bytes,
            limits.total_cache_bytes - limits.utxo_cache_bytes
        );

        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = limits.utxo_cache_bytes as usize;
        config.sync.balance_max_cache_bytes = limits.balance_cache_bytes as usize;
        config.sync.max_memory_percent = 80;
        config.validate_memory_budget(memory_limit).unwrap();
    }

    #[test]
    fn legacy_leave_eight_gib_budget_is_rejected_on_64_gib_host() {
        let memory_limit = 64 * 1024 * 1024 * 1024;
        let legacy_budget = memory_limit - 8 * 1024 * 1024 * 1024;
        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = (legacy_budget / 4) as usize;
        config.sync.balance_max_cache_bytes = (legacy_budget * 3 / 4) as usize;
        config.sync.max_memory_percent = 90;

        assert!(
            config
                .validate_memory_budget(memory_limit)
                .unwrap_err()
                .contains("memory-pressure threshold")
        );
    }
}
