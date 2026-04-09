use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use usdb_util::{
    BALANCE_HISTORY_SERVICE_HTTP_PORT, USDB_CONTROL_PLANE_HTTP_PORT,
    USDB_CONTROL_PLANE_SERVICE_NAME, USDB_INDEXER_SERVICE_HTTP_PORT,
};

fn default_root_dir() -> PathBuf {
    usdb_util::get_service_dir(USDB_CONTROL_PLANE_SERVICE_NAME)
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    USDB_CONTROL_PLANE_HTTP_PORT
}

fn default_balance_history_rpc_url() -> String {
    format!("http://127.0.0.1:{BALANCE_HISTORY_SERVICE_HTTP_PORT}")
}

fn default_usdb_indexer_rpc_url() -> String {
    format!("http://127.0.0.1:{USDB_INDEXER_SERVICE_HTTP_PORT}")
}

fn default_ethw_rpc_url() -> String {
    "http://127.0.0.1:8545".to_string()
}

fn default_btc_rpc_url() -> String {
    "http://127.0.0.1:8332".to_string()
}

fn default_console_root() -> PathBuf {
    PathBuf::from("web/usdb-console-app/dist")
}

fn default_balance_history_explorer_root() -> PathBuf {
    PathBuf::from("web/balance-history-browser")
}

fn default_usdb_indexer_explorer_root() -> PathBuf {
    PathBuf::from("web/usdb-indexer-browser")
}

fn default_bootstrap_manifest_path() -> PathBuf {
    PathBuf::from("docker/local/dev-sim/bootstrap/bootstrap-manifest.json")
}

fn default_snapshot_marker_path() -> PathBuf {
    PathBuf::from("docker/local/dev-sim/balance-history/bootstrap/snapshot-loader.done.json")
}

fn default_ethw_init_marker_path() -> PathBuf {
    PathBuf::from("docker/local/dev-sim/ethw/bootstrap/ethw-init.done.json")
}

fn default_sourcedao_state_path() -> PathBuf {
    PathBuf::from("docker/local/dev-sim/bootstrap/sourcedao-bootstrap-state.json")
}

fn default_sourcedao_marker_path() -> PathBuf {
    PathBuf::from("docker/local/dev-sim/bootstrap/sourcedao-bootstrap.done.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinAuthMode {
    None,
    Cookie,
    Userpass,
}

fn default_bitcoin_auth_mode() -> BitcoinAuthMode {
    BitcoinAuthMode::None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcTargets {
    #[serde(default = "default_balance_history_rpc_url")]
    pub balance_history_url: String,
    #[serde(default = "default_usdb_indexer_rpc_url")]
    pub usdb_indexer_url: String,
    #[serde(default = "default_ethw_rpc_url")]
    pub ethw_url: String,
}

impl Default for RpcTargets {
    fn default() -> Self {
        Self {
            balance_history_url: default_balance_history_rpc_url(),
            usdb_indexer_url: default_usdb_indexer_rpc_url(),
            ethw_url: default_ethw_rpc_url(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BitcoinRpcConfig {
    #[serde(default = "default_btc_rpc_url")]
    pub url: String,
    #[serde(default = "default_bitcoin_auth_mode")]
    pub auth_mode: BitcoinAuthMode,
    #[serde(default)]
    pub rpc_user: Option<String>,
    #[serde(default)]
    pub rpc_password: Option<String>,
    #[serde(default)]
    pub cookie_file: Option<PathBuf>,
}

impl Default for BitcoinRpcConfig {
    fn default() -> Self {
        Self {
            url: default_btc_rpc_url(),
            auth_mode: default_bitcoin_auth_mode(),
            rpc_user: None,
            rpc_password: None,
            cookie_file: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapPaths {
    #[serde(default = "default_bootstrap_manifest_path")]
    pub bootstrap_manifest: PathBuf,
    #[serde(default = "default_snapshot_marker_path")]
    pub snapshot_marker: PathBuf,
    #[serde(default = "default_ethw_init_marker_path")]
    pub ethw_init_marker: PathBuf,
    #[serde(default = "default_sourcedao_state_path")]
    pub sourcedao_bootstrap_state: PathBuf,
    #[serde(default = "default_sourcedao_marker_path")]
    pub sourcedao_bootstrap_marker: PathBuf,
}

impl Default for BootstrapPaths {
    fn default() -> Self {
        Self {
            bootstrap_manifest: default_bootstrap_manifest_path(),
            snapshot_marker: default_snapshot_marker_path(),
            ethw_init_marker: default_ethw_init_marker_path(),
            sourcedao_bootstrap_state: default_sourcedao_state_path(),
            sourcedao_bootstrap_marker: default_sourcedao_marker_path(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebRoots {
    #[serde(default = "default_console_root")]
    pub console_root: PathBuf,
    #[serde(default = "default_balance_history_explorer_root")]
    pub balance_history_explorer_root: PathBuf,
    #[serde(default = "default_usdb_indexer_explorer_root")]
    pub usdb_indexer_explorer_root: PathBuf,
}

impl Default for WebRoots {
    fn default() -> Self {
        Self {
            console_root: default_console_root(),
            balance_history_explorer_root: default_balance_history_explorer_root(),
            usdb_indexer_explorer_root: default_usdb_indexer_explorer_root(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlPlaneConfig {
    #[serde(default = "default_root_dir")]
    pub root_dir: PathBuf,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub rpc: RpcTargets,
    #[serde(default)]
    pub bitcoin: BitcoinRpcConfig,
    #[serde(default)]
    pub bootstrap: BootstrapPaths,
    #[serde(default)]
    pub web: WebRoots,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            root_dir: default_root_dir(),
            server: ServerConfig::default(),
            rpc: RpcTargets::default(),
            bitcoin: BitcoinRpcConfig::default(),
            bootstrap: BootstrapPaths::default(),
            web: WebRoots::default(),
        }
    }
}

impl ControlPlaneConfig {
    pub fn load(root_dir: &Path) -> Result<Self, String> {
        let path = root_dir.join("config.toml");
        if !path.exists() {
            let default_config = Self {
                root_dir: root_dir.to_path_buf(),
                ..Self::default()
            };
            info!(
                "Config file {} does not exist. Using default configuration.",
                path.display()
            );
            return Ok(default_config);
        }

        let config_data = std::fs::read_to_string(&path).map_err(|e| {
            let msg = format!("Failed to read config file {}: {}", path.display(), e);
            error!("{}", msg);
            msg
        })?;

        let mut config: Self = toml::from_str(&config_data).map_err(|e| {
            let msg = format!("Failed to parse config file {}: {}", path.display(), e);
            error!("{}", msg);
            msg
        })?;
        config.root_dir = root_dir.to_path_buf();
        Ok(config)
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }

    pub fn resolve_runtime_path(&self, path: &Path) -> Result<PathBuf, String> {
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }

        let current_dir = std::env::current_dir().map_err(|e| {
            let msg = format!("Failed to resolve current working directory: {}", e);
            error!("{}", msg);
            msg
        })?;
        Ok(current_dir.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_paths_are_repo_relative() {
        let config = ControlPlaneConfig::default();
        assert_eq!(
            config.web.console_root,
            PathBuf::from("web/usdb-console-app/dist")
        );
        assert_eq!(
            config.bootstrap.bootstrap_manifest,
            PathBuf::from("docker/local/dev-sim/bootstrap/bootstrap-manifest.json")
        );
        assert_eq!(
            config.bootstrap.sourcedao_bootstrap_marker,
            PathBuf::from("docker/local/dev-sim/bootstrap/sourcedao-bootstrap.done.json")
        );
    }
}
