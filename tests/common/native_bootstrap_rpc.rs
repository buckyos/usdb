//! Serve reviewed Core block fixtures through the production HTTP RPC client.

#[path = "http_rpc.rs"]
mod http_rpc;

use std::path::Path;

use bitcoincore_rpc::bitcoin::{Block, Network, consensus};
use serde_json::{Value, json};

pub use http_rpc::RpcServer;

/// Allow faults at a specific bootstrap phase without replacing native replay/verification.
pub fn serve(
    blocks: Vec<Block>,
    root: &Path,
    mut intercept: impl FnMut(&Value, &[u8]) -> Option<(u16, Value)> + Send + 'static,
) -> RpcServer {
    let progress = root.join("bootstrap-progress.json");
    RpcServer::new(move |request| {
        let journal = std::fs::read(&progress).unwrap_or_default();
        if let Some(reply) = intercept(&request, &journal) {
            return reply;
        }
        let result = match request["method"].as_str().unwrap() {
            "getblockcount" => json!(
                blocks.len() as u32 - 1
                    + usdb_util::embedded_btc_stable_lag_blocks(Network::Regtest).unwrap()
            ),
            "getblockhash" => {
                json!(blocks[request["params"][0].as_u64().unwrap() as usize].block_hash())
            }
            "getblock" => {
                let hash = request["params"][0].as_str().unwrap();
                let block = blocks
                    .iter()
                    .find(|block| block.block_hash().to_string() == hash)
                    .unwrap();
                json!(consensus::encode::serialize_hex(block))
            }
            method => panic!("Unexpected native bootstrap RPC: {method}"),
        };
        (
            200,
            json!({"id":request["id"], "result":result, "error":null}),
        )
    })
}
