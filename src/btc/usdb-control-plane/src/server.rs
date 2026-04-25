use crate::config::ControlPlaneConfig;
use crate::models::{
    ApiError, AppEntry, ArtifactSummary, BalanceHistoryServiceSummary, BootstrapStepSummary,
    BootstrapSummary, BtcMintExecuteRequest, BtcMintExecuteResponse,
    BtcMintPrepareActivePassSummary, BtcMintPrepareRequest, BtcMintPrepareResponse,
    BtcMintPrepareRuntimeSummary, BtcNodeServiceSummary, BtcWorldSimDevSignerResponse,
    BtcWorldSimIdentitiesResponse, BtcWorldSimIdentity, CapabilitiesSummary,
    EthwAddressStatusResponse, EthwDevIdentityResponse, EthwServiceSummary, ExplorerLinks,
    OrdServiceSummary, OverviewResponse, ServiceProbe, ServiceRpcRequest, ServicesSummary,
    UsdbIndexerServiceSummary,
};
use crate::rpc_client::{RpcClient, decode_hex_quantity};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, get_service, post};
use axum::{Router, serve};
use bitcoincore_rpc::bitcoin::Network;
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tower_http::services::ServeDir;
use usdb_util::{
    USDB_CONTROL_PLANE_SERVICE_NAME, address_string_to_script_hash, parse_script_hash_any,
};

#[derive(Debug, Deserialize)]
struct WorldSimBootstrapMarker {
    agent_wallets: Vec<String>,
    agent_addresses: Vec<String>,
    #[serde(default)]
    ethw_miner_agent_id: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WorldSimDevSignerQuery {
    wallet_name: String,
}

#[derive(Debug, Deserialize)]
struct EthwAddressStatusQuery {
    address: String,
}

#[derive(Debug, Deserialize)]
struct EthwDevIdentityMarker {
    ethw_miner_address: String,
    #[serde(default)]
    identity_mode: Option<String>,
    #[serde(default)]
    identity_scheme: Option<String>,
    #[serde(default)]
    identity_fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ControlPlaneConfig>,
    pub rpc_client: RpcClient,
}

struct PreparedBtcMintContext {
    response: BtcMintPrepareResponse,
    runtime_profile: String,
}

#[derive(Debug, Deserialize)]
struct OrdMintJsonOutput {
    #[serde(default)]
    inscriptions: Vec<OrdMintJsonInscription>,
    #[serde(default)]
    reveal: Option<String>,
    #[serde(default)]
    commit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrdMintJsonInscription {
    id: String,
}

const BALANCE_HISTORY_PROXY_METHODS: &[&str] = &[
    "get_network_type",
    "get_block_height",
    "get_sync_status",
    "get_readiness",
    "get_address_balance",
    "get_addresses_balances",
    "get_address_balance_delta",
    "get_addresses_balances_delta",
    "get_address_balance_summary",
    "get_address_balance_timeseries",
    "get_address_flow_buckets",
];

const USDB_INDEXER_PROXY_METHODS: &[&str] = &[
    "get_rpc_info",
    "get_sync_status",
    "get_readiness",
    "get_pass_block_commit",
    "get_pass_snapshot",
    "get_active_passes_at_height",
    "get_owner_active_pass_at_height",
    "get_owner_passes_at_height",
    "get_pass_stats_at_height",
    "get_pass_history",
    "get_pass_energy",
    "get_pass_energy_range",
    "get_pass_energy_leaderboard",
    "get_active_balance_snapshot",
    "get_latest_active_balance_snapshot",
];

pub async fn run_server(config: ControlPlaneConfig) -> Result<(), String> {
    let console_root = config.resolve_runtime_path(&config.web.console_root)?;
    ensure_dir_exists("console web root", &console_root)?;

    let bh_explorer_root =
        config.resolve_runtime_path(&config.web.balance_history_explorer_root)?;
    ensure_dir_exists("balance-history explorer root", &bh_explorer_root)?;

    let indexer_explorer_root =
        config.resolve_runtime_path(&config.web.usdb_indexer_explorer_root)?;
    ensure_dir_exists("usdb-indexer explorer root", &indexer_explorer_root)?;

    let rpc_client = RpcClient::new()?;
    let listen_addr: SocketAddr = config.listen_addr().parse().map_err(|e| {
        let msg = format!("Invalid listen address {}: {}", config.listen_addr(), e);
        error!("{}", msg);
        msg
    })?;
    let console_root_label = config.web.console_root.display().to_string();

    let state = AppState {
        config: Arc::new(config),
        rpc_client,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/system/overview", get(get_overview))
        .route("/api/system/services", get(get_services))
        .route("/api/system/bootstrap", get(get_bootstrap))
        .route(
            "/api/btc/world-sim/identities",
            get(get_btc_world_sim_identities),
        )
        .route(
            "/api/btc/world-sim/dev-signer",
            get(get_btc_world_sim_dev_signer),
        )
        .route("/api/ethw/dev-sim/identity", get(get_ethw_dev_identity))
        .route("/api/ethw/address-status", get(get_ethw_address_status))
        .route("/api/btc/mint/prepare", post(post_prepare_btc_mint))
        .route("/api/btc/mint/execute", post(post_execute_btc_mint))
        .route(
            "/api/services/balance-history/rpc",
            post(post_balance_history_rpc),
        )
        .route(
            "/api/services/usdb-indexer/rpc",
            post(post_usdb_indexer_rpc),
        )
        .nest_service(
            "/explorers/balance-history",
            get_service(ServeDir::new(bh_explorer_root).append_index_html_on_directories(true)),
        )
        .nest_service(
            "/explorers/usdb-indexer",
            get_service(
                ServeDir::new(indexer_explorer_root).append_index_html_on_directories(true),
            ),
        )
        .fallback_service(get_service(
            ServeDir::new(console_root).append_index_html_on_directories(true),
        ))
        .with_state(state);

    info!(
        "Starting USDB control plane: listen_addr={}, console_root={}",
        listen_addr, console_root_label
    );
    serve(
        tokio::net::TcpListener::bind(listen_addr)
            .await
            .map_err(|e| {
                let msg = format!("Failed to bind control plane on {}: {}", listen_addr, e);
                error!("{}", msg);
                msg
            })?,
        app,
    )
    .await
    .map_err(|e| {
        let msg = format!("Control plane server exited with error: {}", e);
        error!("{}", msg);
        msg
    })
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({
        "service": USDB_CONTROL_PLANE_SERVICE_NAME,
        "ok": true,
    }))
}

async fn get_overview(State(state): State<AppState>) -> Result<Json<OverviewResponse>, StatusCode> {
    Ok(Json(build_overview(&state).await))
}

async fn get_services(State(state): State<AppState>) -> Result<Json<ServicesSummary>, StatusCode> {
    Ok(Json(build_services_summary(&state).await))
}

async fn get_bootstrap(
    State(state): State<AppState>,
) -> Result<Json<BootstrapSummary>, StatusCode> {
    Ok(Json(build_bootstrap_summary(&state)))
}

async fn get_btc_world_sim_identities(
    State(state): State<AppState>,
) -> Result<Json<BtcWorldSimIdentitiesResponse>, StatusCode> {
    let services = build_services_summary(&state).await;
    let btc_network = resolve_runtime_btc_network_name(&services);
    let runtime_profile = classify_btc_runtime_profile(btc_network.as_deref()).to_string();
    let marker = read_artifact_summary(
        &state.config,
        &state.config.bootstrap.world_sim_bootstrap_marker,
    );

    if runtime_profile != "development" {
        return Ok(Json(BtcWorldSimIdentitiesResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            identities: Vec::new(),
            error: None,
        }));
    }

    if !marker.exists {
        return Ok(Json(BtcWorldSimIdentitiesResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            identities: Vec::new(),
            error: marker.error,
        }));
    }

    let Some(marker_data) = marker.data else {
        return Ok(Json(BtcWorldSimIdentitiesResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            identities: Vec::new(),
            error: marker.error.or_else(|| {
                Some("World-sim bootstrap marker did not expose JSON data".to_string())
            }),
        }));
    };

    match decode_world_sim_identities(marker_data) {
        Ok(identities) => Ok(Json(BtcWorldSimIdentitiesResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: true,
            marker_path: marker.path,
            identities,
            error: None,
        })),
        Err(error) => Ok(Json(BtcWorldSimIdentitiesResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            identities: Vec::new(),
            error: Some(error),
        })),
    }
}

async fn get_btc_world_sim_dev_signer(
    State(state): State<AppState>,
    Query(query): Query<WorldSimDevSignerQuery>,
) -> Result<Json<BtcWorldSimDevSignerResponse>, StatusCode> {
    let services = build_services_summary(&state).await;
    let btc_network = resolve_runtime_btc_network_name(&services);
    let runtime_profile = classify_btc_runtime_profile(btc_network.as_deref()).to_string();
    let wallet_name = query.wallet_name.trim().to_string();

    if wallet_name.is_empty() {
        return Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name,
            owner_address: None,
            wif: None,
            error: Some("wallet_name is required".to_string()),
        }));
    }

    if runtime_profile != "development" {
        return Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name,
            owner_address: None,
            wif: None,
            error: None,
        }));
    }

    let marker = read_artifact_summary(
        &state.config,
        &state.config.bootstrap.world_sim_bootstrap_marker,
    );
    if !marker.exists {
        return Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name,
            owner_address: None,
            wif: None,
            error: marker.error,
        }));
    }

    let Some(marker_data) = marker.data else {
        return Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name,
            owner_address: None,
            wif: None,
            error: marker.error.or_else(|| {
                Some("World-sim bootstrap marker did not expose JSON data".to_string())
            }),
        }));
    };

    let identities = match decode_world_sim_identities(marker_data) {
        Ok(identities) => identities,
        Err(error) => {
            return Ok(Json(BtcWorldSimDevSignerResponse {
                btc_network,
                btc_runtime_profile: runtime_profile,
                available: false,
                wallet_name,
                owner_address: None,
                wif: None,
                error: Some(error),
            }));
        }
    };

    let Some(identity) = identities
        .into_iter()
        .find(|identity| identity.wallet_name == wallet_name)
    else {
        return Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name,
            owner_address: None,
            wif: None,
            error: Some(
                "Selected world-sim wallet_name is not present in the bootstrap marker".to_string(),
            ),
        }));
    };

    match state
        .rpc_client
        .bitcoin_wallet_resolve_private_key(
            &state.config,
            &identity.wallet_name,
            &identity.owner_address,
        )
        .await
    {
        Ok(wif) => Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: true,
            wallet_name: identity.wallet_name,
            owner_address: Some(identity.owner_address),
            wif: Some(wif),
            error: None,
        })),
        Err(error) => Ok(Json(BtcWorldSimDevSignerResponse {
            btc_network,
            btc_runtime_profile: runtime_profile,
            available: false,
            wallet_name: identity.wallet_name.clone(),
            owner_address: Some(identity.owner_address),
            wif: None,
            error: Some(format!(
                "Failed to resolve dev signer material for world-sim wallet {}: {}",
                identity.wallet_name, error
            )),
        })),
    }
}

async fn get_ethw_dev_identity(
    State(state): State<AppState>,
) -> Result<Json<EthwDevIdentityResponse>, StatusCode> {
    let services = build_services_summary(&state).await;
    let ethw_chain_id = services
        .ethw
        .data
        .as_ref()
        .and_then(|summary| summary.chain_id.clone());
    let ethw_network_id = services
        .ethw
        .data
        .as_ref()
        .and_then(|summary| summary.network_id.clone());
    let runtime_profile =
        classify_ethw_runtime_profile(ethw_chain_id.as_deref(), ethw_network_id.as_deref())
            .to_string();
    let marker = read_artifact_summary(&state.config, &state.config.bootstrap.ethw_identity_marker);

    if runtime_profile != "development" {
        return Ok(Json(EthwDevIdentityResponse {
            ethw_chain_id,
            ethw_network_id,
            ethw_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            address: None,
            identity_mode: None,
            identity_scheme: None,
            identity_fingerprint: None,
            error: None,
        }));
    }

    if !marker.exists {
        return Ok(Json(EthwDevIdentityResponse {
            ethw_chain_id,
            ethw_network_id,
            ethw_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            address: None,
            identity_mode: None,
            identity_scheme: None,
            identity_fingerprint: None,
            error: marker.error,
        }));
    }

    let Some(marker_data) = marker.data else {
        return Ok(Json(EthwDevIdentityResponse {
            ethw_chain_id,
            ethw_network_id,
            ethw_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            address: None,
            identity_mode: None,
            identity_scheme: None,
            identity_fingerprint: None,
            error: marker
                .error
                .or_else(|| Some("ETHW dev identity marker did not expose JSON data".to_string())),
        }));
    };

    match serde_json::from_value::<EthwDevIdentityMarker>(marker_data) {
        Ok(identity) => Ok(Json(EthwDevIdentityResponse {
            ethw_chain_id,
            ethw_network_id,
            ethw_runtime_profile: runtime_profile,
            available: true,
            marker_path: marker.path,
            address: Some(identity.ethw_miner_address),
            identity_mode: identity.identity_mode,
            identity_scheme: identity.identity_scheme,
            identity_fingerprint: identity.identity_fingerprint,
            error: None,
        })),
        Err(error) => Ok(Json(EthwDevIdentityResponse {
            ethw_chain_id,
            ethw_network_id,
            ethw_runtime_profile: runtime_profile,
            available: false,
            marker_path: marker.path,
            address: None,
            identity_mode: None,
            identity_scheme: None,
            identity_fingerprint: None,
            error: Some(format!(
                "Failed to decode ETHW dev identity marker: {}",
                error
            )),
        })),
    }
}

async fn get_ethw_address_status(
    State(state): State<AppState>,
    Query(query): Query<EthwAddressStatusQuery>,
) -> impl IntoResponse {
    let address = match normalize_evm_address("address", &query.address) {
        Ok(address) => address,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(ApiError { error })).into_response();
        }
    };
    let services = build_services_summary(&state).await;
    let ethw_chain_id = services
        .ethw
        .data
        .as_ref()
        .and_then(|summary| summary.chain_id.clone());
    let ethw_network_id = services
        .ethw
        .data
        .as_ref()
        .and_then(|summary| summary.network_id.clone());
    let runtime_profile =
        classify_ethw_runtime_profile(ethw_chain_id.as_deref(), ethw_network_id.as_deref())
            .to_string();
    let latest_block_number = services
        .ethw
        .data
        .as_ref()
        .and_then(|summary| summary.block_number.map(|value| value.to_string()));

    let balance_result = state
        .rpc_client
        .ethw_balance(&state.config.rpc.ethw_url, &address)
        .await;
    let (available, balance_wei, error) = match balance_result {
        Ok(balance_wei) => (true, Some(balance_wei), None),
        Err(error) => (false, None, Some(error)),
    };

    Json(EthwAddressStatusResponse {
        ethw_chain_id,
        ethw_network_id,
        ethw_runtime_profile: runtime_profile,
        address,
        balance_wei,
        latest_block_number,
        available,
        error,
    })
    .into_response()
}

async fn post_prepare_btc_mint(
    State(state): State<AppState>,
    Json(request): Json<BtcMintPrepareRequest>,
) -> Result<Json<BtcMintPrepareResponse>, (StatusCode, Json<ApiError>)> {
    let prepared = prepare_btc_mint_context(&state, &request).await?;
    Ok(Json(prepared.response))
}

async fn post_execute_btc_mint(
    State(state): State<AppState>,
    Json(request): Json<BtcMintExecuteRequest>,
) -> Result<Json<BtcMintExecuteResponse>, (StatusCode, Json<ApiError>)> {
    let prepare_request = BtcMintPrepareRequest {
        owner_address: request.owner_address.clone(),
        eth_main: request.eth_main.clone(),
        eth_collab: request.eth_collab.clone(),
        prev: request.prev.clone(),
    };
    let prepared = prepare_btc_mint_context(&state, &prepare_request).await?;
    if !prepared.response.eligible {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!(
                    "BTC mint execute is blocked until prepare blockers are cleared: {}",
                    prepared.response.blockers.join(" | ")
                ),
            }),
        ));
    }
    if prepared.runtime_profile != "development" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!(
                    "BTC mint execute is only available in development runtime, current runtime is {}",
                    prepared.runtime_profile
                ),
            }),
        ));
    }

    let wallet_name = normalize_required_text("wallet_name", &request.wallet_name)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;
    let marker = read_artifact_summary(
        &state.config,
        &state.config.bootstrap.world_sim_bootstrap_marker,
    );
    let marker_data = marker.data.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: marker.error.unwrap_or_else(|| {
                    "World-sim bootstrap marker is not available for mint execution".to_string()
                }),
            }),
        )
    })?;
    let identities = decode_world_sim_identities(marker_data)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;
    let identity = identities
        .into_iter()
        .find(|item| item.wallet_name == wallet_name)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError {
                    error: format!(
                        "Selected wallet_name {} is not present in the world-sim bootstrap marker",
                        wallet_name
                    ),
                }),
            )
        })?;
    if identity.owner_address != prepared.response.owner_address {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: format!(
                    "World-sim wallet {} resolves to owner {} but the mint request targets {}",
                    identity.wallet_name, identity.owner_address, prepared.response.owner_address
                ),
            }),
        ));
    }

    let execution = execute_world_sim_ord_mint(
        &state,
        &identity.wallet_name,
        &prepared.response.owner_address,
        &prepared.response.inscription_payload_json,
    )
    .await
    .map_err(|error| (StatusCode::BAD_GATEWAY, Json(ApiError { error })))?;

    Ok(Json(BtcMintExecuteResponse {
        btc_network: prepared.response.runtime.btc_network,
        btc_runtime_profile: prepared.runtime_profile,
        wallet_name: identity.wallet_name,
        owner_address: prepared.response.owner_address,
        inscription_payload_json: prepared.response.inscription_payload_json,
        inscription_id: execution.inscription_id,
        txid: execution.txid,
        ord_output: execution.ord_output,
    }))
}

async fn prepare_btc_mint_context(
    state: &AppState,
    request: &BtcMintPrepareRequest,
) -> Result<PreparedBtcMintContext, (StatusCode, Json<ApiError>)> {
    let services = build_services_summary(state).await;
    let capabilities = build_capabilities_summary(&services);
    let btc_network_name = resolve_runtime_btc_network_name(&services).ok_or_else(|| {
        let error =
            "Failed to resolve the active BTC runtime network from btc-node or balance-history"
                .to_string();
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                error: error.clone(),
            }),
        )
    })?;
    let btc_network = parse_balance_history_network(&btc_network_name).map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                error: format!(
                    "Unsupported BTC runtime network {}: {}",
                    btc_network_name, error
                ),
            }),
        )
    })?;
    let owner_address = normalize_required_text("owner_address", &request.owner_address)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;
    let owner_script_hash = address_string_to_script_hash(&owner_address, &btc_network)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?
        .to_string();
    let eth_main = normalize_evm_address("eth_main", &request.eth_main)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;
    let eth_collab = normalize_optional_evm_address("eth_collab", request.eth_collab.as_deref())
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;
    let prev = normalize_prev_list(&request.prev)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ApiError { error })))?;

    let ord_query_ready = services
        .ord
        .data
        .as_ref()
        .and_then(|item| item.query_ready)
        .unwrap_or(false);
    let balance_history_ready = services
        .balance_history
        .data
        .as_ref()
        .and_then(|item| item.query_ready)
        .unwrap_or(false);
    let usdb_indexer_ready = services
        .usdb_indexer
        .data
        .as_ref()
        .and_then(|item| item.query_ready)
        .unwrap_or(false);

    let active_pass = if usdb_indexer_ready {
        fetch_owner_active_pass_summary(state, &owner_script_hash)
            .await
            .map_err(|error| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(ApiError {
                        error: format!(
                            "Failed to resolve the current active pass for {}: {}",
                            owner_address, error
                        ),
                    }),
                )
            })?
    } else {
        None
    };

    let mut blockers = Vec::new();
    if capabilities.btc_console_mode != "inscription_enabled" {
        blockers.push(
            "The current BTC runtime is still in read-only mode. Start an inscription-enabled runtime before preparing mint flow."
                .to_string(),
        );
    }
    if !capabilities.ord_available {
        blockers.push("ORD backend is not available in the current stack.".to_string());
    } else if !ord_query_ready {
        blockers.push("ORD backend is online but not fully synced to the BTC tip yet.".to_string());
    }
    if !balance_history_ready {
        blockers.push(
            "balance-history is not query-ready yet. Wait until address balance queries are ready."
                .to_string(),
        );
    }
    if !usdb_indexer_ready {
        blockers.push(
            "usdb-indexer is not query-ready yet. Wait until miner-pass state queries are ready."
                .to_string(),
        );
    }

    let mut warnings = Vec::new();
    let suggested_prev = active_pass
        .as_ref()
        .map(|item| vec![item.inscription_id.clone()])
        .unwrap_or_default();
    if let Some(active_pass) = active_pass.as_ref() {
        if prev.is_empty() {
            warnings.push(format!(
                "Owner {} already has an active pass {}. A remint usually references the current active pass via prev.",
                owner_address, active_pass.inscription_id
            ));
        } else if !prev.contains(&active_pass.inscription_id) {
            warnings.push(format!(
                "Owner {} already has an active pass {}. Current prev does not reference it.",
                owner_address, active_pass.inscription_id
            ));
        }
    }

    let mut inscription_map = serde_json::Map::new();
    inscription_map.insert("p".to_string(), Value::String("usdb".to_string()));
    inscription_map.insert("op".to_string(), Value::String("mint".to_string()));
    inscription_map.insert("eth_main".to_string(), Value::String(eth_main.clone()));
    if let Some(value) = eth_collab.as_ref() {
        inscription_map.insert("eth_collab".to_string(), Value::String(value.clone()));
    }
    if !prev.is_empty() {
        inscription_map.insert(
            "prev".to_string(),
            Value::Array(prev.iter().cloned().map(Value::String).collect()),
        );
    }
    let inscription_payload = Value::Object(inscription_map);
    let inscription_payload_json =
        serde_json::to_string_pretty(&inscription_payload).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError {
                    error: format!("Failed to render inscription payload: {}", error),
                }),
            )
        })?;
    let prepare_request = json!({
        "prepare_mode": "draft_only",
        "protocol": "usdb-btc-mint-draft-v1",
        "wallet_signing": "psbt",
        "runtime_btc_network": btc_network_name.clone(),
        "owner_address": owner_address.clone(),
        "owner_script_hash": owner_script_hash.clone(),
        "inscription": {
            "content_type": "application/json",
            "payload": inscription_payload.clone(),
        },
        "psbt": Value::Null,
    });

    let runtime_profile = capabilities.btc_runtime_profile.clone();
    Ok(PreparedBtcMintContext {
        runtime_profile,
        response: BtcMintPrepareResponse {
            eligible: blockers.is_empty(),
            prepare_mode: "draft_only".to_string(),
            blockers,
            warnings,
            runtime: BtcMintPrepareRuntimeSummary {
                btc_network: btc_network_name,
                btc_runtime_profile: capabilities.btc_runtime_profile.clone(),
                btc_console_mode: capabilities.btc_console_mode,
                ord_available: capabilities.ord_available,
                ord_query_ready,
                balance_history_ready,
                usdb_indexer_ready,
                ord_synced_block_height: services
                    .ord
                    .data
                    .as_ref()
                    .and_then(|item| item.synced_block_height),
                btc_tip_height: services
                    .ord
                    .data
                    .as_ref()
                    .and_then(|item| item.btc_tip_height),
                ord_sync_gap: services.ord.data.as_ref().and_then(|item| item.sync_gap),
            },
            owner_address,
            owner_script_hash,
            eth_main,
            eth_collab,
            prev,
            suggested_prev,
            active_pass,
            inscription_payload,
            inscription_payload_json,
            prepare_request,
        },
    })
}

async fn post_balance_history_rpc(
    State(state): State<AppState>,
    Json(request): Json<ServiceRpcRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ApiError>)> {
    let request = normalize_balance_history_request(&state, request).await?;
    proxy_service_rpc(
        &state,
        &state.config.rpc.balance_history_url,
        "balance-history",
        request,
        BALANCE_HISTORY_PROXY_METHODS,
    )
    .await
}

async fn post_usdb_indexer_rpc(
    State(state): State<AppState>,
    Json(request): Json<ServiceRpcRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ApiError>)> {
    let request = normalize_usdb_indexer_request(&state, request).await?;
    proxy_service_rpc(
        &state,
        &state.config.rpc.usdb_indexer_url,
        "usdb-indexer",
        request,
        USDB_INDEXER_PROXY_METHODS,
    )
    .await
}

async fn build_overview(state: &AppState) -> OverviewResponse {
    let services = build_services_summary(state).await;
    let capabilities = build_capabilities_summary(&services);
    let bootstrap = build_bootstrap_summary(state);
    let explorers = build_explorer_links(state);
    let apps = build_app_entries(&services, &capabilities, &bootstrap, &explorers);

    OverviewResponse {
        service: USDB_CONTROL_PLANE_SERVICE_NAME.to_string(),
        generated_at_ms: current_unix_ms(),
        capabilities,
        services,
        bootstrap,
        explorers,
        apps,
    }
}

fn build_explorer_links(state: &AppState) -> ExplorerLinks {
    ExplorerLinks {
        control_console: "/#/overview".to_string(),
        balance_history: "/explorers/balance-history/".to_string(),
        usdb_indexer: "/explorers/usdb-indexer/".to_string(),
        sourcedao_web: state.config.web.sourcedao_web_url.clone(),
    }
}

fn app_status_from_service<T>(
    service: &ServiceProbe<T>,
    data: Option<&impl AppReadiness>,
) -> (bool, String, Option<String>) {
    if !service.reachable {
        return (
            false,
            "offline".to_string(),
            service
                .error
                .clone()
                .or_else(|| Some("Service is not reachable".to_string())),
        );
    }

    if data
        .and_then(AppReadiness::consensus_ready)
        .unwrap_or(false)
    {
        return (
            true,
            "ready".to_string(),
            data.and_then(AppReadiness::message),
        );
    }

    if data.and_then(AppReadiness::query_ready).unwrap_or(false) {
        return (
            true,
            "degraded".to_string(),
            data.and_then(AppReadiness::message),
        );
    }

    (
        true,
        "starting".to_string(),
        data.and_then(AppReadiness::message),
    )
}

trait AppReadiness {
    fn query_ready(&self) -> Option<bool>;
    fn consensus_ready(&self) -> Option<bool>;
    fn message(&self) -> Option<String>;
}

impl AppReadiness for BalanceHistoryServiceSummary {
    fn query_ready(&self) -> Option<bool> {
        self.query_ready
    }

    fn consensus_ready(&self) -> Option<bool> {
        self.consensus_ready
    }

    fn message(&self) -> Option<String> {
        self.message.clone()
    }
}

impl AppReadiness for UsdbIndexerServiceSummary {
    fn query_ready(&self) -> Option<bool> {
        self.query_ready
    }

    fn consensus_ready(&self) -> Option<bool> {
        self.consensus_ready
    }

    fn message(&self) -> Option<String> {
        self.message.clone()
    }
}

fn build_app_entries(
    services: &ServicesSummary,
    capabilities: &CapabilitiesSummary,
    bootstrap: &BootstrapSummary,
    explorers: &ExplorerLinks,
) -> Vec<AppEntry> {
    let balance_history_data = services.balance_history.data.as_ref();
    let (balance_history_available, balance_history_status, balance_history_message) =
        app_status_from_service(&services.balance_history, balance_history_data);

    let usdb_indexer_data = services.usdb_indexer.data.as_ref();
    let (usdb_indexer_available, usdb_indexer_status, usdb_indexer_message) =
        app_status_from_service(&services.usdb_indexer, usdb_indexer_data);

    let sourcedao_available =
        services.ethw.reachable && bootstrap.sourcedao_bootstrap_marker.exists;
    let sourcedao_status = if sourcedao_available {
        "configured"
    } else if services.ethw.reachable {
        "pending"
    } else {
        "offline"
    };
    let sourcedao_message = if sourcedao_available {
        Some(
            "SourceDAO bootstrap is configured. Start the standalone web app with docker/scripts/tools/run_local_sourcedao_web.sh up before opening this URL."
                .to_string(),
        )
    } else if let Some(error) = services.ethw.error.clone() {
        Some(error)
    } else {
        Some("SourceDAO bootstrap marker is not available yet".to_string())
    };

    vec![
        AppEntry {
            id: "balance_history_browser".to_string(),
            kind: "embedded_explorer".to_string(),
            url: explorers.balance_history.clone(),
            target: "same_origin".to_string(),
            runtime_profile: capabilities.btc_runtime_profile.clone(),
            network: balance_history_data
                .and_then(|item| item.network.clone())
                .or_else(|| {
                    services
                        .btc_node
                        .data
                        .as_ref()
                        .and_then(|item| item.chain.clone())
                }),
            service_id: Some("balance-history".to_string()),
            available: balance_history_available,
            status: balance_history_status,
            status_message: balance_history_message,
            depends_on: vec!["balance-history".to_string(), "btc-node".to_string()],
        },
        AppEntry {
            id: "usdb_indexer_browser".to_string(),
            kind: "embedded_explorer".to_string(),
            url: explorers.usdb_indexer.clone(),
            target: "same_origin".to_string(),
            runtime_profile: capabilities.btc_runtime_profile.clone(),
            network: usdb_indexer_data
                .and_then(|item| item.network.clone())
                .or_else(|| {
                    services
                        .btc_node
                        .data
                        .as_ref()
                        .and_then(|item| item.chain.clone())
                }),
            service_id: Some("usdb-indexer".to_string()),
            available: usdb_indexer_available,
            status: usdb_indexer_status,
            status_message: usdb_indexer_message,
            depends_on: vec![
                "usdb-indexer".to_string(),
                "balance-history".to_string(),
                "btc-node".to_string(),
            ],
        },
        AppEntry {
            id: "sourcedao_web".to_string(),
            kind: "external_web".to_string(),
            url: explorers.sourcedao_web.clone(),
            target: "external".to_string(),
            runtime_profile: capabilities.ethw_runtime_profile.clone(),
            network: services
                .ethw
                .data
                .as_ref()
                .and_then(|item| item.chain_id.clone()),
            service_id: Some("ethw".to_string()),
            available: sourcedao_available,
            status: sourcedao_status.to_string(),
            status_message: sourcedao_message,
            depends_on: vec!["ethw".to_string(), "sourcedao-bootstrap".to_string()],
        },
    ]
}

async fn proxy_service_rpc(
    state: &AppState,
    rpc_url: &str,
    service_name: &str,
    request: ServiceRpcRequest,
    allowed_methods: &[&str],
) -> Result<Json<Value>, (StatusCode, Json<ApiError>)> {
    if !request.params.is_array() {
        let msg = format!(
            "Rejected {} proxy call for method {}: params must be a JSON array",
            service_name, request.method
        );
        warn!("{}", msg);
        return Err((StatusCode::BAD_REQUEST, Json(ApiError { error: msg })));
    }

    if !allowed_methods.contains(&request.method.as_str()) {
        let msg = format!(
            "Rejected {} proxy call for disallowed method {}",
            service_name, request.method
        );
        warn!("{}", msg);
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: msg })));
    }

    let result = match service_name {
        "balance-history" => {
            state
                .rpc_client
                .balance_history_proxy(rpc_url, &request.method, request.params)
                .await
        }
        "usdb-indexer" => {
            state
                .rpc_client
                .usdb_indexer_proxy(rpc_url, &request.method, request.params)
                .await
        }
        _ => Err(format!("Unsupported proxy service {}", service_name)),
    };

    result.map(Json).map_err(|error| {
        warn!(
            "Proxy request failed: service={}, method={}, error={}",
            service_name, request.method, error
        );
        (StatusCode::BAD_GATEWAY, Json(ApiError { error }))
    })
}

async fn normalize_balance_history_request(
    state: &AppState,
    request: ServiceRpcRequest,
) -> Result<ServiceRpcRequest, (StatusCode, Json<ApiError>)> {
    if !matches!(
        request.method.as_str(),
        "get_address_balance"
            | "get_addresses_balances"
            | "get_address_balance_delta"
            | "get_addresses_balances_delta"
            | "get_address_balance_summary"
            | "get_address_balance_timeseries"
            | "get_address_flow_buckets"
    ) {
        return Ok(request);
    }

    let network_name = state
        .rpc_client
        .balance_history_network(&state.config.rpc.balance_history_url)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiError {
                    error: format!("Failed to resolve balance-history network: {}", error),
                }),
            )
        })?;
    let network = parse_balance_history_network(&network_name).map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                error: format!(
                    "Unsupported balance-history network {}: {}",
                    network_name, error
                ),
            }),
        )
    })?;

    let params = normalize_balance_history_params(&request.method, request.params, network)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ApiError { error: e })))?;

    Ok(ServiceRpcRequest {
        method: request.method,
        params,
    })
}

async fn normalize_usdb_indexer_request(
    state: &AppState,
    request: ServiceRpcRequest,
) -> Result<ServiceRpcRequest, (StatusCode, Json<ApiError>)> {
    if !matches!(
        request.method.as_str(),
        "get_owner_active_pass_at_height" | "get_owner_passes_at_height"
    ) {
        return Ok(request);
    }

    let blockchain_info = state
        .rpc_client
        .bitcoin_blockchain_info(&state.config)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(ApiError {
                    error: format!("Failed to resolve btc-node network: {}", error),
                }),
            )
        })?;
    let network = parse_balance_history_network(&blockchain_info.chain).map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                error: format!(
                    "Unsupported btc-node network {}: {}",
                    blockchain_info.chain, error
                ),
            }),
        )
    })?;

    let params = normalize_usdb_indexer_params(&request.method, request.params, network)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ApiError { error: e })))?;

    Ok(ServiceRpcRequest {
        method: request.method,
        params,
    })
}

fn parse_balance_history_network(network: &str) -> Result<Network, String> {
    match network.to_ascii_lowercase().as_str() {
        "bitcoin" | "main" | "mainnet" => Ok(Network::Bitcoin),
        "test" | "testnet" | "testnet3" => Ok(Network::Testnet),
        "testnet4" => Ok(Network::Testnet4),
        "regtest" => Ok(Network::Regtest),
        "signet" => Ok(Network::Signet),
        other => Err(format!("unknown network {}", other)),
    }
}

fn resolve_runtime_btc_network_name(services: &ServicesSummary) -> Option<String> {
    services
        .btc_node
        .data
        .as_ref()
        .and_then(|item| item.chain.as_ref())
        .or_else(|| {
            services
                .balance_history
                .data
                .as_ref()
                .and_then(|item| item.network.as_ref())
        })
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn classify_btc_runtime_profile(network: Option<&str>) -> &'static str {
    match network.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "regtest" => "development",
        Some(value)
            if matches!(
                value.as_str(),
                "bitcoin"
                    | "main"
                    | "mainnet"
                    | "test"
                    | "testnet"
                    | "testnet3"
                    | "testnet4"
                    | "signet"
            ) =>
        {
            "public"
        }
        _ => "unknown",
    }
}

fn classify_ethw_runtime_profile(chain_id: Option<&str>, network_id: Option<&str>) -> &'static str {
    let normalized_chain_id = chain_id.map(|value| value.trim().to_ascii_lowercase());
    let normalized_network_id = network_id.map(|value| value.trim().to_ascii_lowercase());
    let is_local_full_sim = normalized_chain_id
        .as_deref()
        .is_some_and(|value| value == "0x13525e3" || value == "20260323")
        || normalized_network_id
            .as_deref()
            .is_some_and(|value| value == "0x13525e3" || value == "20260323");

    if is_local_full_sim {
        return "development";
    }

    if normalized_chain_id
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || normalized_network_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    {
        return "public";
    }

    "unknown"
}

fn decode_world_sim_identities(marker_data: Value) -> Result<Vec<BtcWorldSimIdentity>, String> {
    let marker: WorldSimBootstrapMarker = serde_json::from_value(marker_data)
        .map_err(|error| format!("Failed to decode world-sim bootstrap marker: {}", error))?;
    let ethw_miner_agent_id = marker.ethw_miner_agent_id;

    if marker.agent_wallets.len() != marker.agent_addresses.len() {
        return Err(format!(
            "World-sim bootstrap marker contains mismatched identities: {} wallet names vs {} addresses",
            marker.agent_wallets.len(),
            marker.agent_addresses.len()
        ));
    }
    if marker.agent_wallets.is_empty() {
        return Err("World-sim bootstrap marker does not contain any agent identities".to_string());
    }
    if let Some(agent_id) = ethw_miner_agent_id
        && agent_id >= marker.agent_wallets.len()
    {
        return Err(format!(
            "World-sim bootstrap marker has out-of-range ethw_miner_agent_id {} for {} identities",
            agent_id,
            marker.agent_wallets.len()
        ));
    }

    marker
        .agent_wallets
        .into_iter()
        .zip(marker.agent_addresses)
        .enumerate()
        .map(|(agent_id, (wallet_name, owner_address))| {
            let wallet_name = wallet_name.trim();
            if wallet_name.is_empty() {
                return Err(format!(
                    "World-sim bootstrap marker contains an empty wallet_name at agent {}",
                    agent_id
                ));
            }

            let owner_address = owner_address.trim();
            if owner_address.is_empty() {
                return Err(format!(
                    "World-sim bootstrap marker contains an empty owner_address at agent {}",
                    agent_id
                ));
            }

            Ok(BtcWorldSimIdentity {
                agent_id,
                wallet_name: wallet_name.to_string(),
                owner_address: owner_address.to_string(),
                is_ethw_aligned: ethw_miner_agent_id == Some(agent_id),
            })
        })
        .collect()
}

fn normalize_balance_history_params(
    method: &str,
    params: Value,
    network: Network,
) -> Result<Value, String> {
    let Some(items) = params.as_array() else {
        return Err("Balance-history params must be a JSON array".to_string());
    };
    if items.is_empty() {
        return Err(format!("{} requires one params object", method));
    }

    let mut normalized = items.clone();
    let first = normalized
        .get_mut(0)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("{} requires the first param to be an object", method))?;

    match method {
        "get_address_balance"
        | "get_address_balance_delta"
        | "get_address_balance_summary"
        | "get_address_balance_timeseries"
        | "get_address_flow_buckets" => {
            let candidate = first
                .get("script_hash")
                .and_then(Value::as_str)
                .or_else(|| first.get("address").and_then(Value::as_str))
                .ok_or_else(|| {
                    "Provide either script_hash or address for balance-history single queries"
                        .to_string()
                })?
                .to_string();
            let normalized_hash = parse_script_hash_any(candidate.as_str(), &network)
                .map_err(|e| format!("Failed to resolve {}: {}", candidate, e))?
                .to_string();
            first.insert("script_hash".to_string(), Value::String(normalized_hash));
            first.remove("address");
        }
        "get_addresses_balances" | "get_addresses_balances_delta" => {
            let candidates = first
                .get("script_hashes")
                .and_then(Value::as_array)
                .or_else(|| first.get("addresses").and_then(Value::as_array))
                .ok_or_else(|| {
                    "Provide either script_hashes or addresses for balance-history batch queries"
                        .to_string()
                })?;

            let mut normalized_hashes = Vec::with_capacity(candidates.len());
            for value in candidates {
                let candidate = value
                    .as_str()
                    .ok_or_else(|| {
                        "Every balance-history batch query target must be a string".to_string()
                    })?
                    .to_string();
                let normalized_hash = parse_script_hash_any(candidate.as_str(), &network)
                    .map_err(|e| format!("Failed to resolve {}: {}", candidate, e))?
                    .to_string();
                normalized_hashes.push(Value::String(normalized_hash));
            }
            first.insert("script_hashes".to_string(), Value::Array(normalized_hashes));
            first.remove("addresses");
        }
        _ => {}
    }

    Ok(Value::Array(normalized))
}

fn normalize_usdb_indexer_params(
    method: &str,
    params: Value,
    network: Network,
) -> Result<Value, String> {
    let Some(items) = params.as_array() else {
        return Err("usdb-indexer params must be a JSON array".to_string());
    };
    if items.is_empty() {
        return Err(format!("{} requires one params object", method));
    }

    let mut normalized = items.clone();
    let first = normalized
        .get_mut(0)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("{} requires the first param to be an object", method))?;

    if matches!(
        method,
        "get_owner_active_pass_at_height" | "get_owner_passes_at_height"
    ) {
        let candidate = first
            .get("owner")
            .and_then(Value::as_str)
            .or_else(|| first.get("address").and_then(Value::as_str))
            .ok_or_else(|| "Provide either owner or address for owner pass queries".to_string())?
            .to_string();
        let normalized_owner = parse_script_hash_any(candidate.as_str(), &network)
            .map_err(|e| format!("Failed to resolve {}: {}", candidate, e))?
            .to_string();
        first.insert("owner".to_string(), Value::String(normalized_owner));
        first.remove("address");
    }

    Ok(Value::Array(normalized))
}

fn normalize_required_text(field: &str, value: &str) -> Result<String, String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(format!("{} is required", field));
    }
    Ok(normalized.to_string())
}

fn normalize_evm_address(field: &str, value: &str) -> Result<String, String> {
    let normalized = normalize_required_text(field, value)?;
    if !is_valid_evm_address(&normalized) {
        return Err(format!(
            "{} must be a valid EVM address with 0x prefix and 40 hexadecimal characters",
            field
        ));
    }
    Ok(normalized)
}

fn normalize_optional_evm_address(
    field: &str,
    value: Option<&str>,
) -> Result<Option<String>, String> {
    match value {
        Some(item) if !item.trim().is_empty() => normalize_evm_address(field, item).map(Some),
        _ => Ok(None),
    }
}

fn normalize_prev_list(values: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::with_capacity(values.len());
    for item in values {
        let candidate = item.trim();
        if candidate.is_empty() {
            continue;
        }
        if !is_valid_inscription_id(candidate) {
            return Err(format!(
                "prev entry {} must follow the inscription id format <txid>i<index>",
                candidate
            ));
        }
        if !normalized.iter().any(|existing| existing == candidate) {
            normalized.push(candidate.to_string());
        }
    }
    Ok(normalized)
}

fn is_valid_evm_address(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..].chars().all(|char| char.is_ascii_hexdigit())
}

fn is_valid_inscription_id(value: &str) -> bool {
    let Some((txid, index)) = value.split_once('i') else {
        return false;
    };
    txid.len() == 64
        && txid.chars().all(|char| char.is_ascii_hexdigit())
        && !index.is_empty()
        && index.chars().all(|char| char.is_ascii_digit())
}

struct OrdMintExecution {
    inscription_id: String,
    txid: Option<String>,
    ord_output: String,
}

async fn execute_world_sim_ord_mint(
    state: &AppState,
    wallet_name: &str,
    destination: &str,
    inscription_payload_json: &str,
) -> Result<OrdMintExecution, String> {
    let payload_dir = state
        .config
        .resolve_runtime_path(&state.config.root_dir)?
        .join("runtime")
        .join("btc-mint");
    std::fs::create_dir_all(&payload_dir).map_err(|error| {
        let msg = format!(
            "Failed to create BTC mint runtime directory {}: {}",
            payload_dir.display(),
            error
        );
        error!("{}", msg);
        msg
    })?;
    let payload_path = payload_dir.join(format!("{}-mint.json", wallet_name));
    std::fs::write(&payload_path, inscription_payload_json).map_err(|error| {
        let msg = format!(
            "Failed to write BTC mint payload file {}: {}",
            payload_path.display(),
            error
        );
        error!("{}", msg);
        msg
    })?;

    let ord_bin = state
        .config
        .resolve_runtime_path(&state.config.development_mint.ord_bin)?;
    let ord_data_dir = state
        .config
        .resolve_runtime_path(&state.config.development_mint.ord_data_dir)?;
    let cookie_file = state
        .config
        .bitcoin
        .cookie_file
        .as_ref()
        .map(|path| state.config.resolve_runtime_path(path))
        .transpose()?;

    let mut command = Command::new(&ord_bin);
    command
        .arg("--data-dir")
        .arg(&ord_data_dir)
        .arg("--bitcoin-rpc-url")
        .arg(&state.config.bitcoin.url)
        .arg("--chain")
        .arg("regtest")
        .arg("--format")
        .arg("json");
    match state.config.bitcoin.auth_mode {
        crate::config::BitcoinAuthMode::None => {}
        crate::config::BitcoinAuthMode::Cookie => {
            let cookie_path = cookie_file.ok_or_else(|| {
                "Development mint execution requires btc cookie_file when auth_mode=cookie"
                    .to_string()
            })?;
            command.arg("--cookie-file").arg(cookie_path);
        }
        crate::config::BitcoinAuthMode::Userpass => {
            let rpc_user = state.config.bitcoin.rpc_user.as_deref().ok_or_else(|| {
                "Development mint execution requires bitcoin.rpc_user".to_string()
            })?;
            let rpc_password = state
                .config
                .bitcoin
                .rpc_password
                .as_deref()
                .ok_or_else(|| {
                    "Development mint execution requires bitcoin.rpc_password".to_string()
                })?;
            command
                .arg("--bitcoin-rpc-username")
                .arg(rpc_user)
                .arg("--bitcoin-rpc-password")
                .arg(rpc_password);
        }
    }
    command
        .arg("wallet")
        .arg("--no-sync")
        .arg("--server-url")
        .arg(&state.config.rpc.ord_url)
        .arg("--name")
        .arg(wallet_name)
        .arg("inscribe")
        .arg("--fee-rate")
        .arg(format!("{}", state.config.development_mint.ord_fee_rate))
        .arg("--destination")
        .arg(destination)
        .arg("--file")
        .arg(&payload_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().await.map_err(|error| {
        let msg = format!(
            "Failed to launch ord mint execution for wallet {} via {}: {}",
            wallet_name,
            ord_bin.display(),
            error
        );
        error!("{}", msg);
        msg
    })?;
    let mut ord_output = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_output = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr_output.is_empty() {
        if !ord_output.is_empty() {
            ord_output.push('\n');
        }
        ord_output.push_str(&stderr_output);
    }
    if !output.status.success() {
        let msg = format!(
            "ord wallet inscribe failed for wallet {} (status={}): {}",
            wallet_name,
            output.status,
            ord_output.trim()
        );
        error!("{}", msg);
        return Err(msg);
    }

    let inscription_id = extract_inscription_id(&ord_output).ok_or_else(|| {
        let msg = format!(
            "Failed to parse inscription id from ord mint output for wallet {}: {}",
            wallet_name,
            ord_output.trim()
        );
        error!("{}", msg);
        msg
    })?;
    Ok(OrdMintExecution {
        inscription_id,
        txid: extract_hex64(&ord_output),
        ord_output,
    })
}

fn extract_inscription_id(raw: &str) -> Option<String> {
    if let Some(parsed) = extract_ord_mint_json_output(raw)
        && let Some(inscription_id) = parsed
            .inscriptions
            .into_iter()
            .map(|item| item.id)
            .find(|candidate| is_valid_inscription_id(candidate))
    {
        return Some(inscription_id);
    }
    extract_whitespace_separated(raw)
        .find(|candidate| is_valid_inscription_id(candidate))
        .map(|candidate| candidate.to_string())
}

fn extract_hex64(raw: &str) -> Option<String> {
    if let Some(parsed) = extract_ord_mint_json_output(raw) {
        if let Some(reveal_txid) = parsed.reveal.filter(|candidate| is_hex64(candidate)) {
            return Some(reveal_txid);
        }
        if let Some(inscription_id) = parsed
            .inscriptions
            .first()
            .map(|item| item.id.as_str())
            .filter(|candidate| is_valid_inscription_id(candidate))
            && let Some((txid, _)) = inscription_id.split_once('i')
            && is_hex64(txid)
        {
            return Some(txid.to_string());
        }
        if let Some(commit_txid) = parsed.commit.filter(|candidate| is_hex64(candidate)) {
            return Some(commit_txid);
        }
    }
    extract_whitespace_separated(raw)
        .find(|candidate| is_hex64(candidate))
        .map(|candidate| candidate.to_string())
}

fn extract_ord_mint_json_output(raw: &str) -> Option<OrdMintJsonOutput> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<OrdMintJsonOutput>(trimmed) {
        return Some(parsed);
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<OrdMintJsonOutput>(&trimmed[start..=end]).ok()
}

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|char| char.is_ascii_hexdigit())
}

fn extract_whitespace_separated(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(|char: char| {
        char.is_whitespace()
            || matches!(
                char,
                '"' | '\'' | '{' | '}' | '[' | ']' | ',' | ':' | '(' | ')' | ';'
            )
    })
    .filter(|candidate| !candidate.is_empty())
}

async fn fetch_owner_active_pass_summary(
    state: &AppState,
    owner_script_hash: &str,
) -> Result<Option<BtcMintPrepareActivePassSummary>, String> {
    let response = state
        .rpc_client
        .usdb_indexer_proxy(
            &state.config.rpc.usdb_indexer_url,
            "get_owner_active_pass_at_height",
            json!([{
                "owner": owner_script_hash,
                "at_height": Value::Null,
            }]),
        )
        .await?;

    serde_json::from_value(response).map_err(|error| {
        format!(
            "Failed to decode active pass summary for owner {}: {}",
            owner_script_hash, error
        )
    })
}

async fn build_services_summary(state: &AppState) -> ServicesSummary {
    let btc_node = probe_btc_node(state).await;
    let balance_history = probe_balance_history(state).await;
    let usdb_indexer = probe_usdb_indexer(state).await;
    let ethw = probe_ethw(state).await;
    let ord = probe_ord(state).await;
    ServicesSummary {
        btc_node,
        balance_history,
        usdb_indexer,
        ethw,
        ord,
    }
}

fn build_capabilities_summary(services: &ServicesSummary) -> CapabilitiesSummary {
    let ord_available = services.ord.reachable
        && services
            .ord
            .data
            .as_ref()
            .and_then(|item| item.backend_ready)
            .unwrap_or(false);
    let btc_runtime_profile =
        classify_btc_runtime_profile(resolve_runtime_btc_network_name(services).as_deref())
            .to_string();
    let ethw_runtime_profile = classify_ethw_runtime_profile(
        services
            .ethw
            .data
            .as_ref()
            .and_then(|summary| summary.chain_id.as_deref()),
        services
            .ethw
            .data
            .as_ref()
            .and_then(|summary| summary.network_id.as_deref()),
    )
    .to_string();

    CapabilitiesSummary {
        ord_available,
        btc_runtime_profile,
        ethw_runtime_profile,
        btc_console_mode: if ord_available {
            "inscription_enabled".to_string()
        } else {
            "read_only".to_string()
        },
    }
}

fn build_bootstrap_summary(state: &AppState) -> BootstrapSummary {
    let bootstrap_manifest =
        read_artifact_summary(&state.config, &state.config.bootstrap.bootstrap_manifest);
    let snapshot_marker =
        read_artifact_summary(&state.config, &state.config.bootstrap.snapshot_marker);
    let ethw_init_marker =
        read_artifact_summary(&state.config, &state.config.bootstrap.ethw_init_marker);
    let ethw_genesis = read_artifact_summary(&state.config, &state.config.bootstrap.ethw_genesis);
    let sourcedao_bootstrap_state = read_artifact_summary(
        &state.config,
        &state.config.bootstrap.sourcedao_bootstrap_state,
    );
    let sourcedao_bootstrap_marker = read_artifact_summary(
        &state.config,
        &state.config.bootstrap.sourcedao_bootstrap_marker,
    );
    let steps = vec![
        derive_snapshot_loader_step(&bootstrap_manifest, &snapshot_marker),
        derive_step_state("bootstrap-init", &bootstrap_manifest),
        derive_step_state("ethw-init", &ethw_init_marker),
        derive_step_state("sourcedao-bootstrap", &sourcedao_bootstrap_marker),
    ];
    let overall_state = if steps.iter().any(|step| step.state == "error") {
        "error".to_string()
    } else if steps
        .iter()
        .all(|step| matches!(step.state.as_str(), "completed" | "skipped"))
    {
        "completed".to_string()
    } else if steps
        .iter()
        .any(|step| matches!(step.state.as_str(), "completed" | "skipped"))
    {
        "in_progress".to_string()
    } else {
        "pending".to_string()
    };

    BootstrapSummary {
        bootstrap_manifest,
        snapshot_marker,
        ethw_init_marker,
        ethw_genesis,
        sourcedao_bootstrap_state,
        sourcedao_bootstrap_marker,
        steps,
        overall_state,
    }
}

async fn probe_btc_node(state: &AppState) -> ServiceProbe<BtcNodeServiceSummary> {
    let rpc_url = state.config.bitcoin.url.clone();
    let started = Instant::now();
    let info = state
        .rpc_client
        .bitcoin_blockchain_info(&state.config)
        .await;
    let header = if let Ok(info) = &info {
        state
            .rpc_client
            .bitcoin_block_header(&state.config, &info.best_block_hash)
            .await
            .ok()
    } else {
        None
    };
    let latency_ms = started.elapsed().as_millis() as u64;
    let error = info.as_ref().err().cloned();
    let reachable = info.is_ok();
    let data = info.ok().map(|item| BtcNodeServiceSummary {
        chain: Some(item.chain),
        blocks: Some(item.blocks),
        headers: Some(item.headers),
        best_block_hash: Some(item.best_block_hash),
        best_block_time: header.map(|entry| entry.time),
        verification_progress: Some(item.verification_progress),
        initial_block_download: Some(item.initial_block_download),
    });

    ServiceProbe {
        name: "btc-node".to_string(),
        rpc_url,
        reachable,
        latency_ms: Some(latency_ms),
        error,
        data,
    }
}

async fn probe_balance_history(state: &AppState) -> ServiceProbe<BalanceHistoryServiceSummary> {
    let rpc_url = state.config.rpc.balance_history_url.clone();
    let started = Instant::now();
    let (network, readiness) = tokio::join!(
        state.rpc_client.balance_history_network(&rpc_url),
        state.rpc_client.balance_history_readiness(&rpc_url)
    );
    let latency_ms = started.elapsed().as_millis() as u64;
    let network_error = network.as_ref().err().cloned();
    let readiness_error = readiness.as_ref().err().cloned();

    let reachable = network.is_ok() || readiness.is_ok();
    let data = if network.is_ok() || readiness.is_ok() {
        let readiness = readiness.ok();
        Some(BalanceHistoryServiceSummary {
            network: network.ok(),
            rpc_alive: readiness.as_ref().map(|item| item.rpc_alive),
            query_ready: readiness.as_ref().map(|item| item.query_ready),
            consensus_ready: readiness.as_ref().map(|item| item.consensus_ready),
            phase: readiness.as_ref().map(|item| item.phase.clone()),
            message: readiness.as_ref().and_then(|item| item.message.clone()),
            current: readiness.as_ref().map(|item| item.current),
            total: readiness.as_ref().map(|item| item.total),
            stable_height: readiness.as_ref().and_then(|item| item.stable_height),
            stable_block_hash: readiness
                .as_ref()
                .and_then(|item| item.stable_block_hash.clone()),
            latest_block_commit: readiness
                .as_ref()
                .and_then(|item| item.latest_block_commit.clone()),
            snapshot_verification_state: readiness
                .as_ref()
                .and_then(|item| item.snapshot_verification_state.clone()),
            snapshot_signing_key_id: readiness
                .as_ref()
                .and_then(|item| item.snapshot_signing_key_id.clone()),
            blockers: readiness
                .as_ref()
                .map(|item| item.blockers.clone())
                .unwrap_or_default(),
        })
    } else {
        None
    };

    ServiceProbe {
        name: "balance-history".to_string(),
        rpc_url,
        reachable,
        latency_ms: Some(latency_ms),
        error: merge_errors(&[network_error, readiness_error]),
        data,
    }
}

async fn probe_usdb_indexer(state: &AppState) -> ServiceProbe<UsdbIndexerServiceSummary> {
    let rpc_url = state.config.rpc.usdb_indexer_url.clone();
    let started = Instant::now();
    let (network, readiness) = tokio::join!(
        state.rpc_client.usdb_indexer_network(&rpc_url),
        state.rpc_client.usdb_indexer_readiness(&rpc_url)
    );
    let latency_ms = started.elapsed().as_millis() as u64;
    let network_error = network.as_ref().err().cloned();
    let readiness_error = readiness.as_ref().err().cloned();

    let reachable = network.is_ok() || readiness.is_ok();
    let data = if network.is_ok() || readiness.is_ok() {
        let readiness = readiness.ok();
        Some(UsdbIndexerServiceSummary {
            network: network.ok(),
            rpc_alive: readiness.as_ref().map(|item| item.rpc_alive),
            query_ready: readiness.as_ref().map(|item| item.query_ready),
            consensus_ready: readiness.as_ref().map(|item| item.consensus_ready),
            message: readiness.as_ref().and_then(|item| item.message.clone()),
            current: readiness.as_ref().map(|item| item.current),
            total: readiness.as_ref().map(|item| item.total),
            synced_block_height: readiness.as_ref().and_then(|item| item.synced_block_height),
            balance_history_stable_height: readiness
                .as_ref()
                .and_then(|item| item.balance_history_stable_height),
            upstream_snapshot_id: readiness
                .as_ref()
                .and_then(|item| item.upstream_snapshot_id.clone()),
            local_state_commit: readiness
                .as_ref()
                .and_then(|item| item.local_state_commit.clone()),
            system_state_id: readiness
                .as_ref()
                .and_then(|item| item.system_state_id.clone()),
            blockers: readiness
                .as_ref()
                .map(|item| item.blockers.clone())
                .unwrap_or_default(),
        })
    } else {
        None
    };

    ServiceProbe {
        name: "usdb-indexer".to_string(),
        rpc_url,
        reachable,
        latency_ms: Some(latency_ms),
        error: merge_errors(&[network_error, readiness_error]),
        data,
    }
}

async fn probe_ethw(state: &AppState) -> ServiceProbe<EthwServiceSummary> {
    let rpc_url = state.config.rpc.ethw_url.clone();
    let started = Instant::now();
    let (client_version, chain_id, network_id, block_number, latest_block, syncing) = tokio::join!(
        state.rpc_client.ethw_client_version(&rpc_url),
        state.rpc_client.ethw_chain_id(&rpc_url),
        state.rpc_client.ethw_network_id(&rpc_url),
        state.rpc_client.ethw_block_number(&rpc_url),
        state.rpc_client.ethw_latest_block(&rpc_url),
        state.rpc_client.ethw_syncing(&rpc_url)
    );
    let latency_ms = started.elapsed().as_millis() as u64;
    let client_version_error = client_version.as_ref().err().cloned();
    let chain_id_error = chain_id.as_ref().err().cloned();
    let network_id_error = network_id.as_ref().err().cloned();
    let block_number_error = block_number.as_ref().err().cloned();
    let latest_block_error = latest_block.as_ref().err().cloned();
    let syncing_error = syncing.as_ref().err().cloned();
    let reachable = client_version.is_ok()
        || chain_id.is_ok()
        || network_id.is_ok()
        || block_number.is_ok()
        || latest_block.is_ok()
        || syncing.is_ok();

    let block_number_value = block_number
        .as_ref()
        .ok()
        .and_then(|value| decode_hex_quantity(value).ok());
    let latest_block_hash = latest_block
        .as_ref()
        .ok()
        .and_then(|value| value.as_ref())
        .and_then(|value| value.hash.clone());
    let latest_block_time = latest_block
        .as_ref()
        .ok()
        .and_then(|value| value.as_ref())
        .and_then(|value| decode_hex_quantity(&value.timestamp).ok());
    let syncing_value = syncing.ok();
    let query_ready = Some(reachable && block_number_value.is_some());
    let consensus_ready = match (query_ready, syncing_value.as_ref()) {
        (Some(false), _) => Some(false),
        (Some(true), Some(Value::Bool(false))) => Some(true),
        (Some(true), Some(_)) => Some(false),
        (Some(true), None) => None,
        _ => None,
    };

    let data = if reachable {
        Some(EthwServiceSummary {
            client_version: client_version.ok(),
            chain_id: chain_id.ok(),
            network_id: network_id.ok(),
            block_number: block_number_value,
            latest_block_hash,
            latest_block_time,
            syncing: syncing_value,
            query_ready,
            consensus_ready,
        })
    } else {
        None
    };

    ServiceProbe {
        name: "ethw".to_string(),
        rpc_url,
        reachable,
        latency_ms: Some(latency_ms),
        error: merge_errors(&[
            client_version_error,
            chain_id_error,
            network_id_error,
            block_number_error,
            latest_block_error,
            syncing_error,
        ]),
        data,
    }
}

async fn probe_ord(state: &AppState) -> ServiceProbe<OrdServiceSummary> {
    let rpc_url = state.config.rpc.ord_url.clone();
    let blockcount_url = format!("{}/blockcount", rpc_url.trim_end_matches('/'));
    let started = Instant::now();
    let (probe, ord_height, btc_tip_height) = tokio::join!(
        state.rpc_client.http_probe(&rpc_url),
        state.rpc_client.http_text(&blockcount_url),
        state.rpc_client.bitcoin_blockchain_info(&state.config)
    );
    let latency_ms = started.elapsed().as_millis() as u64;
    let error = merge_errors(&[
        probe.as_ref().err().cloned(),
        ord_height.as_ref().err().cloned(),
        btc_tip_height.as_ref().err().cloned(),
    ]);
    let reachable = probe.is_ok() || ord_height.is_ok();
    let http_status = probe.ok();
    let ord_height_value = ord_height
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let btc_tip_height_value = btc_tip_height.ok().map(|info| info.blocks);
    let sync_gap = match (ord_height_value, btc_tip_height_value) {
        (Some(ord_height), Some(btc_tip_height)) => Some(btc_tip_height.saturating_sub(ord_height)),
        _ => None,
    };
    let backend_ready = http_status.map(|status| (200..500).contains(&status));
    let query_ready = match (backend_ready, sync_gap) {
        (Some(true), Some(0)) => Some(true),
        (Some(true), Some(_)) => Some(false),
        (Some(false), _) => Some(false),
        _ => None,
    };
    let data = if reachable {
        Some(OrdServiceSummary {
            http_status,
            backend_ready,
            query_ready,
            synced_block_height: ord_height_value,
            btc_tip_height: btc_tip_height_value,
            sync_gap,
        })
    } else {
        None
    };

    ServiceProbe {
        name: "ord".to_string(),
        rpc_url,
        reachable,
        latency_ms: Some(latency_ms),
        error,
        data,
    }
}

fn read_artifact_summary(config: &ControlPlaneConfig, configured_path: &Path) -> ArtifactSummary {
    let resolved = config
        .resolve_runtime_path(configured_path)
        .unwrap_or_else(|_| configured_path.to_path_buf());
    let path_str = resolved.display().to_string();
    if !resolved.exists() {
        return ArtifactSummary {
            path: path_str,
            exists: false,
            error: Some("artifact file does not exist".to_string()),
            data: None,
        };
    }

    match std::fs::read_to_string(&resolved) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(data) => ArtifactSummary {
                path: path_str,
                exists: true,
                error: None,
                data: Some(data),
            },
            Err(e) => ArtifactSummary {
                path: path_str,
                exists: true,
                error: Some(format!("failed to parse JSON: {}", e)),
                data: None,
            },
        },
        Err(e) => ArtifactSummary {
            path: path_str,
            exists: true,
            error: Some(format!("failed to read file: {}", e)),
            data: None,
        },
    }
}

fn derive_step_state(step: &str, artifact: &ArtifactSummary) -> BootstrapStepSummary {
    let state = if artifact.exists && artifact.error.is_none() {
        "completed"
    } else if artifact.exists && artifact.error.is_some() {
        "error"
    } else {
        "pending"
    };

    BootstrapStepSummary {
        step: step.to_string(),
        state: state.to_string(),
        artifact_path: short_path(&artifact.path),
        error: artifact.error.clone(),
    }
}

fn derive_snapshot_loader_step(
    bootstrap_manifest: &ArtifactSummary,
    snapshot_marker: &ArtifactSummary,
) -> BootstrapStepSummary {
    let snapshot_mode = bootstrap_manifest
        .data
        .as_ref()
        .and_then(|value| value.get("balance_history_snapshot_mode"))
        .and_then(Value::as_str);

    if snapshot_mode == Some("none") {
        return BootstrapStepSummary {
            step: "snapshot-loader".to_string(),
            state: "skipped".to_string(),
            artifact_path: short_path(&snapshot_marker.path),
            error: None,
        };
    }

    derive_step_state("snapshot-loader", snapshot_marker)
}

fn short_path(path: &str) -> String {
    if path.len() <= 72 {
        path.to_string()
    } else {
        format!("...{}", &path[path.len() - 69..])
    }
}

fn merge_errors(errors: &[Option<String>]) -> Option<String> {
    let values: Vec<String> = errors.iter().filter_map(|item| item.clone()).collect();
    if values.is_empty() {
        None
    } else {
        Some(values.join(" | "))
    }
}

fn ensure_dir_exists(label: &str, path: &Path) -> Result<(), String> {
    if !path.exists() {
        let msg = format!("{} does not exist: {}", label, path.display());
        error!("{}", msg);
        return Err(msg);
    }
    if !path.is_dir() {
        let msg = format!("{} is not a directory: {}", label, path.display());
        error!("{}", msg);
        return Err(msg);
    }
    Ok(())
}

fn current_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use usdb_util::address_string_to_script_hash;

    #[test]
    fn merge_errors_skips_empty_entries() {
        let merged = merge_errors(&[None, Some("a".to_string()), Some("b".to_string())]);
        assert_eq!(merged, Some("a | b".to_string()));
    }

    #[test]
    fn derive_step_state_reports_pending_for_missing_artifact() {
        let artifact = ArtifactSummary {
            path: "/tmp/missing.json".to_string(),
            exists: false,
            error: Some("artifact file does not exist".to_string()),
            data: None,
        };
        let step = derive_step_state("snapshot-loader", &artifact);
        assert_eq!(step.state, "pending");
        assert_eq!(step.artifact_path, "/tmp/missing.json");
    }

    #[test]
    fn derive_snapshot_loader_step_reports_skipped_when_snapshot_mode_is_none() {
        let bootstrap_manifest = ArtifactSummary {
            path: "/tmp/bootstrap-manifest.json".to_string(),
            exists: true,
            error: None,
            data: Some(json!({
                "balance_history_snapshot_mode": "none"
            })),
        };
        let snapshot_marker = ArtifactSummary {
            path: "/tmp/snapshot-loader.done.json".to_string(),
            exists: false,
            error: Some("artifact file does not exist".to_string()),
            data: None,
        };

        let step = derive_snapshot_loader_step(&bootstrap_manifest, &snapshot_marker);
        assert_eq!(step.step, "snapshot-loader");
        assert_eq!(step.state, "skipped");
        assert_eq!(step.artifact_path, "/tmp/snapshot-loader.done.json");
        assert!(step.error.is_none());
    }

    #[test]
    fn normalize_balance_history_single_query_accepts_address() {
        let address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let expected = address_string_to_script_hash(address, &Network::Bitcoin)
            .unwrap()
            .to_string();

        for method in [
            "get_address_balance",
            "get_address_balance_delta",
            "get_address_balance_summary",
            "get_address_balance_timeseries",
            "get_address_flow_buckets",
        ] {
            assert!(BALANCE_HISTORY_PROXY_METHODS.contains(&method));
            let normalized = normalize_balance_history_params(
                method,
                json!([{
                    "address": address,
                    "block_height": null,
                    "block_range": null
                }]),
                Network::Bitcoin,
            )
            .unwrap();

            assert_eq!(normalized[0]["script_hash"], expected);
            assert!(normalized[0].get("address").is_none());
        }
    }

    #[test]
    fn normalize_balance_history_batch_query_accepts_mixed_inputs() {
        let address = "bc1qm34lsc65zpw79lxes69zkqmk6ee3ewf0j77s3h";
        let existing_address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let normalized_address = address_string_to_script_hash(address, &Network::Bitcoin)
            .unwrap()
            .to_string();
        let existing_hash = address_string_to_script_hash(existing_address, &Network::Bitcoin)
            .unwrap()
            .to_string();

        let normalized = normalize_balance_history_params(
            "get_addresses_balances",
            json!([{
                "script_hashes": [address, existing_hash],
                "block_height": null,
                "block_range": null
            }]),
            Network::Bitcoin,
        )
        .unwrap();

        assert_eq!(normalized[0]["script_hashes"][0], normalized_address);
        assert_eq!(normalized[0]["script_hashes"][1], existing_hash);
    }

    #[test]
    fn normalize_usdb_indexer_owner_query_accepts_address() {
        let address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let expected = address_string_to_script_hash(address, &Network::Bitcoin)
            .unwrap()
            .to_string();

        for method in [
            "get_owner_active_pass_at_height",
            "get_owner_passes_at_height",
        ] {
            assert!(USDB_INDEXER_PROXY_METHODS.contains(&method));
            let normalized = normalize_usdb_indexer_params(
                method,
                json!([{
                    "address": address,
                    "at_height": null
                }]),
                Network::Bitcoin,
            )
            .unwrap();

            assert_eq!(normalized[0]["owner"], expected);
            assert!(normalized[0].get("address").is_none());
        }
    }

    #[test]
    fn normalize_evm_address_accepts_lowercase_hex() {
        let value = "0x1111111111111111111111111111111111111111";
        assert_eq!(normalize_evm_address("eth_main", value).unwrap(), value);
    }

    #[test]
    fn normalize_prev_list_deduplicates_and_rejects_invalid_ids() {
        let valid = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdefi0";
        let duplicate = valid.to_string();
        let normalized =
            normalize_prev_list(&[valid.to_string(), duplicate, " ".to_string()]).unwrap();
        assert_eq!(normalized, vec![valid.to_string()]);
        assert!(normalize_prev_list(&["bad-prev".to_string()]).is_err());
    }

    #[test]
    fn classify_btc_runtime_profile_matches_expected_networks() {
        assert_eq!(classify_btc_runtime_profile(Some("regtest")), "development");
        assert_eq!(classify_btc_runtime_profile(Some("bitcoin")), "public");
        assert_eq!(classify_btc_runtime_profile(Some("testnet4")), "public");
        assert_eq!(classify_btc_runtime_profile(Some("signet")), "public");
        assert_eq!(classify_btc_runtime_profile(Some("unknown-net")), "unknown");
        assert_eq!(classify_btc_runtime_profile(None), "unknown");
    }

    #[test]
    fn classify_ethw_runtime_profile_matches_local_full_sim_chain() {
        assert_eq!(
            classify_ethw_runtime_profile(Some("0x13525e3"), Some("20260323")),
            "development"
        );
        assert_eq!(
            classify_ethw_runtime_profile(Some("20260323"), None),
            "development"
        );
        assert_eq!(classify_ethw_runtime_profile(Some("0x1"), None), "public");
        assert_eq!(classify_ethw_runtime_profile(None, Some("10001")), "public");
        assert_eq!(classify_ethw_runtime_profile(None, None), "unknown");
    }

    #[test]
    fn decode_world_sim_identities_builds_typed_rows() {
        let identities = decode_world_sim_identities(json!({
            "agent_wallets": ["usdb-world-agent-1", "usdb-world-agent-2"],
            "agent_addresses": ["bcrt1qa", "bcrt1qb"],
            "ethw_miner_agent_id": 1
        }))
        .unwrap();

        assert_eq!(identities.len(), 2);
        assert_eq!(identities[0].agent_id, 0);
        assert_eq!(identities[0].wallet_name, "usdb-world-agent-1");
        assert_eq!(identities[0].owner_address, "bcrt1qa");
        assert!(!identities[0].is_ethw_aligned);
        assert_eq!(identities[1].agent_id, 1);
        assert!(identities[1].is_ethw_aligned);
    }

    #[test]
    fn decode_world_sim_identities_rejects_mismatched_lengths() {
        let error = decode_world_sim_identities(json!({
            "agent_wallets": ["usdb-world-agent-1"],
            "agent_addresses": []
        }))
        .unwrap_err();

        assert!(error.contains("mismatched identities"));
    }

    #[test]
    fn extract_ord_mint_fields_from_json_output() {
        let raw = r#"{
  "commit": "700264cda3cf04bba0eceedb528c1ec338300ff5bc2fe41cbfa9bfcd79d2cd7c",
  "inscriptions": [
    {
      "destination": "bcrt1ptest",
      "id": "7f1a230913d0627a7a3bd11579e75d7e6ee79336ecbaa94c47b3058dd1ea95bfi0",
      "location": "7f1a230913d0627a7a3bd11579e75d7e6ee79336ecbaa94c47b3058dd1ea95bf:0:0"
    }
  ],
  "parents": [],
  "reveal": "7f1a230913d0627a7a3bd11579e75d7e6ee79336ecbaa94c47b3058dd1ea95bf",
  "reveal_broadcast": false,
  "total_fees": 336
}"#;

        assert_eq!(
            extract_inscription_id(raw),
            Some("7f1a230913d0627a7a3bd11579e75d7e6ee79336ecbaa94c47b3058dd1ea95bfi0".to_string())
        );
        assert_eq!(
            extract_hex64(raw),
            Some("7f1a230913d0627a7a3bd11579e75d7e6ee79336ecbaa94c47b3058dd1ea95bf".to_string())
        );
    }

    #[test]
    fn extract_ord_mint_fields_from_mixed_output_prefers_json_fields() {
        let raw = r#"ord warning: using cached recovery key
{
  "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "inscriptions": [
    {
      "id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbi0"
    }
  ],
  "reveal": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
}
background note"#;

        assert_eq!(
            extract_inscription_id(raw),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbi0".to_string())
        );
        assert_eq!(
            extract_hex64(raw),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string())
        );
    }
}
