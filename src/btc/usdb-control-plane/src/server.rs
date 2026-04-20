use crate::config::ControlPlaneConfig;
use crate::models::{
    ApiError, ArtifactSummary, BalanceHistoryServiceSummary, BootstrapStepSummary,
    BootstrapSummary, BtcNodeServiceSummary, CapabilitiesSummary, EthwServiceSummary,
    ExplorerLinks, OrdServiceSummary, OverviewResponse, ServiceProbe, ServiceRpcRequest,
    ServicesSummary, UsdbIndexerServiceSummary,
};
use crate::rpc_client::{RpcClient, decode_hex_quantity};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, get_service, post};
use axum::{Router, serve};
use bitcoincore_rpc::bitcoin::Network;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tower_http::services::ServeDir;
use usdb_util::{USDB_CONTROL_PLANE_SERVICE_NAME, parse_script_hash_any};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ControlPlaneConfig>,
    pub rpc_client: RpcClient,
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
];

const USDB_INDEXER_PROXY_METHODS: &[&str] = &[
    "get_rpc_info",
    "get_sync_status",
    "get_readiness",
    "get_pass_block_commit",
    "get_pass_snapshot",
    "get_active_passes_at_height",
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
    OverviewResponse {
        service: USDB_CONTROL_PLANE_SERVICE_NAME.to_string(),
        generated_at_ms: current_unix_ms(),
        capabilities: build_capabilities_summary(&services),
        services,
        bootstrap: build_bootstrap_summary(state),
        explorers: ExplorerLinks {
            control_console: "/#/overview".to_string(),
            balance_history: "/#/services/balance-history".to_string(),
            usdb_indexer: "/#/services/usdb-indexer".to_string(),
        },
    }
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

fn parse_balance_history_network(network: &str) -> Result<Network, String> {
    match network.to_ascii_lowercase().as_str() {
        "bitcoin" | "mainnet" => Ok(Network::Bitcoin),
        "testnet" | "testnet3" => Ok(Network::Testnet),
        "testnet4" => Ok(Network::Testnet4),
        "regtest" => Ok(Network::Regtest),
        "signet" => Ok(Network::Signet),
        other => Err(format!("unknown network {}", other)),
    }
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
        "get_address_balance" | "get_address_balance_delta" => {
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

    CapabilitiesSummary {
        ord_available,
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

        let normalized = normalize_balance_history_params(
            "get_address_balance",
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
}
