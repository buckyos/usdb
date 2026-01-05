mod client;
mod file_indexer;
mod local_loader;
mod rpc;

pub use client::*;
pub use file_indexer::*;
pub use local_loader::*;

use crate::config::BalanceHistoryConfigRef;
use crate::db::BalanceHistoryDBRef;
use crate::output::IndexOutputRef;
use std::sync::Arc;
use usdb_util::BTCRpcClient;

pub fn create_btc_rpc_client(config: &BalanceHistoryConfigRef) -> Result<BTCClientRef, String> {
    let rpc_url = config.btc.rpc_url();
    let auth = config.btc.auth();
    let btc_client = BTCRpcClient::new(rpc_url, auth).map_err(|e| {
        let msg = format!("Failed to create BTC client: {}", e);
        error!("{}", msg);
        msg
    })?;

    Ok(Arc::new(Box::new(btc_client) as Box<dyn BTCClient>))
}

pub fn create_local_btc_client(
    rpc_client: BTCClientRef,
    config: &BalanceHistoryConfigRef,
    output: IndexOutputRef,
    db: BalanceHistoryDBRef,
) -> Result<BTCClientRef, String> {
    let loader = BlockLocalLoader::new(
        config.btc.block_magic(),
        &config.btc.data_dir(),
        rpc_client,
        db,
        output,
    )
    .map_err(|e| {
        let msg = format!("Failed to create BlockLocalLoader: {}", e);
        error!("{}", msg);
        msg
    })?;

    Ok(Arc::new(Box::new(loader) as Box<dyn BTCClient>))
}
