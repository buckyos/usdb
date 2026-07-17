use super::economic_cursor::{
    CandidateSetCursor, CollabBreakdownCursor, ECONOMIC_PAGE_MAX_LIMIT, EconomicPageCursor,
    decode_economic_cursor, encode_economic_cursor,
};
use super::rpc::*;
use crate::config::ConfigManagerRef;
use crate::index::{
    COLLAB_WEIGHT_BPS, DerivedCollabBreakdownItem, DerivedPassEnergyMode, Energy,
    InscriptionIndexer, MinerPassKind, MinerPassState, calc_difficulty_factor_bps,
    calc_level_from_effective_energy,
};
use crate::status::StatusManagerRef;
use jsonrpc_core::IoHandler;
use jsonrpc_core::{Error as JsonError, ErrorCode, Result as JsonResult};
use jsonrpc_http_server::{AccessControlAllowOrigin, DomainsValidation, ServerBuilder};
use ord::InscriptionId;
use serde_json::json;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::watch;
#[cfg(test)]
use usdb_util::{CONSENSUS_SOURCE_CHAIN_BTC, build_consensus_snapshot_id};
use usdb_util::{
    ConsensusQueryContext, ConsensusRpcErrorCode, ConsensusRpcErrorData, ConsensusStateReference,
    LocalStateActiveBalanceSnapshot, LocalStatePassCommitIdentity, USDB_INDEXER_SERVICE_NAME,
};
use usdb_util::{USDBScriptHash, parse_script_hash_any};

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

const MAX_RPC_PAGE_SIZE: usize = 1_000;

type CurrentStateForErrorPayload = (
    Option<IndexerSnapshotInfo>,
    Option<LocalStateCommitInfo>,
    Option<SystemStateInfo>,
);

#[derive(Clone, Debug)]
struct PassEnergyLeaderboardCacheEntry {
    resolved_height: u32,
    scope: String,
    top_k: usize,
    total: u64,
    items: Vec<PassEnergyLeaderboardItem>,
}

#[derive(Debug, Default)]
struct PassEnergyLeaderboardCache {
    latest: Option<PassEnergyLeaderboardCacheEntry>,
}

#[derive(Clone, Debug)]
struct RankedPassEnergyItem {
    item: PassEnergyLeaderboardItem,
    energy: Energy,
}

#[derive(Clone, Debug)]
struct RankedCandidateSetItem {
    item: CandidateSetViewItem,
    effective_energy: Energy,
}

fn encode_energy_decimal(energy: Energy) -> String {
    energy.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEnergyLeaderboardScope {
    Active,
    ActiveDormant,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CollabBreakdownSort {
    CollabPassIdAsc,
    ContributionDescPassIdAsc,
}

impl CollabBreakdownSort {
    fn as_str(self) -> &'static str {
        match self {
            Self::CollabPassIdAsc => "collab_pass_id_asc",
            Self::ContributionDescPassIdAsc => "contribution_desc_pass_id_asc",
        }
    }
}

impl PassEnergyLeaderboardScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::ActiveDormant => "active_dormant",
            Self::All => "all",
        }
    }

    fn states(self) -> Vec<MinerPassState> {
        match self {
            Self::Active => vec![MinerPassState::Active],
            Self::ActiveDormant => vec![MinerPassState::Active, MinerPassState::Dormant],
            Self::All => vec![
                MinerPassState::Active,
                MinerPassState::Dormant,
                MinerPassState::Consumed,
                MinerPassState::Burned,
                MinerPassState::Invalid,
            ],
        }
    }
}

#[derive(Clone)]
pub struct UsdbIndexerRpcServer {
    config: ConfigManagerRef,
    status: StatusManagerRef,
    indexer: Arc<InscriptionIndexer>,
    addr: std::net::SocketAddr,
    shutdown_tx: watch::Sender<()>,
    server_handle: Arc<Mutex<Option<jsonrpc_http_server::CloseHandle>>>,
    pass_energy_leaderboard_cache: Arc<Mutex<PassEnergyLeaderboardCache>>,
}

impl UsdbIndexerRpcServer {
    pub fn new(
        config: ConfigManagerRef,
        status: StatusManagerRef,
        indexer: Arc<InscriptionIndexer>,
        addr: std::net::SocketAddr,
        shutdown_tx: watch::Sender<()>,
    ) -> Self {
        Self {
            config,
            status,
            indexer,
            addr,
            shutdown_tx,
            server_handle: Arc::new(Mutex::new(None)),
            pass_energy_leaderboard_cache: Arc::new(Mutex::new(
                PassEnergyLeaderboardCache::default(),
            )),
        }
    }

    pub fn start(
        config: ConfigManagerRef,
        status: StatusManagerRef,
        indexer: Arc<InscriptionIndexer>,
        shutdown_tx: watch::Sender<()>,
    ) -> Result<Self, String> {
        let addr = format!(
            "{}:{}",
            config.config().usdb.rpc_server_host,
            config.config().usdb.rpc_server_port
        )
        .parse()
        .map_err(|e| {
            let msg = format!("Failed to parse usdb-indexer RPC server address: {}", e);
            error!("{}", msg);
            msg
        })?;

        let ret = Self::new(config, status, indexer, addr, shutdown_tx);
        let mut io = IoHandler::new();
        io.extend_with(ret.clone().to_delegate());

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Any,
            ]))
            .start_http(&addr)
            .map_err(|e| {
                let msg = format!("Unable to start usdb-indexer RPC server: {}", e);
                error!("{}", msg);
                msg
            })?;

        let handle = server.close_handle();
        info!("USDB indexer RPC server listening on http://{}", ret.addr);
        tokio::task::spawn_blocking(move || {
            server.wait();
        });

        {
            let mut current = ret.server_handle.lock().unwrap();
            assert!(
                current.is_none(),
                "USDB indexer RPC server is already running"
            );
            *current = Some(handle);
        }
        ret.status.set_rpc_alive(true);

        Ok(ret)
    }

    pub async fn close(&self) {
        let handle = { self.server_handle.lock().unwrap().take() };
        if let Some(handle) = handle {
            info!("Closing USDB indexer RPC server.");
            tokio::task::spawn_blocking(move || {
                handle.close();
            })
            .await
            .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            info!("USDB indexer RPC server closed.");
            self.status.set_rpc_alive(false);
        }
    }

    fn to_internal_error(message: String) -> JsonError {
        JsonError {
            code: ErrorCode::InternalError,
            message,
            data: None,
        }
    }

    fn to_invalid_params(message: String) -> JsonError {
        JsonError {
            code: ErrorCode::InvalidParams,
            message,
            data: None,
        }
    }

    fn to_business_error(code: i64, message: &str, data: serde_json::Value) -> JsonError {
        JsonError {
            code: ErrorCode::ServerError(code),
            message: message.to_string(),
            data: Some(data),
        }
    }

    /// Convert a shared consensus error code plus structured context into a
    /// stable JSON-RPC server error payload that downstream validators can
    /// machine-parse without depending on free-form text.
    fn to_consensus_error(code: ConsensusRpcErrorCode, data: ConsensusRpcErrorData) -> JsonError {
        JsonError {
            code: ErrorCode::ServerError(code.code()),
            message: code.as_str().to_string(),
            data: Some(serde_json::to_value(data).unwrap_or_else(|e| {
                json!({
                    "service": USDB_INDEXER_SERVICE_NAME,
                    "detail": format!("Failed to serialize structured consensus error data: {}", e),
                })
            })),
        }
    }

    fn synced_height(&self) -> Result<Option<u32>, JsonError> {
        self.indexer
            .miner_pass_storage()
            .get_synced_btc_block_height()
            .map_err(Self::to_internal_error)
    }

    /// Build the current externally visible state reference that is attached to
    /// consensus-facing RPC errors. This lets callers compare the service's
    /// actual state with the state they expected to query against.
    fn build_consensus_state_reference(
        &self,
        snapshot: Option<&IndexerSnapshotInfo>,
        local_state: Option<&LocalStateCommitInfo>,
        system_state: Option<&SystemStateInfo>,
    ) -> ConsensusStateReference {
        let mut reference = snapshot
            .map(ConsensusStateReference::from)
            .unwrap_or_default();

        // Current-state errors expose the running protocol/formula pair.
        // Historical queries instead use the versions recorded by the selected
        // snapshot identity so replay never drifts to the current binary.
        if snapshot.is_some() || local_state.is_some() || system_state.is_some() {
            reference.usdb_index_protocol_version = Some(USDB_INDEX_PROTOCOL_VERSION.to_string());
            reference.usdb_index_formula_version = Some(USDB_INDEX_FORMULA_VERSION.to_string());
        }

        if let Some(local_state) = local_state {
            reference.local_state_commit = Some(local_state.local_state_commit.clone());
        }

        if let Some(system_state) = system_state {
            reference.system_state_id = Some(system_state.system_state_id.clone());
            reference.local_state_commit = Some(system_state.local_state_commit.clone());
        }

        reference
    }

    /// Populate the structured `data` payload shared by consensus-facing RPC
    /// errors. The payload is intentionally richer than the error code so
    /// downstream consumers can distinguish not-ready, height drift, and state
    /// mismatch cases without parsing the message string.
    fn build_consensus_error_data(
        &self,
        requested_height: Option<u32>,
        snapshot: Option<&IndexerSnapshotInfo>,
        local_state: Option<&LocalStateCommitInfo>,
        system_state: Option<&SystemStateInfo>,
        detail: impl Into<Option<String>>,
    ) -> ConsensusRpcErrorData {
        let readiness = self.readiness_info().ok();
        let mut data = ConsensusRpcErrorData::new(USDB_INDEXER_SERVICE_NAME);
        data.requested_height = requested_height;
        data.local_synced_height = readiness
            .as_ref()
            .and_then(|value| value.synced_block_height);
        data.upstream_stable_height = readiness
            .as_ref()
            .and_then(|value| value.balance_history_stable_height);
        data.consensus_ready = readiness.as_ref().map(|value| value.consensus_ready);
        data.actual_state =
            self.build_consensus_state_reference(snapshot, local_state, system_state);
        data.detail = detail.into();
        data
    }

    /// Best-effort current-state snapshot used only to enrich structured error
    /// payloads.
    ///
    /// Historical lookup helpers should not require the current adopted state
    /// to be complete on their success path. When a historical row is missing,
    /// we still want `actual_state` in the error payload to describe what this
    /// node currently exposes, but that context is diagnostic only and not a
    /// precondition for resolving the historical row itself.
    fn current_state_for_error_payload(&self) -> Result<CurrentStateForErrorPayload, JsonError> {
        let current_snapshot = self.upstream_snapshot_info()?;
        let current_local_state = current_snapshot.as_ref().and_then(|snapshot| {
            self.build_local_state_commit_info_from_snapshot(snapshot)
                .ok()
        });
        let current_system_state = current_local_state
            .as_ref()
            .map(|local_state| self.build_system_state_info_from_local_state(local_state));
        Ok((current_snapshot, current_local_state, current_system_state))
    }

    fn history_retention_floor(&self) -> u32 {
        self.config.config().usdb.genesis_block_height
    }

    /// Fail closed when a historical query asks for a height that the node has
    /// not promised to retain. In the current phase the retention floor is the
    /// configured BTC genesis height rather than per-component persisted
    /// metadata. This keeps the contract simple until real prune support exists.
    fn ensure_history_height_retained(
        &self,
        requested_height: u32,
        component: &str,
    ) -> Result<(), JsonError> {
        let retention_floor = self.history_retention_floor();
        if requested_height >= retention_floor {
            return Ok(());
        }

        let (current_snapshot, current_local_state, current_system_state) = self
            .current_state_for_error_payload()
            .unwrap_or((None, None, None));
        Err(Self::to_consensus_error(
            ConsensusRpcErrorCode::StateNotRetained,
            self.build_consensus_error_data(
                Some(requested_height),
                current_snapshot.as_ref(),
                current_local_state.as_ref(),
                current_system_state.as_ref(),
                Some(format!(
                    "Requested height {} is below {} retention floor {}",
                    requested_height, component, retention_floor
                )),
            ),
        ))
    }

    fn build_consensus_error_data_for_state(
        &self,
        requested_height: Option<u32>,
        expected_state: ConsensusStateReference,
        actual_state: ConsensusStateReference,
        detail: impl Into<Option<String>>,
    ) -> ConsensusRpcErrorData {
        let readiness = self.readiness_info().ok();
        let mut data = ConsensusRpcErrorData::new(USDB_INDEXER_SERVICE_NAME);
        data.requested_height = requested_height;
        data.local_synced_height = readiness
            .as_ref()
            .and_then(|value| value.synced_block_height);
        data.upstream_stable_height = actual_state.stable_height;
        data.consensus_ready = readiness.as_ref().map(|value| value.consensus_ready);
        data.expected_state = expected_state;
        data.actual_state = actual_state;
        data.detail = detail.into();
        data
    }

    fn validate_consensus_query_context(
        &self,
        block_height: u32,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<ConsensusStateReference, JsonError> {
        let Some(context) = context else {
            return Ok(ConsensusStateReference::default());
        };

        if let Some(requested_height) = context.requested_height
            && requested_height != block_height
        {
            return Err(Self::to_invalid_params(format!(
                "ConsensusQueryContext.requested_height {} does not match block_height {}",
                requested_height, block_height
            )));
        }

        Ok(context.expected_state.clone())
    }

    /// Require a durable adopted upstream snapshot anchor. Current-state RPCs
    /// should fail closed when no snapshot anchor exists instead of returning a
    /// loosely defined "empty" success value.
    fn require_upstream_snapshot_info(&self) -> Result<IndexerSnapshotInfo, JsonError> {
        let snapshot = self.upstream_snapshot_info()?;
        snapshot.ok_or_else(|| {
            Self::to_consensus_error(
                ConsensusRpcErrorCode::SnapshotNotReady,
                self.build_consensus_error_data(
                    None,
                    None,
                    None,
                    None,
                    Some("No adopted upstream snapshot anchor available".to_string()),
                ),
            )
        })
    }

    /// Require the current local-state commit derived from the adopted
    /// upstream snapshot. This keeps current-state RPCs aligned on a single
    /// "not ready" contract when the durable state has not been established.
    fn require_local_state_commit_info(&self) -> Result<LocalStateCommitInfo, JsonError> {
        let snapshot = self.require_upstream_snapshot_info()?;
        let local_state = self.build_local_state_commit_info_from_snapshot(&snapshot)?;
        Ok(local_state)
    }

    /// Require the top-level system-state identity that ETHW-style consumers
    /// use as the fixed external state reference for validation.
    fn require_system_state_info(&self) -> Result<SystemStateInfo, JsonError> {
        let snapshot = self.require_upstream_snapshot_info()?;
        let local_state = self.build_local_state_commit_info_from_snapshot(&snapshot)?;
        Ok(self.build_system_state_info_from_local_state(&local_state))
    }

    /// Validate a caller-supplied height against the current durable synced
    /// height and return the exact height the query is allowed to read. This
    /// fails with a structured consensus error instead of silently clamping or
    /// falling back, because consensus-sensitive callers must know whether they
    /// are reading the requested historical state or not.
    fn resolve_height_with_consensus_error(
        &self,
        requested: Option<u32>,
    ) -> Result<u32, JsonError> {
        let snapshot = self.upstream_snapshot_info()?;
        let local_state = snapshot.as_ref().and_then(|snapshot| {
            self.build_local_state_commit_info_from_snapshot(snapshot)
                .ok()
        });
        let system_state = local_state
            .as_ref()
            .map(|local_state| self.build_system_state_info_from_local_state(local_state));

        let synced_height = self.synced_height()?;
        let synced_height = synced_height.ok_or_else(|| {
            Self::to_consensus_error(
                ConsensusRpcErrorCode::HeightNotSynced,
                self.build_consensus_error_data(
                    requested,
                    snapshot.as_ref(),
                    local_state.as_ref(),
                    system_state.as_ref(),
                    Some("No durable synced height available".to_string()),
                ),
            )
        })?;

        let resolved = requested.unwrap_or(synced_height);
        if resolved > synced_height {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::HeightNotSynced,
                self.build_consensus_error_data(
                    Some(resolved),
                    snapshot.as_ref(),
                    local_state.as_ref(),
                    system_state.as_ref(),
                    Some(format!(
                        "Requested height {} is above current synced height {}",
                        resolved, synced_height
                    )),
                ),
            ));
        }

        Ok(resolved)
    }

    /// Fail closed for validator-style historical queries whenever the current
    /// node is not consensus-ready.
    ///
    /// Historical context RPCs are used to replay a BTC-backed validator view.
    /// During catch-up, restart recovery, or upstream-not-ready windows, the
    /// node may still be alive and have some durable rows locally, but callers
    /// must not treat that partial view as a stable consensus answer.
    fn ensure_consensus_query_ready(
        &self,
        requested_height: Option<u32>,
        query_name: &str,
    ) -> Result<(), JsonError> {
        let readiness = self.readiness_info()?;
        // Direct unit tests invoke server methods without going through the
        // HTTP listener, so `rpc_alive=false` can be a pure fixture artifact.
        // Only enforce the live not-ready contract once the RPC surface is
        // actually up and serving requests.
        if !readiness.rpc_alive || readiness.consensus_ready {
            return Ok(());
        }

        let (current_snapshot, current_local_state, current_system_state) = self
            .current_state_for_error_payload()
            .unwrap_or((None, None, None));
        let blockers = readiness
            .blockers
            .iter()
            .map(|blocker| format!("{:?}", blocker))
            .collect::<Vec<_>>()
            .join(", ");

        Err(Self::to_consensus_error(
            ConsensusRpcErrorCode::SnapshotNotReady,
            self.build_consensus_error_data(
                requested_height,
                current_snapshot.as_ref(),
                current_local_state.as_ref(),
                current_system_state.as_ref(),
                Some(format!(
                    "{} requires consensus_ready=true, current readiness is rpc_alive={}, query_ready={}, consensus_ready={}, blockers=[{}]",
                    query_name,
                    readiness.rpc_alive,
                    readiness.query_ready,
                    readiness.consensus_ready,
                    blockers
                )),
            ),
        ))
    }

    /// Resolve the effective query height and readiness rules without reading
    /// historical state. Callers can then validate and return the same state
    /// object instead of reconstructing it twice across a reorg boundary.
    fn resolve_contextual_query_height(
        &self,
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<u32, JsonError> {
        if let Some(context_requested_height) = context.and_then(|value| value.requested_height)
            && let Some(explicit_height) = requested_height
            && explicit_height != context_requested_height
        {
            return Err(Self::to_invalid_params(format!(
                "Query height {} does not match ConsensusQueryContext.requested_height {}",
                explicit_height, context_requested_height
            )));
        }

        if context.is_some() {
            self.ensure_consensus_query_ready(
                requested_height.or(context.and_then(|value| value.requested_height)),
                "validator contextual query",
            )?;
        }

        let effective_requested_height =
            requested_height.or(context.and_then(|value| value.requested_height));
        if context.is_some() {
            self.resolve_height_with_consensus_error(effective_requested_height)
        } else {
            self.resolve_height(effective_requested_height)
        }
    }

    /// Resolve the effective query height while optionally enforcing a
    /// caller-supplied historical state selector.
    fn resolve_height_for_contextual_query(
        &self,
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<u32, JsonError> {
        let resolved_height = self.resolve_contextual_query_height(requested_height, context)?;

        let Some(context) = context else {
            return Ok(resolved_height);
        };
        if context.expected_state.is_empty() {
            return Ok(resolved_height);
        }

        let state_ref = self.build_historical_state_ref_info(resolved_height)?;
        self.validate_historical_state_ref_expected_state(
            resolved_height,
            &state_ref,
            &context.expected_state,
        )?;
        Ok(resolved_height)
    }

    /// Reject unsupported UIP-0006 view contracts before deriving any economic
    /// fields. The selector is mandatory because response shape and query
    /// semantics are part of the auditable view identity.
    fn validate_economic_view_version(
        &self,
        view_version: &str,
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<(), JsonError> {
        if view_version == USDB_ECONOMIC_STATE_VIEW_VERSION {
            return Ok(());
        }

        let (current_snapshot, current_local_state, current_system_state) = self
            .current_state_for_error_payload()
            .unwrap_or((None, None, None));
        let mut data = self.build_consensus_error_data(
            requested_height.or(context.and_then(|value| value.requested_height)),
            current_snapshot.as_ref(),
            current_local_state.as_ref(),
            current_system_state.as_ref(),
            Some(format!(
                "Unsupported economic view_version {}, expected {}",
                view_version, USDB_ECONOMIC_STATE_VIEW_VERSION
            )),
        );
        data.expected_state = context
            .map(|value| value.expected_state.clone())
            .unwrap_or_default();

        Err(Self::to_consensus_error(
            ConsensusRpcErrorCode::ViewVersionMismatch,
            data.with_mismatch_field("view_version"),
        ))
    }

    /// Resolve and validate the exact historical identity used by a UIP-0006
    /// economic query. Responses derive their `external_state` from this value.
    fn resolve_economic_query_context(
        &self,
        view_version: &str,
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<(u32, HistoricalStateRefInfo), JsonError> {
        self.validate_economic_view_version(view_version, requested_height, context)?;
        let resolved_height = self.resolve_contextual_query_height(requested_height, context)?;
        self.ensure_history_height_retained(resolved_height, "historical state")?;
        let state_ref = self.build_historical_state_ref_info(resolved_height)?;
        if let Some(context) = context
            && !context.expected_state.is_empty()
        {
            self.validate_historical_state_ref_expected_state(
                resolved_height,
                &state_ref,
                &context.expected_state,
            )?;
        }
        Ok((resolved_height, state_ref))
    }

    /// Resolve a continuation page against the exact external state embedded in
    /// its cursor. Explicit request selectors may repeat that state but cannot
    /// override or weaken it.
    fn resolve_economic_cursor_query_context(
        &self,
        view_version: &str,
        requested_height: Option<u32>,
        request_context: Option<&ConsensusQueryContext>,
        cursor_external_state: &EconomicExternalState,
    ) -> Result<(u32, HistoricalStateRefInfo), JsonError> {
        Self::validate_cursor_request_context(
            requested_height,
            request_context,
            cursor_external_state,
        )?;
        let cursor_context = ConsensusQueryContext::from(cursor_external_state);
        self.resolve_economic_query_context(
            view_version,
            Some(cursor_external_state.btc_height),
            Some(&cursor_context),
        )
    }

    /// Rebuild the selected historical state after deriving an economic view.
    /// A concurrent reorg must fail the request instead of pairing pre-reorg
    /// `external_state` with post-reorg pass or energy data.
    fn revalidate_economic_query_context(
        &self,
        query_height: u32,
        initial_state_ref: &HistoricalStateRefInfo,
    ) -> Result<HistoricalStateRefInfo, JsonError> {
        let revalidated_state_ref = self.build_historical_state_ref_info(query_height)?;
        let expected_state = ConsensusStateReference::from(initial_state_ref);
        if let Err(err) = self.validate_historical_state_ref_expected_state(
            query_height,
            &revalidated_state_ref,
            &expected_state,
        ) {
            warn!(
                "Economic view state changed during derivation: module=rpc_server, query_height={}, error_code={:?}, error_message={}",
                query_height, err.code, err.message
            );
            return Err(err);
        }
        Ok(revalidated_state_ref)
    }

    fn validate_economic_limit(limit: usize) -> Result<(), JsonError> {
        if limit == 0 || limit > ECONOMIC_PAGE_MAX_LIMIT {
            return Err(Self::invalid_economic_pagination(format!(
                "limit must be between 1 and {} inclusive, got {}",
                ECONOMIC_PAGE_MAX_LIMIT, limit
            )));
        }
        Ok(())
    }

    fn invalid_economic_pagination(detail: impl Into<String>) -> JsonError {
        let detail = detail.into();
        warn!(
            "Rejected UIP-0006 cursor pagination request: module=rpc_server, detail={}",
            detail
        );
        Self::to_business_error(
            ERR_INVALID_PAGINATION,
            "INVALID_PAGINATION",
            json!({
                "detail": detail,
                "max_limit": ECONOMIC_PAGE_MAX_LIMIT
            }),
        )
    }

    fn decode_candidate_set_cursor(
        value: Option<&str>,
    ) -> Result<Option<CandidateSetCursor>, JsonError> {
        let Some(value) = value else {
            return Ok(None);
        };
        match decode_economic_cursor(value).map_err(Self::invalid_economic_pagination)? {
            EconomicPageCursor::CandidateSet(cursor) => Ok(Some(cursor)),
            EconomicPageCursor::CollabBreakdown(_) => Err(Self::invalid_economic_pagination(
                "collab breakdown cursor cannot continue a candidate set query",
            )),
        }
    }

    fn decode_collab_breakdown_cursor(
        value: Option<&str>,
    ) -> Result<Option<CollabBreakdownCursor>, JsonError> {
        let Some(value) = value else {
            return Ok(None);
        };
        match decode_economic_cursor(value).map_err(Self::invalid_economic_pagination)? {
            EconomicPageCursor::CollabBreakdown(cursor) => Ok(Some(cursor)),
            EconomicPageCursor::CandidateSet(_) => Err(Self::invalid_economic_pagination(
                "candidate set cursor cannot continue a collab breakdown query",
            )),
        }
    }

    fn encode_cursor(cursor: EconomicPageCursor) -> Result<String, JsonError> {
        encode_economic_cursor(cursor).map_err(|e| {
            error!(
                "Failed to encode UIP-0006 cursor: module=rpc_server, error={}",
                e
            );
            Self::to_internal_error(e)
        })
    }

    fn validate_cursor_request_context(
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
        cursor_external_state: &EconomicExternalState,
    ) -> Result<(), JsonError> {
        if let Some(requested_height) = requested_height
            && requested_height != cursor_external_state.btc_height
        {
            return Err(Self::invalid_economic_pagination(format!(
                "cursor btc_height {} does not match request block_height {}",
                cursor_external_state.btc_height, requested_height
            )));
        }

        let Some(context) = context else {
            return Ok(());
        };
        if let Some(requested_height) = context.requested_height
            && requested_height != cursor_external_state.btc_height
        {
            return Err(Self::invalid_economic_pagination(format!(
                "cursor btc_height {} does not match context requested_height {}",
                cursor_external_state.btc_height, requested_height
            )));
        }

        let cursor_state = ConsensusStateReference::from(cursor_external_state);
        let request_state = &context.expected_state;
        macro_rules! require_cursor_field_match {
            ($field:ident) => {
                if let Some(request_value) = request_state.$field.as_ref()
                    && cursor_state.$field.as_ref() != Some(request_value)
                {
                    return Err(Self::invalid_economic_pagination(format!(
                        "cursor-bound {} does not match request context",
                        stringify!($field)
                    )));
                }
            };
        }

        require_cursor_field_match!(snapshot_id);
        require_cursor_field_match!(stable_height);
        require_cursor_field_match!(stable_block_hash);
        require_cursor_field_match!(balance_history_api_version);
        require_cursor_field_match!(balance_history_semantics_version);
        require_cursor_field_match!(usdb_index_protocol_version);
        require_cursor_field_match!(usdb_index_formula_version);
        require_cursor_field_match!(local_state_commit);
        require_cursor_field_match!(system_state_id);
        Ok(())
    }

    fn upstream_snapshot_info(&self) -> Result<Option<IndexerSnapshotInfo>, JsonError> {
        let Some(anchor) = self
            .indexer
            .miner_pass_storage()
            .get_balance_history_snapshot_anchor()
            .map_err(Self::to_internal_error)?
        else {
            return Ok(None);
        };

        let local_synced_block_height = self
            .indexer
            .miner_pass_storage()
            .get_synced_btc_block_height()
            .map_err(Self::to_internal_error)?
            .unwrap_or(anchor.stable_height);

        Ok(Some(IndexerSnapshotInfo::from(IndexerSnapshotInfoSeed {
            network: self.config.config().bitcoin.network().to_string(),
            local_synced_block_height,
            balance_history_stable_height: anchor.stable_height,
            stable_block_hash: anchor.stable_block_hash,
            latest_block_commit: anchor.latest_block_commit,
            stable_lag: anchor.stable_lag,
            commit_protocol_version: anchor.commit_protocol_version,
            commit_hash_algo: anchor.commit_hash_algo,
        })))
    }

    fn upstream_snapshot_info_at_height(
        &self,
        block_height: u32,
    ) -> Result<IndexerSnapshotInfo, JsonError> {
        self.ensure_history_height_retained(block_height, "historical state")?;

        let anchor = self
            .indexer
            .miner_pass_storage()
            .get_balance_history_snapshot_anchor_at_height(block_height)
            .map_err(Self::to_internal_error)?
            .ok_or_else(|| {
                let (current_snapshot, current_local_state, current_system_state) = self
                    .current_state_for_error_payload()
                    .unwrap_or((None, None, None));
                Self::to_consensus_error(
                    ConsensusRpcErrorCode::HistoryNotAvailable,
                    self.build_consensus_error_data(
                        Some(block_height),
                        current_snapshot.as_ref(),
                        current_local_state.as_ref(),
                        current_system_state.as_ref(),
                        Some(format!(
                            "Missing balance-history snapshot history at height {} while building historical state ref",
                            block_height
                        )),
                    ),
                )
            })?;

        Ok(IndexerSnapshotInfo::from(IndexerSnapshotInfoSeed {
            network: self.config.config().bitcoin.network().to_string(),
            local_synced_block_height: block_height,
            balance_history_stable_height: anchor.stable_height,
            stable_block_hash: anchor.stable_block_hash,
            latest_block_commit: anchor.latest_block_commit,
            stable_lag: anchor.stable_lag,
            commit_protocol_version: anchor.commit_protocol_version,
            commit_hash_algo: anchor.commit_hash_algo,
        }))
    }

    fn build_local_state_commit_info_at_height(
        &self,
        snapshot: &IndexerSnapshotInfo,
        require_latest_balance_snapshot_consistency: bool,
    ) -> Result<LocalStateCommitInfo, JsonError> {
        let synced_height = snapshot.local_synced_block_height;
        let latest_pass_block_commit = self
            .indexer
            .miner_pass_storage()
            .get_latest_pass_block_commit_at_or_before(synced_height)
            .map_err(Self::to_internal_error)?
            .map(|entry| LocalStatePassCommitIdentity {
                block_height: entry.block_height,
                block_commit: entry.block_commit,
                commit_protocol_version: entry.commit_protocol_version,
                commit_hash_algo: entry.commit_hash_algo,
            });

        let genesis_block_height = self.config.config().usdb.genesis_block_height;
        let latest_active_balance_snapshot = if synced_height < genesis_block_height {
            None
        } else {
            if !require_latest_balance_snapshot_consistency {
                self.ensure_history_height_retained(synced_height, "historical state")?;
            }

            if require_latest_balance_snapshot_consistency {
                self.indexer
                    .miner_pass_storage()
                    .assert_balance_snapshot_consistency(synced_height, genesis_block_height)
                    .map_err(Self::to_internal_error)?;
            }

            let snapshot = self
                .indexer
                .miner_pass_storage()
                .get_active_balance_snapshot(synced_height)
                .map_err(Self::to_internal_error)?
                .ok_or_else(|| {
                    if require_latest_balance_snapshot_consistency {
                        Self::to_internal_error(format!(
                            "Missing active balance snapshot at height {} while building local state commit",
                            synced_height
                        ))
                    } else {
                        let current_snapshot = self.upstream_snapshot_info().ok().flatten();
                        let current_local_state = current_snapshot.as_ref().and_then(|snapshot| {
                            self.build_local_state_commit_info_from_snapshot(snapshot).ok()
                        });
                        let current_system_state = current_local_state.as_ref().map(|local_state| {
                            self.build_system_state_info_from_local_state(local_state)
                        });
                        Self::to_consensus_error(
                            ConsensusRpcErrorCode::HistoryNotAvailable,
                            self.build_consensus_error_data(
                                Some(synced_height),
                                current_snapshot.as_ref(),
                                current_local_state.as_ref(),
                                current_system_state.as_ref(),
                                Some(format!(
                                    "Missing active balance snapshot at height {} while building historical local state commit",
                                    synced_height
                                )),
                            ),
                        )
                    }
                })?;

            Some(LocalStateActiveBalanceSnapshot {
                block_height: snapshot.block_height,
                total_balance: snapshot.total_balance,
                active_address_count: snapshot.active_address_count,
            })
        };

        Ok(LocalStateCommitInfo::from(LocalStateCommitInfoSeed {
            local_synced_block_height: synced_height,
            upstream_snapshot_id: snapshot.snapshot_id.clone(),
            latest_pass_block_commit,
            latest_active_balance_snapshot,
        }))
    }

    fn build_local_state_commit_info_from_snapshot(
        &self,
        snapshot: &IndexerSnapshotInfo,
    ) -> Result<LocalStateCommitInfo, JsonError> {
        self.build_local_state_commit_info_at_height(snapshot, true)
    }

    // Build a locally durable core-state commit without changing the meaning of snapshot_info.
    // snapshot_info continues to describe only the upstream consensus anchor, while this method
    // binds that anchor to local pass state and active-balance settlement state.
    fn local_state_commit_info(&self) -> Result<Option<LocalStateCommitInfo>, JsonError> {
        let Some(snapshot) = self.upstream_snapshot_info()? else {
            return Ok(None);
        };
        self.build_local_state_commit_info_from_snapshot(&snapshot)
            .map(Some)
    }

    fn build_system_state_info_from_local_state(
        &self,
        local_state: &LocalStateCommitInfo,
    ) -> SystemStateInfo {
        SystemStateInfo::from(local_state)
    }

    fn system_state_info(&self) -> Result<Option<SystemStateInfo>, JsonError> {
        let Some(local_state) = self.local_state_commit_info()? else {
            return Ok(None);
        };

        Ok(Some(
            self.build_system_state_info_from_local_state(&local_state),
        ))
    }

    fn build_historical_state_ref_info(
        &self,
        block_height: u32,
    ) -> Result<HistoricalStateRefInfo, JsonError> {
        let snapshot_info = self.upstream_snapshot_info_at_height(block_height)?;
        let local_state_commit_info =
            self.build_local_state_commit_info_at_height(&snapshot_info, false)?;
        let system_state_info =
            self.build_system_state_info_from_local_state(&local_state_commit_info);

        Ok(HistoricalStateRefInfo::from(HistoricalStateRefInfoSeed {
            block_height,
            snapshot_info,
            local_state_commit_info,
            system_state_info,
        }))
    }

    fn build_consensus_state_reference_from_historical_state_ref(
        &self,
        state_ref: &HistoricalStateRefInfo,
    ) -> ConsensusStateReference {
        ConsensusStateReference::from(state_ref)
    }

    fn validate_historical_state_ref_expected_state(
        &self,
        block_height: u32,
        state_ref: &HistoricalStateRefInfo,
        expected_state: &ConsensusStateReference,
    ) -> Result<(), JsonError> {
        if expected_state.is_empty() {
            return Ok(());
        }

        let actual_state =
            self.build_consensus_state_reference_from_historical_state_ref(state_ref);

        if let Some(expected_snapshot_id) = expected_state.snapshot_id.as_ref()
            && expected_snapshot_id != &state_ref.snapshot_info.snapshot_id
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::SnapshotIdMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected historical snapshot_id {} at height {}, got {}",
                        expected_snapshot_id, block_height, state_ref.snapshot_info.snapshot_id
                    )),
                )
                .with_mismatch_field("snapshot_id"),
            ));
        }

        if let Some(expected_stable_height) = expected_state.stable_height
            && expected_stable_height != state_ref.snapshot_info.balance_history_stable_height
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::SnapshotIdMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected historical stable height {} at height {}, got {}",
                        expected_stable_height,
                        block_height,
                        state_ref.snapshot_info.balance_history_stable_height
                    )),
                )
                .with_mismatch_field("stable_height"),
            ));
        }

        if let Some(expected_block_hash) = expected_state.stable_block_hash.as_ref()
            && expected_block_hash != &state_ref.snapshot_info.stable_block_hash
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::BlockHashMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected historical stable block hash {} at height {}, got {}",
                        expected_block_hash,
                        block_height,
                        state_ref.snapshot_info.stable_block_hash
                    )),
                )
                .with_mismatch_field("stable_block_hash"),
            ));
        }

        if let Some(expected_api_version) = expected_state.balance_history_api_version.as_ref()
            && expected_api_version
                != &state_ref
                    .snapshot_info
                    .consensus_identity
                    .balance_history_api_version
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::VersionMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected balance-history API version {} at height {}, got {}",
                        expected_api_version,
                        block_height,
                        state_ref
                            .snapshot_info
                            .consensus_identity
                            .balance_history_api_version
                    )),
                )
                .with_mismatch_field("balance_history_api_version"),
            ));
        }

        if let Some(expected_semantics_version) =
            expected_state.balance_history_semantics_version.as_ref()
            && expected_semantics_version
                != &state_ref
                    .snapshot_info
                    .consensus_identity
                    .balance_history_semantics_version
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::VersionMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected balance-history semantics version {} at height {}, got {}",
                        expected_semantics_version,
                        block_height,
                        state_ref
                            .snapshot_info
                            .consensus_identity
                            .balance_history_semantics_version
                    )),
                )
                .with_mismatch_field("balance_history_semantics_version"),
            ));
        }

        if let Some(expected_usdb_protocol_version) =
            expected_state.usdb_index_protocol_version.as_ref()
            && expected_usdb_protocol_version
                != &state_ref
                    .snapshot_info
                    .consensus_identity
                    .usdb_index_protocol_version
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::ProtocolVersionMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected usdb-index protocol version {} at height {}, got {}",
                        expected_usdb_protocol_version,
                        block_height,
                        state_ref
                            .snapshot_info
                            .consensus_identity
                            .usdb_index_protocol_version
                    )),
                )
                .with_mismatch_field("usdb_index_protocol_version"),
            ));
        }

        if let Some(expected_usdb_formula_version) =
            expected_state.usdb_index_formula_version.as_ref()
            && expected_usdb_formula_version
                != &state_ref
                    .snapshot_info
                    .consensus_identity
                    .usdb_index_formula_version
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::FormulaVersionMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected usdb-index formula version {} at height {}, got {}",
                        expected_usdb_formula_version,
                        block_height,
                        state_ref
                            .snapshot_info
                            .consensus_identity
                            .usdb_index_formula_version
                    )),
                )
                .with_mismatch_field("usdb_index_formula_version"),
            ));
        }

        if let Some(expected_local_state_commit) = expected_state.local_state_commit.as_ref()
            && expected_local_state_commit != &state_ref.local_state_commit_info.local_state_commit
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::LocalStateCommitMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected local_state_commit {} at height {}, got {}",
                        expected_local_state_commit,
                        block_height,
                        state_ref.local_state_commit_info.local_state_commit
                    )),
                )
                .with_mismatch_field("local_state_commit"),
            ));
        }

        if let Some(expected_system_state_id) = expected_state.system_state_id.as_ref()
            && expected_system_state_id != &state_ref.system_state_info.system_state_id
        {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::SystemStateIdMismatch,
                self.build_consensus_error_data_for_state(
                    Some(block_height),
                    expected_state.clone(),
                    actual_state,
                    Some(format!(
                        "Expected system_state_id {} at height {}, got {}",
                        expected_system_state_id,
                        block_height,
                        state_ref.system_state_info.system_state_id
                    )),
                )
                .with_mismatch_field("system_state_id"),
            ));
        }

        Ok(())
    }

    fn readiness_info(&self) -> Result<ReadinessInfo, JsonError> {
        let sync_status = self.status.get_index_status_snapshot();
        let runtime = self.status.get_runtime_readiness();
        let synced_height = self.synced_height()?;
        let durable_reorg_recovery_pending = self
            .indexer
            .miner_pass_storage()
            .get_upstream_reorg_recovery_pending_height()
            .map_err(Self::to_internal_error)?
            .is_some();
        let reorg_recovery_pending =
            runtime.upstream_reorg_recovery_pending || durable_reorg_recovery_pending;

        let upstream_readiness = self.status.balance_history_readiness();
        let upstream_snapshot = match self.upstream_snapshot_info() {
            Ok(snapshot) => snapshot,
            Err(e) => {
                error!(
                    "Failed to build upstream snapshot readiness state: module=rpc_server, error={}",
                    e.message
                );
                None
            }
        };
        let local_state = match upstream_snapshot.as_ref() {
            Some(snapshot) => match self.build_local_state_commit_info_from_snapshot(snapshot) {
                Ok(info) => Some(info),
                Err(e) => {
                    error!(
                        "Failed to build local state commit readiness state: module=rpc_server, error={}",
                        e.message
                    );
                    None
                }
            },
            None => None,
        };
        let system_state = local_state
            .as_ref()
            .map(|local_state| self.build_system_state_info_from_local_state(local_state));

        let observed_upstream_height = sync_status.balance_history_stable_height.or_else(|| {
            upstream_snapshot
                .as_ref()
                .map(|snapshot| snapshot.balance_history_stable_height)
        });
        let catching_up = match (synced_height, observed_upstream_height) {
            (Some(local_height), Some(upstream_height)) => local_height < upstream_height,
            _ => false,
        };

        let mut blockers = Vec::new();
        if !runtime.rpc_alive {
            blockers.push(ReadinessBlocker::RpcNotListening);
        }
        if runtime.shutdown_requested {
            blockers.push(ReadinessBlocker::ShutdownRequested);
        }
        if synced_height.is_none() {
            blockers.push(ReadinessBlocker::SyncedHeightMissing);
        }
        if catching_up {
            blockers.push(ReadinessBlocker::CatchingUp);
        }
        match upstream_readiness.as_ref() {
            Some(readiness) => {
                if !readiness.consensus_ready {
                    blockers.push(ReadinessBlocker::UpstreamConsensusNotReady);
                }
            }
            None => blockers.push(ReadinessBlocker::UpstreamReadinessUnknown),
        }
        if upstream_snapshot.is_none() {
            blockers.push(ReadinessBlocker::UpstreamSnapshotMissing);
        } else if let (Some(snapshot), Some(local_height)) =
            (upstream_snapshot.as_ref(), synced_height)
            && snapshot.balance_history_stable_height != local_height
        {
            blockers.push(ReadinessBlocker::UpstreamSnapshotHeightMismatch);
        }
        if reorg_recovery_pending {
            blockers.push(ReadinessBlocker::ReorgRecoveryPending);
        }
        if upstream_snapshot.is_some() && local_state.is_none() {
            blockers.push(ReadinessBlocker::LocalStateCommitMissing);
        }
        if local_state.is_some() && system_state.is_none() {
            blockers.push(ReadinessBlocker::SystemStateMissing);
        }

        let query_ready = runtime.rpc_alive
            && !runtime.shutdown_requested
            && !reorg_recovery_pending
            && synced_height.is_some();
        let consensus_ready = query_ready
            && upstream_readiness
                .as_ref()
                .map(|readiness| readiness.consensus_ready)
                .unwrap_or(false)
            && !catching_up
            && upstream_snapshot.is_some()
            && local_state.is_some()
            && system_state.is_some()
            && blockers.is_empty();

        Ok(ReadinessInfo {
            service: USDB_INDEXER_SERVICE_NAME.to_string(),
            rpc_alive: runtime.rpc_alive,
            query_ready,
            consensus_ready,
            synced_block_height: synced_height,
            balance_history_stable_height: observed_upstream_height,
            upstream_snapshot_id: upstream_snapshot
                .as_ref()
                .map(|snapshot| snapshot.snapshot_id.clone()),
            local_state_commit: local_state
                .as_ref()
                .map(|local_state| local_state.local_state_commit.clone()),
            system_state_id: system_state
                .as_ref()
                .map(|system_state| system_state.system_state_id.clone()),
            current: sync_status.current,
            total: sync_status.total,
            message: sync_status.message,
            blockers,
        })
    }

    fn resolve_height(&self, requested: Option<u32>) -> Result<u32, JsonError> {
        let synced_height = self.synced_height()?;
        let synced_height = synced_height.ok_or_else(|| {
            Self::to_business_error(
                ERR_HEIGHT_NOT_SYNCED,
                "HEIGHT_NOT_SYNCED",
                json!({"requested_height": requested, "synced_height": null}),
            )
        })?;

        let resolved = requested.unwrap_or(synced_height);
        if resolved > synced_height {
            return Err(Self::to_business_error(
                ERR_HEIGHT_NOT_SYNCED,
                "HEIGHT_NOT_SYNCED",
                json!({
                    "requested_height": resolved,
                    "synced_height": synced_height
                }),
            ));
        }

        Ok(resolved)
    }

    fn parse_inscription_id(&self, value: &str) -> Result<InscriptionId, JsonError> {
        InscriptionId::from_str(value).map_err(|e| {
            Self::to_invalid_params(format!("Invalid inscription_id {}: {}", value, e))
        })
    }

    fn parse_owner(&self, value: &str) -> Result<USDBScriptHash, JsonError> {
        parse_script_hash_any(value, &self.config.config().bitcoin.network())
            .map_err(|e| Self::to_invalid_params(format!("Invalid owner {}: {}", value, e)))
    }

    fn validate_pagination(&self, page: usize, page_size: usize) -> Result<(), JsonError> {
        if page_size == 0 || page_size > MAX_RPC_PAGE_SIZE || page.checked_mul(page_size).is_none()
        {
            return Err(Self::to_business_error(
                ERR_INVALID_PAGINATION,
                "INVALID_PAGINATION",
                json!({
                    "page": page,
                    "page_size": page_size,
                    "max_page_size": MAX_RPC_PAGE_SIZE
                }),
            ));
        }
        Ok(())
    }

    fn resolve_height_range(&self, from_height: u32, to_height: u32) -> Result<u32, JsonError> {
        let resolved_to = self.resolve_height(Some(to_height))?;
        if from_height > resolved_to {
            return Err(Self::to_business_error(
                ERR_INVALID_HEIGHT_RANGE,
                "INVALID_HEIGHT_RANGE",
                json!({
                    "from_height": from_height,
                    "to_height": to_height,
                    "resolved_to_height": resolved_to
                }),
            ));
        }
        Ok(resolved_to)
    }

    fn parse_leaderboard_scope(
        &self,
        value: Option<&str>,
    ) -> Result<PassEnergyLeaderboardScope, JsonError> {
        let normalized = value.unwrap_or("active").trim().to_ascii_lowercase();
        match normalized.as_str() {
            "active" => Ok(PassEnergyLeaderboardScope::Active),
            "active_dormant" => Ok(PassEnergyLeaderboardScope::ActiveDormant),
            "all" => Ok(PassEnergyLeaderboardScope::All),
            _ => Err(Self::to_invalid_params(format!(
                "Invalid leaderboard scope {}, expected active, active_dormant, or all",
                normalized
            ))),
        }
    }

    fn parse_candidate_set_selection_rule(
        &self,
        value: Option<&str>,
    ) -> Result<&'static str, JsonError> {
        let normalized = value.unwrap_or(CANDIDATE_SET_SELECTION_RULE).trim();
        if normalized == CANDIDATE_SET_SELECTION_RULE {
            return Ok(CANDIDATE_SET_SELECTION_RULE);
        }

        Err(Self::to_invalid_params(format!(
            "Invalid candidate set selection_rule {}, expected {}",
            normalized, CANDIDATE_SET_SELECTION_RULE
        )))
    }

    fn parse_collab_breakdown_sort(
        &self,
        value: Option<&str>,
    ) -> Result<CollabBreakdownSort, JsonError> {
        let normalized = value
            .unwrap_or("collab_pass_id_asc")
            .trim()
            .to_ascii_lowercase();
        match normalized.as_str() {
            "collab_pass_id_asc" => Ok(CollabBreakdownSort::CollabPassIdAsc),
            "contribution_desc_pass_id_asc" => Ok(CollabBreakdownSort::ContributionDescPassIdAsc),
            _ => Err(Self::to_invalid_params(format!(
                "Invalid collab breakdown sort {}, expected collab_pass_id_asc or contribution_desc_pass_id_asc",
                normalized
            ))),
        }
    }

    fn sort_collab_breakdown_items(
        items: &mut [DerivedCollabBreakdownItem],
        sort: CollabBreakdownSort,
    ) {
        match sort {
            CollabBreakdownSort::CollabPassIdAsc => {
                items.sort_by(|a, b| a.collab_pass_id.cmp(&b.collab_pass_id));
            }
            CollabBreakdownSort::ContributionDescPassIdAsc => {
                items.sort_by(|a, b| {
                    b.collab_contribution
                        .cmp(&a.collab_contribution)
                        .then_with(|| a.collab_pass_id.cmp(&b.collab_pass_id))
                });
            }
        }
    }

    fn encode_collab_breakdown_item(item: DerivedCollabBreakdownItem) -> CollabBreakdownItem {
        CollabBreakdownItem {
            collab_pass_id: item.collab_pass_id.to_string(),
            collab_owner_script_hash: item.collab_owner.to_string(),
            collab_owner_btc_addr: None,
            record_block_height: item.record_block_height,
            collab_raw_energy: encode_energy_decimal(item.collab_raw_energy),
            collab_weight_bps: COLLAB_WEIGHT_BPS as u64,
            collab_contribution: encode_energy_decimal(item.collab_contribution),
            leader_ref_kind: item.leader_ref_kind,
            leader_ref_value: item.leader_ref_value,
        }
    }

    fn parse_optional_pass_states(
        &self,
        values: Option<Vec<String>>,
    ) -> Result<Vec<MinerPassState>, JsonError> {
        let Some(values) = values else {
            return Ok(Vec::new());
        };

        let mut states = Vec::new();
        for value in values {
            let normalized = value.trim();
            if normalized.is_empty() {
                continue;
            }
            let state = MinerPassState::from_str(normalized).map_err(|e| {
                Self::to_invalid_params(format!("Invalid pass state {}: {}", normalized, e))
            })?;
            if !states.contains(&state) {
                states.push(state);
            }
        }
        Ok(states)
    }

    fn parse_order_desc(&self, value: Option<&str>) -> Result<bool, JsonError> {
        let normalized = value.unwrap_or("desc").trim().to_ascii_lowercase();
        match normalized.as_str() {
            "desc" => Ok(true),
            "asc" => Ok(false),
            _ => Err(Self::to_invalid_params(format!(
                "Invalid order {}, expected asc or desc",
                normalized
            ))),
        }
    }

    fn build_pass_snapshot(
        &self,
        inscription_id: &InscriptionId,
        resolved_height: u32,
    ) -> Result<Option<PassSnapshot>, JsonError> {
        let storage = self.indexer.miner_pass_storage();
        let pass = storage
            .get_pass_by_inscription_id(inscription_id)
            .map_err(Self::to_internal_error)?;

        let Some(pass) = pass else {
            return Ok(None);
        };

        let history = storage
            .get_last_pass_history_at_or_before_height(inscription_id, resolved_height)
            .map_err(Self::to_internal_error)?;

        let Some(history) = history else {
            return Ok(None);
        };

        Ok(Some(PassSnapshot {
            inscription_id: pass.inscription_id.to_string(),
            inscription_number: pass.inscription_number,
            mint_txid: pass.mint_txid.to_string(),
            mint_block_height: pass.mint_block_height,
            mint_owner: pass.mint_owner.to_string(),
            mint_version: pass.mint_version,
            pass_kind: pass.pass_kind.as_str().to_string(),
            usdb_main: pass.usdb_main,
            leader_pass_id: pass.leader_pass_id.map(|id| id.to_string()),
            leader_btc_addr: pass.leader_btc_addr,
            prev: pass.prev.into_iter().map(|v| v.to_string()).collect(),
            invalid_code: pass.invalid_code,
            invalid_reason: pass.invalid_reason,
            owner: history.owner.to_string(),
            state: history.state.as_str().to_string(),
            satpoint: history.satpoint.to_string(),
            last_event_id: history.event_id,
            last_event_type: history.event_type,
            resolved_height,
        }))
    }

    fn leaderboard_cache_settings(&self) -> (bool, usize) {
        let cfg = &self.config.config().usdb;
        (
            cfg.pass_energy_leaderboard_cache_enabled,
            cfg.pass_energy_leaderboard_cache_top_k.max(1),
        )
    }

    fn pagination_offset(page: usize, page_size: usize) -> Result<usize, JsonError> {
        page.checked_mul(page_size).ok_or_else(|| {
            Self::to_business_error(
                ERR_INVALID_PAGINATION,
                "INVALID_PAGINATION",
                json!({"page": page, "page_size": page_size}),
            )
        })
    }

    fn paginate_leaderboard_items(
        items: &[PassEnergyLeaderboardItem],
        total: u64,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<PassEnergyLeaderboardItem>, JsonError> {
        let offset = Self::pagination_offset(page, page_size)?;
        if (offset as u64) >= total {
            return Ok(Vec::new());
        }

        if offset >= items.len() {
            return Ok(Vec::new());
        }

        let end = offset.saturating_add(page_size).min(items.len());
        Ok(items[offset..end].to_vec())
    }

    fn candidate_cursor_start(
        items: &[CandidateSetViewItem],
        cursor: Option<&CandidateSetCursor>,
    ) -> Result<usize, JsonError> {
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        let position = items.iter().position(|item| {
            item.pass_id == cursor.last_pass_id
                && item.effective_energy == cursor.last_effective_energy
        });
        position
            .and_then(|position| position.checked_add(1))
            .ok_or_else(|| {
                Self::invalid_economic_pagination(
                    "candidate cursor continuation key is not present in the bound state",
                )
            })
    }

    fn collab_cursor_start(
        items: &[DerivedCollabBreakdownItem],
        cursor: Option<&CollabBreakdownCursor>,
    ) -> Result<usize, JsonError> {
        let Some(cursor) = cursor else {
            return Ok(0);
        };
        let position = items.iter().position(|item| {
            item.collab_pass_id.to_string() == cursor.last_collab_pass_id
                && encode_energy_decimal(item.collab_contribution)
                    == cursor.last_collab_contribution
        });
        position
            .and_then(|position| position.checked_add(1))
            .ok_or_else(|| {
                Self::invalid_economic_pagination(
                    "collab cursor continuation key is not present in the bound state",
                )
            })
    }

    fn try_get_cached_leaderboard_page(
        &self,
        resolved_height: u32,
        scope: PassEnergyLeaderboardScope,
        top_k: usize,
        page: usize,
        page_size: usize,
    ) -> Result<Option<PassEnergyLeaderboardPage>, JsonError> {
        let offset = Self::pagination_offset(page, page_size)?;
        let cache = self.pass_energy_leaderboard_cache.lock().unwrap();
        let Some(entry) = &cache.latest else {
            return Ok(None);
        };
        if entry.resolved_height != resolved_height
            || entry.scope != scope.as_str()
            || entry.top_k != top_k
        {
            return Ok(None);
        }

        if offset >= top_k {
            return Ok(Some(PassEnergyLeaderboardPage {
                resolved_height,
                total: entry.total,
                items: Vec::new(),
            }));
        }

        if (offset as u64) >= entry.total {
            return Ok(Some(PassEnergyLeaderboardPage {
                resolved_height,
                total: entry.total,
                items: Vec::new(),
            }));
        }

        if offset >= entry.items.len() {
            // Cache only keeps top-k rows. Any deeper page is intentionally empty.
            return Ok(Some(PassEnergyLeaderboardPage {
                resolved_height,
                total: entry.total,
                items: Vec::new(),
            }));
        }

        let items = Self::paginate_leaderboard_items(&entry.items, entry.total, page, page_size)?;
        Ok(Some(PassEnergyLeaderboardPage {
            resolved_height,
            total: entry.total,
            items,
        }))
    }

    fn update_leaderboard_cache(
        &self,
        resolved_height: u32,
        scope: PassEnergyLeaderboardScope,
        top_k: usize,
        total: u64,
        ranked: &[PassEnergyLeaderboardItem],
    ) {
        let mut cache = self.pass_energy_leaderboard_cache.lock().unwrap();
        let cached_items = ranked
            .iter()
            .take(top_k)
            .cloned()
            .collect::<Vec<PassEnergyLeaderboardItem>>();
        cache.latest = Some(PassEnergyLeaderboardCacheEntry {
            resolved_height,
            scope: scope.as_str().to_string(),
            top_k,
            total,
            items: cached_items,
        });
    }

    fn build_pass_energy_leaderboard_dataset(
        &self,
        resolved_height: u32,
        scope: PassEnergyLeaderboardScope,
    ) -> Result<(u64, Vec<PassEnergyLeaderboardItem>), JsonError> {
        let build_start = Instant::now();
        let states = scope.states();
        let storage = self.indexer.miner_pass_storage();
        let total_passes = storage
            .get_pass_count_from_history_at_height_by_states(resolved_height, &states)
            .map_err(Self::to_internal_error)?;

        if total_passes == 0 {
            return Ok((0, Vec::new()));
        }

        let load_page_size = self.config.config().usdb.active_address_page_size.max(1);
        let total_passes_usize = usize::try_from(total_passes).map_err(|_| {
            Self::to_internal_error(format!(
                "Pass count overflow when building energy leaderboard: total_passes={}, scope={}",
                total_passes,
                scope.as_str()
            ))
        })?;
        let total_pages = total_passes_usize.div_ceil(load_page_size);
        let mut rows = Vec::with_capacity(total_passes_usize);
        for page in 0..total_pages {
            let page_rows = storage
                .get_passes_by_page_from_history_at_height_by_states(
                    page,
                    load_page_size,
                    resolved_height,
                    &states,
                )
                .map_err(Self::to_internal_error)?;
            if page_rows.is_empty() {
                break;
            }
            rows.extend(page_rows);
        }

        let mut ranked = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(record) = self
                .indexer
                .pass_energy_manager()
                .get_pass_energy_record_at_or_before(&row.inscription_id, resolved_height)
                .map_err(Self::to_internal_error)?
            else {
                warn!(
                    "Missing energy record when building energy leaderboard: inscription_id={}, resolved_height={}",
                    row.inscription_id, resolved_height
                );
                continue;
            };
            let projected = self
                .indexer
                .pass_energy_manager()
                .project_energy_record_no_balance_change(&record, resolved_height);

            ranked.push(RankedPassEnergyItem {
                energy: projected.energy,
                item: PassEnergyLeaderboardItem {
                    inscription_id: row.inscription_id.to_string(),
                    owner: row.owner.to_string(),
                    record_block_height: record.block_height,
                    state: projected.state.as_str().to_string(),
                    energy: encode_energy_decimal(projected.energy),
                },
            });
        }

        ranked.sort_by(|a, b| {
            b.energy
                .cmp(&a.energy)
                .then_with(|| b.item.record_block_height.cmp(&a.item.record_block_height))
                .then_with(|| a.item.inscription_id.cmp(&b.item.inscription_id))
        });
        let ranked = ranked
            .into_iter()
            .map(|ranked| ranked.item)
            .collect::<Vec<_>>();

        let total = ranked.len() as u64;
        let elapsed_ms = build_start.elapsed().as_millis();
        info!(
            "Pass energy leaderboard dataset built: module=rpc_server, scope={}, resolved_height={}, pass_count={}, ranked_count={}, missing_energy_count={}, elapsed_ms={}",
            scope.as_str(),
            resolved_height,
            total_passes,
            total,
            total_passes.saturating_sub(total),
            elapsed_ms
        );

        Ok((total, ranked))
    }

    fn build_candidate_set_view_dataset(
        &self,
        resolved_height: u32,
    ) -> Result<(u64, Vec<CandidateSetViewItem>), JsonError> {
        let build_start = Instant::now();
        let storage = self.indexer.miner_pass_storage();
        let total_candidates = storage
            .get_active_standard_pass_count_from_history_at_height(resolved_height)
            .map_err(Self::to_internal_error)?;

        if total_candidates == 0 {
            return Ok((0, Vec::new()));
        }

        let load_page_size = self.config.config().usdb.active_address_page_size.max(1);
        let total_candidates_usize = usize::try_from(total_candidates).map_err(|_| {
            Self::to_internal_error(format!(
                "Candidate count overflow when building candidate set view: total_candidates={}",
                total_candidates
            ))
        })?;
        let total_pages = total_candidates_usize.div_ceil(load_page_size);
        let mut rows = Vec::with_capacity(total_candidates_usize);
        for page in 0..total_pages {
            let page_rows = storage
                .get_active_standard_passes_by_page_from_history_at_height(
                    page,
                    load_page_size,
                    resolved_height,
                )
                .map_err(Self::to_internal_error)?;
            if page_rows.is_empty() {
                break;
            }
            rows.extend(page_rows);
        }

        let mut ranked = Vec::with_capacity(rows.len());
        for row in rows {
            let pass_id = row.pass.inscription_id;
            let Some(snapshot) = self
                .indexer
                .effective_energy_resolver()
                .resolve_pass_energy(&pass_id, resolved_height, DerivedPassEnergyMode::AtOrBefore)
                .map_err(Self::to_internal_error)?
            else {
                return Err(Self::to_business_error(
                    ERR_INTERNAL_INVARIANT_BROKEN,
                    "INTERNAL_INVARIANT_BROKEN",
                    json!({
                        "inscription_id": pass_id.to_string(),
                        "resolved_height": resolved_height,
                        "detail": "Active standard candidate is missing a raw energy record"
                    }),
                ));
            };

            if snapshot.state != MinerPassState::Active
                || snapshot.pass_kind != MinerPassKind::Standard
            {
                return Err(Self::to_business_error(
                    ERR_INTERNAL_INVARIANT_BROKEN,
                    "INTERNAL_INVARIANT_BROKEN",
                    json!({
                        "inscription_id": pass_id.to_string(),
                        "resolved_height": resolved_height,
                        "state": snapshot.state.as_str(),
                        "pass_kind": snapshot.pass_kind.as_str(),
                        "detail": "Kind-aware active standard query returned a non-candidate pass"
                    }),
                ));
            }

            let level = calc_level_from_effective_energy(snapshot.effective_energy);
            let difficulty_factor_bps = calc_difficulty_factor_bps(level);

            ranked.push(RankedCandidateSetItem {
                effective_energy: snapshot.effective_energy,
                item: CandidateSetViewItem {
                    pass_id: pass_id.to_string(),
                    owner_script_hash: row.pass.owner.to_string(),
                    state: snapshot.state.as_str().to_string(),
                    pass_kind: snapshot.pass_kind.as_str().to_string(),
                    record_block_height: snapshot.record.block_height,
                    raw_energy: encode_energy_decimal(snapshot.raw_energy),
                    collab_contribution: encode_energy_decimal(snapshot.collab_contribution),
                    effective_energy: encode_energy_decimal(snapshot.effective_energy),
                    level,
                    difficulty_factor_bps: difficulty_factor_bps as u64,
                },
            });
        }

        ranked.sort_by(|a, b| {
            b.effective_energy
                .cmp(&a.effective_energy)
                .then_with(|| a.item.pass_id.cmp(&b.item.pass_id))
        });
        let ranked = ranked
            .into_iter()
            .map(|ranked| ranked.item)
            .collect::<Vec<_>>();

        let total = ranked.len() as u64;
        info!(
            "Candidate set view dataset built: module=rpc_server, resolved_height={}, candidate_count={}, ranked_count={}, elapsed_ms={}",
            resolved_height,
            total_candidates,
            total,
            build_start.elapsed().as_millis()
        );

        Ok((total, ranked))
    }
}

impl UsdbIndexerRpc for UsdbIndexerRpcServer {
    fn get_rpc_info(&self) -> JsonResult<RpcInfo> {
        Ok(RpcInfo {
            service: "usdb-indexer".to_string(),
            api_version: "1.0.0".to_string(),
            network: self.config.config().bitcoin.network().to_string(),
            features: vec![
                "snapshot_info".to_string(),
                "pass_block_commit".to_string(),
                "local_state_commit_info".to_string(),
                "system_state_info".to_string(),
                "readiness".to_string(),
                "pass_snapshot".to_string(),
                "pass_history".to_string(),
                "active_passes_at_height".to_string(),
                "pass_stats_at_height".to_string(),
                "owner_active_pass_at_height".to_string(),
                "owner_passes_at_height".to_string(),
                "recent_passes".to_string(),
                "energy_snapshot".to_string(),
                "energy_range".to_string(),
                "pass_energy_leaderboard".to_string(),
                "pass_economic_profile".to_string(),
                "candidate_set_view".to_string(),
                "collab_breakdown".to_string(),
                "invalid_passes".to_string(),
                "active_balance_snapshot".to_string(),
                "latest_active_balance_snapshot".to_string(),
                "stop".to_string(),
            ],
        })
    }

    fn get_network_type(&self) -> JsonResult<String> {
        Ok(self.config.config().bitcoin.network().to_string())
    }

    fn get_sync_status(&self) -> JsonResult<IndexerSyncStatus> {
        let status = self.status.get_index_status_snapshot();
        let synced_block_height = self.synced_height()?;
        Ok(IndexerSyncStatus {
            genesis_block_height: status.genesis_block_height,
            synced_block_height,
            balance_history_stable_height: status.balance_history_stable_height,
            current: status.current,
            total: status.total,
            message: status.message,
        })
    }

    fn get_synced_block_height(&self) -> JsonResult<Option<u64>> {
        Ok(self.synced_height()?.map(|v| v as u64))
    }

    fn get_snapshot_info(&self) -> JsonResult<Option<IndexerSnapshotInfo>> {
        Ok(Some(self.require_upstream_snapshot_info()?))
    }

    fn get_pass_block_commit(
        &self,
        params: GetPassBlockCommitParams,
    ) -> JsonResult<Option<PassBlockCommitInfo>> {
        let resolved_height = self.resolve_height(params.block_height)?;
        let entry = self
            .indexer
            .miner_pass_storage()
            .get_pass_block_commit(resolved_height)
            .map_err(Self::to_internal_error)?;

        Ok(entry.map(|entry| PassBlockCommitInfo {
            block_height: entry.block_height,
            balance_history_block_height: entry.balance_history_block_height,
            balance_history_block_commit: entry.balance_history_block_commit,
            mutation_root: entry.mutation_root,
            block_commit: entry.block_commit,
            commit_protocol_version: entry.commit_protocol_version,
            commit_hash_algo: entry.commit_hash_algo,
        }))
    }

    fn get_local_state_commit_info(&self) -> JsonResult<Option<LocalStateCommitInfo>> {
        Ok(Some(self.require_local_state_commit_info()?))
    }

    fn get_system_state_info(&self) -> JsonResult<Option<SystemStateInfo>> {
        Ok(Some(self.require_system_state_info()?))
    }

    fn get_state_ref_at_height(
        &self,
        params: GetStateRefAtHeightParams,
    ) -> JsonResult<HistoricalStateRefInfo> {
        let expected_state =
            self.validate_consensus_query_context(params.block_height, params.context.as_ref())?;
        self.ensure_consensus_query_ready(Some(params.block_height), "historical state ref query")?;
        let requested_height =
            self.resolve_height_with_consensus_error(Some(params.block_height))?;
        let state_ref = self.build_historical_state_ref_info(requested_height)?;
        self.validate_historical_state_ref_expected_state(
            requested_height,
            &state_ref,
            &expected_state,
        )?;
        Ok(state_ref)
    }

    fn get_readiness(&self) -> JsonResult<ReadinessInfo> {
        self.readiness_info()
    }

    fn get_pass_snapshot(&self, params: GetPassSnapshotParams) -> JsonResult<Option<PassSnapshot>> {
        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_height =
            self.resolve_height_for_contextual_query(params.at_height, params.context.as_ref())?;
        self.ensure_history_height_retained(resolved_height, "historical state")?;
        self.build_pass_snapshot(&inscription_id, resolved_height)
    }

    fn get_active_passes_at_height(
        &self,
        params: GetActivePassesAtHeightParams,
    ) -> JsonResult<ActivePassesAtHeight> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_height = self.resolve_height(params.at_height)?;
        let storage = self.indexer.miner_pass_storage();
        let total = storage
            .get_active_pass_count_from_history_at_height(resolved_height)
            .map_err(Self::to_internal_error)?;
        let rows = storage
            .get_all_active_pass_by_page_from_history_at_height(
                params.page,
                params.page_size,
                resolved_height,
            )
            .map_err(Self::to_internal_error)?;

        Ok(ActivePassesAtHeight {
            resolved_height,
            total,
            items: rows
                .into_iter()
                .map(|row| ActivePassItem {
                    inscription_id: row.inscription_id.to_string(),
                    owner: row.owner.to_string(),
                })
                .collect(),
        })
    }

    fn get_pass_stats_at_height(
        &self,
        params: GetPassStatsAtHeightParams,
    ) -> JsonResult<PassStatsAtHeight> {
        let resolved_height = self.resolve_height(params.at_height)?;
        let stats = self
            .indexer
            .miner_pass_storage()
            .get_pass_state_stats_from_history_at_height(resolved_height)
            .map_err(Self::to_internal_error)?;

        Ok(PassStatsAtHeight {
            resolved_height,
            total_count: stats.total_count,
            active_count: stats.active_count,
            dormant_count: stats.dormant_count,
            consumed_count: stats.consumed_count,
            burned_count: stats.burned_count,
            invalid_count: stats.invalid_count,
        })
    }

    fn get_pass_history(&self, params: GetPassHistoryParams) -> JsonResult<PassHistoryPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;
        let total = self
            .indexer
            .miner_pass_storage()
            .get_pass_history_count_in_height_range(
                &inscription_id,
                params.from_height,
                resolved_to_height,
            )
            .map_err(Self::to_internal_error)?;

        let order = params.order.as_deref().unwrap_or("asc");
        let desc = match order {
            "asc" => false,
            "desc" => true,
            _ => {
                return Err(Self::to_invalid_params(format!(
                    "Invalid history order {}, expected asc or desc",
                    order
                )));
            }
        };

        let items = self
            .indexer
            .miner_pass_storage()
            .get_pass_history_by_page_in_height_range(
                &inscription_id,
                params.from_height,
                resolved_to_height,
                params.page,
                params.page_size,
                desc,
            )
            .map_err(Self::to_internal_error)?;

        Ok(PassHistoryPage {
            resolved_height: resolved_to_height,
            total,
            items: items
                .into_iter()
                .map(|event| PassHistoryEvent {
                    event_id: event.event_id,
                    inscription_id: event.inscription_id.to_string(),
                    block_height: event.block_height,
                    event_type: event.event_type,
                    state: event.state.as_str().to_string(),
                    owner: event.owner.to_string(),
                    satpoint: event.satpoint.to_string(),
                })
                .collect(),
        })
    }

    fn get_owner_active_pass_at_height(
        &self,
        params: GetOwnerActivePassAtHeightParams,
    ) -> JsonResult<Option<PassSnapshot>> {
        let owner_text = params.owner;
        let owner_text_for_duplicate = owner_text.clone();
        let owner = self.parse_owner(&owner_text)?;
        let resolved_height = self.resolve_height(params.at_height)?;

        let active_pass = self
            .indexer
            .miner_pass_storage()
            .get_owner_active_pass_from_history_at_height(&owner, resolved_height)
            .map_err(|e| {
                if e.contains("Duplicate active owner detected") {
                    Self::to_business_error(
                        ERR_DUPLICATE_ACTIVE_OWNER,
                        "DUPLICATE_ACTIVE_OWNER",
                        json!({
                            "owner": owner_text_for_duplicate,
                            "resolved_height": resolved_height
                        }),
                    )
                } else {
                    Self::to_internal_error(e)
                }
            })?;

        let Some(active_pass) = active_pass else {
            return Ok(None);
        };

        match self.build_pass_snapshot(&active_pass.inscription_id, resolved_height)? {
            Some(snapshot) => Ok(Some(snapshot)),
            None => Err(Self::to_business_error(
                ERR_INTERNAL_INVARIANT_BROKEN,
                "INTERNAL_INVARIANT_BROKEN",
                json!({
                    "owner": owner_text,
                    "resolved_height": resolved_height,
                    "inscription_id": active_pass.inscription_id.to_string()
                }),
            )),
        }
    }

    fn get_owner_passes_at_height(
        &self,
        params: GetOwnerPassesAtHeightParams,
    ) -> JsonResult<OwnerPassesAtHeight> {
        self.validate_pagination(params.page, params.page_size)?;

        let owner = self.parse_owner(&params.owner)?;
        let resolved_height = self.resolve_height(params.at_height)?;
        let states = self.parse_optional_pass_states(params.states)?;
        let desc = self.parse_order_desc(params.order.as_deref())?;
        let storage = self.indexer.miner_pass_storage();
        let total = storage
            .get_owner_pass_count_from_history_at_height_by_states(&owner, resolved_height, &states)
            .map_err(Self::to_internal_error)?;
        let rows = storage
            .get_owner_passes_by_page_from_history_at_height_by_states(
                &owner,
                resolved_height,
                &states,
                params.page,
                params.page_size,
                desc,
            )
            .map_err(Self::to_internal_error)?;

        Ok(OwnerPassesAtHeight {
            resolved_height,
            owner: owner.to_string(),
            total,
            items: rows
                .into_iter()
                .map(|row| OwnerPassItem {
                    inscription_id: row.pass.inscription_id.to_string(),
                    inscription_number: row.pass.inscription_number,
                    mint_block_height: row.pass.mint_block_height,
                    owner: row.pass.owner.to_string(),
                    state: row.pass.state.as_str().to_string(),
                    latest_event_height: row.latest_event_height,
                    mint_version: row.pass.mint_version,
                    pass_kind: row.pass.pass_kind.as_str().to_string(),
                    usdb_main: row.pass.usdb_main,
                    leader_pass_id: row.pass.leader_pass_id.map(|id| id.to_string()),
                    leader_btc_addr: row.pass.leader_btc_addr,
                    satpoint: row.pass.satpoint.to_string(),
                })
                .collect(),
        })
    }

    fn get_recent_passes(&self, params: GetRecentPassesParams) -> JsonResult<RecentPassesPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_height = self.resolve_height(params.at_height)?;
        let states = self.parse_optional_pass_states(params.states)?;
        let desc = self.parse_order_desc(params.order.as_deref())?;
        let storage = self.indexer.miner_pass_storage();
        let total = storage
            .get_recent_pass_count_from_history_at_height_by_states(resolved_height, &states)
            .map_err(Self::to_internal_error)?;
        let rows = storage
            .get_recent_passes_by_page_from_history_at_height_by_states(
                resolved_height,
                &states,
                params.page,
                params.page_size,
                desc,
            )
            .map_err(Self::to_internal_error)?;

        Ok(RecentPassesPage {
            resolved_height,
            total,
            items: rows
                .into_iter()
                .map(|row| RecentPassItem {
                    inscription_id: row.pass.inscription_id.to_string(),
                    inscription_number: row.pass.inscription_number,
                    mint_block_height: row.pass.mint_block_height,
                    owner: row.pass.owner.to_string(),
                    state: row.pass.state.as_str().to_string(),
                    latest_event_height: row.latest_event_height,
                    mint_version: row.pass.mint_version,
                    pass_kind: row.pass.pass_kind.as_str().to_string(),
                    usdb_main: row.pass.usdb_main,
                    leader_pass_id: row.pass.leader_pass_id.map(|id| id.to_string()),
                    leader_btc_addr: row.pass.leader_btc_addr,
                    satpoint: row.pass.satpoint.to_string(),
                })
                .collect(),
        })
    }

    fn get_pass_energy(&self, params: GetPassEnergyParams) -> JsonResult<PassEnergySnapshot> {
        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let query_height =
            self.resolve_height_for_contextual_query(params.block_height, params.context.as_ref())?;
        self.ensure_history_height_retained(query_height, "historical state")?;
        let mode = params.mode.unwrap_or_else(|| "at_or_before".to_string());

        let energy_mode = match mode.as_str() {
            "exact" => DerivedPassEnergyMode::Exact,
            "at_or_before" => DerivedPassEnergyMode::AtOrBefore,
            _ => {
                return Err(Self::to_invalid_params(format!(
                    "Invalid energy mode {}, expected exact or at_or_before",
                    mode
                )));
            }
        };

        let Some(snapshot) = self
            .indexer
            .effective_energy_resolver()
            .resolve_pass_energy(&inscription_id, query_height, energy_mode)
            .map_err(Self::to_internal_error)?
        else {
            return Err(Self::to_business_error(
                ERR_ENERGY_NOT_FOUND,
                "ENERGY_NOT_FOUND",
                json!({
                    "inscription_id": params.inscription_id,
                    "query_block_height": query_height,
                    "mode": mode
                }),
            ));
        };

        let record = snapshot.record;
        let level = calc_level_from_effective_energy(snapshot.effective_energy);
        let difficulty_factor_bps = calc_difficulty_factor_bps(level);

        Ok(PassEnergySnapshot {
            inscription_id: record.inscription_id.to_string(),
            query_block_height: query_height,
            record_block_height: record.block_height,
            state: snapshot.state.as_str().to_string(),
            active_block_height: record.active_block_height,
            owner_address: record.owner_address.to_string(),
            owner_balance: record.owner_balance,
            owner_delta: record.owner_delta,
            raw_energy: encode_energy_decimal(snapshot.raw_energy),
            collab_contribution: encode_energy_decimal(snapshot.collab_contribution),
            effective_energy: encode_energy_decimal(snapshot.effective_energy),
            level,
            difficulty_factor_bps: difficulty_factor_bps as u64,
        })
    }

    fn get_collab_breakdown(
        &self,
        params: GetCollabBreakdownParams,
    ) -> JsonResult<CollabBreakdownPage> {
        self.validate_economic_view_version(
            &params.view_version,
            params.block_height,
            params.context.as_ref(),
        )?;
        Self::validate_economic_limit(params.limit)?;
        let leader_pass_id = self.parse_inscription_id(&params.leader_pass_id)?;
        let sort = self.parse_collab_breakdown_sort(params.sort.as_deref())?;
        let cursor = Self::decode_collab_breakdown_cursor(params.cursor.as_deref())?;
        if let Some(cursor) = cursor.as_ref() {
            if cursor.view_version != params.view_version {
                return Err(Self::invalid_economic_pagination(
                    "cursor view_version does not match request view_version",
                ));
            }
            if cursor.leader_pass_id != leader_pass_id.to_string() {
                return Err(Self::invalid_economic_pagination(
                    "cursor leader_pass_id does not match request leader_pass_id",
                ));
            }
            if cursor.sort != sort.as_str() {
                return Err(Self::invalid_economic_pagination(
                    "cursor sort does not match request sort",
                ));
            }
            if cursor.limit != params.limit {
                return Err(Self::invalid_economic_pagination(
                    "cursor limit does not match request limit",
                ));
            }
        }

        let (query_height, initial_state_ref) = match cursor.as_ref() {
            Some(cursor) => self.resolve_economic_cursor_query_context(
                &params.view_version,
                params.block_height,
                params.context.as_ref(),
                &cursor.external_state,
            )?,
            None => self.resolve_economic_query_context(
                &params.view_version,
                params.block_height,
                params.context.as_ref(),
            )?,
        };

        let Some(mut breakdown) = self
            .indexer
            .effective_energy_resolver()
            .resolve_collab_breakdown(&leader_pass_id, query_height)
            .map_err(Self::to_internal_error)?
        else {
            return Err(Self::to_business_error(
                ERR_PASS_NOT_FOUND,
                "PASS_NOT_FOUND",
                json!({
                    "leader_pass_id": params.leader_pass_id,
                    "query_block_height": query_height
                }),
            ));
        };

        Self::sort_collab_breakdown_items(&mut breakdown.items, sort);
        let total = breakdown.items.len() as u64;
        let start = Self::collab_cursor_start(&breakdown.items, cursor.as_ref())?;
        let end = start
            .saturating_add(params.limit)
            .min(breakdown.items.len());
        let items = breakdown.items[start..end]
            .iter()
            .cloned()
            .map(Self::encode_collab_breakdown_item)
            .collect();

        let state_ref = self.revalidate_economic_query_context(query_height, &initial_state_ref)?;
        let external_state = EconomicExternalState::from(&state_ref);
        let next_cursor = if end < breakdown.items.len() {
            let last = &breakdown.items[end - 1];
            Some(Self::encode_cursor(EconomicPageCursor::CollabBreakdown(
                CollabBreakdownCursor {
                    view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                    external_state: external_state.clone(),
                    leader_pass_id: leader_pass_id.to_string(),
                    sort: sort.as_str().to_string(),
                    limit: params.limit,
                    last_collab_contribution: encode_energy_decimal(last.collab_contribution),
                    last_collab_pass_id: last.collab_pass_id.to_string(),
                },
            ))?)
        } else {
            None
        };

        Ok(CollabBreakdownPage {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            external_state,
            leader_pass_id: leader_pass_id.to_string(),
            leader_state: breakdown.leader.pass.state.as_str().to_string(),
            leader_pass_kind: breakdown.leader.pass.pass_kind.as_str().to_string(),
            sort: sort.as_str().to_string(),
            total,
            aggregate_collab_contribution: encode_energy_decimal(
                breakdown.aggregate_collab_contribution,
            ),
            limit: params.limit,
            max_limit: ECONOMIC_PAGE_MAX_LIMIT,
            next_cursor,
            items,
        })
    }

    fn get_pass_energy_range(
        &self,
        params: GetPassEnergyRangeParams,
    ) -> JsonResult<PassEnergyRangePage> {
        self.validate_pagination(params.page, params.page_size)?;

        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;
        let total = self
            .indexer
            .pass_energy_manager()
            .count_pass_energy_records_in_height_range(
                &inscription_id,
                params.from_height,
                resolved_to_height,
            )
            .map_err(Self::to_internal_error)?;

        let order = params.order.as_deref().unwrap_or("asc");
        let desc = match order {
            "asc" => false,
            "desc" => true,
            _ => {
                return Err(Self::to_invalid_params(format!(
                    "Invalid energy range order {}, expected asc or desc",
                    order
                )));
            }
        };

        let records = self
            .indexer
            .pass_energy_manager()
            .get_pass_energy_records_by_page_in_height_range_with_order(
                &inscription_id,
                params.from_height,
                resolved_to_height,
                params.page,
                params.page_size,
                desc,
            )
            .map_err(Self::to_internal_error)?;

        Ok(PassEnergyRangePage {
            resolved_height: resolved_to_height,
            total,
            items: records
                .into_iter()
                .map(|record| PassEnergyRangeItem {
                    inscription_id: record.inscription_id.to_string(),
                    record_block_height: record.block_height,
                    state: record.state.as_str().to_string(),
                    active_block_height: record.active_block_height,
                    owner_address: record.owner_address.to_string(),
                    owner_balance: record.owner_balance,
                    owner_delta: record.owner_delta,
                    energy: encode_energy_decimal(record.energy),
                })
                .collect(),
        })
    }

    fn get_pass_energy_leaderboard(
        &self,
        params: GetPassEnergyLeaderboardParams,
    ) -> JsonResult<PassEnergyLeaderboardPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_height = self.resolve_height(params.at_height)?;
        let scope = self.parse_leaderboard_scope(params.scope.as_deref())?;
        let call_start = Instant::now();
        let (cache_enabled, cache_top_k) = self.leaderboard_cache_settings();
        let offset = Self::pagination_offset(params.page, params.page_size)?;
        let should_use_cache = cache_enabled && params.at_height.is_none();

        if offset >= cache_top_k {
            if should_use_cache
                && let Some(cached_page) = self.try_get_cached_leaderboard_page(
                    resolved_height,
                    scope,
                    cache_top_k,
                    params.page,
                    params.page_size,
                )?
            {
                info!(
                    "Pass energy leaderboard top-k overflow served from cache metadata: module=rpc_server, scope={}, resolved_height={}, top_k={}, page={}, page_size={}, elapsed_ms={}",
                    scope.as_str(),
                    resolved_height,
                    cache_top_k,
                    params.page,
                    params.page_size,
                    call_start.elapsed().as_millis()
                );
                return Ok(cached_page);
            }

            info!(
                "Pass energy leaderboard top-k overflow returned empty: module=rpc_server, scope={}, resolved_height={}, top_k={}, at_height={:?}, page={}, page_size={}, elapsed_ms={}",
                scope.as_str(),
                resolved_height,
                cache_top_k,
                params.at_height,
                params.page,
                params.page_size,
                call_start.elapsed().as_millis()
            );
            return Ok(PassEnergyLeaderboardPage {
                resolved_height,
                total: cache_top_k as u64,
                items: Vec::new(),
            });
        }

        if should_use_cache
            && let Some(cached_page) = self.try_get_cached_leaderboard_page(
                resolved_height,
                scope,
                cache_top_k,
                params.page,
                params.page_size,
            )?
        {
            info!(
                "Pass energy leaderboard served from cache: module=rpc_server, scope={}, resolved_height={}, page={}, page_size={}, total={}, elapsed_ms={}",
                scope.as_str(),
                resolved_height,
                params.page,
                params.page_size,
                cached_page.total,
                call_start.elapsed().as_millis()
            );
            return Ok(cached_page);
        }

        let (raw_total, ranked) =
            self.build_pass_energy_leaderboard_dataset(resolved_height, scope)?;
        let capped_total = raw_total.min(cache_top_k as u64);
        let capped_len = capped_total as usize;
        let capped_ranked = if ranked.len() > capped_len {
            &ranked[..capped_len]
        } else {
            &ranked[..]
        };
        let items = Self::paginate_leaderboard_items(
            capped_ranked,
            capped_total,
            params.page,
            params.page_size,
        )?;

        if should_use_cache {
            self.update_leaderboard_cache(
                resolved_height,
                scope,
                cache_top_k,
                capped_total,
                capped_ranked,
            );
            info!(
                "Pass energy leaderboard cache refreshed: module=rpc_server, scope={}, resolved_height={}, top_k={}, raw_total={}, capped_total={}, page={}, page_size={}, elapsed_ms={}",
                scope.as_str(),
                resolved_height,
                cache_top_k,
                raw_total,
                capped_total,
                params.page,
                params.page_size,
                call_start.elapsed().as_millis()
            );
        } else {
            info!(
                "Pass energy leaderboard served without cache: module=rpc_server, scope={}, resolved_height={}, at_height={:?}, top_k={}, raw_total={}, capped_total={}, page={}, page_size={}, elapsed_ms={}",
                scope.as_str(),
                resolved_height,
                params.at_height,
                cache_top_k,
                raw_total,
                capped_total,
                params.page,
                params.page_size,
                call_start.elapsed().as_millis()
            );
        }

        Ok(PassEnergyLeaderboardPage {
            resolved_height,
            total: capped_total,
            items,
        })
    }

    fn get_pass_economic_profile(
        &self,
        params: GetPassEconomicProfileParams,
    ) -> JsonResult<PassEconomicProfileView> {
        let (query_height, state_ref) = self.resolve_economic_query_context(
            &params.view_version,
            params.block_height,
            params.context.as_ref(),
        )?;
        let pass_id = self.parse_inscription_id(&params.pass_id)?;
        let Some(pass_snapshot) = self
            .indexer
            .miner_pass_storage()
            .get_pass_snapshot_from_history_at_height(&pass_id, query_height)
            .map_err(Self::to_internal_error)?
        else {
            return Err(Self::to_business_error(
                ERR_PASS_NOT_FOUND,
                "PASS_NOT_FOUND",
                json!({
                    "pass_id": params.pass_id,
                    "query_block_height": query_height
                }),
            ));
        };

        let pass = pass_snapshot.pass;
        let (raw_energy, collab_contribution, effective_energy, collab_breakdown_count) =
            if pass.state == MinerPassState::Invalid {
                (0, 0, 0, 0)
            } else {
                let Some(energy) = self
                    .indexer
                    .effective_energy_resolver()
                    .resolve_pass_energy(
                        &pass.inscription_id,
                        query_height,
                        DerivedPassEnergyMode::AtOrBefore,
                    )
                    .map_err(Self::to_internal_error)?
                else {
                    return Err(Self::to_business_error(
                        ERR_INTERNAL_INVARIANT_BROKEN,
                        "INTERNAL_INVARIANT_BROKEN",
                        json!({
                            "pass_id": pass.inscription_id.to_string(),
                            "resolved_height": query_height,
                            "state": pass.state.as_str(),
                            "pass_kind": pass.pass_kind.as_str(),
                            "detail": "Non-invalid pass is missing a raw energy record"
                        }),
                    ));
                };

                if energy.state != pass.state || energy.pass_kind != pass.pass_kind {
                    return Err(Self::to_business_error(
                        ERR_INTERNAL_INVARIANT_BROKEN,
                        "INTERNAL_INVARIANT_BROKEN",
                        json!({
                            "pass_id": pass.inscription_id.to_string(),
                            "resolved_height": query_height,
                            "pass_state": pass.state.as_str(),
                            "energy_state": energy.state.as_str(),
                            "pass_kind": pass.pass_kind.as_str(),
                            "energy_pass_kind": energy.pass_kind.as_str(),
                            "detail": "Pass snapshot and derived energy snapshot disagree"
                        }),
                    ));
                }

                (
                    energy.raw_energy,
                    energy.collab_contribution,
                    energy.effective_energy,
                    energy.collab_breakdown_count,
                )
            };

        let level = calc_level_from_effective_energy(effective_energy);
        let difficulty_factor_bps = calc_difficulty_factor_bps(level);
        let state_ref = self.revalidate_economic_query_context(query_height, &state_ref)?;

        Ok(PassEconomicProfileView {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            external_state: EconomicExternalState::from(&state_ref),
            pass: PassEconomicProfile {
                pass_id: pass.inscription_id.to_string(),
                owner_script_hash: pass.owner.to_string(),
                owner_btc_addr: None,
                state: pass.state.as_str().to_string(),
                pass_kind: pass.pass_kind.as_str().to_string(),
                raw_energy: encode_energy_decimal(raw_energy),
                collab_contribution: encode_energy_decimal(collab_contribution),
                effective_energy: encode_energy_decimal(effective_energy),
                level,
                difficulty_factor_bps: difficulty_factor_bps as u64,
                collab_breakdown_count,
            },
        })
    }

    fn get_candidate_set_view(
        &self,
        params: GetCandidateSetViewParams,
    ) -> JsonResult<CandidateSetViewPage> {
        self.validate_economic_view_version(
            &params.view_version,
            params.block_height,
            params.context.as_ref(),
        )?;
        Self::validate_economic_limit(params.limit)?;
        let selection_rule =
            self.parse_candidate_set_selection_rule(params.selection_rule.as_deref())?;
        let cursor = Self::decode_candidate_set_cursor(params.cursor.as_deref())?;
        if let Some(cursor) = cursor.as_ref() {
            if cursor.view_version != params.view_version {
                return Err(Self::invalid_economic_pagination(
                    "cursor view_version does not match request view_version",
                ));
            }
            if cursor.selection_rule != selection_rule {
                return Err(Self::invalid_economic_pagination(
                    "cursor selection_rule does not match request selection_rule",
                ));
            }
            if cursor.limit != params.limit {
                return Err(Self::invalid_economic_pagination(
                    "cursor limit does not match request limit",
                ));
            }
        }

        let (query_height, initial_state_ref) = match cursor.as_ref() {
            Some(cursor) => self.resolve_economic_cursor_query_context(
                &params.view_version,
                params.block_height,
                params.context.as_ref(),
                &cursor.external_state,
            )?,
            None => self.resolve_economic_query_context(
                &params.view_version,
                params.block_height,
                params.context.as_ref(),
            )?,
        };
        let call_start = Instant::now();

        let (total, ranked) = self.build_candidate_set_view_dataset(query_height)?;
        let start = Self::candidate_cursor_start(&ranked, cursor.as_ref())?;
        let end = start.saturating_add(params.limit).min(ranked.len());
        let items = ranked[start..end].to_vec();

        let state_ref = self.revalidate_economic_query_context(query_height, &initial_state_ref)?;
        let external_state = EconomicExternalState::from(&state_ref);
        let next_cursor = if end < ranked.len() {
            let last = &ranked[end - 1];
            Some(Self::encode_cursor(EconomicPageCursor::CandidateSet(
                CandidateSetCursor {
                    view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                    external_state: external_state.clone(),
                    selection_rule: selection_rule.to_string(),
                    limit: params.limit,
                    last_effective_energy: last.effective_energy.clone(),
                    last_pass_id: last.pass_id.clone(),
                },
            ))?)
        } else {
            None
        };

        info!(
            "Candidate set view served: module=rpc_server, resolved_height={}, selection_rule={}, total={}, limit={}, cursor_present={}, next_cursor_present={}, elapsed_ms={}",
            query_height,
            selection_rule,
            total,
            params.limit,
            cursor.is_some(),
            next_cursor.is_some(),
            call_start.elapsed().as_millis()
        );

        Ok(CandidateSetViewPage {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            external_state,
            selection_rule: selection_rule.to_string(),
            total,
            limit: params.limit,
            max_limit: ECONOMIC_PAGE_MAX_LIMIT,
            next_cursor,
            items,
        })
    }

    fn get_invalid_passes(&self, params: GetInvalidPassesParams) -> JsonResult<InvalidPassesPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;
        let storage = self.indexer.miner_pass_storage();
        let total = storage
            .get_invalid_pass_count_in_height_range(
                params.from_height,
                resolved_to_height,
                params.error_code.as_deref(),
            )
            .map_err(Self::to_internal_error)?;
        let rows = storage
            .get_invalid_passes_by_page_in_height_range(
                params.from_height,
                resolved_to_height,
                params.error_code.as_deref(),
                params.page,
                params.page_size,
            )
            .map_err(Self::to_internal_error)?;

        Ok(InvalidPassesPage {
            resolved_height: resolved_to_height,
            total,
            items: rows
                .into_iter()
                .map(|item| InvalidPassItem {
                    inscription_id: item.inscription_id.to_string(),
                    inscription_number: item.inscription_number,
                    mint_txid: item.mint_txid.to_string(),
                    mint_block_height: item.mint_block_height,
                    mint_owner: item.mint_owner.to_string(),
                    mint_version: item.mint_version,
                    pass_kind: item.pass_kind.as_str().to_string(),
                    usdb_main: item.usdb_main,
                    leader_pass_id: item.leader_pass_id.map(|id| id.to_string()),
                    leader_btc_addr: item.leader_btc_addr,
                    prev: item.prev.into_iter().map(|v| v.to_string()).collect(),
                    invalid_code: item.invalid_code,
                    invalid_reason: item.invalid_reason,
                    owner: item.owner.to_string(),
                    state: item.state.as_str().to_string(),
                    satpoint: item.satpoint.to_string(),
                })
                .collect(),
        })
    }

    fn get_active_balance_snapshot(
        &self,
        params: GetActiveBalanceSnapshotParams,
    ) -> JsonResult<RpcActiveBalanceSnapshot> {
        let upstream_snapshot = self.upstream_snapshot_info()?;
        let local_state = upstream_snapshot.as_ref().and_then(|snapshot| {
            self.build_local_state_commit_info_from_snapshot(snapshot)
                .ok()
        });
        let system_state = local_state
            .as_ref()
            .map(|local_state| self.build_system_state_info_from_local_state(local_state));
        let requested_height =
            self.resolve_height_with_consensus_error(Some(params.block_height))?;
        let active_balance_snapshot = self
            .indexer
            .miner_pass_storage()
            .get_active_balance_snapshot(requested_height)
            .map_err(Self::to_internal_error)?;

        let Some(snapshot) = active_balance_snapshot else {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::NoRecord,
                self.build_consensus_error_data(
                    Some(requested_height),
                    upstream_snapshot.as_ref(),
                    local_state.as_ref(),
                    system_state.as_ref(),
                    Some(format!(
                        "No active balance snapshot recorded at exact height {}",
                        requested_height
                    )),
                ),
            ));
        };

        Ok(RpcActiveBalanceSnapshot {
            block_height: snapshot.block_height,
            total_balance: snapshot.total_balance,
            active_address_count: snapshot.active_address_count,
        })
    }

    fn get_latest_active_balance_snapshot(&self) -> JsonResult<Option<RpcActiveBalanceSnapshot>> {
        let snapshot = self
            .indexer
            .miner_pass_storage()
            .get_latest_active_balance_snapshot()
            .map_err(Self::to_internal_error)?;

        Ok(snapshot.map(|v| RpcActiveBalanceSnapshot {
            block_height: v.block_height,
            total_balance: v.total_balance,
            active_address_count: v.active_address_count,
        }))
    }

    fn stop(&self) -> JsonResult<()> {
        info!("Received stop command via USDB indexer RPC.");
        self.status.set_shutdown_requested(true);
        if let Err(e) = self.shutdown_tx.send(()) {
            return Err(Self::to_internal_error(format!(
                "Failed to send shutdown signal: {}",
                e
            )));
        }

        if let Some(handle) = self.server_handle.lock().unwrap().take() {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                handle.close();
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigManager, IndexerConfig};
    use crate::index::energy_formula::{LEVEL_E0, calc_collab_contribution, calc_growth_delta};
    use crate::index::{InscriptionIndexer, MinerPassKind, MinerPassState, PassBlockCommitEntry};
    use crate::output::IndexOutput;
    use crate::status::StatusManager;
    use crate::storage::{MinerPassInfo, PassEnergyRecord};
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
    use ord::InscriptionId;
    use ordinals::SatPoint;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{
        ConsensusQueryContext, ConsensusRpcErrorCode, ConsensusRpcErrorData,
        ConsensusStateReference, LocalStateActiveBalanceSnapshot, LocalStateCommitIdentity,
        LocalStatePassCommitIdentity, SystemStateIdentity, ToUSDBScriptHash, USDBScriptHash,
        address_string_to_script_hash, build_local_state_commit, build_system_state_id,
    };

    fn test_root_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("usdb_rpc_server_test_{}_{}", tag, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_script_hash(tag: u8) -> USDBScriptHash {
        ScriptBuf::from(vec![tag; 32]).to_usdb_script_hash()
    }

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        InscriptionId {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            index,
        }
    }

    fn test_satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
        SatPoint {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout,
            },
            offset,
        }
    }

    fn make_active_pass(ins_tag: u8, owner_tag: u8, mint_height: u32) -> MinerPassInfo {
        let owner = test_script_hash(owner_tag);
        let inscription_id = test_inscription_id(ins_tag, 0);
        MinerPassInfo {
            inscription_id,
            inscription_number: ins_tag as i32,
            mint_txid: inscription_id.txid,
            mint_block_height: mint_height,
            mint_owner: owner,
            satpoint: test_satpoint(ins_tag, 0, 0),
            mint_version: 1,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "0x1111111111111111111111111111111111111111".to_string(),
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state: MinerPassState::Active,
        }
    }

    fn make_collab_pass(
        ins_tag: u8,
        owner_tag: u8,
        mint_height: u32,
        leader_pass_id: InscriptionId,
    ) -> MinerPassInfo {
        let mut pass = make_active_pass(ins_tag, owner_tag, mint_height);
        pass.pass_kind = MinerPassKind::Collab;
        pass.usdb_main = String::new();
        pass.leader_pass_id = Some(leader_pass_id);
        pass
    }

    fn make_collab_pass_with_leader_addr(
        ins_tag: u8,
        owner_tag: u8,
        mint_height: u32,
        leader_btc_addr: &str,
        leader_btc_owner: USDBScriptHash,
    ) -> MinerPassInfo {
        let mut pass = make_active_pass(ins_tag, owner_tag, mint_height);
        pass.pass_kind = MinerPassKind::Collab;
        pass.usdb_main = String::new();
        pass.leader_pass_id = None;
        pass.leader_btc_addr = Some(leader_btc_addr.to_string());
        pass.leader_btc_owner = Some(leader_btc_owner);
        pass
    }

    fn make_invalid_pass(
        ins_tag: u8,
        owner_tag: u8,
        mint_height: u32,
        code: &str,
    ) -> MinerPassInfo {
        let mut pass = make_active_pass(ins_tag, owner_tag, mint_height);
        pass.state = MinerPassState::Invalid;
        pass.invalid_code = Some(code.to_string());
        pass.invalid_reason = Some(format!("mock reason for {}", code));
        pass
    }

    fn seed_energy_record(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
        energy: Energy,
    ) {
        seed_energy_record_with_state(server, pass, block_height, MinerPassState::Active, energy);
    }

    fn seed_energy_record_with_state(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
        state: MinerPassState,
        energy: Energy,
    ) {
        seed_energy_record_with_state_and_balance(
            server,
            pass,
            block_height,
            state,
            energy,
            100_000,
        );
    }

    fn seed_energy_record_with_state_and_balance(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
        state: MinerPassState,
        energy: Energy,
        owner_balance: u64,
    ) {
        server
            .indexer
            .pass_energy_manager()
            .insert_pass_energy_record_for_test(&PassEnergyRecord {
                inscription_id: pass.inscription_id,
                block_height,
                state,
                active_block_height: block_height,
                owner_address: pass.owner,
                owner_balance,
                owner_delta: 0,
                energy,
            })
            .unwrap();
    }

    fn get_pass_energy_exact_for_test(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
    ) -> PassEnergySnapshot {
        server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(block_height),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap()
    }

    fn get_collab_breakdown_for_test(
        server: &UsdbIndexerRpcServer,
        leader: &MinerPassInfo,
        block_height: u32,
    ) -> CollabBreakdownPage {
        server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: Some(block_height),
                context: None,
                sort: None,
                cursor: None,
                limit: 20,
            })
            .unwrap()
    }

    fn get_pass_economic_profile_for_test(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
    ) -> PassEconomicProfileView {
        server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(block_height),
                context: None,
            })
            .unwrap()
    }

    fn build_server(tag: &str, synced_height: u32) -> (UsdbIndexerRpcServer, PathBuf) {
        let root_dir = test_root_dir(tag);
        let mut config_file = IndexerConfig::default();
        // Test helpers that use synthetic low BTC heights should not inherit the
        // production-like default genesis height, otherwise retention-floor
        // checks would classify every query as pruned before the fixture data
        // is even inserted.
        config_file.usdb.genesis_block_height = 0;
        std::fs::write(
            root_dir.join("config.json"),
            serde_json::to_vec_pretty(&config_file).unwrap(),
        )
        .unwrap();
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let output = Arc::new(IndexOutput::new());
        let status = Arc::new(StatusManager::new(config.clone(), output).unwrap());
        let indexer = Arc::new(InscriptionIndexer::new(config.clone(), status.clone()).unwrap());

        indexer
            .miner_pass_storage()
            .update_synced_btc_block_height(synced_height)
            .unwrap();

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(());
        let server = UsdbIndexerRpcServer::new(
            config,
            status,
            indexer,
            "127.0.0.1:0".parse().unwrap(),
            shutdown_tx,
        );
        (server, root_dir)
    }

    fn build_server_with_genesis(
        tag: &str,
        synced_height: u32,
        genesis_block_height: u32,
    ) -> (UsdbIndexerRpcServer, PathBuf) {
        let root_dir = test_root_dir(tag);
        let mut config_file = IndexerConfig::default();
        config_file.usdb.genesis_block_height = genesis_block_height;
        std::fs::write(
            root_dir.join("config.json"),
            serde_json::to_vec_pretty(&config_file).unwrap(),
        )
        .unwrap();
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let output = Arc::new(IndexOutput::new());
        let status = Arc::new(StatusManager::new(config.clone(), output).unwrap());
        let indexer = Arc::new(InscriptionIndexer::new(config.clone(), status.clone()).unwrap());

        indexer
            .miner_pass_storage()
            .update_synced_btc_block_height(synced_height)
            .unwrap();

        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(());
        let server = UsdbIndexerRpcServer::new(
            config,
            status,
            indexer,
            "127.0.0.1:0".parse().unwrap(),
            shutdown_tx,
        );
        (server, root_dir)
    }

    fn ready_balance_history_snapshot(stable_height: u32) -> balance_history::SnapshotInfo {
        balance_history::SnapshotInfo {
            stable_height,
            stable_block_hash: Some("aa".repeat(32)),
            latest_block_commit: Some("bb".repeat(32)),
            stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
            balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION.to_string(),
            balance_history_semantics_version: balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
                .to_string(),
            commit_protocol_version: "1.0.0".to_string(),
            commit_hash_algo: "sha256".to_string(),
        }
    }

    fn ready_balance_history_readiness(stable_height: u32) -> balance_history::ReadinessInfo {
        balance_history::ReadinessInfo {
            service: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
            rpc_alive: true,
            query_ready: true,
            consensus_ready: true,
            phase: balance_history::SyncPhase::Synced,
            current: stable_height as u64,
            total: stable_height as u64,
            message: Some("synced".to_string()),
            stable_height: Some(stable_height),
            stable_block_hash: Some("aa".repeat(32)),
            latest_block_commit: Some("bb".repeat(32)),
            snapshot_origin: None,
            snapshot_verification_state: None,
            snapshot_signing_key_id: None,
            script_registry: balance_history::ScriptRegistryStatus {
                available: true,
                count: Some(0),
                policy: "auxiliary_seen_scripts_non_consensus_v1".to_string(),
            },
            blockers: Vec::new(),
        }
    }

    fn seed_upstream_anchor(server: &UsdbIndexerRpcServer, stable_height: u32) {
        let snapshot = ready_balance_history_snapshot(stable_height);
        server
            .status
            .set_balance_history_snapshot(Some(snapshot.clone()));
        server
            .status
            .set_balance_history_readiness(Some(ready_balance_history_readiness(stable_height)));
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&snapshot)
            .unwrap();
    }

    fn seed_state_ref_context(server: &UsdbIndexerRpcServer, block_height: u32) {
        seed_upstream_anchor(server, block_height);
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(block_height, 5_000, 2)
            .unwrap();
    }

    fn decode_consensus_error_data(err: &JsonError) -> ConsensusRpcErrorData {
        serde_json::from_value(err.data.clone().expect("missing structured error data"))
            .expect("invalid structured error data")
    }

    #[test]
    fn test_get_snapshot_info_success() {
        let (server, root_dir) = build_server("snapshot_info", 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let snapshot = server.get_snapshot_info().unwrap().unwrap();
        assert_eq!(snapshot.local_synced_block_height, 120);
        assert_eq!(snapshot.balance_history_stable_height, 120);
        assert_eq!(snapshot.stable_block_hash, "aa".repeat(32));
        assert_eq!(snapshot.latest_block_commit, "bb".repeat(32));
        assert_eq!(
            snapshot.consensus_identity.source_chain,
            CONSENSUS_SOURCE_CHAIN_BTC
        );
        assert_eq!(snapshot.consensus_identity.stable_height, 120);
        assert_eq!(
            snapshot.consensus_identity.stable_block_hash,
            "aa".repeat(32)
        );
        assert_eq!(snapshot.consensus_identity.stable_lag, 0);
        assert_eq!(
            snapshot.consensus_identity.balance_history_api_version,
            balance_history::BALANCE_HISTORY_API_VERSION
        );
        assert_eq!(
            snapshot
                .consensus_identity
                .balance_history_semantics_version,
            balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
        );
        assert_eq!(
            snapshot.consensus_identity.usdb_index_formula_version,
            USDB_INDEX_FORMULA_VERSION
        );
        assert_eq!(
            snapshot.consensus_identity.usdb_index_protocol_version,
            USDB_INDEX_PROTOCOL_VERSION
        );
        assert_eq!(snapshot.commit_protocol_version, "1.0.0");
        assert_eq!(snapshot.commit_hash_algo, "sha256");
        assert_eq!(snapshot.snapshot_id_hash_algo, SNAPSHOT_ID_HASH_ALGO);
        assert_eq!(snapshot.snapshot_id_version, SNAPSHOT_ID_VERSION);
        assert_eq!(
            snapshot.snapshot_id,
            build_consensus_snapshot_id(&snapshot.consensus_identity)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_snapshot_info_returns_snapshot_not_ready_when_anchor_missing() {
        let (server, root_dir) = build_server("snapshot_info_not_ready", 120);

        let err = server.get_snapshot_info().unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(
            err.message,
            ConsensusRpcErrorCode::SnapshotNotReady.as_str()
        );
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.actual_state.snapshot_id, None);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_readiness_defaults_to_not_ready_before_rpc_alive() {
        let (server, root_dir) = build_server("readiness_default_not_ready", 120);

        let readiness = server.get_readiness().unwrap();
        assert!(!readiness.rpc_alive);
        assert!(!readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::RpcNotListening)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_readiness_consensus_ready_when_caught_up_with_complete_state() {
        let (server, root_dir) = build_server("readiness_consensus_ready", 120);
        server.status.set_rpc_alive(true);
        seed_state_ref_context(&server, 120);

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(readiness.query_ready);
        assert!(readiness.consensus_ready);
        assert_eq!(readiness.synced_block_height, Some(120));
        assert_eq!(readiness.balance_history_stable_height, Some(120));
        assert!(readiness.upstream_snapshot_id.is_some());
        assert!(readiness.local_state_commit.is_some());
        assert!(readiness.system_state_id.is_some());
        assert!(readiness.blockers.is_empty());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_readiness_not_consensus_ready_while_catching_up() {
        let (server, root_dir) = build_server("readiness_catching_up", 100);
        server.status.set_rpc_alive(true);
        seed_upstream_anchor(&server, 100);
        server
            .status
            .set_balance_history_snapshot(Some(ready_balance_history_snapshot(105)));
        server
            .status
            .set_balance_history_readiness(Some(ready_balance_history_readiness(105)));

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert_eq!(readiness.synced_block_height, Some(100));
        assert_eq!(readiness.balance_history_stable_height, Some(105));
        assert!(readiness.blockers.contains(&ReadinessBlocker::CatchingUp));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_readiness_not_query_ready_during_reorg_recovery() {
        let (server, root_dir) = build_server("readiness_reorg_recovery", 100);
        server.status.set_rpc_alive(true);
        seed_upstream_anchor(&server, 100);
        server.status.set_upstream_reorg_recovery_pending(true);

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(!readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::ReorgRecoveryPending)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_readiness_not_consensus_ready_when_upstream_not_ready() {
        let (server, root_dir) = build_server("readiness_upstream_not_ready", 100);
        server.status.set_rpc_alive(true);
        seed_upstream_anchor(&server, 100);

        let mut upstream_readiness = ready_balance_history_readiness(100);
        upstream_readiness.consensus_ready = false;
        upstream_readiness.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
        server
            .status
            .set_balance_history_readiness(Some(upstream_readiness));

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::UpstreamConsensusNotReady)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_snapshot_info_snapshot_id_ignores_local_synced_height() {
        let (server_a, root_dir_a) = build_server("snapshot_id_ignore_local_a", 120);
        server_a
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let (server_b, root_dir_b) = build_server("snapshot_id_ignore_local_b", 135);
        server_b
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server_b
            .indexer
            .miner_pass_storage()
            .update_synced_btc_block_height(135)
            .unwrap();

        let snapshot_a = server_a.get_snapshot_info().unwrap().unwrap();
        let snapshot_b = server_b.get_snapshot_info().unwrap().unwrap();
        assert_ne!(
            snapshot_a.local_synced_block_height,
            snapshot_b.local_synced_block_height
        );
        assert_eq!(snapshot_a.consensus_identity, snapshot_b.consensus_identity);
        assert_eq!(snapshot_a.snapshot_id, snapshot_b.snapshot_id);

        drop(server_a);
        drop(server_b);
        std::fs::remove_dir_all(root_dir_a).unwrap();
        std::fs::remove_dir_all(root_dir_b).unwrap();
    }

    #[test]
    fn test_get_snapshot_info_uses_persisted_stable_lag() {
        let (server, root_dir) = build_server("snapshot_info_stable_lag", 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: 2,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let snapshot = server.get_snapshot_info().unwrap().unwrap();
        assert_eq!(snapshot.consensus_identity.stable_lag, 2);
        assert_eq!(
            snapshot.snapshot_id,
            build_consensus_snapshot_id(&snapshot.consensus_identity)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_block_commit_success() {
        let (server, root_dir) = build_server("pass_block_commit", 140);
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 140,
                balance_history_block_height: 140,
                balance_history_block_commit: "aa".repeat(32),
                mutation_root: "bb".repeat(32),
                block_commit: "cc".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();

        let commit = server
            .get_pass_block_commit(GetPassBlockCommitParams {
                block_height: Some(140),
            })
            .unwrap()
            .unwrap();

        assert_eq!(commit.block_height, 140);
        assert_eq!(commit.balance_history_block_height, 140);
        assert_eq!(commit.balance_history_block_commit, "aa".repeat(32));
        assert_eq!(commit.mutation_root, "bb".repeat(32));
        assert_eq!(commit.block_commit, "cc".repeat(32));
        assert_eq!(commit.commit_protocol_version, "1.0.0");
        assert_eq!(commit.commit_hash_algo, "sha256");

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_local_state_commit_info_success() {
        let (server, root_dir) = build_server_with_genesis("local_state_commit_success", 120, 100);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "cc".repeat(32),
                mutation_root: "dd".repeat(32),
                block_commit: "ee".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let info = server.get_local_state_commit_info().unwrap().unwrap();
        assert_eq!(info.local_synced_block_height, 120);
        assert_eq!(info.local_state_commit_hash_algo, LOCAL_STATE_HASH_ALGO);
        assert_eq!(info.local_state_commit_version, LOCAL_STATE_VERSION);
        assert_eq!(
            info.latest_pass_block_commit,
            Some(LocalStatePassCommitIdentity {
                block_height: 120,
                block_commit: "ee".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
        );
        assert_eq!(
            info.latest_active_balance_snapshot,
            Some(LocalStateActiveBalanceSnapshot {
                block_height: 120,
                total_balance: 5_000,
                active_address_count: 2,
            })
        );
        assert_eq!(
            info.local_state_identity,
            LocalStateCommitIdentity {
                upstream_snapshot_id: info.upstream_snapshot_id.clone(),
                local_synced_block_height: 120,
                latest_pass_block_commit: info.latest_pass_block_commit.clone(),
                latest_active_balance_snapshot: info.latest_active_balance_snapshot.clone(),
                usdb_index_protocol_version: USDB_INDEX_PROTOCOL_VERSION.to_string(),
            }
        );
        assert_eq!(
            info.local_state_commit,
            build_local_state_commit(&info.local_state_identity)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_system_state_info_success() {
        let (server, root_dir) = build_server_with_genesis("system_state_info_success", 120, 100);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("aa".repeat(32)),
                latest_block_commit: Some("bb".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "cc".repeat(32),
                mutation_root: "dd".repeat(32),
                block_commit: "ee".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let local = server.get_local_state_commit_info().unwrap().unwrap();
        let system = server.get_system_state_info().unwrap().unwrap();
        assert_eq!(system.local_synced_block_height, 120);
        assert_eq!(system.upstream_snapshot_id, local.upstream_snapshot_id);
        assert_eq!(system.local_state_commit, local.local_state_commit);
        assert_eq!(
            system.system_state_identity,
            SystemStateIdentity {
                upstream_snapshot_id: system.upstream_snapshot_id.clone(),
                local_state_commit: system.local_state_commit.clone(),
            }
        );
        assert_eq!(system.system_state_id_hash_algo, SYSTEM_STATE_HASH_ALGO);
        assert_eq!(system.system_state_id_version, SYSTEM_STATE_VERSION);
        assert_eq!(
            system.system_state_id,
            build_system_state_id(&system.system_state_identity)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_success() {
        let (server, root_dir) = build_server_with_genesis("state_ref_at_height_success", 130, 100);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 120,
                stable_block_hash: Some("11".repeat(32)),
                latest_block_commit: Some("22".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 130,
                stable_block_hash: Some("33".repeat(32)),
                latest_block_commit: Some("44".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "55".repeat(32),
                mutation_root: "66".repeat(32),
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(130, 7_000, 3)
            .unwrap();

        let state_ref = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: None,
            })
            .unwrap();
        assert_eq!(state_ref.block_height, 120);
        assert_eq!(state_ref.snapshot_info.local_synced_block_height, 120);
        assert_eq!(state_ref.snapshot_info.balance_history_stable_height, 120);
        assert_eq!(state_ref.snapshot_info.stable_block_hash, "11".repeat(32));
        assert_eq!(state_ref.snapshot_info.latest_block_commit, "22".repeat(32));
        assert_eq!(
            state_ref.snapshot_info.snapshot_id,
            build_consensus_snapshot_id(&state_ref.snapshot_info.consensus_identity)
        );
        assert_eq!(
            state_ref.local_state_commit_info.local_synced_block_height,
            120
        );
        assert_eq!(
            state_ref.local_state_commit_info.latest_pass_block_commit,
            Some(LocalStatePassCommitIdentity {
                block_height: 120,
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
        );
        assert_eq!(
            state_ref
                .local_state_commit_info
                .latest_active_balance_snapshot,
            Some(LocalStateActiveBalanceSnapshot {
                block_height: 120,
                total_balance: 5_000,
                active_address_count: 2,
            })
        );
        assert_eq!(
            state_ref.local_state_commit_info.local_state_commit,
            build_local_state_commit(&state_ref.local_state_commit_info.local_state_identity)
        );
        assert_eq!(
            state_ref.system_state_info.system_state_id,
            build_system_state_id(&state_ref.system_state_info.system_state_identity)
        );
        let external_state = EconomicExternalState::from(&state_ref);
        assert_eq!(external_state.btc_height, 120);
        assert_eq!(
            external_state.snapshot_id,
            state_ref.snapshot_info.snapshot_id
        );
        assert_eq!(external_state.stable_block_hash, "11".repeat(32));
        assert_eq!(
            external_state.local_state_commit,
            state_ref.local_state_commit_info.local_state_commit
        );
        assert_eq!(
            external_state.system_state_id,
            state_ref.system_state_info.system_state_id
        );
        assert_eq!(
            external_state.balance_history_api_version,
            balance_history::BALANCE_HISTORY_API_VERSION
        );
        assert_eq!(
            external_state.balance_history_semantics_version,
            balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
        );
        assert_eq!(
            external_state.usdb_index_protocol_version,
            USDB_INDEX_PROTOCOL_VERSION
        );
        assert_eq!(
            external_state.usdb_index_formula_version,
            USDB_INDEX_FORMULA_VERSION
        );
        let context = ConsensusQueryContext::from(&external_state);
        assert_eq!(context.requested_height, Some(120));
        assert_eq!(
            context.expected_state,
            ConsensusStateReference::from(&state_ref)
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_height_not_synced_for_future_height() {
        let (server, root_dir) = build_server_with_genesis("state_ref_at_height_future", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 121,
                context: None,
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::HeightNotSynced.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(err.message, ConsensusRpcErrorCode::HeightNotSynced.as_str());
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.requested_height, Some(121));
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.upstream_stable_height, Some(120));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_snapshot_id_mismatch() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_snapshot_mismatch", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "55".repeat(32),
                mutation_root: "66".repeat(32),
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference {
                        snapshot_id: Some("ff".repeat(32)),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotIdMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.mismatch_field.as_deref(), Some("snapshot_id"));
        assert_eq!(data.expected_state.snapshot_id, Some("ff".repeat(32)));
        assert_eq!(data.actual_state.stable_height, Some(120));
        assert_eq!(data.actual_state.stable_block_hash, Some("aa".repeat(32)));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_local_state_commit_mismatch() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_local_mismatch", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "55".repeat(32),
                mutation_root: "66".repeat(32),
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference {
                        local_state_commit: Some("ee".repeat(32)),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::LocalStateCommitMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.mismatch_field.as_deref(), Some("local_state_commit"));
        assert_eq!(
            data.expected_state.local_state_commit,
            Some("ee".repeat(32))
        );
        assert_eq!(
            data.actual_state.local_state_commit,
            Some(
                server
                    .get_state_ref_at_height(GetStateRefAtHeightParams {
                        block_height: 120,
                        context: None,
                    })
                    .unwrap()
                    .local_state_commit_info
                    .local_state_commit
            )
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_system_state_id_mismatch() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_system_mismatch", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "55".repeat(32),
                mutation_root: "66".repeat(32),
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference {
                        system_state_id: Some("dd".repeat(32)),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SystemStateIdMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.mismatch_field.as_deref(), Some("system_state_id"));
        assert_eq!(data.expected_state.system_state_id, Some("dd".repeat(32)));
        assert!(data.actual_state.system_state_id.is_some());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_version_mismatch() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_version_mismatch", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "55".repeat(32),
                mutation_root: "66".repeat(32),
                block_commit: "77".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference {
                        balance_history_semantics_version: Some(
                            "balance-snapshot-at-or-before:v999".to_string(),
                        ),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::VersionMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(
            data.mismatch_field.as_deref(),
            Some("balance_history_semantics_version")
        );
        assert_eq!(
            data.expected_state.balance_history_semantics_version,
            Some("balance-snapshot-at-or-before:v999".to_string())
        );
        assert_eq!(
            data.actual_state.balance_history_semantics_version,
            Some(balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string())
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_historical_state_ref_uses_recorded_protocol_and_formula_versions() {
        let (server, root_dir) =
            build_server_with_genesis("historical_recorded_versions", 120, 100);
        seed_state_ref_context(&server, 120);
        let mut state_ref = server.build_historical_state_ref_info(120).unwrap();
        state_ref
            .snapshot_info
            .consensus_identity
            .usdb_index_protocol_version = "historical-protocol:v1".to_string();
        state_ref
            .snapshot_info
            .consensus_identity
            .usdb_index_formula_version = "historical-formula:v1".to_string();

        server
            .validate_historical_state_ref_expected_state(
                120,
                &state_ref,
                &ConsensusStateReference {
                    usdb_index_protocol_version: Some("historical-protocol:v1".to_string()),
                    usdb_index_formula_version: Some("historical-formula:v1".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let cases = [
            (
                ConsensusStateReference {
                    usdb_index_protocol_version: Some("historical-protocol:v2".to_string()),
                    ..Default::default()
                },
                ConsensusRpcErrorCode::ProtocolVersionMismatch,
                "usdb_index_protocol_version",
            ),
            (
                ConsensusStateReference {
                    usdb_index_formula_version: Some("historical-formula:v2".to_string()),
                    ..Default::default()
                },
                ConsensusRpcErrorCode::FormulaVersionMismatch,
                "usdb_index_formula_version",
            ),
        ];

        for (expected_state, expected_code, mismatch_field) in cases {
            let err = server
                .validate_historical_state_ref_expected_state(120, &state_ref, &expected_state)
                .unwrap_err();
            match err.code {
                ErrorCode::ServerError(code) => assert_eq!(code, expected_code.code()),
                _ => panic!("unexpected error code: {:?}", err.code),
            }
            assert_eq!(err.message, expected_code.as_str());
            let data = decode_consensus_error_data(&err);
            assert_eq!(data.mismatch_field.as_deref(), Some(mismatch_field));
            assert_eq!(
                data.actual_state.usdb_index_protocol_version.as_deref(),
                Some("historical-protocol:v1")
            );
            assert_eq!(
                data.actual_state.usdb_index_formula_version.as_deref(),
                Some("historical-formula:v1")
            );
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_history_not_available_when_balance_snapshot_missing() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_history_not_available", 120, 100);
        seed_upstream_anchor(&server, 120);

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: None,
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::HistoryNotAvailable.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.actual_state.stable_height, Some(120));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_state_not_retained_below_genesis() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_below_genesis", 120, 110);
        seed_state_ref_context(&server, 120);

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 109,
                context: None,
            })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::StateNotRetained.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(109));
        assert!(
            data.detail
                .as_deref()
                .unwrap_or_default()
                .contains("historical state retention floor 110")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_state_ref_at_height_returns_snapshot_not_ready_when_consensus_not_ready() {
        let (server, root_dir) =
            build_server_with_genesis("state_ref_at_height_not_ready", 120, 100);
        seed_state_ref_context(&server, 120);
        server.status.set_rpc_alive(true);

        let mut upstream_readiness = ready_balance_history_readiness(120);
        upstream_readiness.consensus_ready = false;
        upstream_readiness.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
        server
            .status
            .set_balance_history_readiness(Some(upstream_readiness));

        let err = server
            .get_state_ref_at_height(GetStateRefAtHeightParams {
                block_height: 120,
                context: None,
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.consensus_ready, Some(false));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_local_state_commit_info_returns_snapshot_not_ready_when_anchor_missing() {
        let (server, root_dir) =
            build_server_with_genesis("local_state_commit_not_ready", 120, 100);

        let err = server.get_local_state_commit_info().unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(
            err.message,
            ConsensusRpcErrorCode::SnapshotNotReady.as_str()
        );
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.actual_state.local_state_commit, None);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_system_state_info_returns_snapshot_not_ready_when_anchor_missing() {
        let (server, root_dir) = build_server_with_genesis("system_state_not_ready", 120, 100);

        let err = server.get_system_state_info().unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(
            err.message,
            ConsensusRpcErrorCode::SnapshotNotReady.as_str()
        );
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.actual_state.system_state_id, None);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_local_state_commit_info_uses_latest_pass_commit_at_or_before_synced_height() {
        let (server, root_dir) =
            build_server_with_genesis("local_state_commit_latest_pass", 125, 100);
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&balance_history::SnapshotInfo {
                stable_height: 125,
                stable_block_hash: Some("11".repeat(32)),
                latest_block_commit: Some("22".repeat(32)),
                stable_lag: balance_history::BALANCE_HISTORY_STABLE_LAG,
                balance_history_api_version: balance_history::BALANCE_HISTORY_API_VERSION
                    .to_string(),
                balance_history_semantics_version:
                    balance_history::BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_pass_block_commit(&PassBlockCommitEntry {
                block_height: 120,
                balance_history_block_height: 120,
                balance_history_block_commit: "33".repeat(32),
                mutation_root: "44".repeat(32),
                block_commit: "55".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
            .unwrap();
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(125, 7_500, 3)
            .unwrap();

        let info = server.get_local_state_commit_info().unwrap().unwrap();
        assert_eq!(info.local_synced_block_height, 125);
        assert_eq!(
            info.latest_pass_block_commit,
            Some(LocalStatePassCommitIdentity {
                block_height: 120,
                block_commit: "55".repeat(32),
                commit_protocol_version: "1.0.0".to_string(),
                commit_hash_algo: "sha256".to_string(),
            })
        );
        assert_eq!(
            info.latest_active_balance_snapshot,
            Some(LocalStateActiveBalanceSnapshot {
                block_height: 125,
                total_balance: 7_500,
                active_address_count: 3,
            })
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_active_balance_snapshot_returns_height_not_synced_for_future_height() {
        let (server, root_dir) =
            build_server_with_genesis("active_balance_future_height", 120, 100);
        seed_upstream_anchor(&server, 120);
        server
            .indexer
            .miner_pass_storage()
            .upsert_active_balance_snapshot(120, 5_000, 2)
            .unwrap();

        let err = server
            .get_active_balance_snapshot(GetActiveBalanceSnapshotParams { block_height: 121 })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::HeightNotSynced.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(err.message, ConsensusRpcErrorCode::HeightNotSynced.as_str());
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.requested_height, Some(121));
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.upstream_stable_height, Some(120));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_active_balance_snapshot_returns_no_record_at_valid_height() {
        let (server, root_dir) = build_server_with_genesis("active_balance_no_record", 120, 100);
        seed_upstream_anchor(&server, 120);

        let err = server
            .get_active_balance_snapshot(GetActiveBalanceSnapshotParams { block_height: 120 })
            .unwrap_err();
        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::NoRecord.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(err.message, ConsensusRpcErrorCode::NoRecord.as_str());
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, USDB_INDEXER_SERVICE_NAME);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.local_synced_height, Some(120));
        assert_eq!(data.upstream_stable_height, Some(120));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_and_history_success() {
        let (server, root_dir) = build_server("snapshot_history", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(1, 10, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                101,
            )
            .unwrap();

        let snapshot = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(101),
                context: None,
            })
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.inscription_id, pass.inscription_id.to_string());
        assert_eq!(snapshot.state, MinerPassState::Dormant.as_str());
        assert_eq!(snapshot.resolved_height, 101);

        let history = server
            .get_pass_history(GetPassHistoryParams {
                inscription_id: pass.inscription_id.to_string(),
                from_height: 100,
                to_height: 101,
                order: Some("asc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(history.resolved_height, 101);
        assert_eq!(history.total, 2);
        assert_eq!(history.items.len(), 2);
        assert_eq!(history.items[0].event_type, "mint");
        assert_eq!(history.items[1].event_type, "state_update");

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_rejects_mismatched_context_height() {
        let (server, root_dir) = build_server("snapshot_context_height_mismatch", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(21, 121, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_state_ref_context(&server, 101);

        let err = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(101),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(102),
                    expected_state: ConsensusStateReference::default(),
                }),
            })
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InvalidParams);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_returns_snapshot_id_mismatch_with_context() {
        let (server, root_dir) = build_server("snapshot_context_snapshot_mismatch", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(22, 122, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                101,
            )
            .unwrap();
        seed_state_ref_context(&server, 101);

        let err = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(101),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(101),
                    expected_state: ConsensusStateReference {
                        snapshot_id: Some("ff".repeat(32)),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotIdMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(101));
        assert_eq!(data.expected_state.snapshot_id, Some("ff".repeat(32)));
        assert_eq!(data.actual_state.stable_height, Some(101));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_returns_history_not_available_when_context_state_ref_missing() {
        let (server, root_dir) =
            build_server_with_genesis("snapshot_context_history_not_available", 120, 100);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(25, 125, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                101,
            )
            .unwrap();
        seed_upstream_anchor(&server, 101);

        let err = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(101),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(101),
                    expected_state: ConsensusStateReference {
                        snapshot_id: Some("aa".repeat(32)),
                        ..Default::default()
                    },
                }),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::HistoryNotAvailable.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(101));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_returns_snapshot_not_ready_when_context_consensus_not_ready() {
        let (server, root_dir) = build_server_with_genesis("snapshot_context_not_ready", 120, 100);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(27, 127, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_state_ref_context(&server, 101);
        server.status.set_rpc_alive(true);

        let mut upstream_readiness = ready_balance_history_readiness(101);
        upstream_readiness.consensus_ready = false;
        upstream_readiness.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
        server
            .status
            .set_balance_history_readiness(Some(upstream_readiness));

        let err = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(101),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(101),
                    expected_state: ConsensusStateReference::default(),
                }),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(101));
        assert_eq!(data.consensus_ready, Some(false));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_snapshot_returns_state_not_retained_below_pass_history_floor() {
        let (server, root_dir) = build_server_with_genesis("snapshot_state_not_retained", 120, 110);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(26, 126, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_state_ref_context(&server, 120);

        let err = server
            .get_pass_snapshot(GetPassSnapshotParams {
                inscription_id: pass.inscription_id.to_string(),
                at_height: Some(109),
                context: None,
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::StateNotRetained.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(109));
        assert!(
            data.detail
                .as_deref()
                .unwrap_or_default()
                .contains("historical state retention floor 110")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_owner_active_pass_duplicate_owner_error() {
        let (server, root_dir) = build_server("duplicate_owner", 200);
        let storage = server.indexer.miner_pass_storage();
        let owner = test_script_hash(33);

        let mut pass1 = make_active_pass(2, 33, 100);
        pass1.owner = owner;
        pass1.mint_owner = owner;
        let ins2 = test_inscription_id(3, 0);

        storage.add_new_mint_pass_at_height(&pass1, 100).unwrap();
        // Inject a second active history snapshot for the same owner to emulate
        // corrupted history state and assert RPC defensive behavior.
        storage
            .append_pass_history_event_for_test(
                &ins2,
                101,
                "mint",
                None,
                MinerPassState::Active,
                None,
                owner,
                None,
                test_satpoint(3, 0, 0),
            )
            .unwrap();

        let err = server
            .get_owner_active_pass_at_height(GetOwnerActivePassAtHeightParams {
                owner: owner.to_string(),
                at_height: Some(200),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => assert_eq!(code, ERR_DUPLICATE_ACTIVE_OWNER),
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(err.message, "DUPLICATE_ACTIVE_OWNER");

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_owner_passes_at_height_returns_current_owner_rows_desc() {
        let (server, root_dir) = build_server("owner_passes", 200);
        let storage = server.indexer.miner_pass_storage();
        let owner = test_script_hash(77);

        let mut active = make_active_pass(41, 77, 100);
        active.owner = owner;
        active.mint_owner = owner;
        storage.add_new_mint_pass_at_height(&active, 100).unwrap();

        let dormant_owner = test_script_hash(78);
        let mut dormant = make_active_pass(42, 78, 101);
        dormant.owner = dormant_owner;
        dormant.mint_owner = dormant_owner;
        storage.add_new_mint_pass_at_height(&dormant, 101).unwrap();
        storage
            .update_state_at_height(
                &dormant.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                110,
            )
            .unwrap();
        storage
            .transfer_owner_at_height(
                &dormant.inscription_id,
                &owner,
                &test_satpoint(42, 1, 0),
                125,
            )
            .unwrap();

        let mut invalid = make_invalid_pass(43, 77, 102, "INVALID_USDB_MAIN");
        invalid.owner = owner;
        invalid.mint_owner = owner;
        storage
            .add_invalid_mint_pass_at_height(&invalid, 102)
            .unwrap();

        let page0 = server
            .get_owner_passes_at_height(GetOwnerPassesAtHeightParams {
                owner: owner.to_string(),
                at_height: Some(200),
                states: None,
                order: None,
                page: 0,
                page_size: 2,
            })
            .unwrap();

        assert_eq!(page0.resolved_height, 200);
        assert_eq!(page0.owner, owner.to_string());
        assert_eq!(page0.total, 3);
        assert_eq!(page0.items.len(), 2);
        assert_eq!(
            page0.items[0].inscription_id,
            dormant.inscription_id.to_string()
        );
        assert_eq!(page0.items[0].state, "dormant");
        assert_eq!(page0.items[0].latest_event_height, 125);
        assert_eq!(page0.items[1].latest_event_height, 102);

        let page1 = server
            .get_owner_passes_at_height(GetOwnerPassesAtHeightParams {
                owner: owner.to_string(),
                at_height: Some(200),
                states: None,
                order: None,
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page1.total, 3);
        assert_eq!(page1.items.len(), 1);
        assert_eq!(
            page1.items[0].inscription_id,
            active.inscription_id.to_string()
        );

        let active_only = server
            .get_owner_passes_at_height(GetOwnerPassesAtHeightParams {
                owner: owner.to_string(),
                at_height: Some(200),
                states: Some(vec!["active".to_string()]),
                order: Some("desc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(active_only.total, 1);
        assert_eq!(
            active_only.items[0].inscription_id,
            active.inscription_id.to_string()
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_owner_passes_at_height_accepts_btc_address() {
        let (server, root_dir) = build_server("owner_passes_address", 200);
        let storage = server.indexer.miner_pass_storage();
        let address = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let owner =
            address_string_to_script_hash(address, &server.config.config().bitcoin.network())
                .unwrap();

        let mut pass = make_active_pass(65, 95, 100);
        pass.owner = owner;
        pass.mint_owner = owner;
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();

        let page = server
            .get_owner_passes_at_height(GetOwnerPassesAtHeightParams {
                owner: address.to_string(),
                at_height: Some(200),
                states: Some(vec!["active".to_string()]),
                order: Some("desc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();

        assert_eq!(page.owner, owner.to_string());
        assert_eq!(page.total, 1);
        assert_eq!(
            page.items[0].inscription_id,
            pass.inscription_id.to_string()
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_recent_passes_returns_rows_by_mint_height_desc_and_filters_state() {
        let (server, root_dir) = build_server("recent_passes", 200);
        let storage = server.indexer.miner_pass_storage();

        let older = make_active_pass(61, 91, 100);
        storage.add_new_mint_pass_at_height(&older, 100).unwrap();

        let newer = make_active_pass(62, 92, 130);
        storage.add_new_mint_pass_at_height(&newer, 130).unwrap();

        let dormant = make_active_pass(63, 93, 120);
        storage.add_new_mint_pass_at_height(&dormant, 120).unwrap();
        storage
            .update_state_at_height(
                &dormant.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                140,
            )
            .unwrap();

        let page0 = server
            .get_recent_passes(GetRecentPassesParams {
                at_height: Some(200),
                states: None,
                order: Some("desc".to_string()),
                page: 0,
                page_size: 2,
            })
            .unwrap();

        assert_eq!(page0.resolved_height, 200);
        assert_eq!(page0.total, 3);
        assert_eq!(page0.items.len(), 2);
        assert_eq!(
            page0.items[0].inscription_id,
            newer.inscription_id.to_string()
        );
        assert_eq!(page0.items[0].mint_block_height, 130);
        assert_eq!(
            page0.items[1].inscription_id,
            dormant.inscription_id.to_string()
        );
        assert_eq!(page0.items[1].state, "dormant");
        assert_eq!(page0.items[1].latest_event_height, 140);

        let page1 = server
            .get_recent_passes(GetRecentPassesParams {
                at_height: Some(200),
                states: None,
                order: Some("desc".to_string()),
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page1.total, 3);
        assert_eq!(page1.items.len(), 1);
        assert_eq!(
            page1.items[0].inscription_id,
            older.inscription_id.to_string()
        );

        let dormant_only = server
            .get_recent_passes(GetRecentPassesParams {
                at_height: Some(200),
                states: Some(vec!["dormant".to_string()]),
                order: Some("desc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(dormant_only.total, 1);
        assert_eq!(
            dormant_only.items[0].inscription_id,
            dormant.inscription_id.to_string()
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_invalid_passes_success() {
        let (server, root_dir) = build_server("invalid_passes", 150);
        let storage = server.indexer.miner_pass_storage();

        let invalid = make_invalid_pass(4, 44, 110, "INVALID_USDB_MAIN");
        storage
            .add_invalid_mint_pass_at_height(&invalid, 110)
            .unwrap();

        let page = server
            .get_invalid_passes(GetInvalidPassesParams {
                error_code: Some("INVALID_USDB_MAIN".to_string()),
                from_height: 100,
                to_height: 120,
                page: 0,
                page_size: 10,
            })
            .unwrap();

        assert_eq!(page.resolved_height, 120);
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].inscription_id,
            invalid.inscription_id.to_string()
        );
        assert_eq!(
            page.items[0].invalid_code.as_deref(),
            Some("INVALID_USDB_MAIN")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_stats_at_height_success() {
        let (server, root_dir) = build_server("pass_stats", 150);
        let storage = server.indexer.miner_pass_storage();

        let active = make_active_pass(5, 50, 100);
        storage.add_new_mint_pass_at_height(&active, 100).unwrap();

        let dormant = make_active_pass(6, 60, 100);
        storage.add_new_mint_pass_at_height(&dormant, 100).unwrap();
        storage
            .update_state_at_height(
                &dormant.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();

        let invalid = make_invalid_pass(7, 70, 110, "INVALID_USDB_MAIN");
        storage
            .add_invalid_mint_pass_at_height(&invalid, 110)
            .unwrap();

        let stats = server
            .get_pass_stats_at_height(GetPassStatsAtHeightParams {
                at_height: Some(120),
            })
            .unwrap();
        assert_eq!(stats.resolved_height, 120);
        assert_eq!(stats.total_count, 3);
        assert_eq!(stats.active_count, 1);
        assert_eq!(stats.dormant_count, 1);
        assert_eq!(stats.invalid_count, 1);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_cache_refresh_on_height_change() {
        let (server, root_dir) = build_server("leaderboard_cache", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(8, 80, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 777);

        let page_120 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(page_120.resolved_height, 120);
        assert_eq!(page_120.total, 1);
        assert_eq!(page_120.items.len(), 1);
        assert_eq!(
            page_120.items[0].inscription_id,
            pass.inscription_id.to_string()
        );
        assert_eq!(page_120.items[0].energy, "777");

        {
            let cache = server.pass_energy_leaderboard_cache.lock().unwrap();
            let entry = cache.latest.as_ref().expect("cache should be populated");
            assert_eq!(entry.resolved_height, 120);
            assert_eq!(entry.total, 1);
            assert_eq!(entry.items.len(), 1);
        }

        storage.update_synced_btc_block_height(121).unwrap();
        let page_121 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 0,
                page_size: 10,
            })
            .unwrap();
        let expected_energy_121 = 777u128.saturating_add(calc_growth_delta(100_000, 1));
        assert_eq!(page_121.resolved_height, 121);
        assert_eq!(page_121.total, 1);
        assert_eq!(page_121.items.len(), 1);
        assert_eq!(page_121.items[0].energy, expected_energy_121.to_string());

        {
            let cache = server.pass_energy_leaderboard_cache.lock().unwrap();
            let entry = cache.latest.as_ref().expect("cache should be refreshed");
            assert_eq!(entry.resolved_height, 121);
            assert_eq!(entry.total, 1);
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_economic_profile_aggregates_collabs_and_matches_breakdown() {
        let (server, root_dir) = build_server("economic_profile_collab_aggregate", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(150, 230, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        let collab1 = make_collab_pass(151, 231, 101, leader.inscription_id);
        let collab2 = make_collab_pass(152, 232, 102, leader.inscription_id);
        for collab in [&collab1, &collab2] {
            storage
                .add_new_mint_pass_at_height(collab, collab.mint_block_height)
                .unwrap();
        }

        let raw_energy = LEVEL_E0 - 100;
        seed_energy_record(&server, &leader, 120, raw_energy);
        seed_energy_record(&server, &collab1, 120, 100);
        seed_energy_record(&server, &collab2, 120, 200);

        let profile = get_pass_economic_profile_for_test(&server, &leader, 120);
        let expected_contribution =
            calc_collab_contribution(100).saturating_add(calc_collab_contribution(200));
        let expected_effective = raw_energy.saturating_add(expected_contribution);

        assert_eq!(profile.view_version, USDB_ECONOMIC_STATE_VIEW_VERSION);
        assert_eq!(profile.external_state.btc_height, 120);
        assert_eq!(profile.external_state.stable_block_hash, "aa".repeat(32));
        assert_eq!(profile.pass.pass_id, leader.inscription_id.to_string());
        assert_eq!(profile.pass.owner_script_hash, leader.owner.to_string());
        assert_eq!(profile.pass.owner_btc_addr, None);
        assert_eq!(profile.pass.state, MinerPassState::Active.as_str());
        assert_eq!(profile.pass.pass_kind, MinerPassKind::Standard.as_str());
        assert_eq!(profile.pass.raw_energy, raw_energy.to_string());
        assert_eq!(
            profile.pass.collab_contribution,
            expected_contribution.to_string()
        );
        assert_eq!(
            profile.pass.effective_energy,
            expected_effective.to_string()
        );
        assert_eq!(profile.pass.level, 1);
        assert_eq!(profile.pass.difficulty_factor_bps, 9_900);
        assert_eq!(profile.pass.collab_breakdown_count, 2);

        let breakdown = get_collab_breakdown_for_test(&server, &leader, 120);
        let recomputed_contribution = breakdown.items.iter().fold(0u128, |total, item| {
            total.saturating_add(item.collab_contribution.parse::<u128>().unwrap())
        });
        assert_eq!(breakdown.external_state, profile.external_state);
        assert_eq!(breakdown.total, profile.pass.collab_breakdown_count);
        assert_eq!(recomputed_contribution, expected_contribution);
        assert_eq!(
            breakdown.aggregate_collab_contribution,
            profile.pass.collab_contribution
        );

        let replay = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: leader.inscription_id.to_string(),
                block_height: None,
                context: Some(ConsensusQueryContext::from(&profile.external_state)),
            })
            .unwrap();
        assert_eq!(replay.external_state, profile.external_state);
        assert_eq!(replay.pass.effective_energy, profile.pass.effective_energy);
        assert_eq!(replay.pass.collab_breakdown_count, 2);

        let rpc_info = server.get_rpc_info().unwrap();
        assert!(
            rpc_info
                .features
                .contains(&"pass_economic_profile".to_string())
        );
        assert!(rpc_info.features.contains(&"collab_breakdown".to_string()));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_economic_profile_applies_state_and_invalid_zero_boundaries() {
        let (server, root_dir) = build_server("economic_profile_state_boundaries", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(153, 233, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        let collab = make_collab_pass(154, 234, 101, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
            .unwrap();
        seed_energy_record(&server, &collab, 120, 500);

        let dormant = make_active_pass(155, 235, 100);
        storage
            .add_new_mint_pass_at_height(&dormant, dormant.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &dormant.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                115,
            )
            .unwrap();
        seed_energy_record_with_state(&server, &dormant, 115, MinerPassState::Dormant, 700);

        let invalid = make_invalid_pass(156, 236, 100, "invalid-profile");
        storage
            .add_invalid_mint_pass_at_height(&invalid, invalid.mint_block_height)
            .unwrap();

        let collab_profile = get_pass_economic_profile_for_test(&server, &collab, 120);
        assert_eq!(collab_profile.pass.raw_energy, "500");
        assert_eq!(collab_profile.pass.collab_contribution, "0");
        assert_eq!(collab_profile.pass.effective_energy, "0");
        assert_eq!(collab_profile.pass.level, 0);
        assert_eq!(collab_profile.pass.difficulty_factor_bps, 10_000);
        assert_eq!(collab_profile.pass.collab_breakdown_count, 0);

        let dormant_profile = get_pass_economic_profile_for_test(&server, &dormant, 120);
        assert_eq!(dormant_profile.pass.state, MinerPassState::Dormant.as_str());
        assert_eq!(dormant_profile.pass.raw_energy, "700");
        assert_eq!(dormant_profile.pass.collab_contribution, "0");
        assert_eq!(dormant_profile.pass.effective_energy, "0");
        assert_eq!(dormant_profile.pass.level, 0);
        assert_eq!(dormant_profile.pass.difficulty_factor_bps, 10_000);
        assert_eq!(dormant_profile.pass.collab_breakdown_count, 0);

        assert!(
            server
                .indexer
                .pass_energy_manager()
                .get_pass_energy_record_at_or_before(&invalid.inscription_id, 120)
                .unwrap()
                .is_none()
        );
        let invalid_profile = get_pass_economic_profile_for_test(&server, &invalid, 120);
        assert_eq!(invalid_profile.pass.state, MinerPassState::Invalid.as_str());
        assert_eq!(invalid_profile.pass.raw_energy, "0");
        assert_eq!(invalid_profile.pass.collab_contribution, "0");
        assert_eq!(invalid_profile.pass.effective_energy, "0");
        assert_eq!(invalid_profile.pass.level, 0);
        assert_eq!(invalid_profile.pass.difficulty_factor_bps, 10_000);
        assert_eq!(invalid_profile.pass.collab_breakdown_count, 0);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_economic_profile_distinguishes_missing_pass_and_broken_energy_invariant() {
        let (server, root_dir) = build_server("economic_profile_error_boundaries", 130);
        seed_state_ref_context(&server, 120);

        let missing_id = test_inscription_id(157, 0);
        let missing = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: missing_id.to_string(),
                block_height: Some(120),
                context: None,
            })
            .unwrap_err();
        assert_eq!(missing.code, ErrorCode::ServerError(ERR_PASS_NOT_FOUND));
        assert_eq!(missing.message, "PASS_NOT_FOUND");

        let pass = make_active_pass(158, 238, 100);
        server
            .indexer
            .miner_pass_storage()
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        let broken = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
            })
            .unwrap_err();
        assert_eq!(
            broken.code,
            ErrorCode::ServerError(ERR_INTERNAL_INVARIANT_BROKEN)
        );
        assert_eq!(broken.message, "INTERNAL_INVARIANT_BROKEN");

        seed_energy_record(&server, &pass, 120, 100);
        let valid_profile = get_pass_economic_profile_for_test(&server, &pass, 120);
        let mut formula_mismatch_context =
            ConsensusQueryContext::from(&valid_profile.external_state);
        formula_mismatch_context
            .expected_state
            .usdb_index_formula_version = Some("pass-energy-formula:unexpected".to_string());
        let formula_mismatch = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(formula_mismatch_context),
            })
            .unwrap_err();
        assert_eq!(
            formula_mismatch.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::FormulaVersionMismatch.code())
        );
        assert_eq!(
            decode_consensus_error_data(&formula_mismatch)
                .mismatch_field
                .as_deref(),
            Some("usdb_index_formula_version")
        );

        let unsupported = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: "uip-0006-usdb-economic-state-view:v999".to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
            })
            .unwrap_err();
        assert_eq!(
            unsupported.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::ViewVersionMismatch.code())
        );
        assert_eq!(
            decode_consensus_error_data(&unsupported)
                .mismatch_field
                .as_deref(),
            Some("view_version")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_economic_profile_rejects_replaced_same_height_external_state() {
        let (server, root_dir) = build_server("economic_profile_same_height_reorg", 120);
        seed_state_ref_context(&server, 120);
        let pass = make_active_pass(160, 240, 100);
        server
            .indexer
            .miner_pass_storage()
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        seed_energy_record(&server, &pass, 120, 100);
        let original = get_pass_economic_profile_for_test(&server, &pass, 120);

        let mut replacement = ready_balance_history_snapshot(120);
        replacement.stable_block_hash = Some("cc".repeat(32));
        replacement.latest_block_commit = Some("dd".repeat(32));
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&replacement)
            .unwrap();

        let mismatch = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext::from(&original.external_state)),
            })
            .unwrap_err();
        assert_eq!(
            mismatch.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::SnapshotIdMismatch.code())
        );
        assert_eq!(
            decode_consensus_error_data(&mismatch)
                .mismatch_field
                .as_deref(),
            Some("snapshot_id")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_economic_query_revalidation_rejects_state_change_during_derivation() {
        let (server, root_dir) = build_server("economic_query_revalidation_reorg", 120);
        seed_state_ref_context(&server, 120);
        let (_, initial_state_ref) = server
            .resolve_economic_query_context(USDB_ECONOMIC_STATE_VIEW_VERSION, Some(120), None)
            .unwrap();

        // Simulate a same-height reorg after the request resolved its initial
        // state ref but before the economic response is finalized.
        let mut replacement = ready_balance_history_snapshot(120);
        replacement.stable_block_hash = Some("cc".repeat(32));
        replacement.latest_block_commit = Some("dd".repeat(32));
        server
            .indexer
            .miner_pass_storage()
            .upsert_balance_history_snapshot_anchor(&replacement)
            .unwrap();

        let mismatch = server
            .revalidate_economic_query_context(120, &initial_state_ref)
            .unwrap_err();
        assert_eq!(
            mismatch.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::SnapshotIdMismatch.code())
        );
        assert_eq!(
            decode_consensus_error_data(&mismatch)
                .mismatch_field
                .as_deref(),
            Some("snapshot_id")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_economic_profile_replays_old_external_state_after_head_advances() {
        let (server, root_dir) = build_server("economic_profile_historical_replay", 130);
        seed_state_ref_context(&server, 120);
        let pass = make_active_pass(159, 239, 100);
        server
            .indexer
            .miner_pass_storage()
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        seed_energy_record(&server, &pass, 110, 100);

        let original = get_pass_economic_profile_for_test(&server, &pass, 120);
        let expected_raw = 100u128.saturating_add(calc_growth_delta(100_000, 10));
        assert_eq!(original.pass.raw_energy, expected_raw.to_string());

        seed_state_ref_context(&server, 130);
        let replay = server
            .get_pass_economic_profile(GetPassEconomicProfileParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                pass_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext::from(&original.external_state)),
            })
            .unwrap();

        assert_eq!(replay.external_state, original.external_state);
        assert_eq!(replay.pass.raw_energy, original.pass.raw_energy);
        assert_eq!(replay.pass.effective_energy, original.pass.effective_energy);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_at_or_before_projects_to_query_height() {
        let (server, root_dir) = build_server("energy_projection", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(11, 110, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, LEVEL_E0 - 10);

        let projected = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(130),
                context: None,
                mode: Some("at_or_before".to_string()),
            })
            .unwrap();

        let expected = (LEVEL_E0 - 10).saturating_add(calc_growth_delta(100_000, 10));
        assert_eq!(projected.query_block_height, 130);
        assert_eq!(projected.record_block_height, 120);
        assert_eq!(projected.raw_energy, expected.to_string());
        assert_eq!(projected.collab_contribution, "0");
        assert_eq!(projected.effective_energy, expected.to_string());
        assert_eq!(projected.level, 1);
        assert_eq!(projected.difficulty_factor_bps, 9_900);
        let projected_json = serde_json::to_value(&projected).unwrap();
        assert!(projected_json.get("energy").is_none());
        assert_eq!(projected_json["raw_energy"], expected.to_string());
        assert_eq!(projected_json["collab_contribution"], "0");
        assert_eq!(projected_json["effective_energy"], expected.to_string());
        assert_eq!(projected_json["level"], 1);
        assert_eq!(projected_json["difficulty_factor_bps"], 9_900);

        let exact = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();
        assert_eq!(exact.query_block_height, 120);
        assert_eq!(exact.record_block_height, 120);
        assert_eq!(exact.raw_energy, (LEVEL_E0 - 10).to_string());
        assert_eq!(exact.collab_contribution, "0");
        assert_eq!(exact.effective_energy, (LEVEL_E0 - 10).to_string());
        assert_eq!(exact.level, 0);
        assert_eq!(exact.difficulty_factor_bps, 10_000);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_derives_active_standard_collab_contribution() {
        let (server, root_dir) = build_server("energy_effective_standard_collab", 130);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(70, 170, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();

        let collab1 = make_collab_pass(71, 171, 101, leader.inscription_id);
        let collab2 = make_collab_pass(72, 172, 102, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab1, collab1.mint_block_height)
            .unwrap();
        storage
            .add_new_mint_pass_at_height(&collab2, collab2.mint_block_height)
            .unwrap();

        let leader_raw_energy = LEVEL_E0 - 1;
        seed_energy_record(&server, &leader, 120, leader_raw_energy);
        seed_energy_record(&server, &collab1, 110, 2);
        seed_energy_record(&server, &collab2, 120, 200);

        let snapshot = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();

        let collab1_projected_raw = 2u128.saturating_add(calc_growth_delta(100_000, 10));
        let expected_contribution = calc_collab_contribution(collab1_projected_raw)
            .saturating_add(calc_collab_contribution(200));
        assert_eq!(snapshot.raw_energy, leader_raw_energy.to_string());
        assert_eq!(
            snapshot.collab_contribution,
            expected_contribution.to_string()
        );
        assert_eq!(
            snapshot.effective_energy,
            leader_raw_energy
                .saturating_add(expected_contribution)
                .to_string()
        );
        assert!(
            leader_raw_energy < LEVEL_E0,
            "raw energy alone should still be below level 1"
        );
        assert_eq!(snapshot.level, 1);
        assert_eq!(snapshot.difficulty_factor_bps, 9_900);

        let raw_record = server
            .indexer
            .pass_energy_manager()
            .get_pass_energy_record_exact(&leader.inscription_id, 120)
            .unwrap()
            .unwrap();
        assert_eq!(
            raw_record.energy, leader_raw_energy,
            "derived effective energy must not be written back to raw energy storage"
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_active_collab_effective_zero() {
        let (server, root_dir) = build_server("energy_effective_collab_zero", 130);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(73, 173, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        let collab = make_collab_pass(74, 174, 101, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
            .unwrap();
        seed_energy_record(&server, &leader, 120, 1_000);
        seed_energy_record(&server, &collab, 120, 500);

        let snapshot = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: collab.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();

        assert_eq!(snapshot.raw_energy, "500");
        assert_eq!(snapshot.collab_contribution, "0");
        assert_eq!(snapshot.effective_energy, "0");
        assert_eq!(snapshot.level, 0);
        assert_eq!(snapshot.difficulty_factor_bps, 10_000);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_derives_leader_btc_addr_collab_contribution() {
        let (server, root_dir) = build_server("energy_effective_leader_btc_addr", 130);
        let storage = server.indexer.miner_pass_storage();
        let leader_btc_addr = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let leader_owner = address_string_to_script_hash(
            leader_btc_addr,
            &server.config.config().bitcoin.network(),
        )
        .unwrap();

        let mut leader = make_active_pass(76, 176, 100);
        leader.owner = leader_owner;
        leader.mint_owner = leader_owner;
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();

        let collab = make_collab_pass_with_leader_addr(77, 177, 101, leader_btc_addr, leader_owner);
        storage
            .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
            .unwrap();

        seed_energy_record(&server, &leader, 120, 1_000);
        seed_energy_record(&server, &collab, 120, 300);

        let snapshot = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();

        let expected_contribution = calc_collab_contribution(300);
        assert_eq!(snapshot.raw_energy, "1000");
        assert_eq!(
            snapshot.collab_contribution,
            expected_contribution.to_string()
        );
        assert_eq!(
            snapshot.effective_energy,
            1_000u128.saturating_add(expected_contribution).to_string()
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_non_active_and_invalid_leaders_receive_no_collab_contribution() {
        let (server, root_dir) = build_server("energy_effective_inactive_invalid_leader", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        for (leader_tag, owner_tag, terminal_state) in [
            (100, 200, MinerPassState::Dormant),
            (102, 202, MinerPassState::Consumed),
            (104, 204, MinerPassState::Burned),
        ] {
            let leader = make_active_pass(leader_tag, owner_tag, 100);
            storage
                .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
                .unwrap();
            storage
                .update_state_at_height(
                    &leader.inscription_id,
                    terminal_state.clone(),
                    MinerPassState::Active,
                    120,
                )
                .unwrap();
            let collab =
                make_collab_pass(leader_tag + 1, owner_tag + 1, 101, leader.inscription_id);
            storage
                .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
                .unwrap();

            seed_energy_record_with_state(&server, &leader, 120, terminal_state.clone(), 1_000);
            seed_energy_record(&server, &collab, 120, 400);

            let snapshot = get_pass_energy_exact_for_test(&server, &leader, 120);
            assert_eq!(snapshot.state, terminal_state.as_str());
            assert_eq!(snapshot.raw_energy, "1000");
            assert_eq!(snapshot.collab_contribution, "0");
            assert_eq!(snapshot.effective_energy, "0");
            assert_eq!(snapshot.level, 0);
            assert_eq!(snapshot.difficulty_factor_bps, 10_000);

            let breakdown = get_collab_breakdown_for_test(&server, &leader, 120);
            assert_eq!(breakdown.total, 0);
            assert_eq!(breakdown.aggregate_collab_contribution, "0");
            assert!(breakdown.items.is_empty());
        }

        let invalid_leader = make_invalid_pass(106, 206, 100, "invalid-leader");
        storage
            .add_invalid_mint_pass_at_height(&invalid_leader, invalid_leader.mint_block_height)
            .unwrap();
        let invalid_collab = make_collab_pass(107, 207, 101, invalid_leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&invalid_collab, invalid_collab.mint_block_height)
            .unwrap();
        seed_energy_record_with_state(
            &server,
            &invalid_leader,
            120,
            MinerPassState::Invalid,
            1_000,
        );
        seed_energy_record(&server, &invalid_collab, 120, 400);

        let invalid_snapshot = get_pass_energy_exact_for_test(&server, &invalid_leader, 120);
        assert_eq!(invalid_snapshot.state, MinerPassState::Invalid.as_str());
        assert_eq!(invalid_snapshot.raw_energy, "1000");
        assert_eq!(invalid_snapshot.collab_contribution, "0");
        assert_eq!(invalid_snapshot.effective_energy, "0");
        assert_eq!(invalid_snapshot.level, 0);
        assert_eq!(invalid_snapshot.difficulty_factor_bps, 10_000);

        let invalid_breakdown = get_collab_breakdown_for_test(&server, &invalid_leader, 120);
        assert_eq!(invalid_breakdown.total, 0);
        assert_eq!(invalid_breakdown.aggregate_collab_contribution, "0");
        assert!(invalid_breakdown.items.is_empty());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_leader_btc_addr_follows_remint_but_fixed_leader_pass_id_does_not() {
        let (server, root_dir) = build_server("energy_effective_leader_remint_follow", 150);
        seed_state_ref_context(&server, 140);
        let storage = server.indexer.miner_pass_storage();
        let leader_btc_addr = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let leader_owner = address_string_to_script_hash(
            leader_btc_addr,
            &server.config.config().bitcoin.network(),
        )
        .unwrap();

        let mut leader1 = make_active_pass(108, 208, 100);
        leader1.owner = leader_owner;
        leader1.mint_owner = leader_owner;
        storage
            .add_new_mint_pass_at_height(&leader1, leader1.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &leader1.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                130,
            )
            .unwrap();

        let mut leader2 = make_active_pass(109, 209, 130);
        leader2.owner = leader_owner;
        leader2.mint_owner = leader_owner;
        storage
            .add_new_mint_pass_at_height(&leader2, leader2.mint_block_height)
            .unwrap();

        let collab_by_addr =
            make_collab_pass_with_leader_addr(110, 210, 101, leader_btc_addr, leader_owner);
        let collab_fixed = make_collab_pass(111, 211, 101, leader1.inscription_id);
        for pass in [&collab_by_addr, &collab_fixed] {
            storage
                .add_new_mint_pass_at_height(pass, pass.mint_block_height)
                .unwrap();
        }

        seed_energy_record_with_state(&server, &leader1, 140, MinerPassState::Dormant, 1_000);
        seed_energy_record(&server, &leader2, 140, 200);
        seed_energy_record(&server, &collab_by_addr, 140, 300);
        seed_energy_record(&server, &collab_fixed, 140, 500);

        let leader2_snapshot = get_pass_energy_exact_for_test(&server, &leader2, 140);
        let addr_contribution = calc_collab_contribution(300);
        assert_eq!(leader2_snapshot.raw_energy, "200");
        assert_eq!(
            leader2_snapshot.collab_contribution,
            addr_contribution.to_string()
        );
        assert_eq!(
            leader2_snapshot.effective_energy,
            200u128.saturating_add(addr_contribution).to_string()
        );

        let leader2_breakdown = get_collab_breakdown_for_test(&server, &leader2, 140);
        assert_eq!(leader2_breakdown.total, 1);
        assert_eq!(
            leader2_breakdown.items[0].collab_pass_id,
            collab_by_addr.inscription_id.to_string()
        );
        assert_eq!(
            leader2_breakdown.items[0].leader_ref_kind,
            "leader_btc_addr"
        );

        let leader1_snapshot = get_pass_energy_exact_for_test(&server, &leader1, 140);
        assert_eq!(leader1_snapshot.state, MinerPassState::Dormant.as_str());
        assert_eq!(leader1_snapshot.collab_contribution, "0");
        assert_eq!(leader1_snapshot.effective_energy, "0");
        assert_eq!(leader1_snapshot.level, 0);
        assert_eq!(leader1_snapshot.difficulty_factor_bps, 10_000);

        let leader1_breakdown = get_collab_breakdown_for_test(&server, &leader1, 140);
        assert_eq!(leader1_breakdown.total, 0);
        assert_eq!(leader1_breakdown.aggregate_collab_contribution, "0");

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_consumed_collab_stops_contributing_to_old_leader() {
        let (server, root_dir) = build_server("energy_effective_consumed_collab", 150);
        seed_state_ref_context(&server, 130);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(112, 212, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        let collab = make_collab_pass(113, 213, 101, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
            .unwrap();

        seed_energy_record(&server, &leader, 120, 100);
        seed_energy_record(&server, &collab, 120, 600);

        let active_snapshot = get_pass_energy_exact_for_test(&server, &leader, 120);
        let active_contribution = calc_collab_contribution(600);
        assert_eq!(
            active_snapshot.collab_contribution,
            active_contribution.to_string()
        );

        storage
            .update_state_at_height(
                &collab.inscription_id,
                MinerPassState::Consumed,
                MinerPassState::Active,
                130,
            )
            .unwrap();
        seed_energy_record(&server, &leader, 130, 150);
        seed_energy_record_with_state(&server, &collab, 130, MinerPassState::Consumed, 0);

        let after_consumed = get_pass_energy_exact_for_test(&server, &leader, 130);
        assert_eq!(after_consumed.raw_energy, "150");
        assert_eq!(after_consumed.collab_contribution, "0");
        assert_eq!(after_consumed.effective_energy, "150");
        assert_eq!(after_consumed.level, 0);
        assert_eq!(after_consumed.difficulty_factor_bps, 10_000);

        let breakdown = get_collab_breakdown_for_test(&server, &leader, 130);
        assert_eq!(breakdown.total, 0);
        assert_eq!(breakdown.aggregate_collab_contribution, "0");
        assert!(breakdown.items.is_empty());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_non_active_standard_effective_zero() {
        let (server, root_dir) = build_server("energy_effective_non_active_zero", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(75, 175, 100);
        storage
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();
        seed_energy_record_with_state(&server, &pass, 120, MinerPassState::Dormant, 700);

        let snapshot = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();

        assert_eq!(snapshot.state, MinerPassState::Dormant.as_str());
        assert_eq!(snapshot.raw_energy, "700");
        assert_eq!(snapshot.collab_contribution, "0");
        assert_eq!(snapshot.effective_energy, "0");
        assert_eq!(snapshot.level, 0);
        assert_eq!(snapshot.difficulty_factor_bps, 10_000);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_collab_breakdown_returns_stable_pages_and_aggregate() {
        let (server, root_dir) = build_server("collab_breakdown_pages", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();
        let leader_btc_addr = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let leader_owner = address_string_to_script_hash(
            leader_btc_addr,
            &server.config.config().bitcoin.network(),
        )
        .unwrap();

        let mut leader = make_active_pass(78, 178, 100);
        leader.owner = leader_owner;
        leader.mint_owner = leader_owner;
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        let other_leader = make_active_pass(79, 179, 100);
        storage
            .add_new_mint_pass_at_height(&other_leader, other_leader.mint_block_height)
            .unwrap();

        let collab_fixed = make_collab_pass(80, 180, 101, leader.inscription_id);
        let collab_addr =
            make_collab_pass_with_leader_addr(81, 181, 102, leader_btc_addr, leader_owner);
        let collab_other = make_collab_pass(82, 182, 103, other_leader.inscription_id);
        let dormant_collab = make_collab_pass(83, 183, 104, leader.inscription_id);
        for pass in [&collab_fixed, &collab_addr, &collab_other, &dormant_collab] {
            storage
                .add_new_mint_pass_at_height(pass, pass.mint_block_height)
                .unwrap();
        }
        storage
            .update_state_at_height(
                &dormant_collab.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();

        seed_energy_record(&server, &leader, 120, 1_000);
        seed_energy_record(&server, &collab_fixed, 120, 200);
        seed_energy_record(&server, &collab_addr, 110, 101);
        seed_energy_record(&server, &collab_other, 120, 900);
        seed_energy_record_with_state(&server, &dormant_collab, 120, MinerPassState::Dormant, 700);

        let collab_addr_projected_raw = 101u128.saturating_add(calc_growth_delta(100_000, 10));
        let fixed_contribution = calc_collab_contribution(200);
        let addr_contribution = calc_collab_contribution(collab_addr_projected_raw);
        let expected_aggregate = fixed_contribution.saturating_add(addr_contribution);

        let page0 = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                sort: Some("contribution_desc_pass_id_asc".to_string()),
                cursor: None,
                limit: 1,
            })
            .unwrap();
        assert_eq!(page0.view_version, USDB_ECONOMIC_STATE_VIEW_VERSION);
        assert_eq!(page0.external_state.btc_height, 120);
        assert_eq!(page0.external_state.stable_block_hash, "aa".repeat(32));
        assert_eq!(
            page0.external_state.usdb_index_protocol_version,
            USDB_INDEX_PROTOCOL_VERSION
        );
        assert_eq!(
            page0.external_state.usdb_index_formula_version,
            USDB_INDEX_FORMULA_VERSION
        );
        assert_eq!(page0.limit, 1);
        assert_eq!(page0.max_limit, ECONOMIC_PAGE_MAX_LIMIT);
        assert!(page0.next_cursor.is_some());
        assert!(
            serde_json::to_value(&page0)
                .unwrap()
                .get("resolved_height")
                .is_none()
        );
        assert_eq!(page0.leader_pass_id, leader.inscription_id.to_string());
        assert_eq!(page0.leader_state, MinerPassState::Active.as_str());
        assert_eq!(page0.leader_pass_kind, MinerPassKind::Standard.as_str());
        assert_eq!(page0.sort, "contribution_desc_pass_id_asc");
        assert_eq!(page0.total, 2);
        assert_eq!(
            page0.aggregate_collab_contribution,
            expected_aggregate.to_string()
        );
        assert_eq!(page0.items.len(), 1);
        assert_eq!(
            page0.items[0].collab_pass_id,
            collab_fixed.inscription_id.to_string()
        );
        assert_eq!(page0.items[0].collab_raw_energy, "200");
        assert_eq!(
            page0.items[0].collab_contribution,
            fixed_contribution.to_string()
        );
        assert_eq!(page0.items[0].leader_ref_kind, "leader_pass_id");
        assert_eq!(
            page0.items[0].leader_ref_value,
            leader.inscription_id.to_string()
        );

        let changed_sort = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: None,
                context: None,
                sort: Some("collab_pass_id_asc".to_string()),
                cursor: page0.next_cursor.clone(),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            changed_sort.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let changed_limit = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: None,
                context: None,
                sort: Some("contribution_desc_pass_id_asc".to_string()),
                cursor: page0.next_cursor.clone(),
                limit: 2,
            })
            .unwrap_err();
        assert_eq!(
            changed_limit.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let changed_leader = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: other_leader.inscription_id.to_string(),
                block_height: None,
                context: None,
                sort: Some("contribution_desc_pass_id_asc".to_string()),
                cursor: page0.next_cursor.clone(),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            changed_leader.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        // The cursor remains pinned to height 120 after the current head moves.
        seed_state_ref_context(&server, 130);
        let page1 = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: None,
                context: None,
                sort: Some("contribution_desc_pass_id_asc".to_string()),
                cursor: page0.next_cursor.clone(),
                limit: 1,
            })
            .unwrap();
        assert_eq!(page1.total, 2);
        assert_eq!(page1.external_state, page0.external_state);
        assert_eq!(page1.items.len(), 1);
        assert!(page1.next_cursor.is_none());
        assert_eq!(
            page1.items[0].collab_pass_id,
            collab_addr.inscription_id.to_string()
        );
        assert_eq!(
            page1.items[0].collab_raw_energy,
            collab_addr_projected_raw.to_string()
        );
        assert_eq!(
            page1.items[0].collab_contribution,
            addr_contribution.to_string()
        );
        assert_eq!(page1.items[0].leader_ref_kind, "leader_btc_addr");
        assert_eq!(page1.items[0].leader_ref_value, leader_btc_addr);

        let full_page = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                sort: Some("collab_pass_id_asc".to_string()),
                cursor: None,
                limit: 10,
            })
            .unwrap();
        let recomputed_aggregate = full_page
            .items
            .iter()
            .map(|item| item.collab_contribution.parse::<Energy>().unwrap())
            .fold(0u128, Energy::saturating_add);
        assert_eq!(full_page.total, 2);
        assert_eq!(
            full_page.aggregate_collab_contribution,
            recomputed_aggregate.to_string()
        );

        let energy = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();
        assert_eq!(
            energy.collab_contribution,
            page0.aggregate_collab_contribution
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_collab_breakdown_collab_pass_id_sort_and_non_active_leader_empty() {
        let (server, root_dir) = build_server("collab_breakdown_non_active", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(84, 184, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &leader.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();
        let collab = make_collab_pass(85, 185, 101, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab, collab.mint_block_height)
            .unwrap();
        seed_energy_record_with_state(&server, &leader, 120, MinerPassState::Dormant, 1_000);
        seed_energy_record(&server, &collab, 120, 200);

        let page = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                sort: None,
                cursor: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.sort, "collab_pass_id_asc");
        assert_eq!(page.leader_state, MinerPassState::Dormant.as_str());
        assert_eq!(page.total, 0);
        assert_eq!(page.aggregate_collab_contribution, "0");
        assert!(page.items.is_empty());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_collab_breakdown_rejects_mismatched_context_height() {
        let (server, root_dir) = build_server("collab_breakdown_context_height_mismatch", 130);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(86, 186, 100);
        storage
            .add_new_mint_pass_at_height(&leader, leader.mint_block_height)
            .unwrap();
        seed_state_ref_context(&server, 120);

        let err = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(121),
                    expected_state: ConsensusStateReference::default(),
                }),
                sort: None,
                cursor: None,
                limit: 10,
            })
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InvalidParams);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_candidate_set_view_filters_collab_and_sorts_by_effective_energy() {
        let (server, root_dir) = build_server("candidate_set_effective_sort", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        let leader = make_active_pass(87, 187, 100);
        let tie_low_id = make_active_pass(88, 188, 100);
        let tie_high_id = make_active_pass(89, 189, 100);
        let dormant_standard = make_active_pass(90, 190, 100);
        for pass in [&leader, &tie_low_id, &tie_high_id, &dormant_standard] {
            storage
                .add_new_mint_pass_at_height(pass, pass.mint_block_height)
                .unwrap();
        }
        storage
            .update_state_at_height(
                &dormant_standard.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();

        let collab_high_raw = make_collab_pass(91, 191, 101, leader.inscription_id);
        storage
            .add_new_mint_pass_at_height(&collab_high_raw, collab_high_raw.mint_block_height)
            .unwrap();

        seed_energy_record(&server, &leader, 120, 100);
        seed_energy_record(&server, &tie_low_id, 120, 500);
        seed_energy_record(&server, &tie_high_id, 120, 500);
        seed_energy_record_with_state(
            &server,
            &dormant_standard,
            120,
            MinerPassState::Dormant,
            10_000,
        );
        seed_energy_record(&server, &collab_high_raw, 120, 2_000_000);

        let contribution = calc_collab_contribution(2_000_000);
        let expected_leader_effective = 100u128.saturating_add(contribution);
        let leader_energy = get_pass_energy_exact_for_test(&server, &leader, 120);
        assert_eq!(leader_energy.raw_energy, "100");
        assert_eq!(leader_energy.collab_contribution, contribution.to_string());
        assert_eq!(
            leader_energy.effective_energy,
            expected_leader_effective.to_string()
        );
        assert_eq!(leader_energy.level, 1);
        assert_eq!(leader_energy.difficulty_factor_bps, 9_900);

        let collab_energy = get_pass_energy_exact_for_test(&server, &collab_high_raw, 120);
        assert_eq!(collab_energy.raw_energy, "2000000");
        assert_eq!(collab_energy.collab_contribution, "0");
        assert_eq!(collab_energy.effective_energy, "0");
        assert_eq!(collab_energy.level, 0);
        assert_eq!(collab_energy.difficulty_factor_bps, 10_000);

        let raw_leaderboard = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: Some(120),
                scope: Some("active".to_string()),
                page: 0,
                page_size: 1,
            })
            .unwrap();
        assert_eq!(raw_leaderboard.total, 4);
        assert_eq!(
            raw_leaderboard.items[0].inscription_id,
            collab_high_raw.inscription_id.to_string(),
            "legacy pass energy leaderboard keeps raw active collab passes visible"
        );

        let page0 = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(120),
                context: None,
                selection_rule: None,
                cursor: None,
                limit: 2,
            })
            .unwrap();

        assert_eq!(
            page0.view_version,
            USDB_ECONOMIC_STATE_VIEW_VERSION.to_string()
        );
        assert_eq!(page0.selection_rule, CANDIDATE_SET_SELECTION_RULE);
        assert_eq!(page0.external_state.btc_height, 120);
        assert_eq!(page0.external_state.stable_block_hash, "aa".repeat(32));
        assert_eq!(
            page0.external_state.balance_history_api_version,
            balance_history::BALANCE_HISTORY_API_VERSION
        );
        assert_eq!(
            page0.external_state.balance_history_semantics_version,
            balance_history::BALANCE_HISTORY_SEMANTICS_VERSION
        );
        assert_eq!(
            page0.external_state.usdb_index_protocol_version,
            USDB_INDEX_PROTOCOL_VERSION
        );
        assert_eq!(
            page0.external_state.usdb_index_formula_version,
            USDB_INDEX_FORMULA_VERSION
        );
        assert_eq!(page0.total, 3);
        assert_eq!(page0.limit, 2);
        assert_eq!(page0.max_limit, ECONOMIC_PAGE_MAX_LIMIT);
        assert!(page0.next_cursor.is_some());
        assert_eq!(page0.items.len(), 2);
        assert_eq!(page0.items[0].pass_id, leader.inscription_id.to_string());
        assert_eq!(page0.items[0].owner_script_hash, leader.owner.to_string());
        assert_eq!(page0.items[0].state, MinerPassState::Active.as_str());
        assert_eq!(page0.items[0].pass_kind, MinerPassKind::Standard.as_str());
        assert_eq!(page0.items[0].raw_energy, leader_energy.raw_energy);
        assert_eq!(
            page0.items[0].collab_contribution,
            leader_energy.collab_contribution
        );
        assert_eq!(
            page0.items[0].effective_energy,
            leader_energy.effective_energy
        );
        assert_eq!(page0.items[0].level, leader_energy.level);
        assert_eq!(
            page0.items[0].difficulty_factor_bps,
            leader_energy.difficulty_factor_bps
        );
        let first_item_json = serde_json::to_value(&page0.items[0]).unwrap();
        assert_eq!(first_item_json["level"], 1);
        assert_eq!(first_item_json["difficulty_factor_bps"], 9_900);
        assert_eq!(
            page0.items[1].pass_id,
            tie_low_id.inscription_id.to_string(),
            "same effective energy must use pass id ascending as tie-breaker"
        );
        assert_eq!(page0.items[1].effective_energy, "500");
        assert_eq!(page0.items[1].level, 0);
        assert_eq!(page0.items[1].difficulty_factor_bps, 10_000);

        // The continuation cursor owns the historical context, so advancing
        // the current head must not move page 1 to a different state.
        seed_state_ref_context(&server, 130);
        let page1 = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: Some(CANDIDATE_SET_SELECTION_RULE.to_string()),
                cursor: page0.next_cursor.clone(),
                limit: 2,
            })
            .unwrap();
        assert_eq!(page1.total, 3);
        assert_eq!(page1.external_state, page0.external_state);
        assert_eq!(page1.items.len(), 1);
        assert!(page1.next_cursor.is_none());
        assert_eq!(
            page1.items[0].pass_id,
            tie_high_id.inscription_id.to_string()
        );

        let candidate_ids = page0
            .items
            .iter()
            .chain(page1.items.iter())
            .map(|item| item.pass_id.clone())
            .collect::<Vec<_>>();
        assert!(!candidate_ids.contains(&collab_high_raw.inscription_id.to_string()));
        assert!(!candidate_ids.contains(&dormant_standard.inscription_id.to_string()));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_candidate_cursor_rejects_tamper_changed_bindings_and_same_height_reorg() {
        let (server, root_dir) = build_server("candidate_cursor_binding_guards", 120);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();
        let candidate1 = make_active_pass(161, 241, 100);
        let candidate2 = make_active_pass(162, 242, 100);
        for candidate in [&candidate1, &candidate2] {
            storage
                .add_new_mint_pass_at_height(candidate, candidate.mint_block_height)
                .unwrap();
        }
        seed_energy_record(&server, &candidate1, 120, 200);
        seed_energy_record(&server, &candidate2, 120, 100);

        let first_page = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(120),
                context: None,
                selection_rule: None,
                cursor: None,
                limit: 1,
            })
            .unwrap();
        let cursor = first_page.next_cursor.clone().unwrap();
        let page_json = serde_json::to_value(&first_page).unwrap();
        assert!(page_json.get("resolved_height").is_none());
        assert_eq!(page_json["limit"], 1);
        assert_eq!(page_json["max_limit"], ECONOMIC_PAGE_MAX_LIMIT);

        for invalid_limit in [0, ECONOMIC_PAGE_MAX_LIMIT + 1] {
            let err = server
                .get_candidate_set_view(GetCandidateSetViewParams {
                    view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                    block_height: Some(120),
                    context: None,
                    selection_rule: None,
                    cursor: None,
                    limit: invalid_limit,
                })
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::ServerError(ERR_INVALID_PAGINATION));
        }

        let changed_limit = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: None,
                cursor: Some(cursor.clone()),
                limit: 2,
            })
            .unwrap_err();
        assert_eq!(
            changed_limit.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let changed_height = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(121),
                context: None,
                selection_rule: None,
                cursor: Some(cursor.clone()),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            changed_height.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let mut tampered_bytes = cursor.as_bytes().to_vec();
        let tampered_index = tampered_bytes.len() / 2;
        tampered_bytes[tampered_index] = if tampered_bytes[tampered_index] == b'A' {
            b'B'
        } else {
            b'A'
        };
        let tampered = String::from_utf8(tampered_bytes).unwrap();
        let tampered_err = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: None,
                cursor: Some(tampered),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            tampered_err.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let wrong_resource_cursor = UsdbIndexerRpcServer::encode_cursor(
            EconomicPageCursor::CollabBreakdown(CollabBreakdownCursor {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                external_state: first_page.external_state.clone(),
                leader_pass_id: candidate1.inscription_id.to_string(),
                sort: "collab_pass_id_asc".to_string(),
                limit: 1,
                last_collab_contribution: "0".to_string(),
                last_collab_pass_id: candidate2.inscription_id.to_string(),
            }),
        )
        .unwrap();
        let wrong_resource = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: None,
                cursor: Some(wrong_resource_cursor),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            wrong_resource.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let missing_key_cursor = UsdbIndexerRpcServer::encode_cursor(
            EconomicPageCursor::CandidateSet(CandidateSetCursor {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                external_state: first_page.external_state.clone(),
                selection_rule: CANDIDATE_SET_SELECTION_RULE.to_string(),
                limit: 1,
                last_effective_energy: "999".to_string(),
                last_pass_id: candidate1.inscription_id.to_string(),
            }),
        )
        .unwrap();
        let missing_key = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: None,
                cursor: Some(missing_key_cursor),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            missing_key.code,
            ErrorCode::ServerError(ERR_INVALID_PAGINATION)
        );

        let mut replacement = ready_balance_history_snapshot(120);
        replacement.stable_block_hash = Some("cc".repeat(32));
        replacement.latest_block_commit = Some("dd".repeat(32));
        storage
            .upsert_balance_history_snapshot_anchor(&replacement)
            .unwrap();
        let reorg_mismatch = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: None,
                context: None,
                selection_rule: None,
                cursor: Some(cursor),
                limit: 1,
            })
            .unwrap_err();
        assert_eq!(
            reorg_mismatch.code,
            ErrorCode::ServerError(ConsensusRpcErrorCode::SnapshotIdMismatch.code())
        );
        assert_eq!(
            decode_consensus_error_data(&reorg_mismatch)
                .mismatch_field
                .as_deref(),
            Some("snapshot_id")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_candidate_set_view_rejects_mismatched_context_height() {
        let (server, root_dir) = build_server("candidate_set_context_height_mismatch", 130);
        seed_state_ref_context(&server, 120);

        let err = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(121),
                    expected_state: ConsensusStateReference::default(),
                }),
                selection_rule: None,
                cursor: None,
                limit: 10,
            })
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InvalidParams);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_candidate_set_view_rejects_unsupported_view_version() {
        let (server, root_dir) = build_server("candidate_set_view_version_mismatch", 130);

        let err = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: "uip-0006-usdb-economic-state-view:v999".to_string(),
                block_height: Some(120),
                context: None,
                selection_rule: None,
                cursor: None,
                limit: 10,
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::ViewVersionMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(
            err.message,
            ConsensusRpcErrorCode::ViewVersionMismatch.as_str()
        );
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.mismatch_field.as_deref(), Some("view_version"));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_uip0006_query_params_require_view_version() {
        let profile_err = serde_json::from_value::<GetPassEconomicProfileParams>(
            serde_json::json!({"pass_id": "txidi0"}),
        )
        .unwrap_err();
        assert!(profile_err.to_string().contains("view_version"));

        let candidate_err =
            serde_json::from_value::<GetCandidateSetViewParams>(serde_json::json!({"limit": 10}))
                .unwrap_err();
        assert!(candidate_err.to_string().contains("view_version"));

        let breakdown_err = serde_json::from_value::<GetCollabBreakdownParams>(serde_json::json!({
            "leader_pass_id": "txidi0",
            "limit": 10
        }))
        .unwrap_err();
        assert!(breakdown_err.to_string().contains("view_version"));
    }

    #[test]
    fn test_uip0006_cursor_queries_reject_legacy_page_fields() {
        let candidate_err =
            serde_json::from_value::<GetCandidateSetViewParams>(serde_json::json!({
                "view_version": USDB_ECONOMIC_STATE_VIEW_VERSION,
                "block_height": 120,
                "page": 0,
                "page_size": 10,
                "limit": 10
            }))
            .unwrap_err();
        assert!(candidate_err.to_string().contains("unknown field"));

        let breakdown_err = serde_json::from_value::<GetCollabBreakdownParams>(serde_json::json!({
            "view_version": USDB_ECONOMIC_STATE_VIEW_VERSION,
            "leader_pass_id": "txidi0",
            "block_height": 120,
            "page": 0,
            "page_size": 10,
            "limit": 10
        }))
        .unwrap_err();
        assert!(breakdown_err.to_string().contains("unknown field"));
    }

    #[test]
    fn test_get_candidate_set_view_invalid_selection_rule_and_missing_energy_fail_closed() {
        let (server, root_dir) = build_server("candidate_set_invalid_rule_missing_energy", 130);
        seed_state_ref_context(&server, 120);
        let storage = server.indexer.miner_pass_storage();

        let candidate = make_active_pass(92, 192, 100);
        storage
            .add_new_mint_pass_at_height(&candidate, candidate.mint_block_height)
            .unwrap();

        let invalid_rule = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(120),
                context: None,
                selection_rule: Some("bad-rule".to_string()),
                cursor: None,
                limit: 10,
            })
            .unwrap_err();
        assert_eq!(invalid_rule.code, ErrorCode::InvalidParams);
        assert!(
            invalid_rule
                .message
                .contains("Invalid candidate set selection_rule")
        );

        let missing_energy = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: Some(120),
                context: None,
                selection_rule: None,
                cursor: None,
                limit: 10,
            })
            .unwrap_err();
        assert_eq!(
            missing_energy.code,
            ErrorCode::ServerError(ERR_INTERNAL_INVARIANT_BROKEN)
        );
        assert_eq!(missing_energy.message, "INTERNAL_INVARIANT_BROKEN");

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_returns_decimal_string_for_u128_and_saturated_projection() {
        let (server, root_dir) = build_server("energy_projection_u128_decimal", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(35, 135, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, Energy::MAX - 1);

        let exact = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap();
        assert_eq!(exact.raw_energy, (Energy::MAX - 1).to_string());
        assert_eq!(exact.collab_contribution, "0");
        assert_eq!(exact.effective_energy, (Energy::MAX - 1).to_string());
        assert_eq!(exact.level, 50);
        assert_eq!(exact.difficulty_factor_bps, 5_000);

        let projected = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(121),
                context: None,
                mode: Some("at_or_before".to_string()),
            })
            .unwrap();
        assert_eq!(projected.raw_energy, Energy::MAX.to_string());
        assert_eq!(projected.collab_contribution, "0");
        assert_eq!(projected.effective_energy, Energy::MAX.to_string());
        assert_eq!(projected.level, 50);
        assert_eq!(projected.difficulty_factor_bps, 5_000);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_rejects_mismatched_context_height() {
        let (server, root_dir) = build_server("energy_context_height_mismatch", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(23, 123, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 500);
        seed_state_ref_context(&server, 120);

        let err = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(121),
                    expected_state: ConsensusStateReference::default(),
                }),
                mode: Some("exact".to_string()),
            })
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InvalidParams);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_returns_system_state_mismatch_with_context() {
        let (server, root_dir) = build_server("energy_context_system_mismatch", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(24, 124, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 500);
        seed_state_ref_context(&server, 120);

        let err = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference {
                        system_state_id: Some("dd".repeat(32)),
                        ..Default::default()
                    },
                }),
                mode: Some("exact".to_string()),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SystemStateIdMismatch.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.expected_state.system_state_id, Some("dd".repeat(32)));
        assert!(data.actual_state.system_state_id.is_some());

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_returns_snapshot_not_ready_when_context_consensus_not_ready() {
        let (server, root_dir) = build_server("energy_context_not_ready", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(28, 128, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 500);
        seed_state_ref_context(&server, 120);
        server.status.set_rpc_alive(true);

        let mut upstream_readiness = ready_balance_history_readiness(120);
        upstream_readiness.consensus_ready = false;
        upstream_readiness.blockers = vec![balance_history::ReadinessBlocker::CatchingUp];
        server
            .status
            .set_balance_history_readiness(Some(upstream_readiness));

        let err = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: Some(ConsensusQueryContext {
                    requested_height: Some(120),
                    expected_state: ConsensusStateReference::default(),
                }),
                mode: Some("exact".to_string()),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert_eq!(data.consensus_ready, Some(false));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_returns_state_not_retained_below_energy_history_floor() {
        let (server, root_dir) = build_server_with_genesis("energy_state_not_retained", 130, 121);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(27, 127, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_state_ref_context(&server, 130);
        seed_energy_record(&server, &pass, 120, 500);

        let err = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(120),
                context: None,
                mode: Some("exact".to_string()),
            })
            .unwrap_err();

        match err.code {
            ErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::StateNotRetained.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        let data = decode_consensus_error_data(&err);
        assert_eq!(data.requested_height, Some(120));
        assert!(
            data.detail
                .as_deref()
                .unwrap_or_default()
                .contains("historical state retention floor 121")
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_range_supports_desc_order() {
        let (server, root_dir) = build_server("energy_range_desc_order", 150);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(12, 120, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 110, 1100);
        seed_energy_record(&server, &pass, 120, 1200);
        seed_energy_record(&server, &pass, 130, 1300);

        let desc_page0 = server
            .get_pass_energy_range(GetPassEnergyRangeParams {
                inscription_id: pass.inscription_id.to_string(),
                from_height: 100,
                to_height: 130,
                order: Some("desc".to_string()),
                page: 0,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(desc_page0.total, 3);
        assert_eq!(desc_page0.items.len(), 2);
        assert_eq!(desc_page0.items[0].record_block_height, 130);
        assert_eq!(desc_page0.items[1].record_block_height, 120);

        let desc_page1 = server
            .get_pass_energy_range(GetPassEnergyRangeParams {
                inscription_id: pass.inscription_id.to_string(),
                from_height: 100,
                to_height: 130,
                order: Some("desc".to_string()),
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(desc_page1.total, 3);
        assert_eq!(desc_page1.items.len(), 1);
        assert_eq!(desc_page1.items[0].record_block_height, 110);

        let asc_page0 = server
            .get_pass_energy_range(GetPassEnergyRangeParams {
                inscription_id: pass.inscription_id.to_string(),
                from_height: 100,
                to_height: 130,
                order: Some("asc".to_string()),
                page: 0,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(asc_page0.total, 3);
        assert_eq!(asc_page0.items.len(), 2);
        assert_eq!(asc_page0.items[0].record_block_height, 110);
        assert_eq!(asc_page0.items[1].record_block_height, 120);

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_get_pass_energy_range_encodes_u128_decimal_strings() {
        let (server, root_dir) = build_server("energy_range_u128_decimal", 150);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(36, 136, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        let above_u64_max = (u64::MAX as Energy) + 1;
        seed_energy_record(&server, &pass, 110, 9);
        seed_energy_record(&server, &pass, 120, above_u64_max);
        seed_energy_record(&server, &pass, 130, Energy::MAX);

        let desc = server
            .get_pass_energy_range(GetPassEnergyRangeParams {
                inscription_id: pass.inscription_id.to_string(),
                from_height: 100,
                to_height: 130,
                order: Some("desc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        let energies = desc
            .items
            .iter()
            .map(|item| item.energy.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            energies,
            vec![
                Energy::MAX.to_string(),
                above_u64_max.to_string(),
                "9".to_string()
            ]
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_explicit_height_bypass_cache() {
        let (server, root_dir) = build_server("leaderboard_no_cache", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(9, 90, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 888);

        let page = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: Some(120),
                scope: None,
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(page.resolved_height, 120);
        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].energy, "888");

        {
            let cache = server.pass_energy_leaderboard_cache.lock().unwrap();
            assert!(
                cache.latest.is_none(),
                "Explicit-height leaderboard query should bypass latest-height cache"
            );
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_top_k_overflow_returns_empty_without_rebuild() {
        let (server, root_dir) = build_server("leaderboard_top_k_overflow", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(10, 100, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 999);

        // default top_k is 1000, so this query is guaranteed to overflow.
        let page = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 100,
                page_size: 20,
            })
            .unwrap();
        assert_eq!(page.resolved_height, 120);
        assert_eq!(page.total, 1000);
        assert!(page.items.is_empty());

        // Overflow path should return directly and not build/refresh cache.
        {
            let cache = server.pass_energy_leaderboard_cache.lock().unwrap();
            assert!(cache.latest.is_none());
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_cached_pagination_consistent_across_pages_and_height() {
        let (server, root_dir) = build_server("leaderboard_cached_pagination", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass1 = make_active_pass(31, 41, 100);
        let pass2 = make_active_pass(32, 42, 100);
        let pass3 = make_active_pass(33, 43, 100);
        let pass4 = make_active_pass(34, 44, 100);
        for pass in [&pass1, &pass2, &pass3, &pass4] {
            storage.add_new_mint_pass_at_height(pass, 100).unwrap();
        }
        seed_energy_record(&server, &pass1, 120, 400);
        seed_energy_record(&server, &pass2, 120, 300);
        seed_energy_record(&server, &pass3, 120, 200);
        seed_energy_record(&server, &pass4, 120, 100);

        // First query builds cache at latest synced height.
        let page0_h120 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 0,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page0_h120.resolved_height, 120);
        assert_eq!(page0_h120.total, 4);
        assert_eq!(page0_h120.items.len(), 2);
        assert_eq!(page0_h120.items[0].energy, "400");
        assert_eq!(page0_h120.items[1].energy, "300");

        // Second page should be served from the same cache entry.
        let page1_h120 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page1_h120.resolved_height, 120);
        assert_eq!(page1_h120.total, 4);
        assert_eq!(page1_h120.items.len(), 2);
        assert_eq!(page1_h120.items[0].energy, "200");
        assert_eq!(page1_h120.items[1].energy, "100");

        // Explicit height bypasses cache and should still return identical pagination.
        let explicit_page1_h120 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: Some(120),
                scope: None,
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(
            explicit_page1_h120.resolved_height,
            page1_h120.resolved_height
        );
        assert_eq!(explicit_page1_h120.total, page1_h120.total);
        assert_eq!(explicit_page1_h120.items.len(), page1_h120.items.len());
        for (lhs, rhs) in explicit_page1_h120
            .items
            .iter()
            .zip(page1_h120.items.iter())
        {
            assert_eq!(lhs.inscription_id, rhs.inscription_id);
            assert_eq!(lhs.owner, rhs.owner);
            assert_eq!(lhs.record_block_height, rhs.record_block_height);
            assert_eq!(lhs.state, rhs.state);
            assert_eq!(lhs.energy, rhs.energy);
        }

        // Move synced height forward: cache must refresh and both pages remain internally consistent.
        storage.update_synced_btc_block_height(121).unwrap();
        let growth = calc_growth_delta(100_000, 1);

        let page0_h121 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 0,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page0_h121.resolved_height, 121);
        assert_eq!(page0_h121.total, 4);
        assert_eq!(page0_h121.items.len(), 2);
        assert_eq!(
            page0_h121.items[0].energy,
            400u128.saturating_add(growth).to_string()
        );
        assert_eq!(
            page0_h121.items[1].energy,
            300u128.saturating_add(growth).to_string()
        );

        let page1_h121 = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: None,
                page: 1,
                page_size: 2,
            })
            .unwrap();
        assert_eq!(page1_h121.resolved_height, 121);
        assert_eq!(page1_h121.total, 4);
        assert_eq!(page1_h121.items.len(), 2);
        assert_eq!(
            page1_h121.items[0].energy,
            200u128.saturating_add(growth).to_string()
        );
        assert_eq!(
            page1_h121.items[1].energy,
            100u128.saturating_add(growth).to_string()
        );

        {
            let cache = server.pass_energy_leaderboard_cache.lock().unwrap();
            let entry = cache.latest.as_ref().expect("cache should exist");
            assert_eq!(entry.resolved_height, 121);
            assert_eq!(entry.total, 4);
            assert_eq!(entry.items.len(), 4);
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_sorts_by_u128_not_decimal_string() {
        let (server, root_dir) = build_server("leaderboard_numeric_u128_sort", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass_9 = make_active_pass(41, 141, 100);
        let pass_100 = make_active_pass(42, 142, 100);
        let pass_10 = make_active_pass(43, 143, 100);
        let pass_above_u64 = make_active_pass(44, 144, 100);
        let pass_max = make_active_pass(45, 145, 100);
        for pass in [&pass_9, &pass_100, &pass_10, &pass_above_u64, &pass_max] {
            storage.add_new_mint_pass_at_height(pass, 100).unwrap();
        }

        let above_u64_max = (u64::MAX as Energy) + 1;
        seed_energy_record(&server, &pass_9, 120, 9);
        seed_energy_record(&server, &pass_100, 120, 100);
        seed_energy_record(&server, &pass_10, 120, 10);
        seed_energy_record(&server, &pass_above_u64, 120, above_u64_max);
        seed_energy_record(&server, &pass_max, 120, Energy::MAX);

        let page = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: Some(120),
                scope: None,
                page: 0,
                page_size: 10,
            })
            .unwrap();
        let energies = page
            .items
            .iter()
            .map(|item| item.energy.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            energies,
            vec![
                Energy::MAX.to_string(),
                above_u64_max.to_string(),
                "100".to_string(),
                "10".to_string(),
                "9".to_string(),
            ]
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_tiebreakers_are_deterministic() {
        let (server, root_dir) = build_server("leaderboard_numeric_tiebreakers", 120);
        let storage = server.indexer.miner_pass_storage();

        let pass_older = make_active_pass(46, 146, 100);
        let pass_newer_low_id = make_active_pass(47, 147, 100);
        let pass_newer_high_id = make_active_pass(48, 148, 100);
        for pass in [&pass_older, &pass_newer_low_id, &pass_newer_high_id] {
            storage.add_new_mint_pass_at_height(pass, 100).unwrap();
        }

        seed_energy_record_with_state_and_balance(
            &server,
            &pass_older,
            119,
            MinerPassState::Active,
            500,
            0,
        );
        seed_energy_record_with_state_and_balance(
            &server,
            &pass_newer_low_id,
            120,
            MinerPassState::Active,
            500,
            0,
        );
        seed_energy_record_with_state_and_balance(
            &server,
            &pass_newer_high_id,
            120,
            MinerPassState::Active,
            500,
            0,
        );

        let page = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: Some(120),
                scope: None,
                page: 0,
                page_size: 10,
            })
            .unwrap();
        let ids = page
            .items
            .iter()
            .map(|item| item.inscription_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                pass_newer_low_id.inscription_id.to_string(),
                pass_newer_high_id.inscription_id.to_string(),
                pass_older.inscription_id.to_string(),
            ]
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_scope_filters_states() {
        let (server, root_dir) = build_server("leaderboard_scope_filters", 130);
        let storage = server.indexer.miner_pass_storage();

        let active = make_active_pass(51, 61, 100);
        storage.add_new_mint_pass_at_height(&active, 100).unwrap();
        seed_energy_record_with_state(&server, &active, 130, MinerPassState::Active, 900);

        let dormant = make_active_pass(52, 62, 100);
        storage.add_new_mint_pass_at_height(&dormant, 100).unwrap();
        storage
            .update_state_at_height(
                &dormant.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                110,
            )
            .unwrap();
        seed_energy_record_with_state(&server, &dormant, 110, MinerPassState::Dormant, 800);

        let invalid = make_invalid_pass(53, 63, 100, "INVALID_USDB_MAIN");
        storage
            .add_invalid_mint_pass_at_height(&invalid, 100)
            .unwrap();
        seed_energy_record_with_state(&server, &invalid, 100, MinerPassState::Invalid, 700);

        let active_only = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: Some("active".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(active_only.total, 1);
        assert_eq!(active_only.items.len(), 1);
        assert_eq!(
            active_only.items[0].inscription_id,
            active.inscription_id.to_string()
        );

        // Keep `at_height=None` to verify cache key includes scope.
        let active_dormant = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: Some("active_dormant".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(active_dormant.total, 2);
        assert_eq!(active_dormant.items.len(), 2);
        assert_eq!(
            active_dormant.items[0].inscription_id,
            active.inscription_id.to_string()
        );
        assert_eq!(
            active_dormant.items[1].inscription_id,
            dormant.inscription_id.to_string()
        );

        let all_states = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: Some("all".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap();
        assert_eq!(all_states.total, 3);
        assert_eq!(all_states.items.len(), 3);
        assert_eq!(
            all_states.items[2].inscription_id,
            invalid.inscription_id.to_string()
        );

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pass_energy_leaderboard_invalid_scope_returns_invalid_params() {
        let (server, root_dir) = build_server("leaderboard_invalid_scope", 120);
        let err = server
            .get_pass_energy_leaderboard(GetPassEnergyLeaderboardParams {
                at_height: None,
                scope: Some("bad_scope".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(err.message.contains("Invalid leaderboard scope"));

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_pagination_and_height_range_errors() {
        let (server, root_dir) = build_server("params_error", 300);

        let pagination_err = server
            .get_active_passes_at_height(GetActivePassesAtHeightParams {
                at_height: Some(200),
                page: 0,
                page_size: 0,
            })
            .unwrap_err();
        match pagination_err.code {
            ErrorCode::ServerError(code) => assert_eq!(code, ERR_INVALID_PAGINATION),
            _ => panic!("unexpected error code: {:?}", pagination_err.code),
        }
        assert_eq!(pagination_err.message, "INVALID_PAGINATION");

        let range_err = server
            .get_pass_history(GetPassHistoryParams {
                inscription_id: test_inscription_id(9, 0).to_string(),
                from_height: 201,
                to_height: 200,
                order: Some("asc".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap_err();
        match range_err.code {
            ErrorCode::ServerError(code) => assert_eq!(code, ERR_INVALID_HEIGHT_RANGE),
            _ => panic!("unexpected error code: {:?}", range_err.code),
        }
        assert_eq!(range_err.message, "INVALID_HEIGHT_RANGE");

        let energy_order_err = server
            .get_pass_energy_range(GetPassEnergyRangeParams {
                inscription_id: test_inscription_id(9, 0).to_string(),
                from_height: 100,
                to_height: 120,
                order: Some("bad".to_string()),
                page: 0,
                page_size: 10,
            })
            .unwrap_err();
        match energy_order_err.code {
            ErrorCode::InvalidParams => {}
            _ => panic!("unexpected error code: {:?}", energy_order_err.code),
        }

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
