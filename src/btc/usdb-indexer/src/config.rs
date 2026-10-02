use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::{BTCConfig, BalanceHistoryConfig, OrdConfig, USDB_INDEXER_SERVICE_HTTP_PORT};

fn default_genesis_block_height() -> u32 {
    900000
}

fn default_active_address_page_size() -> usize {
    1024
}

fn default_balance_query_batch_size() -> usize {
    1024
}

fn default_balance_query_concurrency() -> usize {
    4
}

fn default_balance_query_timeout_ms() -> u64 {
    10_000
}

fn default_balance_query_max_retries() -> u32 {
    2
}

fn default_upstream_poll_interval_ms() -> u64 {
    5_000
}

fn deserialize_upstream_poll_interval_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if !(100..=60_000).contains(&value) {
        return Err(serde::de::Error::custom(
            "upstream_poll_interval_ms must be between 100 and 60000",
        ));
    }
    Ok(value)
}

fn default_inscription_source() -> String {
    "ord".to_string()
}

fn default_inscription_fixture_file() -> Option<String> {
    None
}

fn default_inscription_source_shadow_compare() -> bool {
    false
}

fn default_inscription_source_shadow_fail_fast() -> bool {
    false
}

fn default_rpc_server_port() -> u16 {
    USDB_INDEXER_SERVICE_HTTP_PORT
}

fn default_rpc_server_host() -> String {
    "127.0.0.1".to_string()
}

fn default_rpc_server_enabled() -> bool {
    true
}

fn default_pass_energy_leaderboard_cache_enabled() -> bool {
    true
}

fn default_pass_energy_leaderboard_cache_top_k() -> usize {
    1000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct USDBConfig {
    /// Immutable USDB rule domain; omitted legacy configurations retain their registry identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_scope: Option<String>,

    /// Frozen registry revision expected by this deployment, never a moving latest selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_registry_id: Option<String>,

    /// Reviewed catalog artifact, resolved relative to the service root when not absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_registry_catalog_file: Option<String>,

    // First BTC block height that the indexer should process for USDB protocol data.
    #[serde(default = "default_genesis_block_height")]
    pub genesis_block_height: u32,

    // Number of active pass records loaded per storage page when scanning owners.
    #[serde(default = "default_active_address_page_size")]
    pub active_address_page_size: usize,

    // Number of addresses included in each balance-history RPC request batch.
    #[serde(default = "default_balance_query_batch_size")]
    pub balance_query_batch_size: usize,

    // Maximum number of in-flight balance-history RPC batches.
    #[serde(default = "default_balance_query_concurrency")]
    pub balance_query_concurrency: usize,

    // Per-batch balance-history RPC timeout in milliseconds.
    #[serde(default = "default_balance_query_timeout_ms")]
    pub balance_query_timeout_ms: u64,

    // Maximum retry attempts for a failed balance-history RPC batch.
    #[serde(default = "default_balance_query_max_retries")]
    pub balance_query_max_retries: u32,

    /// Idle upstream polling interval; smaller values reduce regtest latency.
    /// This affects scheduling only, never stable lag or consensus readiness.
    #[serde(
        default = "default_upstream_poll_interval_ms",
        deserialize_with = "deserialize_upstream_poll_interval_ms"
    )]
    pub upstream_poll_interval_ms: u64,

    // Primary inscription source backend: supported values are "ord" and "bitcoind".
    #[serde(default = "default_inscription_source")]
    pub inscription_source: String,

    // Optional fixture JSON file path used when inscription_source is "fixture".
    // Relative paths are resolved from usdb-indexer root directory.
    #[serde(default = "default_inscription_fixture_file")]
    pub inscription_fixture_file: Option<String>,

    // Enable primary-vs-shadow inscription source comparison for diagnostics.
    #[serde(default = "default_inscription_source_shadow_compare")]
    pub inscription_source_shadow_compare: bool,

    // Stop block processing immediately when shadow comparison finds mismatches.
    #[serde(default = "default_inscription_source_shadow_fail_fast")]
    pub inscription_source_shadow_fail_fast: bool,

    // JSON-RPC server listen host for usdb-indexer external query APIs.
    #[serde(default = "default_rpc_server_host")]
    pub rpc_server_host: String,

    // JSON-RPC server listen port for usdb-indexer external query APIs.
    #[serde(default = "default_rpc_server_port")]
    pub rpc_server_port: u16,

    // Enable or disable JSON-RPC server startup.
    #[serde(default = "default_rpc_server_enabled")]
    pub rpc_server_enabled: bool,

    // Enable in-memory cache for latest-height pass energy leaderboard queries.
    #[serde(default = "default_pass_energy_leaderboard_cache_enabled")]
    pub pass_energy_leaderboard_cache_enabled: bool,

    // Maximum number of top-ranked leaderboard rows cached in memory.
    #[serde(default = "default_pass_energy_leaderboard_cache_top_k")]
    pub pass_energy_leaderboard_cache_top_k: usize,
}

impl Default for USDBConfig {
    fn default() -> Self {
        USDBConfig {
            rules_scope: None,
            activation_registry_id: None,
            activation_registry_catalog_file: None,
            genesis_block_height: default_genesis_block_height(),
            active_address_page_size: default_active_address_page_size(),
            balance_query_batch_size: default_balance_query_batch_size(),
            balance_query_concurrency: default_balance_query_concurrency(),
            balance_query_timeout_ms: default_balance_query_timeout_ms(),
            balance_query_max_retries: default_balance_query_max_retries(),
            upstream_poll_interval_ms: default_upstream_poll_interval_ms(),
            inscription_source: default_inscription_source(),
            inscription_fixture_file: default_inscription_fixture_file(),
            inscription_source_shadow_compare: default_inscription_source_shadow_compare(),
            inscription_source_shadow_fail_fast: default_inscription_source_shadow_fail_fast(),
            rpc_server_host: default_rpc_server_host(),
            rpc_server_port: default_rpc_server_port(),
            rpc_server_enabled: default_rpc_server_enabled(),
            pass_energy_leaderboard_cache_enabled: default_pass_energy_leaderboard_cache_enabled(),
            pass_energy_leaderboard_cache_top_k: default_pass_energy_leaderboard_cache_top_k(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IndexerConfig {
    // Optional namespace to isolate runtime data and logs under <root>/<isolate>/.
    pub isolate: Option<String>,

    // Bitcoin RPC connectivity settings.
    pub bitcoin: BTCConfig,

    // Ord service RPC settings.
    pub ordinals: OrdConfig,

    // Balance-history service RPC settings.
    pub balance_history: BalanceHistoryConfig,

    // USDB indexer behavior and performance tuning settings.
    pub usdb: USDBConfig,
}

pub struct ConfigManager {
    root_dir: PathBuf,
    config: IndexerConfig,
}

impl ConfigManager {
    pub fn load(root_dir: Option<PathBuf>) -> Result<Self, String> {
        let root_dir = if let Some(dir) = root_dir {
            dir
        } else {
            let home = dirs::home_dir().expect("Could not determine home directory");
            home.join(".usdb")
        };

        if !root_dir.exists() {
            std::fs::create_dir_all(&root_dir).map_err(|e| {
                let msg = format!(
                    "Could not create root directory at {}: {}",
                    root_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        let config_path = root_dir.join("config.json");
        if !config_path.exists() {
            let default_config = IndexerConfig::default();

            info!(
                "Config file not found at {}. Using default config.",
                config_path.display()
            );
            info!(
                "Default config: {}",
                serde_json::to_string_pretty(&default_config).unwrap()
            );

            return Ok(Self {
                root_dir,
                config: default_config,
            });
        }

        let config_data = std::fs::read_to_string(&config_path).map_err(|e| {
            let msg = format!(
                "Could not read config file at {}: {}",
                config_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let config: IndexerConfig = serde_json::from_str(&config_data).map_err(|e| {
            let msg = format!(
                "Could not parse config file at {}: {}",
                config_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        info!(
            "Loaded upstream polling configuration: root_dir={}, upstream_poll_interval_ms={}, status_poll_interval_ms={}",
            root_dir.display(),
            config.usdb.upstream_poll_interval_ms,
            config.usdb.upstream_poll_interval_ms.min(1_000)
        );
        Ok(Self { root_dir, config })
    }

    pub fn root_dir(&self) -> &PathBuf {
        &self.root_dir
    }

    pub fn data_dir(&self) -> PathBuf {
        let dir = match &self.config.isolate {
            Some(isolate) => self.root_dir.join(isolate).join("data"),
            None => self.root_dir.join("data"),
        };

        if !dir.exists() {
            std::fs::create_dir_all(&dir).expect("Could not create data directory");
        }

        dir
    }

    pub fn config(&self) -> &IndexerConfig {
        &self.config
    }

    /// Resolve the network's exact catalog before opening any index data.
    pub fn activation_registry_catalog(
        &self,
    ) -> Result<usdb_util::BtcActivationRegistryCatalog, String> {
        let result = (|| {
            let catalog_json = self
                .config
                .usdb
                .activation_registry_catalog_file
                .as_ref()
                .map(|file| {
                    if file.is_empty() {
                        return Err(
                            "activation_registry_catalog_file must not be empty".to_string()
                        );
                    }
                    let path = self.root_dir.join(file);
                    std::fs::read_to_string(&path).map_err(|error| {
                        format!(
                            "Failed to read activation registry catalog: path={}, error={error}",
                            path.display()
                        )
                    })
                })
                .transpose()?;
            usdb_util::resolve_btc_activation_registry_catalog(
                self.config.bitcoin.network(),
                self.config.usdb.rules_scope.as_deref(),
                self.config.usdb.activation_registry_id.as_deref(),
                catalog_json.as_deref(),
            )
            .map_err(|error| error.to_string())
        })();
        result.map_err(|error| {
            let msg = format!("Failed to resolve configured activation registry: rules_scope={:?}, registry_id={:?}, error={error}", self.config.usdb.rules_scope, self.config.usdb.activation_registry_id);
            error!("{msg}");
            msg
        })
    }
}

pub type ConfigManagerRef = Arc<ConfigManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_selection_is_explicit_pinned_and_resolves_relative_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "usdb_registry_config_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let artifact =
            include_str!("../../usdb-util/tests/fixtures/btc-mainnet-usdb-mainnet-catalog.json");
        std::fs::write(root.join("catalog.json"), artifact).unwrap();
        let catalog = usdb_util::BtcActivationRegistryCatalog::from_json(artifact).unwrap();
        let mut manager = ConfigManager {
            root_dir: root.clone(),
            config: IndexerConfig::default(),
        };
        manager.config.bitcoin.network = bitcoincore_rpc::bitcoin::Network::Bitcoin;
        manager.config.usdb.rules_scope = Some("usdb-mainnet-fixture".into());
        manager.config.usdb.activation_registry_id = Some(catalog.current_registry_id().into());
        manager.config.usdb.activation_registry_catalog_file = Some("catalog.json".into());
        assert_eq!(
            manager
                .activation_registry_catalog()
                .unwrap()
                .current_registry_id(),
            catalog.current_registry_id()
        );
        manager.config.usdb.rules_scope = Some("usdb-testnet-fixture".into());
        assert!(manager.activation_registry_catalog().is_err());
        manager.config.usdb.rules_scope = Some("usdb-mainnet-fixture".into());
        manager.config.usdb.activation_registry_id = Some("11".repeat(32));
        assert!(manager.activation_registry_catalog().is_err());
        manager.config.usdb.activation_registry_id = Some(catalog.current_registry_id().into());
        manager.config.bitcoin.network = bitcoincore_rpc::bitcoin::Network::Regtest;
        assert!(manager.activation_registry_catalog().is_err());
        manager.config.bitcoin.network = bitcoincore_rpc::bitcoin::Network::Bitcoin;
        manager.config.usdb.activation_registry_catalog_file = None;
        assert!(manager.activation_registry_catalog().is_err());
        manager.config.usdb.rules_scope = None;
        manager.config.usdb.activation_registry_id = None;
        let legacy = manager.activation_registry_catalog().unwrap();
        assert_eq!(
            legacy.current_registry_id(),
            usdb_util::embedded_btc_activation_registry_catalog(manager.config.bitcoin.network())
                .unwrap()
                .current_registry_id()
        );
        manager.config.usdb.activation_registry_id = Some(legacy.current_registry_id().into());
        assert!(manager.activation_registry_catalog().is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upstream_poll_interval_preserves_legacy_defaults() {
        let legacy: USDBConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.upstream_poll_interval_ms, 5_000);
        assert_eq!(legacy, USDBConfig::default());
    }

    #[test]
    fn upstream_poll_interval_accepts_bounded_tuning() {
        for value in [100, 200, 5_000, 60_000] {
            let config: USDBConfig =
                serde_json::from_value(serde_json::json!({"upstream_poll_interval_ms": value}))
                    .unwrap();
            assert_eq!(config.upstream_poll_interval_ms, value);
        }
        for value in [0, 99, 60_001] {
            assert!(
                serde_json::from_value::<USDBConfig>(
                    serde_json::json!({"upstream_poll_interval_ms": value})
                )
                .is_err()
            );
        }
    }
}
