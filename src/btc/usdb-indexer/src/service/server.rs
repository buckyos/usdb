use super::rpc::*;
use crate::config::ConfigManagerRef;
use crate::index::{InscriptionIndexer, MinerPassState};
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
use usdb_util::{
    CONSENSUS_SOURCE_CHAIN_BTC, ConsensusQueryContext, ConsensusRpcErrorCode,
    ConsensusRpcErrorData, ConsensusStateReference, LocalStateActiveBalanceSnapshot,
    LocalStatePassCommitIdentity, USDB_INDEXER_SERVICE_NAME, build_consensus_snapshot_id,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassEnergyLeaderboardScope {
    Active,
    ActiveDormant,
    All,
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
        if let Some(handle) = self.server_handle.lock().unwrap().take() {
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

        // For current-state error payloads we expose the protocol version of
        // the currently running usdb-indexer binary, not whichever nested
        // sub-structure happened to be present. Historical state-ref RPCs keep
        // using the version recorded in their historical identities.
        if snapshot.is_some() || local_state.is_some() || system_state.is_some() {
            reference.usdb_index_protocol_version = Some(USDB_INDEX_PROTOCOL_VERSION.to_string());
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
    fn current_state_for_error_payload(
        &self,
    ) -> Result<
        (
            Option<IndexerSnapshotInfo>,
            Option<LocalStateCommitInfo>,
            Option<SystemStateInfo>,
        ),
        JsonError,
    > {
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

        if let Some(requested_height) = context.requested_height {
            if requested_height != block_height {
                return Err(Self::to_invalid_params(format!(
                    "ConsensusQueryContext.requested_height {} does not match block_height {}",
                    requested_height, block_height
                )));
            }
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

    /// Resolve the effective query height for pass/energy RPCs while
    /// optionally enforcing a caller-supplied consensus context.
    ///
    /// Compatibility rule:
    /// - without `context`, legacy business-query behavior is preserved
    /// - with `context`, height resolution and readiness switch to the shared
    ///   consensus contract and the historical state ref at that height must
    ///   match the caller's expected selectors
    fn resolve_height_for_contextual_query(
        &self,
        requested_height: Option<u32>,
        context: Option<&ConsensusQueryContext>,
    ) -> Result<u32, JsonError> {
        if let Some(context_requested_height) = context.and_then(|value| value.requested_height) {
            if let Some(explicit_height) = requested_height {
                if explicit_height != context_requested_height {
                    return Err(Self::to_invalid_params(format!(
                        "Query height {} does not match ConsensusQueryContext.requested_height {}",
                        explicit_height, context_requested_height
                    )));
                }
            }
        }

        if context.is_some() {
            self.ensure_consensus_query_ready(
                requested_height.or(context.and_then(|value| value.requested_height)),
                "validator contextual query",
            )?;
        }

        let effective_requested_height =
            requested_height.or(context.and_then(|value| value.requested_height));
        let resolved_height = if context.is_some() {
            self.resolve_height_with_consensus_error(effective_requested_height)?
        } else {
            self.resolve_height(effective_requested_height)?
        };

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

        if let Some(expected_snapshot_id) = expected_state.snapshot_id.as_ref() {
            if expected_snapshot_id != &state_ref.snapshot_info.snapshot_id {
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
                    ),
                ));
            }
        }

        if let Some(expected_stable_height) = expected_state.stable_height {
            if expected_stable_height != state_ref.snapshot_info.balance_history_stable_height {
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
                    ),
                ));
            }
        }

        if let Some(expected_block_hash) = expected_state.stable_block_hash.as_ref() {
            if expected_block_hash != &state_ref.snapshot_info.stable_block_hash {
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
                    ),
                ));
            }
        }

        if let Some(expected_api_version) = expected_state.balance_history_api_version.as_ref() {
            if expected_api_version
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
                    ),
                ));
            }
        }

        if let Some(expected_semantics_version) =
            expected_state.balance_history_semantics_version.as_ref()
        {
            if expected_semantics_version
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
                    ),
                ));
            }
        }

        if let Some(expected_usdb_protocol_version) =
            expected_state.usdb_index_protocol_version.as_ref()
        {
            if expected_usdb_protocol_version != USDB_INDEX_PROTOCOL_VERSION {
                return Err(Self::to_consensus_error(
                    ConsensusRpcErrorCode::VersionMismatch,
                    self.build_consensus_error_data_for_state(
                        Some(block_height),
                        expected_state.clone(),
                        actual_state,
                        Some(format!(
                            "Expected usdb-index protocol version {} at height {}, got {}",
                            expected_usdb_protocol_version,
                            block_height,
                            USDB_INDEX_PROTOCOL_VERSION
                        )),
                    ),
                ));
            }
        }

        if let Some(expected_local_state_commit) = expected_state.local_state_commit.as_ref() {
            if expected_local_state_commit != &state_ref.local_state_commit_info.local_state_commit
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
                    ),
                ));
            }
        }

        if let Some(expected_system_state_id) = expected_state.system_state_id.as_ref() {
            if expected_system_state_id != &state_ref.system_state_info.system_state_id {
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
                    ),
                ));
            }
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
        {
            if snapshot.balance_history_stable_height != local_height {
                blockers.push(ReadinessBlocker::UpstreamSnapshotHeightMismatch);
            }
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
            eth_main: pass.eth_main,
            eth_collab: pass.eth_collab,
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
        let total_pages = (total_passes_usize + load_page_size - 1) / load_page_size;
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

            ranked.push(PassEnergyLeaderboardItem {
                inscription_id: row.inscription_id.to_string(),
                owner: row.owner.to_string(),
                record_block_height: record.block_height,
                state: projected.state.as_str().to_string(),
                energy: projected.energy,
            });
        }

        ranked.sort_by(|a, b| {
            b.energy
                .cmp(&a.energy)
                .then_with(|| b.record_block_height.cmp(&a.record_block_height))
                .then_with(|| a.inscription_id.cmp(&b.inscription_id))
        });

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
                    eth_main: row.pass.eth_main,
                    eth_collab: row.pass.eth_collab,
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
                    eth_main: row.pass.eth_main,
                    eth_collab: row.pass.eth_collab,
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

        let record = match mode.as_str() {
            "exact" => self
                .indexer
                .pass_energy_manager()
                .get_pass_energy_record_exact(&inscription_id, query_height)
                .map_err(Self::to_internal_error)?,
            "at_or_before" => self
                .indexer
                .pass_energy_manager()
                .get_pass_energy_record_at_or_before(&inscription_id, query_height)
                .map_err(Self::to_internal_error)?,
            _ => {
                return Err(Self::to_invalid_params(format!(
                    "Invalid energy mode {}, expected exact or at_or_before",
                    mode
                )));
            }
        };

        let Some(record) = record else {
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
        let (effective_state, effective_energy) = match mode.as_str() {
            "exact" => (record.state.clone(), record.energy),
            "at_or_before" => {
                let projected = self
                    .indexer
                    .pass_energy_manager()
                    .project_energy_record_no_balance_change(&record, query_height);
                (projected.state, projected.energy)
            }
            _ => unreachable!(),
        };

        Ok(PassEnergySnapshot {
            inscription_id: record.inscription_id.to_string(),
            query_block_height: query_height,
            record_block_height: record.block_height,
            state: effective_state.as_str().to_string(),
            active_block_height: record.active_block_height,
            owner_address: record.owner_address.to_string(),
            owner_balance: record.owner_balance,
            owner_delta: record.owner_delta,
            energy: effective_energy,
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
                    energy: record.energy,
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
            if should_use_cache {
                if let Some(cached_page) = self.try_get_cached_leaderboard_page(
                    resolved_height,
                    scope,
                    cache_top_k,
                    params.page,
                    params.page_size,
                )? {
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

        if should_use_cache {
            if let Some(cached_page) = self.try_get_cached_leaderboard_page(
                resolved_height,
                scope,
                cache_top_k,
                params.page,
                params.page_size,
            )? {
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
                    eth_main: item.eth_main,
                    eth_collab: item.eth_collab,
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
    use crate::index::energy_formula::calc_growth_delta;
    use crate::index::{InscriptionIndexer, MinerPassState, PassBlockCommitEntry};
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
            inscription_id: inscription_id.clone(),
            inscription_number: ins_tag as i32,
            mint_txid: inscription_id.txid,
            mint_block_height: mint_height,
            mint_owner: owner,
            satpoint: test_satpoint(ins_tag, 0, 0),
            eth_main: "0x1111111111111111111111111111111111111111".to_string(),
            eth_collab: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state: MinerPassState::Active,
        }
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
        energy: u64,
    ) {
        seed_energy_record_with_state(server, pass, block_height, MinerPassState::Active, energy);
    }

    fn seed_energy_record_with_state(
        server: &UsdbIndexerRpcServer,
        pass: &MinerPassInfo,
        block_height: u32,
        state: MinerPassState,
        energy: u64,
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
                owner_balance: 100_000,
                owner_delta: 0,
                energy,
            })
            .unwrap();
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

        let mut invalid = make_invalid_pass(43, 77, 102, "INVALID_ETH_MAIN");
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

        let invalid = make_invalid_pass(4, 44, 110, "INVALID_ETH_MAIN");
        storage
            .add_invalid_mint_pass_at_height(&invalid, 110)
            .unwrap();

        let page = server
            .get_invalid_passes(GetInvalidPassesParams {
                error_code: Some("INVALID_ETH_MAIN".to_string()),
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
            Some("INVALID_ETH_MAIN")
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

        let invalid = make_invalid_pass(7, 70, 110, "INVALID_ETH_MAIN");
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
        assert_eq!(page_120.items[0].energy, 777);

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
        let expected_energy_121 = 777u64.saturating_add(calc_growth_delta(100_000, 1));
        assert_eq!(page_121.resolved_height, 121);
        assert_eq!(page_121.total, 1);
        assert_eq!(page_121.items.len(), 1);
        assert_eq!(page_121.items[0].energy, expected_energy_121);

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
    fn test_get_pass_energy_at_or_before_projects_to_query_height() {
        let (server, root_dir) = build_server("energy_projection", 130);
        let storage = server.indexer.miner_pass_storage();

        let pass = make_active_pass(11, 110, 100);
        storage.add_new_mint_pass_at_height(&pass, 100).unwrap();
        seed_energy_record(&server, &pass, 120, 500);

        let projected = server
            .get_pass_energy(GetPassEnergyParams {
                inscription_id: pass.inscription_id.to_string(),
                block_height: Some(130),
                context: None,
                mode: Some("at_or_before".to_string()),
            })
            .unwrap();

        let expected = 500u64.saturating_add(calc_growth_delta(100_000, 10));
        assert_eq!(projected.query_block_height, 130);
        assert_eq!(projected.record_block_height, 120);
        assert_eq!(projected.energy, expected);

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
        assert_eq!(exact.energy, 500);

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
        assert_eq!(page.items[0].energy, 888);

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
        assert_eq!(page0_h120.items[0].energy, 400);
        assert_eq!(page0_h120.items[1].energy, 300);

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
        assert_eq!(page1_h120.items[0].energy, 200);
        assert_eq!(page1_h120.items[1].energy, 100);

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
        assert_eq!(page0_h121.items[0].energy, 400u64.saturating_add(growth));
        assert_eq!(page0_h121.items[1].energy, 300u64.saturating_add(growth));

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
        assert_eq!(page1_h121.items[0].energy, 200u64.saturating_add(growth));
        assert_eq!(page1_h121.items[1].energy, 100u64.saturating_add(growth));

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

        let invalid = make_invalid_pass(53, 63, 100, "INVALID_ETH_MAIN");
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
