use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceRpcRequest {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverviewResponse {
    pub service: String,
    pub generated_at_ms: u64,
    pub services: ServicesSummary,
    pub capabilities: CapabilitiesSummary,
    pub bootstrap: BootstrapSummary,
    pub explorers: ExplorerLinks,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServicesSummary {
    pub btc_node: ServiceProbe<BtcNodeServiceSummary>,
    pub balance_history: ServiceProbe<BalanceHistoryServiceSummary>,
    pub usdb_indexer: ServiceProbe<UsdbIndexerServiceSummary>,
    pub ethw: ServiceProbe<EthwServiceSummary>,
    pub ord: ServiceProbe<OrdServiceSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesSummary {
    pub ord_available: bool,
    pub btc_console_mode: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapSummary {
    pub bootstrap_manifest: ArtifactSummary,
    pub snapshot_marker: ArtifactSummary,
    pub ethw_init_marker: ArtifactSummary,
    pub sourcedao_bootstrap_state: ArtifactSummary,
    pub sourcedao_bootstrap_marker: ArtifactSummary,
    pub steps: Vec<BootstrapStepSummary>,
    pub overall_state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplorerLinks {
    pub control_console: String,
    pub balance_history: String,
    pub usdb_indexer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactSummary {
    pub path: String,
    pub exists: bool,
    pub error: Option<String>,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceProbe<T> {
    pub name: String,
    pub rpc_url: String,
    pub reachable: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    pub data: Option<T>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BalanceHistoryServiceSummary {
    pub network: Option<String>,
    pub rpc_alive: Option<bool>,
    pub query_ready: Option<bool>,
    pub consensus_ready: Option<bool>,
    pub phase: Option<String>,
    pub message: Option<String>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub stable_height: Option<u32>,
    pub stable_block_hash: Option<String>,
    pub latest_block_commit: Option<String>,
    pub snapshot_verification_state: Option<String>,
    pub snapshot_signing_key_id: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsdbIndexerServiceSummary {
    pub network: Option<String>,
    pub rpc_alive: Option<bool>,
    pub query_ready: Option<bool>,
    pub consensus_ready: Option<bool>,
    pub message: Option<String>,
    pub current: Option<u32>,
    pub total: Option<u32>,
    pub synced_block_height: Option<u32>,
    pub balance_history_stable_height: Option<u32>,
    pub upstream_snapshot_id: Option<String>,
    pub local_state_commit: Option<String>,
    pub system_state_id: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BtcNodeServiceSummary {
    pub chain: Option<String>,
    pub blocks: Option<u64>,
    pub headers: Option<u64>,
    pub best_block_hash: Option<String>,
    pub best_block_time: Option<u64>,
    pub verification_progress: Option<f64>,
    pub initial_block_download: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EthwServiceSummary {
    pub client_version: Option<String>,
    pub chain_id: Option<String>,
    pub network_id: Option<String>,
    pub block_number: Option<u64>,
    pub latest_block_hash: Option<String>,
    pub latest_block_time: Option<u64>,
    pub syncing: Option<Value>,
    pub query_ready: Option<bool>,
    pub consensus_ready: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrdServiceSummary {
    pub http_status: Option<u16>,
    pub backend_ready: Option<bool>,
    pub query_ready: Option<bool>,
    pub synced_block_height: Option<u64>,
    pub btc_tip_height: Option<u64>,
    pub sync_gap: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapStepSummary {
    pub step: String,
    pub state: String,
    pub artifact_path: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceHistoryReadiness {
    pub rpc_alive: bool,
    pub query_ready: bool,
    pub consensus_ready: bool,
    pub phase: String,
    pub current: u64,
    pub total: u64,
    pub message: Option<String>,
    pub stable_height: Option<u32>,
    pub stable_block_hash: Option<String>,
    pub latest_block_commit: Option<String>,
    pub snapshot_verification_state: Option<String>,
    pub snapshot_signing_key_id: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UsdbIndexerReadiness {
    pub rpc_alive: bool,
    pub query_ready: bool,
    pub consensus_ready: bool,
    pub synced_block_height: Option<u32>,
    pub balance_history_stable_height: Option<u32>,
    pub upstream_snapshot_id: Option<String>,
    pub local_state_commit: Option<String>,
    pub system_state_id: Option<String>,
    pub current: u32,
    pub total: u32,
    pub message: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BitcoinBlockchainInfo {
    pub chain: String,
    pub blocks: u64,
    pub headers: u64,
    #[serde(rename = "bestblockhash")]
    pub best_block_hash: String,
    #[serde(rename = "verificationprogress")]
    pub verification_progress: f64,
    #[serde(rename = "initialblockdownload")]
    pub initial_block_download: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BitcoinBlockHeader {
    pub time: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EthBlockHeader {
    pub hash: Option<String>,
    pub timestamp: String,
}
