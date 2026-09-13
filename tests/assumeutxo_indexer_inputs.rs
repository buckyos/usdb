//! P6.4 production input/reveal/transfer paths without a transaction index or live historical UTXOs.

#[path = "common/block_input_rpc.rs"]
mod block_input_rpc;

use std::collections::HashMap;
use std::sync::Arc;

use bitcoincore_rpc::bitcoin::{
    Amount, Block, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    consensus,
    hashes::Hash,
    opcodes::all::{OP_ENDIF, OP_IF},
    script::Builder,
    transaction,
};
use ord::InscriptionId;
use ordinals::SatPoint;
use serde_json::json;
use usdb_util::ToBtcScriptHash;

use crate::btc::{TxItem, UTXOValueManager};
use crate::config::{ConfigManager, IndexerConfig};
use crate::index::transfer::InscriptionTransferTracker;
use crate::storage::MinerPassStorage;
use block_input_rpc::{CoreStub, verbose_block};

fn fixture() -> (Arc<Block>, HashMap<OutPoint, Amount>, InscriptionId) {
    let old = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let commit = OutPoint::new(Txid::from_byte_array([2; 32]), 1);
    let script = Builder::new()
        .push_int(0)
        .push_opcode(OP_IF)
        .push_slice(b"ord")
        .push_int(1)
        .push_slice(b"text/plain")
        .push_int(0)
        .push_slice(b"p64")
        .push_opcode(OP_ENDIF)
        .into_script();
    let reveal = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: old,
                ..TxIn::default()
            },
            TxIn {
                previous_output: commit,
                witness: Witness::from_slice(&[script.as_bytes(), &[0xc0; 33]]),
                ..TxIn::default()
            },
        ],
        output: vec![
            TxOut {
                value: Amount::from_sat(500),
                script_pubkey: ScriptBuf::from(vec![0x51]),
            },
            TxOut {
                value: Amount::from_sat(9000),
                script_pubkey: ScriptBuf::from(vec![0x52]),
            },
        ],
    };
    let id = InscriptionId {
        txid: reveal.compute_txid(),
        index: 0,
    };
    let prefix = OutPoint::new(id.txid, 0);
    let minted = OutPoint::new(id.txid, 1);
    let transfer = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: prefix,
                ..TxIn::default()
            },
            TxIn {
                previous_output: minted,
                ..TxIn::default()
            },
        ],
        output: vec![
            TxOut {
                value: Amount::from_sat(700),
                script_pubkey: ScriptBuf::from(vec![0x53]),
            },
            TxOut {
                value: Amount::from_sat(8000),
                script_pubkey: ScriptBuf::from(vec![0x54]),
            },
        ],
    };
    let mut block = bitcoincore_rpc::bitcoin::constants::genesis_block(Network::Regtest);
    block.txdata.extend([reveal, transfer]);
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    let values = [(old, 1000), (commit, 9000), (prefix, 500), (minted, 9000)]
        .into_iter()
        .map(|(point, amount)| (point, Amount::from_sat(amount)))
        .collect();
    (Arc::new(block), values, id)
}

fn tracker(core: &CoreStub) -> (InscriptionTransferTracker, std::path::PathBuf) {
    open_tracker(&core.url, usdb_util::BTCAuth::None)
}

fn open_tracker(
    url: &str,
    auth: usdb_util::BTCAuth,
) -> (InscriptionTransferTracker, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "usdb-p64-inputs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut config = IndexerConfig::default();
    config.bitcoin.network = Network::Regtest;
    config.bitcoin.rpc_url = Some(url.to_string());
    config.bitcoin.auth = Some(auth);
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let config = Arc::new(ConfigManager::load(Some(root.clone())).unwrap());
    let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
    (
        InscriptionTransferTracker::new(config, storage).unwrap(),
        root,
    )
}

#[tokio::test]
async fn mint_and_same_block_transfer_use_spent_inputs_without_txindex() {
    let (block, values, id) = fixture();
    let core = CoreStub::new(&block, verbose_block(963800, &block, &values));
    let (tracker, root) = tracker(&core);
    let created = tracker
        .calc_create_satpoint(&id, 963800, block.clone())
        .await
        .unwrap();
    assert_eq!(
        created.satpoint,
        SatPoint {
            outpoint: OutPoint::new(id.txid, 1),
            offset: 500
        }
    );
    tracker
        .add_new_inscription(id, created.address.unwrap(), created.satpoint)
        .await
        .unwrap();
    let moves = tracker
        .process_block_with_hint(963800, Some(block.clone()), Vec::new())
        .await
        .unwrap();
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].satpoint.offset, 300);
    assert_eq!(
        moves[0].to_address,
        Some(block.txdata[2].output[1].script_pubkey.to_btc_script_hash())
    );
    tracker.rollback_staged_block(963800).unwrap();
    let retry = tracker
        .process_block_with_hint(963800, Some(block), Vec::new())
        .await
        .unwrap();
    assert_eq!(retry[0].satpoint, moves[0].satpoint);
    tracker.commit_staged_block(963800).unwrap();
    let calls = core.state.lock().unwrap().calls.clone();
    assert_eq!(calls.iter().filter(|s| *s == "getblock").count(), 1);
    assert!(
        !calls
            .iter()
            .any(|s| s == "getrawtransaction" || s == "gettxout")
    );
    drop(tracker);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn missing_undo_is_retryable_and_no_partial_values_are_cached() {
    let (block, values, _) = fixture();
    let response = verbose_block(963800, &block, &values);
    let mut missing = response.clone();
    missing["tx"][1]["vin"][1]
        .as_object_mut()
        .unwrap()
        .remove("prevout");
    let core = CoreStub::new(&block, missing);
    let context = UTXOValueManager::new(core.client(), 963800, block.clone());
    let point = block.txdata[1].input[0].previous_output;
    assert!(
        context
            .get_utxo(&point)
            .await
            .unwrap_err()
            .contains("undo/prevout unavailable")
    );
    core.state.lock().unwrap().verbose = response;
    assert_eq!(context.get_utxo(&point).await.unwrap().to_sat(), 1000);
    assert!(context.get_utxo(&OutPoint::null()).await.is_err());
}

#[test]
fn reject_wrong_block_transaction_input_amount_and_inflight_reorg() {
    let (block, values, _) = fixture();
    let response = verbose_block(963800, &block, &values);
    let core = CoreStub::new(&block, response.clone());
    let client = core.client();
    let mut invalids = Vec::new();
    for (key, value) in [
        ("hash", json!("00".repeat(32))),
        ("height", json!(963799)),
        ("confirmations", json!(-1)),
    ] {
        let mut invalid = response.clone();
        invalid[key] = value;
        invalids.push(invalid);
    }
    let mut wrong_tx = response.clone();
    wrong_tx["tx"][1]["hex"] = json!(consensus::encode::serialize_hex(&block.txdata[2]));
    invalids.push(wrong_tx);
    let mut wrong_input = response.clone();
    wrong_input["tx"][1]["vin"][0]["vout"] = json!(99);
    invalids.push(wrong_input);
    for amount in [json!(-1), json!(true), json!(0.000000001), json!(21000001)] {
        let mut invalid = response.clone();
        invalid["tx"][1]["vin"][0]["prevout"]["value"] = amount;
        invalids.push(invalid);
    }
    for invalid in invalids {
        core.state.lock().unwrap().verbose = invalid;
        assert!(client.get_block_input_values(963800, &block).is_err());
    }
    {
        let mut state = core.state.lock().unwrap();
        state.verbose = response;
        state.reorg_after_verbose = true;
    }
    assert!(
        client
            .get_block_input_values(963800, &block)
            .unwrap_err()
            .contains("changed during retrieval")
    );
}

#[tokio::test]
async fn block_context_cannot_reuse_values_on_a_replacement_branch_or_missing_reveal() {
    let (block, values, id) = fixture();
    let core = CoreStub::new(&block, verbose_block(963800, &block, &values));
    let (tracker, root) = tracker(&core);
    tracker
        .calc_create_satpoint(&id, 963800, block.clone())
        .await
        .unwrap();
    let mut fork = (*block).clone();
    fork.header.nonce += 1;
    let mut replacement_values = values;
    replacement_values.insert(
        block.txdata[1].input[0].previous_output,
        Amount::from_sat(1100),
    );
    {
        let mut state = core.state.lock().unwrap();
        state.verbose = verbose_block(963800, &fork, &replacement_values);
        state.canonical_hash = fork.block_hash().to_string();
    }
    assert!(
        tracker
            .calc_create_satpoint(&id, 963800, block)
            .await
            .is_err()
    );
    let created = tracker
        .calc_create_satpoint(&id, 963800, Arc::new(fork.clone()))
        .await
        .unwrap();
    assert_eq!(created.satpoint.offset, 600);
    let unknown = InscriptionId {
        txid: Txid::from_byte_array([9; 32]),
        index: 0,
    };
    assert!(
        tracker
            .calc_create_satpoint(&unknown, 963800, Arc::new(fork))
            .await
            .err()
            .unwrap()
            .contains("Reveal transaction absent")
    );
    drop(tracker);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn zero_valued_spent_input_and_fee_burn_mapping_remain_valid() {
    let (block, mut values, _) = fixture();
    values.insert(block.txdata[1].input[0].previous_output, Amount::ZERO);
    values.insert(
        block.txdata[1].input[1].previous_output,
        Amount::from_sat(10000),
    );
    let core = CoreStub::new(&block, verbose_block(963800, &block, &values));
    let context = UTXOValueManager::new(core.client(), 963800, block.clone());
    let result = TxItem::from_tx(block.txdata[1].clone())
        .calc_output_satpoint(
            SatPoint {
                outpoint: block.txdata[1].input[1].previous_output,
                offset: 9999,
            },
            &context,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.address, None);
}

#[tokio::test]
#[ignore = "Run with tests/run_assumeutxo_p64_live.py against its fresh isolated Core node"]
async fn real_core_spent_prevouts_reveal_and_restart_without_txindex() {
    use bitcoincore_rpc::bitcoin::{
        secp256k1::{Keypair, Secp256k1, SecretKey},
        taproot::{LeafVersion, TaprootBuilder},
    };
    use bitcoincore_rpc::{Client, RpcApi};
    let url = std::env::var("P64_CORE_URL").expect("isolated runner RPC required");
    let cookie = std::path::PathBuf::from(std::env::var("P64_CORE_COOKIE").unwrap());
    let core = Client::new(&url, bitcoincore_rpc::Auth::CookieFile(cookie.clone())).unwrap();
    assert_eq!(core.get_blockchain_info().unwrap().chain, Network::Regtest);
    assert_eq!(
        core.get_block_count().unwrap(),
        0,
        "only an empty isolated regtest node is allowed"
    );
    let indexes: serde_json::Value = core.call("getindexinfo", &[]).unwrap();
    assert_eq!(indexes, json!({}));
    let _: serde_json::Value = core
        .call("generatetodescriptor", &[json!(100), json!("raw(51)")])
        .unwrap();
    let first = core
        .get_block(&core.get_block_hash(1).unwrap())
        .unwrap()
        .txdata[0]
        .compute_txid();
    let secp = Secp256k1::new();
    let internal = Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[1; 32]).unwrap())
        .x_only_public_key()
        .0;
    let script = Builder::new()
        .push_int(0)
        .push_opcode(OP_IF)
        .push_slice(b"ord")
        .push_int(1)
        .push_slice(b"text/plain")
        .push_int(0)
        .push_slice(b"p64")
        .push_opcode(OP_ENDIF)
        .push_int(1)
        .into_script();
    let spend = TaprootBuilder::new()
        .add_leaf(0, script.clone())
        .unwrap()
        .finalize(&secp, internal)
        .unwrap();
    let control = spend
        .control_block(&(script.clone(), LeafVersion::TapScript))
        .unwrap()
        .serialize();
    let output = |amount, script| TxOut {
        value: Amount::from_sat(amount),
        script_pubkey: script,
    };
    let anyone = ScriptBuf::from(vec![0x51]);
    let funding = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(first, 0),
            ..TxIn::default()
        }],
        output: vec![
            output(10000, ScriptBuf::new_p2tr_tweaked(spend.output_key())),
            output(1000, anyone.clone()),
            output(4_999_988_000, anyone.clone()),
        ],
    };
    let _: serde_json::Value = core
        .call(
            "generateblock",
            &[
                json!("raw(51)"),
                json!([consensus::encode::serialize_hex(&funding)]),
            ],
        )
        .unwrap();
    assert_eq!(core.get_block_count().unwrap(), 101);
    let prefix = OutPoint::new(funding.compute_txid(), 1);
    let reveal = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: prefix,
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(funding.compute_txid(), 0),
                witness: Witness::from_slice(&[script.as_bytes(), &control]),
                ..TxIn::default()
            },
        ],
        output: vec![output(500, anyone.clone()), output(9000, anyone.clone())],
    };
    let id = InscriptionId {
        txid: reveal.compute_txid(),
        index: 0,
    };
    let transfer = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![
            TxIn {
                previous_output: OutPoint::new(id.txid, 0),
                ..TxIn::default()
            },
            TxIn {
                previous_output: OutPoint::new(id.txid, 1),
                ..TxIn::default()
            },
        ],
        output: vec![output(700, anyone.clone()), output(8000, anyone)],
    };
    let _: serde_json::Value = core
        .call(
            "generateblock",
            &[
                json!("raw(51)"),
                json!([
                    consensus::encode::serialize_hex(&reveal),
                    consensus::encode::serialize_hex(&transfer)
                ]),
            ],
        )
        .unwrap();
    let hash = core.get_block_hash(102).unwrap();
    let block = Arc::new(core.get_block(&hash).unwrap());
    let _: serde_json::Value = core
        .call("generatetodescriptor", &[json!(2050), json!("raw(51)")])
        .unwrap();
    assert!(
        core.get_raw_transaction(&funding.compute_txid(), None)
            .is_err()
    );
    assert!(
        core.get_tx_out(&prefix.txid, prefix.vout, Some(false))
            .unwrap()
            .is_none()
    );
    let bound_funding = core
        .get_raw_transaction(
            &funding.compute_txid(),
            Some(&core.get_block_hash(101).unwrap()),
        )
        .unwrap();
    assert_eq!(bound_funding.output[1].value.to_sat(), 1000);
    let mut observations = Vec::new();
    for _ in 0..2 {
        let (tracker, root) = open_tracker(&url, usdb_util::BTCAuth::CookieFile(cookie.clone()));
        let created = tracker
            .calc_create_satpoint(&id, 102, block.clone())
            .await
            .unwrap();
        assert_eq!(created.satpoint.offset, 500);
        tracker
            .add_new_inscription(id, created.address.unwrap(), created.satpoint)
            .await
            .unwrap();
        let moves = tracker
            .process_block_with_hint(102, Some(block.clone()), Vec::new())
            .await
            .unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(
            moves[0].satpoint,
            SatPoint {
                outpoint: OutPoint::new(transfer.compute_txid(), 1),
                offset: 300
            }
        );
        observations.push(json!({"created":created.satpoint.to_string(),"transferred":moves[0].satpoint.to_string()}));
        tracker.commit_staged_block(102).unwrap();
        drop(tracker);
        std::fs::remove_dir_all(root).unwrap();
    }
    assert_eq!(observations[0], observations[1]);
    let result = json!({"status":"pass","core_tip":core.get_block_count().unwrap(),"height":102,"hash":hash,"indexes":indexes,
        "txindex_lookup_failed":true,"old_output_is_spent":true,"lag_blocks":2050,"observations":observations});
    std::fs::write(
        std::env::var("P64_LIVE_RESULT").unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
