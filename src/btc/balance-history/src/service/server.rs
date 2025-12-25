use super::rpc::*;
use crate::config::BalanceHistoryConfigRef;
use crate::db::BalanceHistoryDBRef;
use crate::status::{SyncStatus, SyncStatusManagerRef};
use jsonrpc_core::IoHandler;
use jsonrpc_core::{Error as JsonError, ErrorCode, Result as JsonResult};
use jsonrpc_http_server::{AccessControlAllowOrigin, DomainsValidation, ServerBuilder};

#[derive(Clone)]
pub struct BalanceHistoryRpcServer {
    config: BalanceHistoryConfigRef,
    status: SyncStatusManagerRef,
    db: BalanceHistoryDBRef,
}

impl BalanceHistoryRpcServer {
    pub fn new(config: BalanceHistoryConfigRef, status: SyncStatusManagerRef, db: BalanceHistoryDBRef) -> Self {
        Self { config, status, db }
    }

    pub fn start(
        config: BalanceHistoryConfigRef,
        status: SyncStatusManagerRef,
        db: BalanceHistoryDBRef,
    ) -> Result<(), String> {
        let ret = Self::new(config.clone(), status, db);

        let mut io = IoHandler::new();
        io.extend_with(ret.clone().to_delegate());

        let addr = format!("127.0.0.1:{}", config.rpc_server.port)
            .parse()
            .map_err(|e| {
                let msg = format!("Failed to parse RPC server address: {}", e);
                log::error!("{}", msg);
                msg
            })?;

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

        info!("RPC server listening on {}", addr);
        tokio::spawn(async move {
            server.wait();
        });

        Ok(())
    }
}

impl BalanceHistoryRpc for BalanceHistoryRpcServer {
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

    fn get_address_balance(&self, params: GetBalanceParams) -> JsonResult<Vec<AddressBalance>> {
        if let Some(height) = params.block_height {
            let ret = self
                .db
                .get_balance_at_block_height(params.script_hash, height)
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
            let ret = self
                .db
                .get_balance_in_range(params.script_hash, range.start, range.end)
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
            let msg = format!("Either block_height or block_range must be specified");
            Err(JsonError {
                code: ErrorCode::InvalidParams,
                message: msg,
                data: None,
            })
        }
    }

    fn get_addresses_balances(
        &self,
        params: GetBalancesParams,
    ) -> JsonResult<Vec<Vec<AddressBalance>>> {
        if params.block_height.is_none() && params.block_range.is_none() {
            let msg = format!(
                "Either block_height or block_range must be specified for script_hash: {}",
                params.script_hashes.len()
            );
            return Err(JsonError {
                code: ErrorCode::InvalidParams,
                message: msg,
                data: None,
            });
        }

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
}
