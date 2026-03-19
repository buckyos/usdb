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
        } else {
            warn!("RPC server handle not found.");
        }
    }
}

impl BalanceHistoryRpc for BalanceHistoryRpcServer {
    fn stop(&self) -> JsonResult<()> {
        info!("Received stop command via RPC.");
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
        let stable_height = self.db.get_btc_block_height().map_err(|e| JsonError {
            code: ErrorCode::InternalError,
            message: format!("Failed to get stable height: {}", e),
            data: None,
        })?;

        let latest_commit = self
            .db
            .get_block_commit(stable_height)
            .map_err(|e| JsonError {
                code: ErrorCode::InternalError,
                message: format!(
                    "Failed to get block commit at height {}: {}",
                    stable_height, e
                ),
                data: None,
            })?;

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
            // This endpoint uses at-or-before semantics:
            // return the latest balance record with block_height <= query height.
            // Callers that need exact block delta should use get_address_balance_delta.
            let ret = self
                .db
                .get_balance_at_block_height(&params.script_hash, height)
                .map_err(|e| JsonError {
                    code: ErrorCode::InternalError,
                    message: format!("Failed to get balance at block height {}: {}", height, e),
                    data: None,
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

            let ret = self
                .db
                .get_balance_in_range(&params.script_hash, range.start, range.end)
                .map_err(|e| JsonError {
                    code: ErrorCode::InternalError,
                    message: format!("Failed to get balance in block range: {}", e),
                    data: None,
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
            let ret = self
                .db
                .get_latest_balance(&params.script_hash)
                .map_err(|e| JsonError {
                    code: ErrorCode::InternalError,
                    message: format!("Failed to get latest balance: {}", e),
                    data: None,
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
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::USDBScriptHash;

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

    #[test]
    fn test_get_snapshot_info_without_commit() {
        let server = make_test_server("empty");

        let snapshot = server.get_snapshot_info().unwrap();
        assert_eq!(snapshot.stable_height, 0);
        assert_eq!(snapshot.stable_block_hash, None);
        assert_eq!(snapshot.latest_block_commit, None);
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
