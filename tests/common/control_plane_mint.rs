//! Private HTTP dependencies for the control-plane mint workflow; no external service access.
use crate::{config::ControlPlaneConfig, rpc_client::RpcClient, server::AppState};
use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    routing::post,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub const SOURCE: &str = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
pub const RECIPIENT: &str = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
pub fn id(byte: char) -> String {
    format!("{}i0", byte.to_string().repeat(64))
}
pub fn hash(address: &str) -> String {
    usdb_util::address_string_to_script_hash(address, &bitcoincore_rpc::bitcoin::Network::Bitcoin)
        .unwrap()
        .to_string()
}

pub struct Fixture {
    pub state: AppState,
    pub data: Arc<Mutex<Value>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn new() -> Self {
        let data = Arc::new(Mutex::new(json!({
            "balance":0,"history":false,"passes":[],"confirmations":8,"overrides":{},"calls":[],
            "state":{"block_height":100,"snapshot_info":{"snapshot_id":"snapshot","stable_block_hash":"canonical",
                "consensus_identity":{"stable_lag":6,"network":"bitcoin"}},
                "local_state_commit_info":{"active_version_set":{"inscription_schema_version":"uip-0001-miner-pass-inscription:v2","pass_state_machine_version":"uip-0002-pass-state-machine:v2"}}},
            "snapshot":{"inscription_id":id('a'),"mint_block_height":90,"state":"active","mint_version":2,
                "owner":hash(RECIPIENT),"mint_owner":hash(RECIPIENT),"pass_kind":"standard",
                "usdb_main":format!("0x{}","1".repeat(40)),"leader_pass_id":null,"leader_btc_addr":null,"prev":[]},
            "source":{"source_owner":hash(SOURCE),"source_outpoint":format!("{}:0","b".repeat(64)),"authorization":"p2pkh_all"},
            "audit":{"inscription_id":id('a'),"block_height":90,"block_hash":"reveal","recipient":hash(RECIPIENT),"error_code":null}
        })));
        let router = Router::new()
            .route("/", post(rpc))
            .route("/bh", post(rpc))
            .with_state(data.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config = ControlPlaneConfig {
            root_dir: std::env::temp_dir().join("usdb-mint-no-runtime-artifacts"),
            ..ControlPlaneConfig::default()
        };
        config.bitcoin.url = url.clone();
        config.rpc.balance_history_url = format!("{url}/bh");
        config.rpc.usdb_indexer_url = url;
        config.rpc.usdb_chain_url = "http://127.0.0.1:1".into();
        config.rpc.ord_url = "http://127.0.0.1:1".into();
        let state = AppState {
            config: Arc::new(config),
            rpc_client: RpcClient::new().unwrap(),
            services_cache: Arc::new(tokio::sync::Mutex::new(None)),
        };
        Self { state, data, task }
    }
}
async fn rpc(
    State(shared): State<Arc<Mutex<Value>>>,
    OriginalUri(uri): OriginalUri,
    Json(request): Json<Value>,
) -> Json<Value> {
    let mut data = shared.lock().unwrap();
    let method = request["method"].as_str().unwrap();
    data["calls"].as_array_mut().unwrap().push(request.clone());
    if data["fail"] == method {
        return Json(
            json!({"id":request["id"],"error":{"code":-1,"message":"fixture evidence unavailable"}}),
        );
    }
    let params = &request["params"][0];
    let value = if let Some(value) = data["overrides"].get(method) {
        value.clone()
    } else {
        match method {
            "get_network_type" => json!("bitcoin"),
            "get_readiness" => {
                json!({"rpc_alive":true,"query_ready":true,"consensus_ready":true,"phase":"ready","current":100,"total":100,"blockers":[],"synced_block_height":100,"stable_height":100})
            }
            "getblockchaininfo" => {
                json!({"chain":"main","blocks":107,"headers":107,"bestblockhash":"tip","verificationprogress":1.0,"initialblockdownload":false})
            }
            "getblockhash" => json!("canonical"),
            "getblockheader" => json!({"time":1,"confirmations":data["confirmations"]}),
            "get_state_ref_at_height" if uri.path() == "/bh" => json!({"snapshot_id":"snapshot"}),
            "get_state_ref_at_height" => data["state"].clone(),
            "get_addresses_balances" => {
                json!([[{"block_height":100,"balance":200_000}],[{"block_height":100,"balance":data["balance"]}]])
            }
            "get_owner_passes_at_height" => {
                let owner = &params["owner"];
                let rows = if *owner == hash(SOURCE) {
                    data["passes"].as_array().unwrap().clone()
                } else {
                    vec![]
                };
                let start = params["page"].as_u64().unwrap() as usize
                    * params["page_size"].as_u64().unwrap() as usize;
                let items = rows
                    .iter()
                    .skip(start)
                    .take(params["page_size"].as_u64().unwrap() as usize)
                    .cloned()
                    .collect::<Vec<_>>();
                json!({"owner":owner,"resolved_height":100,"total":rows.len(),"items":items,"ever_valid_owner":data["history"]})
            }
            "get_pass_snapshot" | "get_owner_active_pass_at_height" => {
                if params["inscription_id"] == id('c') {
                    json!({"state":data.get("prev_state").cloned().unwrap_or(json!("consumed"))})
                } else {
                    data["snapshot"].clone()
                }
            }
            "get_pass_mint_source" => {
                json!({"mint":{"observed_at_height":100,"audit":data["audit"]},"source":data["source"]})
            }
            _ => Value::Null,
        }
    };
    Json(json!({"id":request["id"],"result":value,"error":null}))
}
