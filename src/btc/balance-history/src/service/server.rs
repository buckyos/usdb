use super::rpc::*;
use crate::config::BalanceHistoryConfigRef;
use crate::db::BalanceHistoryDBRef;
use crate::status::{SyncStatus, SyncStatusManagerRef};
use bitcoincore_rpc::bitcoin::OutPoint;
use jsonrpc_core::IoHandler;
use jsonrpc_core::{Error as JsonError, ErrorCode, Result as JsonResult};
use jsonrpc_http_server::{AccessControlAllowOrigin, DomainsValidation, ServerBuilder};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use usdb_util::{
    BALANCE_HISTORY_SERVICE_NAME, ConsensusRpcErrorCode, ConsensusRpcErrorData,
    ConsensusStateReference,
};

// Public version string of the first balance-history block commit protocol.
const COMMIT_PROTOCOL_VERSION: &str = "1.0.0";
// Hash algorithm used by both balance delta roots and rolling block commits.
const COMMIT_HASH_ALGO: &str = "sha256";

// encode_hex converts internal commit bytes to the lowercase hex strings returned by RPC.
fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

#[derive(Clone)]
pub struct BalanceHistoryRpcServer {
    config: BalanceHistoryConfigRef,
    addr: std::net::SocketAddr,
    status: SyncStatusManagerRef,
    db: BalanceHistoryDBRef,
    shutdown_tx: watch::Sender<()>,
    server_handle: Arc<Mutex<Option<jsonrpc_http_server::CloseHandle>>>,
}

impl BalanceHistoryRpcServer {
    pub fn new(
        config: BalanceHistoryConfigRef,
        addr: std::net::SocketAddr,
        status: SyncStatusManagerRef,
        db: BalanceHistoryDBRef,
        shutdown_tx: watch::Sender<()>,
    ) -> Self {
        Self {
            config,
            addr,
            status,
            db,
            shutdown_tx,
            server_handle: Arc::new(Mutex::new(None)),
        }
    }

    pub fn get_listen_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn start(
        config: BalanceHistoryConfigRef,
        status: SyncStatusManagerRef,
        db: BalanceHistoryDBRef,
        shutdown_tx: watch::Sender<()>,
    ) -> Result<Self, String> {
        let addr = format!("127.0.0.1:{}", config.rpc_server.port)
            .parse()
            .map_err(|e| {
                let msg = format!("Failed to parse RPC server address: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let ret = Self::new(config.clone(), addr, status, db, shutdown_tx.clone());

        let mut io = IoHandler::new();
        io.extend_with(ret.clone().to_delegate());

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Any,
            ]))
            .start_http(&addr)
            .map_err(|e| {
                let msg = format!("Unable to start RPC server: {}", e);
                log::error!("{}", msg);
                msg
            })?;

        let handle = server.close_handle();
        info!("RPC server listening on {}", addr);
        tokio::task::spawn_blocking(move || {
            server.wait();
        });

        {
            let mut current = ret.server_handle.lock().unwrap();
            assert!(current.is_none(), "RPC server is already running");
            *current = Some(handle);
        }
        ret.status.set_rpc_alive(true);

        Ok(ret)
    }

    pub async fn close(&self) {
        if let Some(handle) = self.server_handle.lock().unwrap().take() {
            info!("Closing RPC server.");
            tokio::task::spawn_blocking(move || {
                handle.close();
            })
            .await
            .unwrap();

            tokio::time::sleep(Duration::from_millis(500)).await;
            info!("RPC server closed.");
            self.status.set_rpc_alive(false);
        } else {
            warn!("RPC server handle not found.");
        }
    }

    fn to_internal_error(message: String) -> JsonError {
        JsonError {
            code: ErrorCode::InternalError,
            message,
            data: None,
        }
    }

    fn to_consensus_error(code: ConsensusRpcErrorCode, data: ConsensusRpcErrorData) -> JsonError {
        JsonError {
            code: ErrorCode::ServerError(code.code()),
            message: code.as_str().to_string(),
            data: Some(serde_json::to_value(data).unwrap_or_else(|e| {
                serde_json::json!({
                    "service": BALANCE_HISTORY_SERVICE_NAME,
                    "detail": format!("Failed to serialize structured consensus error data: {}", e),
                })
            })),
        }
    }

    fn build_snapshot_info(&self) -> Result<SnapshotInfo, String> {
        let stable_height = self.db.get_btc_block_height()?;
        let latest_commit = self.db.get_block_commit(stable_height)?;

        Ok(SnapshotInfo {
            stable_height,
            stable_block_hash: latest_commit
                .as_ref()
                .map(|entry| format!("{:x}", entry.btc_block_hash)),
            latest_block_commit: latest_commit
                .as_ref()
                .map(|entry| encode_hex(&entry.block_commit)),
            stable_lag: BALANCE_HISTORY_STABLE_LAG,
            balance_history_api_version: BALANCE_HISTORY_API_VERSION.to_string(),
            balance_history_semantics_version: BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
        })
    }

    fn build_consensus_state_reference(
        &self,
        snapshot: Option<&SnapshotInfo>,
    ) -> ConsensusStateReference {
        let Some(snapshot) = snapshot else {
            return ConsensusStateReference::default();
        };

        ConsensusStateReference {
            snapshot_id: None,
            stable_height: Some(snapshot.stable_height),
            stable_block_hash: snapshot.stable_block_hash.clone(),
            balance_history_api_version: Some(snapshot.balance_history_api_version.clone()),
            balance_history_semantics_version: Some(
                snapshot.balance_history_semantics_version.clone(),
            ),
            usdb_index_protocol_version: None,
            local_state_commit: None,
            system_state_id: None,
        }
    }

    fn build_consensus_error_data(
        &self,
        requested_height: Option<u32>,
        snapshot: Option<&SnapshotInfo>,
        detail: impl Into<Option<String>>,
    ) -> ConsensusRpcErrorData {
        let readiness = self.readiness_info().ok();
        let mut data = ConsensusRpcErrorData::new(BALANCE_HISTORY_SERVICE_NAME);
        data.requested_height = requested_height;
        data.upstream_stable_height = snapshot.map(|value| value.stable_height);
        data.consensus_ready = readiness.as_ref().map(|value| value.consensus_ready);
        data.actual_state = self.build_consensus_state_reference(snapshot);
        data.detail = detail.into();
        data
    }

    fn resolve_queryable_snapshot(&self) -> Result<SnapshotInfo, JsonError> {
        let snapshot = self
            .build_snapshot_info()
            .map_err(Self::to_internal_error)?;
        if snapshot.stable_block_hash.is_none() || snapshot.latest_block_commit.is_none() {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::SnapshotNotReady,
                self.build_consensus_error_data(
                    None,
                    Some(&snapshot),
                    Some(
                        "Current stable snapshot is incomplete: missing stable block hash or latest block commit"
                            .to_string(),
                    ),
                ),
            ));
        }

        Ok(snapshot)
    }

    fn validate_requested_height(&self, requested_height: u32) -> Result<SnapshotInfo, JsonError> {
        let snapshot = self.resolve_queryable_snapshot()?;
        if requested_height > snapshot.stable_height {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::HeightNotSynced,
                self.build_consensus_error_data(
                    Some(requested_height),
                    Some(&snapshot),
                    Some(format!(
                        "Requested height {} is above current stable height {}",
                        requested_height, snapshot.stable_height
                    )),
                ),
            ));
        }

        Ok(snapshot)
    }

    fn validate_requested_range(
        &self,
        range: &std::ops::Range<u32>,
    ) -> Result<SnapshotInfo, JsonError> {
        let snapshot = self.resolve_queryable_snapshot()?;
        if !range.is_empty() && range.end.saturating_sub(1) > snapshot.stable_height {
            return Err(Self::to_consensus_error(
                ConsensusRpcErrorCode::HeightNotSynced,
                self.build_consensus_error_data(
                    Some(range.end.saturating_sub(1)),
                    Some(&snapshot),
                    Some(format!(
                        "Requested range [{} , {}) exceeds current stable height {}",
                        range.start, range.end, snapshot.stable_height
                    )),
                ),
            ));
        }

        Ok(snapshot)
    }

    fn readiness_info(&self) -> Result<ReadinessInfo, String> {
        let sync_status = self.status.get_status();
        let runtime = self.status.get_runtime_readiness();
        let stable_height = self.db.get_btc_block_height()?;
        let latest_commit = self.db.get_block_commit(stable_height)?;
        let stable_block_hash = latest_commit
            .as_ref()
            .map(|entry| format!("{:x}", entry.btc_block_hash));
        let latest_block_commit = latest_commit
            .as_ref()
            .map(|entry| encode_hex(&entry.block_commit));

        let mut blockers = Vec::new();
        if !runtime.rpc_alive {
            blockers.push(ReadinessBlocker::RpcNotListening);
        }

        match sync_status.phase {
            crate::status::SyncPhase::Initializing => blockers.push(ReadinessBlocker::Initializing),
            crate::status::SyncPhase::Loading => blockers.push(ReadinessBlocker::Loading),
            crate::status::SyncPhase::Indexing | crate::status::SyncPhase::Synced => {}
        }

        if runtime.rollback_in_progress {
            blockers.push(ReadinessBlocker::RollbackInProgress);
        }
        if runtime.shutdown_requested {
            blockers.push(ReadinessBlocker::ShutdownRequested);
        }
        if sync_status.phase == crate::status::SyncPhase::Indexing
            && sync_status.total > 0
            && sync_status.current < sync_status.total
        {
            blockers.push(ReadinessBlocker::CatchingUp);
        }
        if stable_block_hash.is_none() {
            blockers.push(ReadinessBlocker::StableBlockHashMissing);
        }
        if latest_block_commit.is_none() {
            blockers.push(ReadinessBlocker::LatestBlockCommitMissing);
        }

        let query_ready = runtime.rpc_alive
            && !runtime.rollback_in_progress
            && !runtime.shutdown_requested
            && !matches!(
                sync_status.phase,
                crate::status::SyncPhase::Initializing | crate::status::SyncPhase::Loading
            );
        let consensus_ready = query_ready
            && sync_status.current >= sync_status.total
            && stable_block_hash.is_some()
            && latest_block_commit.is_some();

        Ok(ReadinessInfo {
            service: usdb_util::BALANCE_HISTORY_SERVICE_NAME.to_string(),
            rpc_alive: runtime.rpc_alive,
            query_ready,
            consensus_ready,
            phase: sync_status.phase,
            current: sync_status.current,
            total: sync_status.total,
            message: sync_status.message,
            stable_height: Some(stable_height),
            stable_block_hash,
            latest_block_commit,
            blockers,
        })
    }
}

impl BalanceHistoryRpc for BalanceHistoryRpcServer {
    fn stop(&self) -> JsonResult<()> {
        info!("Received stop command via RPC.");
        self.status.set_shutdown_requested(true);
        if let Err(e) = self.shutdown_tx.send(()) {
            let msg = format!("Failed to send shutdown signal: {}", e);
            log::error!("{}", msg);
            return Err(JsonError {
                code: ErrorCode::InternalError,
                message: msg,
                data: None,
            });
        }

        if let Some(handle) = self.server_handle.lock().unwrap().take() {
            info!("Closing RPC server.");
            tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(500));
                handle.close();
            });
        } else {
            warn!("RPC server handle not found.");
        }

        Ok(())
    }

    fn get_network_type(&self) -> JsonResult<String> {
        let network = self.config.btc.network();

        Ok(network.to_string())
    }

    fn get_block_height(&self) -> JsonResult<u64> {
        let height = self.db.get_btc_block_height().map_err(|e| JsonError {
            code: ErrorCode::InternalError,
            message: format!("Failed to get block height: {}", e),
            data: None,
        })?;

        Ok(height as u64)
    }

    fn get_sync_status(&self) -> JsonResult<SyncStatus> {
        let status = self.status.get_status();
        Ok(status)
    }

    fn get_snapshot_info(&self) -> JsonResult<SnapshotInfo> {
        self.resolve_queryable_snapshot()
    }

    fn get_readiness(&self) -> JsonResult<ReadinessInfo> {
        self.readiness_info().map_err(|e| JsonError {
            code: ErrorCode::InternalError,
            message: format!("Failed to build readiness info: {}", e),
            data: None,
        })
    }

    fn get_block_commit(&self, block_height: u32) -> JsonResult<Option<BlockCommitInfo>> {
        let commit = self
            .db
            .get_block_commit(block_height)
            .map_err(|e| JsonError {
                code: ErrorCode::InternalError,
                message: format!(
                    "Failed to get block commit at height {}: {}",
                    block_height, e
                ),
                data: None,
            })?;

        Ok(commit.map(|entry| BlockCommitInfo {
            block_height: entry.block_height,
            btc_block_hash: format!("{:x}", entry.btc_block_hash),
            balance_delta_root: encode_hex(&entry.balance_delta_root),
            block_commit: encode_hex(&entry.block_commit),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
        }))
    }

    fn get_address_balance(&self, params: GetBalanceParams) -> JsonResult<Vec<AddressBalance>> {
        if let Some(height) = params.block_height {
            self.validate_requested_height(height)?;
            // This endpoint uses at-or-before semantics:
            // return the latest balance record with block_height <= query height.
            // Callers that need exact block delta should use get_address_balance_delta.
            let ret = self
                .db
                .get_balance_at_block_height(&params.script_hash, height)
                .map_err(|e| {
                    Self::to_internal_error(format!(
                        "Failed to get balance at block height {}: {}",
                        height, e
                    ))
                })?;

            let ret = AddressBalance {
                block_height: ret.block_height,
                balance: ret.balance,
                delta: ret.delta,
            };

            Ok(vec![ret])
        } else if let Some(range) = params.block_range {
            // Handle empty range
            if range.is_empty() {
                return Ok(Vec::new());
            }

            self.validate_requested_range(&range)?;

            let ret = self
                .db
                .get_balance_in_range(&params.script_hash, range.start, range.end)
                .map_err(|e| {
                    Self::to_internal_error(format!("Failed to get balance in block range: {}", e))
                })?;

            let balances: Vec<AddressBalance> = ret
                .into_iter()
                .map(|b| AddressBalance {
                    block_height: b.block_height,
                    balance: b.balance,
                    delta: b.delta,
                })
                .collect();

            Ok(balances)
        } else {
            self.resolve_queryable_snapshot()?;
            let ret = self
                .db
                .get_latest_balance(&params.script_hash)
                .map_err(|e| {
                    Self::to_internal_error(format!("Failed to get latest balance: {}", e))
                })?;
            let ret = AddressBalance {
                block_height: ret.block_height,
                balance: ret.balance,
                delta: ret.delta,
            };

            Ok(vec![ret])
        }
    }

    fn get_addresses_balances(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<AddressBalance>>> {
        use rayon::prelude::*;

        let results: JsonResult<Vec<Vec<AddressBalance>>> = params
            .script_hashes
            .par_iter()
            .map(|script_hash| {
                let single_params = GetBalanceParams {
                    script_hash: *script_hash,
                    block_height: params.block_height,
                    block_range: params.block_range.clone(),
                };
                self.get_address_balance(single_params)
            })
            .collect();

        results
    }

    fn get_address_balance_delta(
        &self,
        params: GetBalanceParams,
    ) -> JsonResult<Vec<Option<AddressBalance>>> {
        if let Some(height) = params.block_height {
            let ret = self
                .db
                .get_balance_delta_at_block_height(&params.script_hash, height)
                .map_err(|e| JsonError {
                    code: ErrorCode::InternalError,
                    message: format!(
                        "Failed to get balance delta at block height {}: {}",
                        height, e
                    ),
                    data: None,
                })?;

            let ret = ret.map(|b| AddressBalance {
                block_height: b.block_height,
                balance: b.balance,
                delta: b.delta,
            });

            Ok(vec![ret])
        } else if let Some(range) = params.block_range {
            // Handle empty range
            if range.is_empty() {
                return Ok(Vec::new());
            }

            let ret = self
                .db
                .get_balance_in_range(&params.script_hash, range.start, range.end)
                .map_err(|e| JsonError {
                    code: ErrorCode::InternalError,
                    message: format!("Failed to get balance in block range: {}", e),
                    data: None,
                })?;

            let balances: Vec<Option<AddressBalance>> = ret
                .into_iter()
                .map(|b| {
                    Some(AddressBalance {
                        block_height: b.block_height,
                        balance: b.balance,
                        delta: b.delta,
                    })
                })
                .collect();

            Ok(balances)
        } else {
            let msg =
                "Block height or block range must be specified to get balance delta".to_string();
            error!("{}", msg);
            Err(JsonError {
                code: ErrorCode::InvalidParams,
                message: msg,
                data: None,
            })
        }
    }

    fn get_addresses_balances_delta(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<Option<AddressBalance>>>> {
        use rayon::prelude::*;

        let results: JsonResult<Vec<Vec<Option<AddressBalance>>>> = params
            .script_hashes
            .par_iter()
            .map(|script_hash| {
                let single_params = GetBalanceParams {
                    script_hash: *script_hash,
                    block_height: params.block_height,
                    block_range: params.block_range.clone(),
                };
                self.get_address_balance_delta(single_params)
            })
            .collect();

        results
    }

    fn get_live_utxo(&self, outpoint: OutPoint) -> JsonResult<Option<UtxoInfo>> {
        let utxo = self.db.get_utxo(&outpoint).map_err(|e| JsonError {
            code: ErrorCode::InternalError,
            message: format!(
                "Failed to get utxo {}:{}: {}",
                outpoint.txid, outpoint.vout, e
            ),
            data: None,
        })?;

        Ok(utxo.map(|entry| UtxoInfo {
            txid: outpoint.txid.to_string(),
            vout: outpoint.vout,
            script_hash: format!("{:x}", entry.script_hash),
            value: entry.value,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use crate::db::{
        BalanceHistoryDB, BalanceHistoryDBMode, BalanceHistoryEntry, BlockCommitEntry,
    };
    use crate::status::SyncStatusManager;
    use bitcoincore_rpc::bitcoin::BlockHash;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use jsonrpc_core::ErrorCode as JsonErrorCode;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{
        BALANCE_HISTORY_SERVICE_NAME, ConsensusRpcErrorCode, ConsensusRpcErrorData, USDBScriptHash,
    };

    fn make_test_server(tag: &str) -> BalanceHistoryRpcServer {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root_dir = std::env::temp_dir().join(format!("balance_history_rpc_{}_{}", tag, nanos));
        std::fs::create_dir_all(&root_dir).unwrap();

        let mut config = BalanceHistoryConfig::default();
        config.root_dir = root_dir;
        let config = Arc::new(config);
        let db =
            Arc::new(BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap());
        let status = Arc::new(SyncStatusManager::new());
        let (shutdown_tx, _) = watch::channel(());

        BalanceHistoryRpcServer::new(
            config,
            "127.0.0.1:0".parse().unwrap(),
            status,
            db,
            shutdown_tx,
        )
    }

    fn make_script_hash(byte: u8) -> USDBScriptHash {
        USDBScriptHash::from_byte_array([byte; 32])
    }

    fn seed_balance_entries(server: &BalanceHistoryRpcServer, entries: &[BalanceHistoryEntry]) {
        server
            .db
            .put_address_history_async(&entries.to_vec())
            .unwrap();
    }

    fn seed_stable_commit(server: &BalanceHistoryRpcServer, block_height: u32, byte: u8) {
        let commit = BlockCommitEntry {
            block_height,
            btc_block_hash: BlockHash::from_slice(&[byte; 32]).unwrap(),
            balance_delta_root: [byte.wrapping_add(1); 32],
            block_commit: [byte.wrapping_add(2); 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), block_height, &[commit])
            .unwrap();
    }

    fn decode_consensus_error_data(err: &JsonError) -> ConsensusRpcErrorData {
        serde_json::from_value(err.data.clone().expect("missing structured error data"))
            .expect("invalid structured error data")
    }

    #[test]
    fn test_get_snapshot_info_without_commit() {
        let server = make_test_server("empty");

        let err = server.get_snapshot_info().unwrap_err();
        match err.code {
            JsonErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::SnapshotNotReady.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(
            err.message,
            ConsensusRpcErrorCode::SnapshotNotReady.as_str()
        );

        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, BALANCE_HISTORY_SERVICE_NAME);
        assert_eq!(data.upstream_stable_height, Some(0));
        assert_eq!(data.actual_state.stable_height, Some(0));
        assert_eq!(
            data.actual_state.balance_history_api_version.as_deref(),
            Some(BALANCE_HISTORY_API_VERSION)
        );
        assert_eq!(
            data.actual_state
                .balance_history_semantics_version
                .as_deref(),
            Some(BALANCE_HISTORY_SEMANTICS_VERSION)
        );
        assert_eq!(data.actual_state.stable_block_hash, None);
        assert_eq!(data.consensus_ready, Some(false));
    }

    #[test]
    fn test_get_snapshot_info_with_commit() {
        let server = make_test_server("commit");

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit.clone()])
            .unwrap();

        let snapshot = server.get_snapshot_info().unwrap();
        assert_eq!(snapshot.stable_height, 12);
        assert_eq!(
            snapshot.stable_block_hash,
            Some(format!("{:x}", commit.btc_block_hash))
        );
        assert_eq!(
            snapshot.latest_block_commit,
            Some(encode_hex(&commit.block_commit))
        );
        assert_eq!(snapshot.stable_lag, BALANCE_HISTORY_STABLE_LAG);
        assert_eq!(
            snapshot.balance_history_api_version,
            BALANCE_HISTORY_API_VERSION
        );
        assert_eq!(
            snapshot.balance_history_semantics_version,
            BALANCE_HISTORY_SEMANTICS_VERSION
        );
        assert_eq!(snapshot.commit_protocol_version, COMMIT_PROTOCOL_VERSION);
        assert_eq!(snapshot.commit_hash_algo, COMMIT_HASH_ALGO);
    }

    #[test]
    fn test_get_address_balance_returns_height_not_synced_for_future_height() {
        let server = make_test_server("future_height");

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit.clone()])
            .unwrap();

        let err = server
            .get_address_balance(GetBalanceParams {
                script_hash: make_script_hash(1),
                block_height: Some(13),
                block_range: None,
            })
            .unwrap_err();
        match err.code {
            JsonErrorCode::ServerError(code) => {
                assert_eq!(code, ConsensusRpcErrorCode::HeightNotSynced.code())
            }
            _ => panic!("unexpected error code: {:?}", err.code),
        }
        assert_eq!(err.message, ConsensusRpcErrorCode::HeightNotSynced.as_str());

        let data = decode_consensus_error_data(&err);
        assert_eq!(data.service, BALANCE_HISTORY_SERVICE_NAME);
        assert_eq!(data.requested_height, Some(13));
        assert_eq!(data.upstream_stable_height, Some(12));
        assert_eq!(data.consensus_ready, Some(false));
        assert_eq!(data.actual_state.stable_height, Some(12));
        assert_eq!(
            data.actual_state.stable_block_hash,
            Some(format!("{:x}", commit.btc_block_hash))
        );
    }

    #[test]
    fn test_get_readiness_defaults_to_not_ready_before_rpc_alive() {
        let server = make_test_server("readiness_defaults");

        let readiness = server.get_readiness().unwrap();
        assert!(!readiness.rpc_alive);
        assert!(!readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert_eq!(readiness.phase, crate::status::SyncPhase::Initializing);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::RpcNotListening)
        );
        assert!(readiness.blockers.contains(&ReadinessBlocker::Initializing));
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::StableBlockHashMissing)
        );
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::LatestBlockCommitMissing)
        );
    }

    #[test]
    fn test_get_readiness_consensus_ready_when_caught_up_with_complete_snapshot() {
        let server = make_test_server("readiness_ready");
        server.status.set_rpc_alive(true);
        server
            .status
            .update_phase(crate::status::SyncPhase::Indexing, None);
        server.status.update_total(12, None);
        server.status.update_current(12, None);

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit.clone()])
            .unwrap();

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(readiness.query_ready);
        assert!(readiness.consensus_ready);
        assert_eq!(readiness.phase, crate::status::SyncPhase::Indexing);
        assert_eq!(readiness.current, 12);
        assert_eq!(readiness.total, 12);
        assert_eq!(readiness.stable_height, Some(12));
        assert_eq!(
            readiness.stable_block_hash,
            Some(format!("{:x}", commit.btc_block_hash))
        );
        assert_eq!(
            readiness.latest_block_commit,
            Some(encode_hex(&commit.block_commit))
        );
        assert!(readiness.blockers.is_empty());
    }

    #[test]
    fn test_get_readiness_not_consensus_ready_while_catching_up() {
        let server = make_test_server("readiness_catching_up");
        server.status.set_rpc_alive(true);
        server
            .status
            .update_phase(crate::status::SyncPhase::Indexing, None);
        server.status.update_total(20, None);
        server.status.update_current(12, None);

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit])
            .unwrap();

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert!(readiness.blockers.contains(&ReadinessBlocker::CatchingUp));
    }

    #[test]
    fn test_get_readiness_not_query_ready_during_rollback() {
        let server = make_test_server("readiness_rollback");
        server.status.set_rpc_alive(true);
        server
            .status
            .update_phase(crate::status::SyncPhase::Indexing, None);
        server.status.update_total(12, None);
        server.status.update_current(12, None);
        server.status.set_rollback_in_progress(true);

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit])
            .unwrap();

        let readiness = server.get_readiness().unwrap();
        assert!(readiness.rpc_alive);
        assert!(!readiness.query_ready);
        assert!(!readiness.consensus_ready);
        assert!(
            readiness
                .blockers
                .contains(&ReadinessBlocker::RollbackInProgress)
        );
    }

    #[test]
    fn test_get_block_commit_success() {
        let server = make_test_server("get_block_commit");

        let commit = BlockCommitEntry {
            block_height: 12,
            btc_block_hash: BlockHash::from_slice(&[9u8; 32]).unwrap(),
            balance_delta_root: [10u8; 32],
            block_commit: [11u8; 32],
        };
        server
            .db
            .update_address_history_with_block_commits_async(&Vec::new(), 12, &[commit.clone()])
            .unwrap();

        let loaded = server.get_block_commit(12).unwrap().unwrap();
        assert_eq!(loaded.block_height, 12);
        assert_eq!(
            loaded.btc_block_hash,
            format!("{:x}", commit.btc_block_hash)
        );
        assert_eq!(
            loaded.balance_delta_root,
            encode_hex(&commit.balance_delta_root)
        );
        assert_eq!(loaded.block_commit, encode_hex(&commit.block_commit));
        assert_eq!(loaded.commit_protocol_version, COMMIT_PROTOCOL_VERSION);
        assert_eq!(loaded.commit_hash_algo, COMMIT_HASH_ALGO);
    }

    #[test]
    fn test_get_block_commit_missing_returns_none() {
        let server = make_test_server("get_block_commit_missing");

        let loaded = server.get_block_commit(99).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_get_address_balance_latest_and_at_or_before_semantics() {
        let server = make_test_server("balance_latest_and_before");
        let script_hash = make_script_hash(1);
        seed_balance_entries(
            &server,
            &[
                BalanceHistoryEntry {
                    script_hash,
                    block_height: 10,
                    delta: 50,
                    balance: 50,
                },
                BalanceHistoryEntry {
                    script_hash,
                    block_height: 12,
                    delta: 30,
                    balance: 80,
                },
            ],
        );
        seed_stable_commit(&server, 100, 21);

        let latest = server
            .get_address_balance(GetBalanceParams {
                script_hash,
                block_height: None,
                block_range: None,
            })
            .unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].block_height, 12);
        assert_eq!(latest[0].balance, 80);
        assert_eq!(latest[0].delta, 30);

        let at_or_before = server
            .get_address_balance(GetBalanceParams {
                script_hash,
                block_height: Some(11),
                block_range: None,
            })
            .unwrap();
        assert_eq!(at_or_before.len(), 1);
        assert_eq!(at_or_before[0].block_height, 10);
        assert_eq!(at_or_before[0].balance, 50);
        assert_eq!(at_or_before[0].delta, 50);

        let missing_history = server
            .get_address_balance(GetBalanceParams {
                script_hash: make_script_hash(9),
                block_height: Some(100),
                block_range: None,
            })
            .unwrap();
        assert_eq!(missing_history.len(), 1);
        assert_eq!(missing_history[0].block_height, 0);
        assert_eq!(missing_history[0].balance, 0);
        assert_eq!(missing_history[0].delta, 0);
    }

    #[test]
    fn test_get_address_balance_range_and_empty_range() {
        let server = make_test_server("balance_range");
        let script_hash = make_script_hash(2);
        seed_balance_entries(
            &server,
            &[
                BalanceHistoryEntry {
                    script_hash,
                    block_height: 10,
                    delta: 50,
                    balance: 50,
                },
                BalanceHistoryEntry {
                    script_hash,
                    block_height: 12,
                    delta: 30,
                    balance: 80,
                },
            ],
        );
        seed_stable_commit(&server, 12, 31);

        let range = server
            .get_address_balance(GetBalanceParams {
                script_hash,
                block_height: None,
                block_range: Some(10..13),
            })
            .unwrap();
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].block_height, 10);
        assert_eq!(range[1].block_height, 12);

        let empty = server
            .get_address_balance(GetBalanceParams {
                script_hash,
                block_height: None,
                block_range: Some(20..20),
            })
            .unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_get_address_balance_delta_requires_selector_and_exact_miss_returns_none() {
        let server = make_test_server("balance_delta_params");
        let script_hash = make_script_hash(3);
        seed_balance_entries(
            &server,
            &[BalanceHistoryEntry {
                script_hash,
                block_height: 12,
                delta: 30,
                balance: 80,
            }],
        );

        let err = server
            .get_address_balance_delta(GetBalanceParams {
                script_hash,
                block_height: None,
                block_range: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(
            err.message
                .contains("Block height or block range must be specified")
        );

        let exact_miss = server
            .get_address_balance_delta(GetBalanceParams {
                script_hash,
                block_height: Some(11),
                block_range: None,
            })
            .unwrap();
        assert_eq!(exact_miss.len(), 1);
        assert!(exact_miss[0].is_none());
    }

    #[test]
    fn test_batch_balance_queries_preserve_input_order_and_duplicates() {
        let server = make_test_server("batch_balance_order");
        let script_hash_a = make_script_hash(4);
        let script_hash_b = make_script_hash(5);
        seed_balance_entries(
            &server,
            &[
                BalanceHistoryEntry {
                    script_hash: script_hash_a,
                    block_height: 10,
                    delta: 50,
                    balance: 50,
                },
                BalanceHistoryEntry {
                    script_hash: script_hash_b,
                    block_height: 11,
                    delta: 7,
                    balance: 7,
                },
                BalanceHistoryEntry {
                    script_hash: script_hash_a,
                    block_height: 12,
                    delta: 30,
                    balance: 80,
                },
            ],
        );
        seed_stable_commit(&server, 12, 41);

        let results = server
            .get_addresses_balances(GetBalancesParams {
                script_hashes: vec![script_hash_b, script_hash_a, script_hash_b],
                block_height: Some(12),
                block_range: None,
            })
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].len(), 1);
        assert_eq!(results[0][0].block_height, 11);
        assert_eq!(results[0][0].balance, 7);
        assert_eq!(results[1].len(), 1);
        assert_eq!(results[1][0].block_height, 12);
        assert_eq!(results[1][0].balance, 80);
        assert_eq!(results[2].len(), 1);
        assert_eq!(results[2][0].block_height, 11);
        assert_eq!(results[2][0].balance, 7);
    }

    #[test]
    fn test_get_live_utxo_success() {
        use bitcoincore_rpc::bitcoin::OutPoint;
        use bitcoincore_rpc::bitcoin::Txid;

        let server = make_test_server("get_live_utxo");
        let outpoint = OutPoint {
            txid: Txid::from_slice(&[7u8; 32]).unwrap(),
            vout: 3,
        };
        let script_hash = USDBScriptHash::from_byte_array([3u8; 32]);
        server.db.put_utxo(&outpoint, &script_hash, 12345).unwrap();

        let loaded = server.get_live_utxo(outpoint.clone()).unwrap().unwrap();
        assert_eq!(loaded.txid, outpoint.txid.to_string());
        assert_eq!(loaded.vout, outpoint.vout);
        assert_eq!(loaded.script_hash, format!("{:x}", script_hash));
        assert_eq!(loaded.value, 12345);
    }
}
