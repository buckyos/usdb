use super::rpc::*;
use crate::config::ConfigManagerRef;
use crate::index::InscriptionIndexer;
use crate::status::StatusManagerRef;
use jsonrpc_core::IoHandler;
use jsonrpc_core::{Error as JsonError, ErrorCode, Result as JsonResult};
use jsonrpc_http_server::{AccessControlAllowOrigin, DomainsValidation, ServerBuilder};
use ord::InscriptionId;
use serde_json::json;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use usdb_util::USDBScriptHash;

const ERR_HEIGHT_NOT_SYNCED: i64 = -32010;
const ERR_PASS_NOT_FOUND: i64 = -32011;
const ERR_ENERGY_NOT_FOUND: i64 = -32012;
const ERR_SNAPSHOT_NOT_FOUND: i64 = -32013;
const ERR_DUPLICATE_ACTIVE_OWNER: i64 = -32014;
const ERR_INVALID_PAGINATION: i64 = -32015;
const ERR_INVALID_HEIGHT_RANGE: i64 = -32016;
const ERR_INTERNAL_INVARIANT_BROKEN: i64 = -32017;

#[derive(Clone)]
pub struct UsdbIndexerRpcServer {
    config: ConfigManagerRef,
    status: StatusManagerRef,
    indexer: Arc<InscriptionIndexer>,
    addr: std::net::SocketAddr,
    shutdown_tx: watch::Sender<()>,
    server_handle: Arc<Mutex<Option<jsonrpc_http_server::CloseHandle>>>,
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
        }
    }

    pub fn start(
        config: ConfigManagerRef,
        status: StatusManagerRef,
        indexer: Arc<InscriptionIndexer>,
        shutdown_tx: watch::Sender<()>,
    ) -> Result<Self, String> {
        let addr = format!("127.0.0.1:{}", config.config().usdb.rpc_server_port)
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

    fn synced_height(&self) -> Result<Option<u32>, JsonError> {
        self.indexer
            .miner_pass_storage()
            .get_synced_btc_block_height()
            .map_err(Self::to_internal_error)
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
        USDBScriptHash::from_str(value)
            .map_err(|e| Self::to_invalid_params(format!("Invalid owner {}: {}", value, e)))
    }

    fn validate_pagination(&self, page: usize, page_size: usize) -> Result<(), JsonError> {
        if page_size == 0 {
            return Err(Self::to_business_error(
                ERR_INVALID_PAGINATION,
                "INVALID_PAGINATION",
                json!({"page": page, "page_size": page_size}),
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
}

impl UsdbIndexerRpc for UsdbIndexerRpcServer {
    fn get_rpc_info(&self) -> JsonResult<RpcInfo> {
        Ok(RpcInfo {
            service: "usdb-indexer".to_string(),
            api_version: "1.0.0".to_string(),
            network: self.config.config().bitcoin.network().to_string(),
            features: vec![
                "pass_snapshot".to_string(),
                "pass_history".to_string(),
                "active_passes_at_height".to_string(),
                "owner_active_pass_at_height".to_string(),
                "energy_snapshot".to_string(),
                "energy_range".to_string(),
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
            latest_depend_synced_block_height: status.latest_depend_synced_block_height,
            current: status.current,
            total: status.total,
            message: status.message,
        })
    }

    fn get_synced_block_height(&self) -> JsonResult<Option<u64>> {
        Ok(self.synced_height()?.map(|v| v as u64))
    }

    fn get_pass_snapshot(&self, params: GetPassSnapshotParams) -> JsonResult<Option<PassSnapshot>> {
        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_height = self.resolve_height(params.at_height)?;
        self.build_pass_snapshot(&inscription_id, resolved_height)
    }

    fn get_active_passes_at_height(
        &self,
        params: GetActivePassesAtHeightParams,
    ) -> JsonResult<ActivePassesAtHeight> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_height = self.resolve_height(params.at_height)?;
        let rows = self
            .indexer
            .miner_pass_storage()
            .get_all_active_pass_by_page_from_history_at_height(
                params.page,
                params.page_size,
                resolved_height,
            )
            .map_err(Self::to_internal_error)?;

        Ok(ActivePassesAtHeight {
            resolved_height,
            items: rows
                .into_iter()
                .map(|row| ActivePassItem {
                    inscription_id: row.inscription_id.to_string(),
                    owner: row.owner.to_string(),
                })
                .collect(),
        })
    }

    fn get_pass_history(&self, params: GetPassHistoryParams) -> JsonResult<PassHistoryPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;

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

    fn get_pass_energy(&self, params: GetPassEnergyParams) -> JsonResult<PassEnergySnapshot> {
        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let query_height = self.resolve_height(params.block_height)?;
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

        Ok(PassEnergySnapshot {
            inscription_id: record.inscription_id.to_string(),
            query_block_height: query_height,
            record_block_height: record.block_height,
            state: record.state.as_str().to_string(),
            active_block_height: record.active_block_height,
            owner_address: record.owner_address.to_string(),
            owner_balance: record.owner_balance,
            owner_delta: record.owner_delta,
            energy: record.energy,
        })
    }

    fn get_pass_energy_range(
        &self,
        params: GetPassEnergyRangeParams,
    ) -> JsonResult<PassEnergyRangePage> {
        self.validate_pagination(params.page, params.page_size)?;

        let inscription_id = self.parse_inscription_id(&params.inscription_id)?;
        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;

        let records = self
            .indexer
            .pass_energy_manager()
            .get_pass_energy_records_by_page_in_height_range(
                &inscription_id,
                params.from_height,
                resolved_to_height,
                params.page,
                params.page_size,
            )
            .map_err(Self::to_internal_error)?;

        Ok(PassEnergyRangePage {
            resolved_height: resolved_to_height,
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

    fn get_invalid_passes(&self, params: GetInvalidPassesParams) -> JsonResult<InvalidPassesPage> {
        self.validate_pagination(params.page, params.page_size)?;

        let resolved_to_height = self.resolve_height_range(params.from_height, params.to_height)?;
        let rows = self
            .indexer
            .miner_pass_storage()
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
        let synced_height = self.resolve_height(Some(params.block_height))?;
        let snapshot = self
            .indexer
            .miner_pass_storage()
            .get_active_balance_snapshot(synced_height)
            .map_err(Self::to_internal_error)?;

        let Some(snapshot) = snapshot else {
            return Err(Self::to_business_error(
                ERR_SNAPSHOT_NOT_FOUND,
                "SNAPSHOT_NOT_FOUND",
                json!({"block_height": synced_height}),
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
    use crate::config::ConfigManager;
    use crate::index::{InscriptionIndexer, MinerPassState};
    use crate::output::IndexOutput;
    use crate::status::StatusManager;
    use crate::storage::MinerPassInfo;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
    use ord::InscriptionId;
    use ordinals::SatPoint;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

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

    fn build_server(tag: &str, synced_height: u32) -> (UsdbIndexerRpcServer, PathBuf) {
        let root_dir = test_root_dir(tag);
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
        assert_eq!(history.items.len(), 2);
        assert_eq!(history.items[0].event_type, "mint");
        assert_eq!(history.items[1].event_type, "state_update");

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

        drop(server);
        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
