//! Activated parser, real ordered block execution, durable audit and cross-store recovery.
#[path = "common/miner_pass_pipeline.rs"]
mod pipeline;
use crate::index::MinerPassState;
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use crate::service::rpc::{GetPassMintAuditParams, UsdbIndexerRpc};
use pipeline::*;

#[tokio::test]
async fn pipeline_rejects_withdrawn_schema_v2_and_accepts_schema_v1_from_origin() {
    let mut batches = Vec::new();
    for height in 9..=11 {
        let mut legacy = MintSpec::standard(
            height as u8 + 60,
            cold_recipient(height as u8 + 60),
            0,
            vec![],
        );
        legacy.version = 2;
        let mut schema_v1 = MintSpec::standard(
            height as u8 + 70,
            cold_recipient(height as u8 + 70),
            0,
            vec![],
        );
        // This boundary test must pin both payload versions, independently of fixture defaults.
        schema_v1.version = 1;
        batches.push(MintBlock::new(height, vec![legacy, schema_v1], true));
    }
    let p = Pipeline::new("current-only", &batches.iter().collect::<Vec<_>>(), 9).await;
    p.sync(9, 11).await.unwrap();
    for batch in &batches {
        for (index, expected) in [(0, MinerPassState::Invalid), (1, MinerPassState::Active)] {
            let id = batch.mints[index].inscription_id;
            assert_eq!(
                p.indexer
                    .miner_pass_storage()
                    .get_pass_by_inscription_id(&id)
                    .unwrap()
                    .unwrap()
                    .state,
                expected
            );
            let audit = p
                .rpc()
                .get_pass_mint_audit(GetPassMintAuditParams {
                    inscription_id: id.to_string(),
                    at_height: Some(11),
                    context: None,
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                audit.audit.error_code.as_deref(),
                if index == 0 {
                    Some("INVALID_SCHEMA")
                } else {
                    None
                }
            );
        }
    }
    p.cleanup();
}

#[tokio::test]
async fn pipeline_inherits_current_prev_and_publishes_source_audit() {
    let spec = MintSpec::standard(64, source_script(SpendKind::Witness), 0, vec![]);
    let old = MintBlock::new(9, vec![spec], false);
    let next = MintBlock::new(
        10,
        vec![MintSpec::standard(
            65,
            cold_recipient(65),
            0,
            vec![old.mints[0].inscription_id],
        )],
        false,
    );
    let p = Pipeline::new("inherit", &[&old, &next], 9).await;
    p.sync(9, 10).await.unwrap();
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&old.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Consumed
    );
    let id = next.mints[0].inscription_id;
    let audit = p
        .rpc()
        .get_pass_mint_audit(GetPassMintAuditParams {
            inscription_id: id.to_string(),
            at_height: Some(10),
            context: None,
        })
        .unwrap()
        .unwrap();
    assert_eq!(audit.audit.operation_path.as_deref(), Some("cross_owner"));
    assert_eq!(audit.audit.balance_before_tx, Some(0));
    assert_eq!(audit.audit.ever_valid_owner, Some(false));
    assert_eq!(audit.audit.source.unwrap().authorization, "p2wpkh_all");
    assert_eq!(
        audit.active_version_set_id,
        p.indexer
            .active_version_set_at(10)
            .unwrap()
            .active_version_set_id()
    );
    p.cleanup();
}

#[tokio::test]
async fn pipeline_unavailable_evidence_leaves_no_invalid_or_audit_and_retries() {
    let spec = MintSpec::standard(66, source_script(SpendKind::Witness), 0, vec![]);
    let old = MintBlock::new(9, vec![spec], false);
    let new = MintBlock::new(
        11,
        vec![MintSpec::standard(
            67,
            cold_recipient(67),
            0,
            vec![old.mints[0].inscription_id],
        )],
        false,
    );
    let p = Pipeline::new("evidence-retry", &[&old, &new], 9).await;
    p.sync(9, 10).await.unwrap();
    // Missing commit block after discovery/whole-block balance preparation reaches the actual
    // v2 source lookup and must roll back tracker staging and the entire writer transaction.
    let removed = p.core.state.lock().unwrap().blocks.remove(&10).unwrap();
    assert!(p.sync(11, 11).await.is_err());
    let storage = p.indexer.miner_pass_storage();
    assert_eq!(
        storage.get_committed_synced_btc_block_height().unwrap(),
        Some(10)
    );
    assert!(
        storage
            .get_pass_by_inscription_id(&new.mints[0].inscription_id)
            .unwrap()
            .is_none()
    );
    assert!(
        storage
            .get_mint_audit(&new.mints[0].inscription_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage
            .get_pass_by_inscription_id(&old.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    p.core.state.lock().unwrap().blocks.insert(10, removed);
    p.sync(11, 11).await.unwrap();
    assert_eq!(
        storage
            .get_mint_audit(&new.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .operation_path
            .as_deref(),
        Some("cross_owner")
    );
    p.cleanup();
}

#[tokio::test]
async fn pipeline_recovers_after_energy_finalize_and_after_tracker_publication() {
    for outer_sql_failure in [false, true] {
        let spec = MintSpec::standard(68, source_script(SpendKind::Witness), 0, vec![]);
        let old = MintBlock::new(9, vec![spec], false);
        let new = MintBlock::new(
            10,
            vec![MintSpec::standard(
                69,
                cold_recipient(69),
                0,
                vec![old.mints[0].inscription_id],
            )],
            false,
        );
        let p = Pipeline::new("publication-retry", &[&old, &new], 9).await;
        p.sync(9, 9).await.unwrap();
        let db = rusqlite::Connection::open(
            p.config
                .data_dir()
                .join(crate::constants::MINER_PASS_DB_FILE),
        )
        .unwrap();
        if outer_sql_failure {
            db.execute_batch("CREATE TRIGGER fail_progress BEFORE UPDATE OF value ON state WHEN NEW.name='btc_synced_block_height' AND NEW.value=10 BEGIN SELECT RAISE(ABORT, 'injected outer publication failure'); END;").unwrap();
        } else {
            p.history.lock().unwrap().fail_commit = Some(10);
        }
        assert!(p.sync(10, 10).await.is_err());
        let storage = p.indexer.miner_pass_storage();
        assert_eq!(
            storage.get_committed_synced_btc_block_height().unwrap(),
            Some(9)
        );
        assert_eq!(
            storage
                .get_pass_by_inscription_id(&old.mints[0].inscription_id)
                .unwrap()
                .unwrap()
                .state,
            MinerPassState::Active
        );
        assert!(
            storage
                .get_pass_by_inscription_id(&new.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            storage
                .get_mint_audit(&new.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            p.indexer
                .pass_energy_manager()
                .get_pass_energy_record_exact(&new.mints[0].inscription_id, 10)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            p.indexer
                .pass_energy_manager()
                .get_synced_block_height_for_test()
                .unwrap(),
            Some(9)
        );
        if outer_sql_failure {
            db.execute_batch("DROP TRIGGER fail_progress").unwrap();
        } else {
            p.history.lock().unwrap().fail_commit = None;
        }
        p.sync(10, 10).await.unwrap();
        assert!(
            storage
                .get_mint_audit(&new.mints[0].inscription_id)
                .unwrap()
                .is_some()
        );
        drop(db);
        p.cleanup();
    }
}

#[tokio::test]
async fn pipeline_reopen_and_rollback_replay_preserve_identity_and_audit() {
    let spec = MintSpec::standard(70, source_script(SpendKind::Witness), 0, vec![]);
    let old = MintBlock::new(9, vec![spec], false);
    let new = MintBlock::new(
        10,
        vec![MintSpec::standard(
            71,
            cold_recipient(71),
            0,
            vec![old.mints[0].inscription_id],
        )],
        false,
    );
    let p = Pipeline::new("reorg", &[&old, &new], 9).await;
    p.sync(9, 10).await.unwrap();
    let id = new.mints[0].inscription_id;
    let root = p
        .indexer
        .miner_pass_storage()
        .get_pass_block_commit(10)
        .unwrap()
        .unwrap()
        .block_commit;
    let audit = p
        .indexer
        .miner_pass_storage()
        .get_mint_audit(&id)
        .unwrap()
        .unwrap();
    let energy = p
        .indexer
        .pass_energy_manager()
        .get_pass_energy(&id, 10)
        .await
        .unwrap()
        .unwrap()
        .energy;
    let p = p.reopen().await;
    assert_eq!(
        p.indexer.miner_pass_storage().get_mint_audit(&id).unwrap(),
        Some(audit.clone())
    );
    p.indexer.rollback_and_resume_for_test(9).await.unwrap();
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_mint_audit(&id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&old.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    p.sync(10, 10).await.unwrap();
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_block_commit(10)
            .unwrap()
            .unwrap()
            .block_commit,
        root
    );
    assert_eq!(
        p.indexer.miner_pass_storage().get_mint_audit(&id).unwrap(),
        Some(audit)
    );
    assert_eq!(
        p.indexer
            .pass_energy_manager()
            .get_pass_energy(&id, 10)
            .await
            .unwrap()
            .unwrap()
            .energy,
        energy
    );
    let full = Pipeline::new("fresh-replay", &[&old, &new], 9).await;
    full.sync(9, 10).await.unwrap();
    assert_eq!(
        full.indexer
            .miner_pass_storage()
            .get_pass_block_commit(10)
            .unwrap()
            .unwrap()
            .block_commit,
        root
    );
    full.cleanup();
    p.cleanup();
}

#[tokio::test]
async fn same_reveal_observes_prior_mint_history_and_real_transfer_before_mint() {
    let target = cold_recipient(72);
    let batch = MintBlock::new(
        10,
        vec![
            MintSpec::standard(72, target.clone(), 0, vec![]),
            MintSpec::standard(73, target.clone(), 0, vec![]),
        ],
        true,
    );
    let p = Pipeline::new("same-reveal", &[&batch], 10).await;
    p.sync(10, 10).await.unwrap();
    let storage = p.indexer.miner_pass_storage();
    let first = storage
        .get_mint_audit(&batch.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    let second = storage
        .get_mint_audit(&batch.mints[1].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(first.operation_path.as_deref(), Some("first_opening"));
    assert_eq!(first.balance_before_tx, Some(0));
    assert_eq!(second.balance_before_tx, Some(0));
    assert_eq!(second.ever_valid_owner, Some(true));
    assert_eq!(second.error_code.as_deref(), Some("INELIGIBLE_RECIPIENT"));
    p.cleanup();

    use crate::index::test_miner_evidence as chain;
    use bitcoincore_rpc::bitcoin::{Amount, TxIn, TxOut};
    let spec = MintSpec::standard(74, source_script(SpendKind::Witness), 0, vec![]);
    let old = MintBlock::new(9, vec![spec], false);
    let mut new = MintBlock::new(
        10,
        vec![MintSpec::standard(75, target.clone(), 0, vec![])],
        false,
    );
    let mut tx = new.core.state.lock().unwrap().blocks[&10].0.txdata[1].clone();
    let old_output = TxOut {
        value: Amount::from_sat(4900),
        script_pubkey: source_script(SpendKind::Witness),
    };
    tx.input.insert(
        0,
        TxIn {
            previous_output: old.mints[0].satpoint.outpoint,
            ..TxIn::default()
        },
    );
    tx.output.insert(
        0,
        TxOut {
            value: Amount::from_sat(4900),
            script_pubkey: target,
        },
    );
    let commit_output = new.core.state.lock().unwrap().blocks[&9].0.txdata[1].output[0].clone();
    chain::sign(
        &mut tx,
        0,
        &[old_output.clone(), commit_output],
        SpendKind::Witness,
        1,
        false,
    );
    replace_reveal(
        &mut new,
        10,
        tx,
        std::collections::HashMap::from([(
            old.mints[0].satpoint.outpoint,
            usdb_util::SpentPrevout {
                txout: old_output,
                height: 9,
                coinbase: false,
            },
        )]),
    );
    let p = Pipeline::new("transfer-before-mint", &[&old, &new], 9).await;
    p.sync(9, 10).await.unwrap();
    let storage = p.indexer.miner_pass_storage();
    let acquired = storage
        .get_pass_by_inscription_id(&old.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(acquired.state, MinerPassState::Dormant);
    use usdb_util::ToBtcScriptHash;
    assert_eq!(acquired.owner, cold_recipient(72).to_btc_script_hash());
    let audit = storage
        .get_mint_audit(&new.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(audit.balance_before_tx, Some(0));
    assert_eq!(audit.ever_valid_owner, Some(true));
    assert_eq!(audit.error_code.as_deref(), Some("INELIGIBLE_RECIPIENT"));
    p.cleanup();
}

#[tokio::test]
async fn unsupported_envelope_is_durable_invalid_with_no_invented_recipient() {
    use crate::index::test_miner_evidence as chain;
    use bitcoincore_rpc::bitcoin::{
        OutPoint, ScriptBuf,
        opcodes::all::{OP_ENDIF, OP_IF},
        script::Builder,
    };
    let mut batch = MintBlock::new(
        10,
        vec![MintSpec::standard(76, cold_recipient(76), 0, vec![])],
        false,
    );
    let original_commit = batch.core.state.lock().unwrap().blocks[&9].0.clone();
    let inputs = batch
        .core
        .client
        .get_block_prevouts(9, &original_commit)
        .unwrap();
    let mut commit = original_commit.txdata[1].clone();
    let mut reveal = batch.core.state.lock().unwrap().blocks[&10].0.txdata[1].clone();
    let mut script = reveal.input[0].witness.iter().next().unwrap().to_vec();
    script.extend(
        Builder::new()
            .push_int(0)
            .push_opcode(OP_IF)
            .push_slice(b"ord")
            .push_opcode(OP_ENDIF)
            .into_script()
            .into_bytes(),
    );
    let (output_script, witness) = chain::tap_script(ScriptBuf::from_bytes(script));
    commit.output[0].script_pubkey = output_script;
    let funding = inputs
        .get(&commit.input[0].previous_output)
        .unwrap()
        .clone();
    chain::sign(
        &mut commit,
        0,
        std::slice::from_ref(&funding.txout),
        SpendKind::Witness,
        1,
        false,
    );
    let point = OutPoint::new(commit.compute_txid(), 0);
    reveal.input[0].previous_output = point;
    reveal.input[0].witness = witness;
    let coins = std::collections::HashMap::from([
        (commit.input[0].previous_output, funding),
        (
            point,
            usdb_util::SpentPrevout {
                txout: commit.output[0].clone(),
                height: 9,
                coinbase: false,
            },
        ),
    ]);
    let cb = chain::block(vec![commit]);
    let mut rb = chain::block(vec![reveal]);
    rb.header.prev_blockhash = cb.block_hash();
    batch.mints[0].inscription_id.txid = rb.txdata[1].compute_txid();
    batch
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(9, (cb.clone(), chain::verbose(9, &cb, &coins)));
    batch
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(10, (rb.clone(), chain::verbose(10, &rb, &coins)));
    let p = Pipeline::new("unsupported", &[&batch], 10).await;
    p.sync(10, 10).await.unwrap();
    let storage = p.indexer.miner_pass_storage();
    let invalid = storage
        .get_pass_by_inscription_id(&batch.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(invalid.state, MinerPassState::Invalid);
    assert_eq!(invalid.satpoint.outpoint.vout, u32::MAX);
    let audit = storage
        .get_mint_audit(&batch.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(audit.error_code.as_deref(), Some("UNSUPPORTED_INSCRIPTION"));
    assert_eq!(audit.recipient, None);
    p.cleanup();
}

#[tokio::test]
async fn ordinary_payment_before_reveal_prevents_zero_balance_opening() {
    use crate::index::test_miner_evidence as chain;
    use bitcoincore_rpc::bitcoin::{OutPoint, Txid, hashes::Hash};
    use std::collections::HashMap;
    let target = cold_recipient(77);
    let batch = MintBlock::new(
        10,
        vec![MintSpec::standard(77, target.clone(), 0, vec![])],
        false,
    );
    let (original, _) = batch.core.state.lock().unwrap().blocks[&10].clone();
    let inputs = batch.core.client.get_block_prevouts(10, &original).unwrap();
    let funding = OutPoint::new(Txid::from_byte_array([177; 32]), 0);
    let coin = chain::output(1000, source_script(SpendKind::Witness));
    let mut payment = chain::transaction(
        vec![funding],
        vec![
            chain::output(1, target),
            chain::output(900, source_script(SpendKind::Witness)),
        ],
    );
    chain::sign(
        &mut payment,
        0,
        std::slice::from_ref(&coin),
        SpendKind::Witness,
        1,
        false,
    );
    let reveal = original.txdata[1].clone();
    let coins = HashMap::from([
        (
            funding,
            usdb_util::SpentPrevout {
                txout: coin,
                height: 1,
                coinbase: false,
            },
        ),
        (
            reveal.input[0].previous_output,
            inputs
                .get(&reveal.input[0].previous_output)
                .unwrap()
                .clone(),
        ),
    ]);
    let block = chain::block(vec![payment, reveal]);
    batch
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(10, (block.clone(), chain::verbose(10, &block, &coins)));
    let p = Pipeline::new("ordinary-payment", &[&batch], 10).await;
    p.sync(10, 10).await.unwrap();
    let audit = p
        .indexer
        .miner_pass_storage()
        .get_mint_audit(&batch.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(audit.balance_before_tx, Some(1));
    assert_eq!(audit.ever_valid_owner, Some(false));
    assert_eq!(audit.error_code.as_deref(), Some("INELIGIBLE_RECIPIENT"));
    p.cleanup();
}

// UIP-0016 M11 deliberately permits a previously used BTC address once its
// pre-reveal balance is zero, provided it has never owned a valid MinerPass.
#[tokio::test]
async fn prior_balance_spent_before_reveal_allows_opening_without_source_authorization() {
    use crate::index::test_miner_evidence as chain;
    use bitcoincore_rpc::bitcoin::{OutPoint, Txid, hashes::Hash};
    use std::collections::HashMap;
    use usdb_util::ToBtcScriptHash;

    for drain in [false, true] {
        let target = source_script(SpendKind::Witness);
        let mut spec = MintSpec::standard(81, target.clone(), 100_000_000, vec![]);
        // A different signer funds the inscription: only zero-balance opening
        // can authorize this recipient, not the same-owner path.
        spec.source = SpendKind::Legacy;
        let batch = MintBlock::new(10, vec![spec], false);
        if drain {
            let (original, _) = batch.core.state.lock().unwrap().blocks[&10].clone();
            let inputs = batch.core.client.get_block_prevouts(10, &original).unwrap();
            let funding = OutPoint::new(Txid::from_byte_array([181; 32]), 0);
            let coin = chain::output(100_000_000, target.clone());
            let mut payment = chain::transaction(
                vec![funding],
                vec![chain::output(99_999_000, cold_recipient(82))],
            );
            chain::sign(
                &mut payment,
                0,
                std::slice::from_ref(&coin),
                SpendKind::Witness,
                1,
                false,
            );
            let reveal = original.txdata[1].clone();
            let coins = HashMap::from([
                (
                    funding,
                    usdb_util::SpentPrevout {
                        txout: coin,
                        height: 1,
                        coinbase: false,
                    },
                ),
                (
                    reveal.input[0].previous_output,
                    inputs
                        .get(&reveal.input[0].previous_output)
                        .unwrap()
                        .clone(),
                ),
            ]);
            let block = chain::block(vec![payment, reveal]);
            batch
                .core
                .state
                .lock()
                .unwrap()
                .blocks
                .insert(10, (block.clone(), chain::verbose(10, &block, &coins)));
        }
        let p = Pipeline::new("drained-opening", &[&batch], 9).await;
        assert_eq!(
            p.history.lock().unwrap().values[&target.to_btc_script_hash()][&9],
            100_000_000
        );
        p.sync(9, 10).await.unwrap();
        let storage = p.indexer.miner_pass_storage();
        let id = batch.mints[0].inscription_id;
        let pass = storage.get_pass_by_inscription_id(&id).unwrap().unwrap();
        let audit = storage.get_mint_audit(&id).unwrap().unwrap();
        assert_eq!(audit.ever_valid_owner, Some(false));
        if drain {
            assert_eq!(audit.balance_before_tx, Some(0));
            assert_eq!(audit.operation_path.as_deref(), Some("first_opening"));
            assert_eq!(audit.error_code, None);
            assert_eq!(pass.state, MinerPassState::Active);
        } else {
            assert_eq!(audit.balance_before_tx, Some(100_000_000));
            assert_eq!(audit.error_code.as_deref(), Some("INELIGIBLE_RECIPIENT"));
            assert_eq!(pass.state, MinerPassState::Invalid);
        }
        p.cleanup();
    }
}

#[tokio::test]
async fn audit_query_enforces_selected_state_and_detects_missing_or_stale_data() {
    use crate::service::rpc::GetStateRefAtHeightParams;
    use usdb_util::{ConsensusQueryContext, ConsensusStateReference};
    let batch = MintBlock::new(
        10,
        vec![MintSpec::standard(78, cold_recipient(78), 0, vec![])],
        false,
    );
    let p = Pipeline::new("audit-query", &[&batch], 10).await;
    p.sync(10, 10).await.unwrap();
    let rpc = p.rpc();
    let id = batch.mints[0].inscription_id;
    let state = rpc
        .get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: 10,
            context: None,
        })
        .unwrap();
    let context = ConsensusQueryContext {
        requested_height: Some(10),
        expected_state: ConsensusStateReference::from(&state),
    };
    let params = GetPassMintAuditParams {
        inscription_id: id.to_string(),
        at_height: Some(10),
        context: Some(context),
    };
    let response = rpc.get_pass_mint_audit(params.clone()).unwrap().unwrap();
    assert_eq!(
        response.observed_state.system_state_id,
        Some(state.system_state_info.system_state_id)
    );
    let mut bad = params.clone();
    bad.context.as_mut().unwrap().requested_height = Some(11);
    assert!(
        rpc.get_pass_mint_audit(bad)
            .unwrap_err()
            .message
            .contains("does not match")
    );
    let mut bad = params.clone();
    bad.context
        .as_mut()
        .unwrap()
        .expected_state
        .active_version_set_id = Some("bad-state".into());
    assert!(rpc.get_pass_mint_audit(bad).is_err());
    let commit = state
        .local_state_commit_info
        .latest_pass_block_commit
        .unwrap()
        .block_commit;
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_mint_audit_at_height(&id, 10, "stale-commit")
            .is_err()
    );
    let db = rusqlite::Connection::open(
        p.config
            .data_dir()
            .join(crate::constants::MINER_PASS_DB_FILE),
    )
    .unwrap();
    let mut stale = response.audit.clone();
    stale.block_hash = "00".repeat(32);
    db.execute(
        "UPDATE miner_pass_mint_audit SET audit_json=?1",
        [serde_json::to_string(&stale).unwrap()],
    )
    .unwrap();
    assert!(
        rpc.get_pass_mint_audit(params.clone())
            .unwrap_err()
            .message
            .contains("Audit metadata disagrees")
    );
    db.execute("DELETE FROM miner_pass_mint_audit", []).unwrap();
    assert!(
        rpc.get_pass_mint_audit(params)
            .unwrap_err()
            .message
            .contains("Missing audit for processed MinerPass mint")
    );
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_block_commit(10)
            .unwrap()
            .unwrap()
            .block_commit,
        commit
    );
    drop(db);
    drop(rpc);
    p.cleanup();
}

#[tokio::test]
async fn unsupported_future_rule_pairs_stop_before_block_mutation() {
    for (family, value) in [
        (
            "pass_state_machine_version",
            "uip-0002-pass-state-machine:v1",
        ),
        (
            "pass_state_machine_version",
            "uip-0002-pass-state-machine:v999",
        ),
        (
            "inscription_schema_version",
            "uip-0001-miner-pass-inscription:v2",
        ),
        (
            "energy_formula_version",
            "uip-0003-pass-energy-formula:v999",
        ),
    ] {
        let mut catalog: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
        let records = catalog["registries"][0]["records"].as_array_mut().unwrap();
        let mut activation = records
            .iter()
            .find(|r| r["version_family"] == family)
            .unwrap()
            .clone();
        activation["activation_height"] = 10.into();
        activation["supersedes"] = activation["version_value"].clone();
        activation["version_value"] = value.into();
        records.push(activation);
        let registry: usdb_util::BtcActivationRegistry =
            serde_json::from_value(catalog["registries"][0].clone()).unwrap();
        catalog["current_registry_id"] = registry.activation_registry_id().into();
        let mut old_spec = MintSpec::standard(79, cold_recipient(79), 0, vec![]);
        old_spec.version = 1;
        let old = MintBlock::new(9, vec![old_spec], false);
        let new = MintBlock::new(
            10,
            vec![MintSpec::standard(80, cold_recipient(80), 0, vec![])],
            false,
        );
        let p = Pipeline::with_catalog("unsupported-rules", &[&old, &new], 9, &catalog.to_string())
            .await;
        p.sync(9, 9).await.unwrap();
        let error = p.sync(10, 10).await.unwrap_err();
        assert!(
            error.contains("Unsupported MinerPass rule combination")
                || error.contains("version not supported"),
            "{family}={value}: {error}"
        );
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_committed_synced_btc_block_height()
                .unwrap(),
            Some(9)
        );
        assert!(
            p.indexer
                .miner_pass_storage()
                .get_pass_by_inscription_id(&new.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            p.indexer
                .miner_pass_storage()
                .get_mint_audit(&new.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            p.indexer
                .pass_energy_manager()
                .get_pending_block_height_for_test()
                .unwrap(),
            None
        );
        assert!(!p.indexer.has_active_block_mutation_collection_for_test());
        p.cleanup();
    }
}

#[tokio::test]
async fn startup_rejects_unsupported_intermediate_history_before_reconciling_metadata() {
    use crate::index::InscriptionIndexer;
    use crate::index::energy::PassEnergyManager;
    use usdb_util::{BtcActivationRegistry, ENERGY_FORMULA_VERSION_V1};

    let mut catalog: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    let records = catalog["registries"][0]["records"].as_array_mut().unwrap();
    let mut activation = records
        .iter()
        .find(|r| r["version_family"] == "energy_formula_version")
        .unwrap()
        .clone();
    let unsupported = "uip-0003-pass-energy-formula:v999";
    activation["activation_height"] = 10.into();
    activation["supersedes"] = ENERGY_FORMULA_VERSION_V1.into();
    activation["version_value"] = unsupported.into();
    records.push(activation.clone());
    activation["activation_height"] = 20.into();
    activation["supersedes"] = unsupported.into();
    activation["version_value"] = ENERGY_FORMULA_VERSION_V1.into();
    records.push(activation);
    let registry: BtcActivationRegistry =
        serde_json::from_value(catalog["registries"][0].clone()).unwrap();
    catalog["current_registry_id"] = registry.activation_registry_id().into();
    let batch = MintBlock::new(
        9,
        vec![MintSpec::standard(117, cold_recipient(117), 0, vec![])],
        false,
    );
    let p = Pipeline::with_catalog("unsupported-history", &[&batch], 9, &catalog.to_string()).await;
    p.sync(9, 9).await.unwrap();
    assert_eq!(
        p.indexer.rule_context_at(9).unwrap().active_version_set(),
        p.indexer.rule_context_at(20).unwrap().active_version_set()
    );
    // Simulate durable progress written by a binary supporting an intermediate rule.
    // An older binary must refuse this history even though both endpoints are supported.
    p.indexer
        .miner_pass_storage()
        .update_synced_btc_block_height(20)
        .unwrap();
    p.indexer
        .pass_energy_manager()
        .set_synced_block_height_for_test(20)
        .unwrap();
    let db = rusqlite::Connection::open(
        p.config
            .data_dir()
            .join(crate::constants::MINER_PASS_DB_FILE),
    )
    .unwrap();
    // Reconciliation would repair this stale cursor; rejection must happen first.
    db.execute(
        "UPDATE state SET value=8 WHERE name='snapshot_history_next_height'",
        [],
    )
    .unwrap();
    let metadata = || {
        db.prepare(
            "SELECT name, CAST(value AS TEXT) FROM state \
             UNION ALL SELECT name, value FROM state_text ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    };
    let before = metadata();
    let Pipeline {
        root,
        config,
        indexer,
        status,
        ..
    } = p;
    drop(indexer);
    let error = InscriptionIndexer::new(config.clone(), status)
        .err()
        .expect("Unsupported intermediate history must reject startup");
    assert!(error.contains(unsupported), "{error}");
    assert_eq!(metadata(), before);
    let energy = PassEnergyManager::new(config).unwrap();
    assert_eq!(energy.get_synced_block_height_for_test().unwrap(), Some(20));
    assert_eq!(energy.get_pending_block_height_for_test().unwrap(), None);
    energy
        .validate_rules_binding(&usdb_util::IndexerRulesBinding::new(&registry, 9))
        .unwrap();
    drop(energy);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn pipeline_first_opening_source_query_is_read_only_and_rejects_lost_evidence() {
    let batch = MintBlock::new(
        10,
        vec![MintSpec::standard(111, cold_recipient(111), 0, vec![])],
        false,
    );
    let p = Pipeline::new("opening-tool-source", &[&batch], 10).await;
    p.sync(10, 10).await.unwrap();
    let params = GetPassMintAuditParams {
        inscription_id: batch.mints[0].inscription_id.to_string(),
        at_height: Some(10),
        context: None,
    };
    let rpc = p.rpc();
    let before = rpc.get_pass_mint_audit(params.clone()).unwrap().unwrap();
    assert_eq!(
        before.audit.operation_path.as_deref(),
        Some("first_opening")
    );
    assert!(before.audit.source.is_none());
    let evidence = rpc.get_pass_mint_source(params.clone()).unwrap();
    assert_eq!(evidence["source"]["authorization"], "p2wpkh_all");
    assert_eq!(
        evidence["mint"]["audit"]["inscription_id"],
        params.inscription_id
    );
    assert_eq!(
        rpc.get_pass_mint_audit(params.clone())
            .unwrap()
            .unwrap()
            .audit,
        before.audit
    );
    // A concurrent canonical switch invalidates the evidence even when the persisted audit is unchanged.
    let original_blocks = p.core.state.lock().unwrap().blocks.clone();
    p.core.state.lock().unwrap().reorg_after_verbose = true;
    assert!(rpc.get_pass_mint_source(params.clone()).is_err());
    p.core.state.lock().unwrap().blocks = original_blocks;
    // Pruned/unavailable source blocks prevent tool approval without changing protocol validity.
    p.core.state.lock().unwrap().blocks.clear();
    assert!(rpc.get_pass_mint_source(params.clone()).is_err());
    assert_eq!(
        rpc.get_pass_mint_audit(params).unwrap().unwrap().audit,
        before.audit
    );
    drop(rpc);
    p.cleanup();
}

#[tokio::test]
async fn pipeline_context_preserves_pinned_registry_and_mint_audit_identity() {
    let old = MintBlock::new(
        9,
        vec![MintSpec::standard(92, cold_recipient(92), 0, vec![])],
        false,
    );
    let catalog = include_str!("fixtures/miner-pass-v2/catalog-staged.json");
    let p = Pipeline::with_catalog("timeline-context", &[&old], 9, catalog).await;
    p.sync(9, 9).await.unwrap();
    let current_id = p
        .indexer
        .activation_registry_catalog()
        .current_registry_id()
        .to_string();
    let default_context = p.indexer.rule_context_at(9).unwrap();
    assert_eq!(default_context.activation_registry_id(), current_id);
    assert_eq!(
        default_context.scope().rules_scope(),
        "miner-pass-v2-fixture"
    );
    assert_eq!(default_context.btc_height(), 9);
    for id in p.indexer.activation_registry_catalog().registry_ids() {
        let context = p.indexer.rule_context_at_with_registry(id, 9).unwrap();
        assert_eq!(context.activation_registry_id(), id);
        assert_eq!(
            context.active_version_set(),
            &p.indexer
                .activation_registry_catalog()
                .registry_by_id(id)
                .unwrap()
                .lookup_active_version_set(9)
                .unwrap()
        );
    }
    assert!(matches!(
        p.indexer
            .rule_context_at_with_registry("unknown-registry", 9),
        Err(usdb_util::ActivationRegistryError::ActivationRecordNotFound(_))
    ));
    let audit = p
        .rpc()
        .get_pass_mint_audit(GetPassMintAuditParams {
            inscription_id: old.mints[0].inscription_id.to_string(),
            at_height: Some(9),
            context: None,
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        audit.active_version_set_id,
        default_context.active_version_set_id()
    );
    assert_eq!(
        p.indexer
            .activation_registry_catalog()
            .current_registry_id(),
        current_id
    );
    p.cleanup();
}
