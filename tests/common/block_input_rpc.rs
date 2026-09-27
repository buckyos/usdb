//! Small HTTP Core stub for block-scoped historical-input integration tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, consensus};
use serde_json::{Value, json};
use usdb_util::BTCRpcClient;

#[path = "http_rpc.rs"]
mod http_rpc;
use http_rpc::RpcServer;

pub struct Reply {
    pub canonical_hash: String,
    pub verbose: Value,
    pub reorg_after_verbose: bool,
    pub calls: Vec<String>,
}

pub struct CoreStub {
    pub url: String,
    pub state: Arc<Mutex<Reply>>,
    _server: RpcServer,
}

pub fn verbose_block(height: u32, block: &Block, values: &HashMap<OutPoint, Amount>) -> Value {
    json!({"height":height, "hash":block.block_hash(), "confirmations":10000,
        "tx":block.txdata.iter().map(|tx| json!({"hex":consensus::encode::serialize_hex(tx),
            "vin":tx.input.iter().map(|input| {
                if tx.is_coinbase() { json!({"coinbase":"00"}) }
                else { json!({"txid":input.previous_output.txid,"vout":input.previous_output.vout,
                    "prevout":{"value":values[&input.previous_output].to_btc()}}) }
            }).collect::<Vec<_>>()
        })).collect::<Vec<_>>()})
}

impl CoreStub {
    pub fn new(block: &Block, verbose: Value) -> Self {
        let state = Arc::new(Mutex::new(Reply {
            canonical_hash: block.block_hash().to_string(),
            verbose,
            reorg_after_verbose: false,
            calls: Vec::new(),
        }));
        let worker_state = state.clone();
        let raw = consensus::encode::serialize_hex(block);
        let server = RpcServer::new(move |request| {
            let method = request["method"].as_str().unwrap();
            let mut state = worker_state.lock().unwrap();
            state.calls.push(method.to_string());
            let result = match method {
                "getblockhash" => Some(json!(state.canonical_hash)),
                "getblockcount" => Some(json!(1000000)),
                "gettxout" => Some(Value::Null),
                "getblock" if request["params"][1] == 3 => {
                    let value = state.verbose.clone();
                    if state.reorg_after_verbose {
                        state.canonical_hash = "00".repeat(32);
                    }
                    Some(value)
                }
                "getblock" => Some(json!(raw)),
                _ => None,
            };
            let error = result.is_none().then(
                || json!({"code":-5,"message":"No txindex or historical transaction lookup"}),
            );
            (
                200,
                json!({"result":result,"error":error,"id":request["id"]}),
            )
        });
        Self {
            url: server.url.clone(),
            state,
            _server: server,
        }
    }

    pub fn client(&self) -> Arc<BTCRpcClient> {
        Arc::new(BTCRpcClient::new(self.url.clone(), bitcoincore_rpc::Auth::None).unwrap())
    }
}
