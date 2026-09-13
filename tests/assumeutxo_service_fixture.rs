//! Real transactions for the isolated P6.5 service acceptance runner.

use bitcoincore_rpc::{
    Auth, Client, RpcApi,
    bitcoin::{
        Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Witness, absolute,
        consensus::encode::serialize_hex,
        opcodes::all::{OP_ENDIF, OP_IF},
        script::{Builder, PushBytesBuf},
        secp256k1::{Keypair, Secp256k1, SecretKey},
        taproot::{LeafVersion, TaprootBuilder},
        transaction,
    },
};
use serde_json::{Value, json};
use usdb_util::ToBtcScriptHash;

/// Generate a mint and two competing transfers on the known P5 checkpoint prefix.
#[test]
#[ignore = "Run only through tests/run_assumeutxo_p65_services.py"]
fn generate_assumeutxo_service_chain() {
    let core = Client::new(
        &std::env::var("P65_CORE_URL").unwrap(),
        Auth::CookieFile(std::env::var("P65_CORE_COOKIE").unwrap().into()),
    )
    .unwrap();
    assert_eq!(core.get_blockchain_info().unwrap().chain, Network::Regtest);
    assert_eq!(core.get_block_count().unwrap(), 0);
    assert_eq!(core.call::<Value>("getindexinfo", &[]).unwrap(), json!({}));
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/assumeutxo-p5/chain.json")).unwrap();
    for raw in fixture["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .take(102)
        .skip(1)
    {
        assert_eq!(
            core.call::<Value>("submitblock", std::slice::from_ref(raw))
                .unwrap(),
            Value::Null
        );
    }
    let base_hash = core.get_block_hash(101).unwrap();
    let coinbase = (1..=2)
        .find_map(|height| {
            let txid = core
                .get_block(&core.get_block_hash(height).unwrap())
                .unwrap()
                .txdata[0]
                .compute_txid();
            core.get_tx_out(&txid, 0, Some(false))
                .unwrap()
                .map(|output| (txid, output.value.to_sat()))
        })
        .expect("a mature unspent fixture coinbase is required");
    let secp = Secp256k1::new();
    let taproot = |seed, script: &ScriptBuf| {
        let internal =
            Keypair::from_secret_key(&secp, &SecretKey::from_slice(&[seed; 32]).unwrap())
                .x_only_public_key()
                .0;
        let spend = TaprootBuilder::new()
            .add_leaf(0, script.clone())
            .unwrap()
            .finalize(&secp, internal)
            .unwrap();
        let output = ScriptBuf::new_p2tr_tweaked(spend.output_key());
        let control = spend
            .control_block(&(script.clone(), LeafVersion::TapScript))
            .unwrap()
            .serialize();
        (output, Witness::from_slice(&[script.as_bytes(), &control]))
    };
    let body = PushBytesBuf::try_from(br#"{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":[]}"#.to_vec()).unwrap();
    let inscription = Builder::new()
        .push_int(0)
        .push_opcode(OP_IF)
        .push_slice(b"ord")
        .push_int(1)
        .push_slice(b"application/json")
        .push_int(0)
        .push_slice(body)
        .push_opcode(OP_ENDIF)
        .push_int(1)
        .into_script();
    let anyone = Builder::new().push_int(1).into_script();
    let (commit_script, commit_witness) = taproot(1, &inscription);
    let (owner_a, owner_a_witness) = taproot(2, &anyone);
    let (owner_b, _) = taproot(3, &anyone);
    let output = |value, script_pubkey| TxOut {
        value: Amount::from_sat(value),
        script_pubkey,
    };
    let tx = |input, output| Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input,
        output,
    };
    let input = |txid, vout, witness| TxIn {
        previous_output: OutPoint::new(txid, vout),
        witness,
        ..TxIn::default()
    };
    let funding = tx(
        vec![input(coinbase.0, 0, Witness::new())],
        vec![
            output(1_000_000_000, commit_script),
            output(100_000_000, anyone.clone()),
            output(coinbase.1 - 1_200_000_000, anyone.clone()),
        ],
    );
    let mint = tx(
        vec![
            input(funding.compute_txid(), 1, Witness::new()),
            input(funding.compute_txid(), 0, commit_witness),
        ],
        // Whole-BTC values also exercise nonzero integer energy growth after the fork.
        vec![
            output(50_000_000, anyone.clone()),
            output(900_000_000, owner_a.clone()),
        ],
    );
    let transfer = tx(
        vec![
            input(mint.compute_txid(), 0, Witness::new()),
            input(mint.compute_txid(), 1, owner_a_witness.clone()),
        ],
        vec![
            output(70_000_000, anyone.clone()),
            output(800_000_000, owner_b.clone()),
        ],
    );
    let fork_transfer = tx(
        vec![
            input(mint.compute_txid(), 0, Witness::new()),
            input(mint.compute_txid(), 1, owner_a_witness),
        ],
        vec![
            output(90_000_000, anyone),
            output(700_000_000, owner_a.clone()),
        ],
    );
    let mine = |transaction: &Transaction| {
        core.call::<Value>(
            "generateblock",
            &[json!("raw(51)"), json!([serialize_hex(transaction)])],
        )
        .unwrap();
    };
    mine(&funding);
    mine(&mint);
    mine(&transfer);
    core.call::<Value>("generatetodescriptor", &[json!(14), json!("raw(51)")])
        .unwrap();
    let blocks = |from, to| {
        (from..=to).map(|h| {
        let hash = core.get_block_hash(h).unwrap();
        json!({"height":h, "hash":hash, "raw":serialize_hex(&core.get_block(&hash).unwrap())})
    }).collect::<Vec<_>>()
    };
    let original = blocks(1, 118);
    core.invalidate_block(&core.get_block_hash(104).unwrap())
        .unwrap();
    mine(&fork_transfer);
    core.call::<Value>("generatetodescriptor", &[json!(15), json!("raw(51)")])
        .unwrap();
    let fork = blocks(104, 119);
    let result = json!({"base_hash":base_hash, "genesis_height":102, "blocks":original, "fork":fork,
        "pass_id":format!("{}i0",mint.compute_txid()),
        "mint_satpoint":format!("{}:1:50000000",mint.compute_txid()),
        "transfer_satpoint":format!("{}:1:30000000",transfer.compute_txid()),
        "fork_satpoint":format!("{}:1:10000000",fork_transfer.compute_txid()),
        "owner_a":owner_a.to_btc_script_hash(),
        "owner_b":owner_b.to_btc_script_hash()});
    std::fs::write(
        std::env::var("P65_CHAIN_RESULT").unwrap(),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}
