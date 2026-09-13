//! Small HTTP Core stub for block-scoped historical-input integration tests.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, consensus};
use serde_json::{Value, json};
use usdb_util::BTCRpcClient;

pub struct Reply {
    pub canonical_hash: String,
    pub verbose: Value,
    pub reorg_after_verbose: bool,
    pub calls: Vec<String>,
}

pub struct CoreStub {
    pub url: String,
    pub state: Arc<Mutex<Reply>>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(Mutex::new(Reply {
            canonical_hash: block.block_hash().to_string(),
            verbose,
            reorg_after_verbose: false,
            calls: Vec::new(),
        }));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stopped.clone();
        let raw = consensus::encode::serialize_hex(block);
        let thread = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("Core stub accept: {e}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let (offset, length) = loop {
                    let mut buffer = [0; 4096];
                    let size = stream.read(&mut buffer).unwrap();
                    assert!(size > 0);
                    bytes.extend_from_slice(&buffer[..size]);
                    if let Some(offset) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..offset]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (offset + 4, length);
                    }
                };
                while bytes.len() < offset + length {
                    let mut buffer = [0; 4096];
                    let size = stream.read(&mut buffer).unwrap();
                    assert!(size > 0);
                    bytes.extend_from_slice(&buffer[..size]);
                }
                let request: Value =
                    serde_json::from_slice(&bytes[offset..offset + length]).unwrap();
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
                let body = json!({"result":result,"error":error,"id":request["id"]}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        Self {
            url,
            state,
            stopped,
            thread: Some(thread),
        }
    }

    pub fn client(&self) -> Arc<BTCRpcClient> {
        Arc::new(BTCRpcClient::new(self.url.clone(), bitcoincore_rpc::Auth::None).unwrap())
    }
}

impl Drop for CoreStub {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
