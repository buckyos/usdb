//! UIP-0016 evidence acceptance before connecting the new pass state machine.

#[path = "common/miner_pass_evidence.rs"]
mod fixtures;

use crate::btc::{
    UTXOValueManager,
    mint_evidence::{MintEvidenceContext, MintSatOutcome, MintSourceOutcome},
    transaction_balance::{BalanceBaseline, BalanceBaselineSide, TransactionBalanceContext},
};
use bitcoincore_rpc::bitcoin::{
    Amount, OutPoint, ScriptBuf, Txid,
    hashes::Hash,
    opcodes::all::{OP_ENDIF, OP_IF},
    script::Builder,
};
use fixtures::*;
use ord::InscriptionId;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use usdb_util::{SourceAuthorization, SpentPrevout, ToBtcScriptHash, prove_commit_source};

fn point(byte: u8) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([byte; 32]), 0)
}
fn coin(value: u64, script: ScriptBuf, height: u32) -> SpentPrevout {
    SpentPrevout {
        txout: output(value, script),
        height,
        coinbase: false,
    }
}

#[test]
fn source_forms_sighashes_and_annex_are_verified_not_guessed() {
    for kind in [SpendKind::Legacy, SpendKind::Witness, SpendKind::Taproot] {
        let flags: &[u8] = match kind {
            SpendKind::Taproot => &[0, 1, 2, 3, 0x81, 0x82, 0x83],
            _ => &[1, 2, 3, 0x81, 0x82, 0x83],
        };
        for &flag in flags {
            for annex in [false, true] {
                if annex && !matches!(kind, SpendKind::Taproot) {
                    continue;
                }
                let coins = HashMap::from([(point(1), coin(5000, source_script(kind), 5))]);
                let mut commit = transaction(
                    vec![point(1)],
                    vec![output(4000, ScriptBuf::from(vec![0x51]))],
                );
                sign(
                    &mut commit,
                    0,
                    &[coins[&point(1)].txout.clone()],
                    kind,
                    flag,
                    annex,
                );
                let outpoint = OutPoint::new(commit.compute_txid(), 0);
                let block = block(vec![commit.clone()]);
                let core = ChainCore::new(vec![(10, block.clone(), verbose(10, &block, &coins))]);
                let proof = prove_commit_source(
                    &core.client.get_block_prevouts(10, &block).unwrap(),
                    outpoint,
                    0,
                )
                .unwrap();
                assert_eq!(proof.source_owner, source_script(kind).to_btc_script_hash());
                if flag <= 1 {
                    assert!(
                        !matches!(proof.authorization, SourceAuthorization::Unsupported(_)),
                        "{kind:?} {flag} annex={annex}"
                    );
                    // A supported flag alone is insufficient when the committed outputs change.
                    commit.output[0].value = Amount::from_sat(3999);
                    let tampered = block_with(commit);
                    let bad = ChainCore::new(vec![(
                        10,
                        tampered.clone(),
                        verbose(10, &tampered, &coins),
                    )]);
                    let evidence = bad.client.get_block_prevouts(10, &tampered).unwrap();
                    assert!(
                        prove_commit_source(
                            &evidence,
                            OutPoint::new(tampered.txdata[1].compute_txid(), 0),
                            0
                        )
                        .is_err()
                    );
                } else {
                    assert!(matches!(
                        proof.authorization,
                        SourceAuthorization::Unsupported(_)
                    ));
                }
            }
        }
    }
}

fn block_with(tx: bitcoincore_rpc::bitcoin::Transaction) -> bitcoincore_rpc::bitcoin::Block {
    block(vec![tx])
}

#[test]
fn reverse_sat_mapping_uses_half_open_intervals_and_skips_zero_inputs() {
    for prefix in [700, 600] {
        let a = ScriptBuf::from(vec![0x51]);
        let d = source_script(SpendKind::Witness);
        let coins = HashMap::from([
            (point(1), coin(700, a.clone(), 1)),
            (point(2), coin(0, a.clone(), 1)),
            (point(3), coin(2300, d.clone(), 1)),
        ]);
        let mut commit = transaction(
            vec![point(1), point(2), point(3)],
            vec![
                output(prefix, a.clone()),
                output(1200, a.clone()),
                output(1700 - prefix, a),
            ],
        );
        let prevouts = commit
            .input
            .iter()
            .map(|i| coins[&i.previous_output].txout.clone())
            .collect::<Vec<_>>();
        sign(&mut commit, 2, &prevouts, SpendKind::Witness, 1, false);
        let outpoint = OutPoint::new(commit.compute_txid(), 1);
        let b = block(vec![commit]);
        let core = ChainCore::new(vec![(10, b.clone(), verbose(10, &b, &coins))]);
        let evidence = core.client.get_block_prevouts(10, &b).unwrap();
        let proof = prove_commit_source(&evidence, outpoint, 0).unwrap();
        assert_eq!(proof.input_index, if prefix == 700 { 2 } else { 0 });
        assert_eq!(proof.source_offset, if prefix == 700 { 0 } else { 600 });
        assert_eq!(
            prove_commit_source(&evidence, outpoint, 1199)
                .unwrap()
                .input_index,
            2
        );
        assert!(prove_commit_source(&evidence, outpoint, 1200).is_err());
        assert!(prove_commit_source(&evidence, OutPoint::new(outpoint.txid, 9), 0).is_err());
    }
}

fn envelope(pointer: bool) -> ScriptBuf {
    let mut script = Builder::new()
        .push_int(0)
        .push_opcode(OP_IF)
        .push_slice(b"ord")
        .push_slice([1])
        .push_slice(b"application/json");
    if pointer {
        script = script.push_slice([2]).push_slice([1]);
    }
    script
        .push_int(0)
        .push_slice(b"{\"p\":\"usdb\",\"op\":\"mint\",\"v\":2}")
        .push_opcode(OP_ENDIF)
        .into_script()
}

fn reveal_fixture(
    same_block: bool,
    pointer: bool,
) -> (
    ChainCore,
    u32,
    Arc<bitcoincore_rpc::bitcoin::Block>,
    InscriptionId,
) {
    let source = coin(5000, source_script(SpendKind::Witness), 2);
    let (commit_script, witness) = tap_script(envelope(pointer));
    let mut commit = transaction(vec![point(1)], vec![output(4000, commit_script)]);
    sign(
        &mut commit,
        0,
        std::slice::from_ref(&source.txout),
        SpendKind::Witness,
        1,
        false,
    );
    let commit_point = OutPoint::new(commit.compute_txid(), 0);
    let mut reveal = transaction(
        vec![point(2), commit_point],
        vec![
            output(50, ScriptBuf::from(vec![0x52])),
            output(4000, ScriptBuf::from(vec![0x53])),
        ],
    );
    reveal.input[1].witness = witness;
    let id = InscriptionId {
        txid: reveal.compute_txid(),
        index: 0,
    };
    let coins = HashMap::from([
        (point(1), source),
        (point(2), coin(100, ScriptBuf::from(vec![0x54]), 2)),
        (
            commit_point,
            SpentPrevout {
                txout: commit.output[0].clone(),
                height: 10,
                coinbase: false,
            },
        ),
    ]);
    let commit_block = block(vec![commit.clone()]);
    let mut reveal_block = block(if same_block {
        vec![commit, reveal]
    } else {
        vec![reveal]
    });
    let h = if same_block { 10 } else { 11 };
    if !same_block {
        reveal_block.header.prev_blockhash = commit_block.block_hash();
    }
    let mut blocks = vec![(h, reveal_block.clone(), verbose(h, &reveal_block, &coins))];
    if !same_block {
        blocks.push((10, commit_block.clone(), verbose(10, &commit_block, &coins)));
    }
    (ChainCore::new(blocks), h, Arc::new(reveal_block), id)
}

#[test]
fn historical_and_same_block_commit_before_index_origin_without_txindex() {
    for same_block in [false, true] {
        let (core, h, block, id) = reveal_fixture(same_block, false);
        let context = MintEvidenceContext::new(core.client.clone(), h, block);
        let MintSatOutcome::Located(sat) = context.locate_mint(id).unwrap() else {
            panic!("expected bound inscription");
        };
        assert_eq!(sat.satpoint.offset, 50); // First sat of input 1, not the first reveal input.
        let MintSourceOutcome::Proven(proof) = context.prove_source(&sat).unwrap() else {
            panic!();
        };
        assert_eq!(proof.source_outpoint, point(1));
        assert_eq!(proof.block_height, 10);
        assert_eq!(proof.authorization, SourceAuthorization::P2wpkhAll);
        assert_eq!(
            context.prove_source(&sat).unwrap(),
            MintSourceOutcome::Proven(proof)
        );
        assert!(
            core.state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(m, _)| m != "getrawtransaction" && m != "gettxout")
        );
    }
}

#[test]
fn missing_historical_undo_retries_and_reorg_invalidates_cached_receipts() {
    let (core, h, block, id) = reveal_fixture(false, false);
    let context = MintEvidenceContext::new(core.client.clone(), h, block);
    let MintSatOutcome::Located(sat) = context.locate_mint(id).unwrap() else {
        panic!();
    };
    let good = core.state.lock().unwrap().blocks[&10].1.clone();
    core.state.lock().unwrap().blocks.get_mut(&10).unwrap().1["tx"][1]["vin"][0]
        .as_object_mut()
        .unwrap()
        .remove("prevout");
    assert!(context.prove_source(&sat).unwrap_err().contains("undo"));
    core.state.lock().unwrap().blocks.get_mut(&10).unwrap().1 = good;
    context.prove_source(&sat).unwrap();
    core.state
        .lock()
        .unwrap()
        .blocks
        .get_mut(&h)
        .unwrap()
        .0
        .header
        .nonce += 1;
    assert!(
        context
            .prove_source(&sat)
            .unwrap_err()
            .contains("canonical")
    );
}

#[test]
fn full_evidence_rejects_missing_scripts_heights_same_block_mismatch_and_inflight_reorg() {
    let (core, h, block, _) = reveal_fixture(true, false);
    let good = core.state.lock().unwrap().blocks[&h].1.clone();
    for field in ["height", "generated", "scriptPubKey"] {
        let mut bad = good.clone();
        bad["tx"][1]["vin"][0]["prevout"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        core.state.lock().unwrap().blocks.get_mut(&h).unwrap().1 = bad;
        let inputs = UTXOValueManager::new(core.client.clone(), h, block.clone());
        assert!(inputs.get_prevouts().is_err());
        // The legacy values API still works, and retry loads complete evidence.
        assert!(core.client.get_block_input_values(h, &block).is_ok());
        core.state.lock().unwrap().blocks.get_mut(&h).unwrap().1 = good.clone();
        inputs.get_prevouts().unwrap();
    }
    let mut bad = good.clone();
    bad["tx"][2]["vin"][1]["prevout"]["scriptPubKey"]["hex"] = json!("52");
    core.state.lock().unwrap().blocks.get_mut(&h).unwrap().1 = bad;
    assert!(
        core.client
            .get_block_prevouts(h, &block)
            .unwrap_err()
            .contains("Same-block")
    );
    {
        let mut state = core.state.lock().unwrap();
        state.blocks.get_mut(&h).unwrap().1 = good;
        state.reorg_after_verbose = true;
    }
    assert!(
        core.client
            .get_block_prevouts(h, &block)
            .unwrap_err()
            .contains("changed during")
    );
}

#[test]
fn pointer_is_deterministic_unsupported_and_not_an_availability_error() {
    let (core, h, block, id) = reveal_fixture(false, true);
    assert!(matches!(
        MintEvidenceContext::new(core.client, h, block)
            .locate_mint(id)
            .unwrap(),
        MintSatOutcome::Unsupported(_)
    ));
}

#[test]
fn all_transactions_update_balance_before_reveal_and_post_floor_recovers_same_context() {
    let e = ScriptBuf::from(vec![0x51]);
    let d = ScriptBuf::from(vec![0x52]);
    let owner = e.to_btc_script_hash();
    let before_coin = coin(1, e.clone(), 2);
    let spend = transaction(vec![point(1)], vec![output(1, d.clone())]);
    let deposit = transaction(vec![point(2)], vec![output(1, e.clone())]);
    let reveal = transaction(
        vec![OutPoint::new(deposit.compute_txid(), 0)],
        vec![output(1, e.clone())],
    );
    let coins = HashMap::from([
        (point(1), before_coin),
        (point(2), coin(1, d, 2)),
        (
            OutPoint::new(deposit.compute_txid(), 0),
            coin(1, e.clone(), 10),
        ),
    ]);
    let b = block(vec![spend, deposit, reveal]);
    let core = ChainCore::new(vec![(10, b.clone(), verbose(10, &b, &coins))]);
    let inputs = core.client.get_block_prevouts(10, &b).unwrap();
    for side in [
        BalanceBaselineSide::BeforeBlock,
        BalanceBaselineSide::AfterBlock,
    ] {
        let context = TransactionBalanceContext::from_baseline(
            &inputs,
            BalanceBaseline {
                side,
                height: if side == BalanceBaselineSide::BeforeBlock {
                    9
                } else {
                    10
                },
                block_hash: if side == BalanceBaselineSide::BeforeBlock {
                    b.header.prev_blockhash
                } else {
                    b.block_hash()
                },
                balances: HashMap::from([(owner, 1)]),
            },
        )
        .unwrap();
        assert_eq!(
            (0..4)
                .map(|i| context.balance_before(&owner, i).unwrap())
                .collect::<Vec<_>>(),
            vec![1, 1, 0, 1]
        );
        assert_eq!(context.balance_before(&owner, 3).unwrap(), 1); // Current reveal output is excluded.
        assert!(
            context
                .balance_before(&point(9).txid.to_string().parse().unwrap(), 0)
                .is_err()
        );
        assert!(context.balance_before(&owner, 4).is_err());
    }
    let impossible = BalanceBaseline {
        side: BalanceBaselineSide::BeforeBlock,
        height: 9,
        block_hash: b.header.prev_blockhash,
        balances: HashMap::from([(owner, 0)]),
    };
    assert!(TransactionBalanceContext::from_baseline(&inputs, impossible).is_err());
}

#[tokio::test]
#[ignore = "Run only through tests/run_miner_pass_evidence_live.py against a fresh isolated Core"]
async fn real_core_miner_pass_source_evidence_without_txindex() {
    use bitcoincore_rpc::bitcoin::{Network, consensus};
    use bitcoincore_rpc::{Auth, Client, RpcApi};
    let url = std::env::var("MPE_CORE_URL").expect("isolated runner RPC required");
    let cookie = std::path::PathBuf::from(std::env::var("MPE_CORE_COOKIE").unwrap());
    let core = Client::new(&url, Auth::CookieFile(cookie.clone())).unwrap();
    assert_eq!(core.get_blockchain_info().unwrap().chain, Network::Regtest);
    assert_eq!(
        core.get_block_count().unwrap(),
        0,
        "only a fresh isolated node is allowed"
    );
    assert_eq!(
        core.call::<serde_json::Value>("getindexinfo", &[]).unwrap(),
        json!({})
    );
    core.call::<serde_json::Value>("generatetodescriptor", &[json!(100), json!("raw(51)")])
        .unwrap();
    let first = core
        .get_block(&core.get_block_hash(1).unwrap())
        .unwrap()
        .txdata[0]
        .compute_txid();
    let anyone = ScriptBuf::from(vec![0x51]);
    let mut inscription_script = envelope(false).into_bytes();
    inscription_script.push(0x51); // A valid, anyone-can-reveal inscription TapScript.
    let (commit_script, reveal_witness) = tap_script(ScriptBuf::from(inscription_script));
    let (script_path_source, script_path_witness) = tap_script(anyone.clone());
    let cases = [
        (SpendKind::Legacy, 1, false, false),
        (SpendKind::Witness, 1, false, false),
        (SpendKind::Taproot, 0, false, false),
        (SpendKind::Taproot, 1, true, false),
        (SpendKind::Legacy, 0x81, false, false),
        (SpendKind::Witness, 2, false, false),
        (SpendKind::Taproot, 3, false, false),
        (SpendKind::Taproot, 0, false, true),
    ];
    let mut funded = Vec::new();
    for &(kind, _, _, script_path) in &cases {
        funded.extend([
            output(700, anyone.clone()),
            output(0, anyone.clone()),
            output(
                5000,
                if script_path {
                    script_path_source.clone()
                } else {
                    source_script(kind)
                },
            ),
        ]);
    }
    funded.push(output(
        5_000_000_000 - cases.len() as u64 * 5700 - 1000,
        anyone.clone(),
    ));
    let funding = transaction(vec![OutPoint::new(first, 0)], funded);
    let mine = |transactions: &[bitcoincore_rpc::bitcoin::Transaction]| {
        core.call::<serde_json::Value>(
            "generateblock",
            &[
                json!("raw(51)"),
                json!(
                    transactions
                        .iter()
                        .map(consensus::encode::serialize_hex)
                        .collect::<Vec<_>>()
                ),
            ],
        )
        .unwrap();
    };
    mine(std::slice::from_ref(&funding));
    let mut commits = Vec::new();
    let mut reveals = Vec::new();
    for (i, &(kind, flag, annex, script_path)) in cases.iter().enumerate() {
        let mut commit = transaction(
            (0..3)
                .map(|j| OutPoint::new(funding.compute_txid(), (i * 3 + j) as u32))
                .collect(),
            vec![
                output(700, anyone.clone()),
                output(4000, commit_script.clone()),
                output(900, anyone.clone()),
            ],
        );
        if script_path {
            commit.input[2].witness = script_path_witness.clone();
        } else {
            sign(
                &mut commit,
                2,
                &funding.output[i * 3..i * 3 + 3],
                kind,
                flag,
                annex,
            );
        }
        let mut reveal = transaction(
            vec![OutPoint::new(commit.compute_txid(), 1)],
            vec![output(3900, source_script(SpendKind::Witness))],
        );
        reveal.input[0].witness = reveal_witness.clone();
        commits.push(commit);
        reveals.push(reveal);
    }
    // Historical commits at H=102; the last case has same-block commit/reveal at H=103.
    mine(&commits[..commits.len() - 1]);
    let mut last_block = vec![commits.last().unwrap().clone()];
    last_block.extend(reveals.clone());
    mine(&last_block);
    let height = 103;
    let hash = core.get_block_hash(height).unwrap();
    let block = Arc::new(core.get_block(&hash).unwrap());
    assert!(
        core.get_raw_transaction(&funding.compute_txid(), None)
            .is_err()
    );
    assert!(
        core.get_tx_out(&funding.compute_txid(), 2, Some(false))
            .unwrap()
            .is_none()
    );
    let client = Arc::new(usdb_util::BTCRpcClient::new(url, Auth::CookieFile(cookie)).unwrap());
    let mut observations = Vec::new();
    for _restart in 0..2 {
        let context = MintEvidenceContext::new(client.clone(), height as u32, block.clone());
        let mut pass = Vec::new();
        for (i, reveal) in reveals.iter().enumerate() {
            let id = InscriptionId {
                txid: reveal.compute_txid(),
                index: 0,
            };
            let MintSatOutcome::Located(sat) = context.locate_mint(id).unwrap() else {
                panic!("supported reveal required");
            };
            let MintSourceOutcome::Proven(proof) = context.prove_source(&sat).unwrap() else {
                panic!();
            };
            assert_eq!(proof.input_index, 2);
            assert_eq!(proof.source_offset, 0);
            assert_eq!(
                proof.source_outpoint,
                OutPoint::new(funding.compute_txid(), (i * 3 + 2) as u32)
            );
            assert_eq!(
                matches!(proof.authorization, SourceAuthorization::Unsupported(_)),
                i >= 4
            );
            pass.push(json!({"inscription":id.to_string(), "commit_height":proof.block_height,
                "source":proof.source_owner.to_string(), "authorization":format!("{:?}",proof.authorization)}));
        }
        observations.push(pass);
    }
    assert_eq!(observations[0], observations[1]);
    // Disconnect the reveal, remove it from the canonical branch, and mine an empty replacement.
    core.invalidate_block(&hash).unwrap();
    core.call::<serde_json::Value>("generateblock", &[json!("raw(51)"), json!([])])
        .unwrap();
    let stale = MintEvidenceContext::new(client.clone(), height as u32, block);
    assert!(stale.block_prevouts().is_err());
    core.reconsider_block(&hash).unwrap();
    let result = json!({"status":"pass", "height":height, "hash":hash, "cases":cases.len(),
        "txindex":false,"actual_zero_value_input":true,"historical_and_same_block":true,
        "reopen_matches":true,"reorg_rejected":true,"observations":observations[0],
        "raw_transactions":{"funding":consensus::encode::serialize_hex(&funding),
            "commits":commits.iter().map(consensus::encode::serialize_hex).collect::<Vec<_>>(),
            "reveals":reveals.iter().map(consensus::encode::serialize_hex).collect::<Vec<_>>()}});
    std::fs::write(
        std::env::var("MPE_LIVE_RESULT").unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn balance_rpc_uses_snapshot_floor_and_rejects_missing_or_changed_evidence() {
    use fixtures::RpcServer;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let e = ScriptBuf::from(vec![0x51]);
    let owner = e.to_btc_script_hash();
    let tx = transaction(vec![point(1)], vec![output(1, e)]);
    let coins = HashMap::from([(point(1), coin(1, ScriptBuf::from(vec![0x52]), 2))]);
    let b = block(vec![tx]);
    let core = ChainCore::new(vec![(10, b.clone(), verbose(10, &b, &coins))]);
    let inputs = core.client.get_block_prevouts(10, &b).unwrap();
    let identity = usdb_util::ConsensusSnapshotIdentity {
        source_chain: "btc".into(),
        network: "regtest".into(),
        stable_height: 10,
        stable_block_hash: b.block_hash().to_string(),
        stable_lag: 0,
        balance_history_api_version: "1.0.0".into(),
        balance_history_semantics_version: "1".into(),
    };
    let reference = balance_history::HistoricalSnapshotStateRef {
        block_height: 10,
        stable_block_hash: b.block_hash().to_string(),
        latest_block_commit: "11".repeat(32),
        snapshot_id: usdb_util::build_consensus_snapshot_id(&identity),
        consensus_identity: identity,
        snapshot_id_hash_algo: usdb_util::CONSENSUS_SNAPSHOT_ID_HASH_ALGO.into(),
        snapshot_id_version: usdb_util::CONSENSUS_SNAPSHOT_ID_VERSION.into(),
        commit_protocol_version: "1.0.0".into(),
        commit_hash_algo: "sha256".into(),
    };
    for scenario in [
        "success",
        "empty",
        "missing_owner",
        "future",
        "coverage",
        "reorg",
        "wrong_hash",
        "wrong_id",
    ] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let reference = serde_json::to_value(&reference).unwrap();
        let server = RpcServer::new(move |request| {
            assert_eq!(
                request["params"][0]["block_height"], 10,
                "never query below the snapshot floor"
            );
            let mut error = serde_json::Value::Null;
            let result = match request["method"].as_str().unwrap() {
                "get_state_ref_at_height" => {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    let mut reference = reference.clone();
                    if scenario == "reorg" && n > 0 {
                        reference["latest_block_commit"] = json!("22".repeat(32));
                    }
                    if scenario == "wrong_hash" {
                        reference["stable_block_hash"] = json!("22".repeat(32));
                    }
                    if scenario == "wrong_id" {
                        reference["snapshot_id"] = json!("33".repeat(32));
                    }
                    reference
                }
                "get_addresses_balances" => match scenario {
                    "empty" => json!([[]]),
                    "missing_owner" => json!([]),
                    "future" => json!([[{"block_height":11,"balance":1,"delta":1}]]),
                    "coverage" => {
                        error = json!({"code":-32001,"message":"history unavailable"});
                        serde_json::Value::Null
                    }
                    _ => json!([[{"block_height":10,"balance":1,"delta":1}]]),
                },
                method => panic!("unexpected method: {method}"),
            };
            (
                200,
                json!({"result":result,"error":error,"id":request["id"]}),
            )
        });
        let history = balance_history::RpcClient::new(&server.url).unwrap();
        let result = TransactionBalanceContext::load_at_block_end(
            &core.client,
            &history,
            &inputs,
            vec![owner],
        )
        .await;
        if scenario == "success" {
            assert_eq!(result.unwrap().balance_before(&owner, 1).unwrap(), 0);
            assert_eq!(count.load(Ordering::SeqCst), 2);
        } else {
            assert!(
                result.is_err(),
                "{scenario} must not turn into a zero balance"
            );
        }
    }
}

#[test]
fn coinbase_and_unspendable_outputs_follow_balance_history_accounting() {
    let spendable = ScriptBuf::from(vec![0x51]);
    let unspendable = ScriptBuf::from(vec![0x6a]);
    let tx = transaction(vec![point(1)], vec![output(1, unspendable.clone())]);
    let coins = HashMap::from([(point(1), coin(1, spendable.clone(), 2))]);
    let mut b = block(vec![tx]);
    b.txdata[0].output = vec![output(5000, spendable.clone())];
    b.header.merkle_root = b.compute_merkle_root().unwrap();
    let core = ChainCore::new(vec![(10, b.clone(), verbose(10, &b, &coins))]);
    let inputs = core.client.get_block_prevouts(10, &b).unwrap();
    let context = TransactionBalanceContext::from_baseline(
        &inputs,
        BalanceBaseline {
            side: BalanceBaselineSide::AfterBlock,
            height: 10,
            block_hash: b.block_hash(),
            balances: HashMap::from([
                (spendable.to_btc_script_hash(), 5000),
                (unspendable.to_btc_script_hash(), 0),
            ]),
        },
    )
    .unwrap();
    assert_eq!(
        context
            .balance_before(&spendable.to_btc_script_hash(), 0)
            .unwrap(),
        1
    );
    assert_eq!(
        context
            .balance_before(&spendable.to_btc_script_hash(), 1)
            .unwrap(),
        5001
    );
    assert_eq!(
        context
            .balance_before(&unspendable.to_btc_script_hash(), 1)
            .unwrap(),
        0
    );
}

#[test]
fn envelope_subset_rejects_ambiguous_unbound_and_burned_sats() {
    let basic = || {
        Builder::new()
            .push_int(0)
            .push_opcode(OP_IF)
            .push_slice(b"ord")
    };
    let finish = |b: Builder| {
        b.push_int(0)
            .push_slice(b"{}")
            .push_opcode(OP_ENDIF)
            .into_script()
    };
    let duplicate = finish(
        basic()
            .push_slice([1])
            .push_slice(b"text/plain")
            .push_slice([1])
            .push_slice(b"text/plain"),
    );
    let unknown_even = finish(basic().push_slice([66]).push_slice([1]));
    let pushnum = finish(basic().push_int(1).push_slice(b"text/plain"));
    let mut multiple = envelope(false).into_bytes();
    multiple.extend(envelope(false).into_bytes());
    for (name, script, value, output_value, destination) in [
        ("duplicate", duplicate, 4000, 3900, vec![0x51]),
        ("unknown_even", unknown_even, 4000, 3900, vec![0x51]),
        ("pushnum", pushnum, 4000, 3900, vec![0x51]),
        (
            "multiple",
            ScriptBuf::from(multiple),
            4000,
            3900,
            vec![0x51],
        ),
        ("zero", envelope(false), 0, 0, vec![0x51]),
        ("fees", envelope(false), 4000, 0, vec![0x51]),
        ("burn", envelope(false), 4000, 3900, vec![0x6a]),
    ] {
        let (commit_script, witness) = tap_script(script);
        let mut reveal = transaction(
            vec![point(1)],
            vec![output(output_value, ScriptBuf::from(destination))],
        );
        reveal.input[0].witness = witness;
        let id = InscriptionId {
            txid: reveal.compute_txid(),
            index: 0,
        };
        let coins = HashMap::from([(point(1), coin(value, commit_script, 2))]);
        let b = block(vec![reveal]);
        let core = ChainCore::new(vec![(10, b.clone(), verbose(10, &b, &coins))]);
        let context = MintEvidenceContext::new(core.client, 10, Arc::new(b));
        assert!(
            matches!(
                context.locate_mint(id).unwrap(),
                MintSatOutcome::Unsupported(_)
            ),
            "{name}"
        );
    }
}

#[test]
fn coinbase_commit_is_a_deterministic_non_authorizing_source() {
    let (commit_script, witness) = tap_script(envelope(false));
    let mut commit_block = block(Vec::new());
    commit_block.txdata[0].output = vec![output(5000, commit_script)];
    commit_block.header.merkle_root = commit_block.compute_merkle_root().unwrap();
    let point = OutPoint::new(commit_block.txdata[0].compute_txid(), 0);
    let mut reveal = transaction(vec![point], vec![output(4000, ScriptBuf::from(vec![0x51]))]);
    reveal.input[0].witness = witness;
    let id = InscriptionId {
        txid: reveal.compute_txid(),
        index: 0,
    };
    let b = block(vec![reveal]);
    let coins = HashMap::from([(
        point,
        SpentPrevout {
            txout: commit_block.txdata[0].output[0].clone(),
            height: 10,
            coinbase: true,
        },
    )]);
    let core = ChainCore::new(vec![
        (
            10,
            commit_block.clone(),
            verbose(10, &commit_block, &HashMap::new()),
        ),
        (110, b.clone(), verbose(110, &b, &coins)),
    ]);
    let context = MintEvidenceContext::new(core.client, 110, Arc::new(b));
    let MintSatOutcome::Located(sat) = context.locate_mint(id).unwrap() else {
        panic!()
    };
    assert_eq!(
        context.prove_source(&sat).unwrap(),
        MintSourceOutcome::CoinbaseCommit
    );
}
