//! Shared deterministic signing and multi-block RPC fixtures for MinerPass evidence tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bitcoincore_rpc::bitcoin::{
    Amount, Block, Network, OutPoint, PublicKey, ScriptBuf, Transaction, TxIn, TxOut, Witness,
    absolute, consensus, ecdsa,
    hashes::Hash,
    key::TapTweak,
    script::{Builder, PushBytesBuf},
    secp256k1::{Keypair, Message, Secp256k1, SecretKey},
    sighash::{Annex, EcdsaSighashType, Prevouts, SighashCache, TapSighashType},
    taproot::{self, LeafVersion, TaprootBuilder},
    transaction,
};
use serde_json::{Value, json};
use usdb_util::{BTCRpcClient, SpentPrevout};

pub use crate::index::test_http_rpc::RpcServer;

#[derive(Clone, Copy, Debug)]
pub enum SpendKind {
    Legacy,
    Witness,
    Taproot,
}

pub fn secret() -> SecretKey {
    SecretKey::from_slice(&[41; 32]).unwrap()
}

pub fn source_script(kind: SpendKind) -> ScriptBuf {
    let secp = Secp256k1::new();
    let pair = Keypair::from_secret_key(&secp, &secret());
    let public = PublicKey::new(pair.public_key());
    match kind {
        SpendKind::Legacy => ScriptBuf::new_p2pkh(&public.pubkey_hash()),
        SpendKind::Witness => ScriptBuf::new_p2wpkh(&public.wpubkey_hash().unwrap()),
        SpendKind::Taproot => ScriptBuf::new_p2tr(&secp, pair.x_only_public_key().0, None),
    }
}

pub fn output(value: u64, script: ScriptBuf) -> TxOut {
    TxOut {
        value: Amount::from_sat(value),
        script_pubkey: script,
    }
}

pub fn transaction(inputs: Vec<OutPoint>, outputs: Vec<TxOut>) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: inputs
            .into_iter()
            .map(|previous_output| TxIn {
                previous_output,
                ..TxIn::default()
            })
            .collect(),
        output: outputs,
    }
}

// Sign actual transaction digests, including deliberately unsupported but Bitcoin-valid flags.
pub fn sign(
    tx: &mut Transaction,
    index: usize,
    prevouts: &[TxOut],
    kind: SpendKind,
    flag: u8,
    annex: bool,
) {
    let secp = Secp256k1::new();
    let pair = Keypair::from_secret_key(&secp, &secret());
    let public = PublicKey::new(pair.public_key());
    let mut cache = SighashCache::new(&*tx);
    match kind {
        SpendKind::Taproot => {
            let annex_bytes = [0x50, 0x42];
            let sighash_type = TapSighashType::from_consensus_u8(flag).unwrap();
            let digest = cache
                .taproot_signature_hash(
                    index,
                    &Prevouts::All(prevouts),
                    annex.then(|| Annex::new(&annex_bytes).unwrap()),
                    None,
                    sighash_type,
                )
                .unwrap();
            let signature = taproot::Signature {
                signature: secp.sign_schnorr_no_aux_rand(
                    &Message::from_digest(digest.to_byte_array()),
                    &pair.tap_tweak(&secp, None).to_keypair(),
                ),
                sighash_type,
            };
            let mut stack = vec![signature.to_vec()];
            if annex {
                stack.push(annex_bytes.to_vec());
            }
            tx.input[index].witness = Witness::from_slice(&stack);
        }
        _ => {
            let sighash_type = EcdsaSighashType::from_consensus(u32::from(flag));
            let digest = match kind {
                SpendKind::Legacy => cache
                    .legacy_signature_hash(index, &prevouts[index].script_pubkey, u32::from(flag))
                    .unwrap()
                    .to_byte_array(),
                SpendKind::Witness => cache
                    .p2wpkh_signature_hash(
                        index,
                        &prevouts[index].script_pubkey,
                        prevouts[index].value,
                        sighash_type,
                    )
                    .unwrap()
                    .to_byte_array(),
                _ => unreachable!(),
            };
            let signature = ecdsa::Signature {
                signature: secp.sign_ecdsa(&Message::from_digest(digest), &secret()),
                sighash_type,
            }
            .to_vec();
            match kind {
                SpendKind::Legacy => {
                    tx.input[index].script_sig = Builder::new()
                        .push_slice(PushBytesBuf::try_from(signature).unwrap())
                        .push_key(&public)
                        .into_script()
                }
                SpendKind::Witness => {
                    tx.input[index].witness = Witness::from_slice(&[signature, public.to_bytes()])
                }
                _ => unreachable!(),
            }
        }
    }
}

pub fn block(transactions: Vec<Transaction>) -> Block {
    let mut block = bitcoincore_rpc::bitcoin::constants::genesis_block(Network::Regtest);
    block.txdata.extend(transactions);
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    block
}

pub fn verbose(height: u32, block: &Block, coins: &HashMap<OutPoint, SpentPrevout>) -> Value {
    json!({"height":height, "hash":block.block_hash(), "confirmations":100,
        "tx":block.txdata.iter().map(|tx| json!({"hex":consensus::encode::serialize_hex(tx),
            "vin":tx.input.iter().map(|input| {
                if tx.is_coinbase() { json!({"coinbase":"00"}) }
                else { let coin = &coins[&input.previous_output]; json!({
                    "txid":input.previous_output.txid, "vout":input.previous_output.vout,
                    "prevout":{"value":coin.txout.value.to_btc(), "height":coin.height,
                        "generated":coin.coinbase, "scriptPubKey":{"hex":coin.txout.script_pubkey.to_hex_string()}}}) }
            }).collect::<Vec<_>>()
        })).collect::<Vec<_>>()})
}

pub struct ChainState {
    pub blocks: HashMap<u32, (Block, Value)>,
    pub calls: Vec<(String, Value)>,
    pub reorg_after_verbose: bool,
}

pub struct ChainCore {
    pub state: Arc<Mutex<ChainState>>,
    pub client: Arc<BTCRpcClient>,
    _server: RpcServer,
}

impl ChainCore {
    pub fn new(blocks: Vec<(u32, Block, Value)>) -> Self {
        let state = Arc::new(Mutex::new(ChainState {
            blocks: blocks.into_iter().map(|(h, b, v)| (h, (b, v))).collect(),
            calls: Vec::new(),
            reorg_after_verbose: false,
        }));
        let shared = state.clone();
        let server = RpcServer::new(move |request| {
            let mut state = shared.lock().unwrap();
            let method = request["method"].as_str().unwrap();
            let params = &request["params"];
            state.calls.push((method.to_string(), params.clone()));
            let result = match method {
                "getblockhash" => state
                    .blocks
                    .get(&(params[0].as_u64().unwrap() as u32))
                    .map(|(b, _)| json!(b.block_hash())),
                "getblock" => state
                    .blocks
                    .values()
                    .find(|(b, _)| json!(b.block_hash()) == params[0])
                    .map(|(b, v)| {
                        if params[1] == 3 {
                            v.clone()
                        } else {
                            json!(consensus::encode::serialize_hex(b))
                        }
                    }),
                _ => None,
            };
            if method == "getblock" && params[1] == 3 && state.reorg_after_verbose {
                for (block, _) in state.blocks.values_mut() {
                    block.header.nonce += 1;
                }
                state.reorg_after_verbose = false;
            }
            let error = result.is_none().then(|| json!({"code":-5,"message":"Block unavailable; no txindex or live UTXO fallback"}));
            (
                200,
                json!({"result":result,"error":error,"id":request["id"]}),
            )
        });
        Self {
            client: Arc::new(
                BTCRpcClient::new(server.url.clone(), bitcoincore_rpc::Auth::None).unwrap(),
            ),
            state,
            _server: server,
        }
    }
}

/// Build a real TapScript commitment and matching script-path witness.
pub fn tap_script(script: ScriptBuf) -> (ScriptBuf, Witness) {
    let secp = Secp256k1::new();
    let internal = Keypair::from_secret_key(&secp, &secret())
        .x_only_public_key()
        .0;
    let spend = TaprootBuilder::new()
        .add_leaf(0, script.clone())
        .unwrap()
        .finalize(&secp, internal)
        .unwrap();
    let control = spend
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .unwrap()
        .serialize();
    (
        ScriptBuf::new_p2tr_tweaked(spend.output_key()),
        Witness::from_slice(&[script.as_bytes(), &control]),
    )
}
