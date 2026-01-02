mod client;
mod file_indexer;
mod local_loader;
mod rpc;

pub use client::*;
pub use file_indexer::*;
pub use local_loader::*;
pub use rpc::*;

use crate::config::BalanceHistoryConfigRef;
use crate::db::BalanceHistoryDBRef;
use crate::output::IndexOutputRef;
use std::sync::Arc;
use usdb_util::BTCRpcClient;

pub fn create_btc_client(
    config: &BalanceHistoryConfigRef,
    output: IndexOutputRef,
    db: BalanceHistoryDBRef,
    last_synced_block_height: u32,
) -> Result<BTCClientRef, String> {
    let rpc_url = config.btc.rpc_url();
    let auth = config.btc.auth();
    let btc_client = BTCRpcClient::new(rpc_url, auth).map_err(|e| {
        let msg = format!("Failed to create BTC client: {}", e);
        error!("{}", msg);
        msg
    })?;

    let latest_block_height = btc_client.get_latest_block_height().map_err(|e| {
        let msg = format!("Failed to get latest block height from BTC client: {}", e);
        error!("{}", msg);
        msg
    })?;
    assert!(last_synced_block_height <= latest_block_height);

    // Determine client type, if we behind by more than 500 blocks, use LocalLoader
    let client_type = if latest_block_height - last_synced_block_height > 500 {
        info!("Using LocalLoader BTC client as we are behind by more than 500 blocks");
        BTCClientType::LocalLoader
    } else {
        info!("Using RPC BTC client");
        BTCClientType::RPC
    };

    match client_type {
        BTCClientType::RPC => Ok(Arc::new(Box::new(btc_client) as Box<dyn BTCClient>)),
        BTCClientType::LocalLoader => {
            let client = Arc::new(btc_client);
            let loader = BlockLocalLoader::new(
                config.btc.block_magic(),
                &config.btc.data_dir(),
                client,
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
    }
}
