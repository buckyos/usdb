//! Reveal-height schema and state admission dispatch through the real ordered indexer pipeline.
use crate::index::test_miner_pipeline as pipeline;
use std::sync::Arc;

use bitcoincore_rpc::bitcoin::Network;
use usdb_util::{BtcRuleTimeline, VersionFamily};

use crate::index::rules::{CONFORMANCE_SCHEMA, CONFORMANCE_STATE, validate_indexer_rules};
use crate::index::test_miner_rules::{conformance_catalog, conformance_context};
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use crate::index::{MinerPassKind, MinerPassState};
use crate::inscription::{
    BitcoindInscriptionSource, CompareInscriptionSource, CompareTarget, InscriptionSource,
};
use pipeline::{Pipeline, cold_recipient};

#[tokio::test]
async fn schema_switch_uses_reveal_height_for_discovery_compare_and_local_reparse() {
    let catalog = conformance_catalog(&[(
        VersionFamily::InscriptionSchemaVersion,
        10,
        CONFORMANCE_SCHEMA,
    )]);
    let mut batches = Vec::new();
    for height in 9..=11 {
        let tag = (height * 2) as u8;
        let mut old = MintSpec::standard(tag, cold_recipient(tag), 0, vec![]);
        old.version = 1;
        let mut new = MintSpec::standard(tag + 1, cold_recipient(tag + 1), 0, vec![]);
        new.version = 901;
        batches.push(MintBlock::new(height, vec![old, new], false));
    }
    let mut p = Pipeline::with_catalog(
        "schema-boundary",
        &batches.iter().collect::<Vec<_>>(),
        9,
        &catalog,
    )
    .await;
    let source: Arc<dyn InscriptionSource> =
        Arc::new(BitcoindInscriptionSource::new(p.core.client.clone()));
    // Both external comparison legs and the independent canonical local parser see the same context.
    Arc::get_mut(&mut p.indexer)
        .unwrap()
        .replace_inscription_source_for_test(Arc::new(CompareInscriptionSource::new_with_target(
            source.clone(),
            source,
            true,
            CompareTarget::UsdbMint,
        )));
    p.sync(9, 11).await.unwrap();
    for (height, batch) in (9..=11).zip(&batches) {
        for (index, mint) in batch.mints.iter().enumerate() {
            let valid = (height == 9 && index == 0) || (height >= 10 && index == 1);
            let pass = p
                .indexer
                .miner_pass_storage()
                .get_pass_by_inscription_id(&mint.inscription_id)
                .unwrap()
                .unwrap();
            assert_eq!(
                pass.state,
                if valid {
                    MinerPassState::Active
                } else {
                    MinerPassState::Invalid
                }
            );
            let audit = p
                .indexer
                .miner_pass_storage()
                .get_mint_audit(&mint.inscription_id)
                .unwrap()
                .unwrap();
            assert_eq!(audit.block_height, height);
            assert_eq!(
                audit.error_code.as_deref(),
                if valid { None } else { Some("INVALID_SCHEMA") }
            );
        }
    }
    // The H reveal committed at H-1, while schema v1 was still active.
    assert_ne!(
        p.indexer.active_version_set_at(9).unwrap(),
        p.indexer.active_version_set_at(10).unwrap()
    );
    let first = batches[0].mints[0].inscription_id;
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&first)
            .unwrap()
            .unwrap()
            .mint_version,
        1
    );
    // Restart validates all conformance intervals through the same compiled support gate.
    let p = p.reopen().await;
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&first)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    p.cleanup();
}

#[tokio::test]
async fn schema_switch_inherits_an_old_schema_pass_without_revalidating_its_payload() {
    let catalog = conformance_catalog(&[(
        VersionFamily::InscriptionSchemaVersion,
        10,
        CONFORMANCE_SCHEMA,
    )]);
    let old = MintBlock::new(
        9,
        vec![MintSpec::standard(
            40,
            source_script(SpendKind::Witness),
            0,
            vec![],
        )],
        false,
    );
    let mut spec = MintSpec::standard(41, cold_recipient(41), 0, vec![old.mints[0].inscription_id]);
    spec.version = 901;
    let new = MintBlock::new(10, vec![spec], false);
    let p = Pipeline::with_catalog("schema-prev", &[&old, &new], 9, &catalog).await;
    p.sync(9, 9).await.unwrap();
    p.history.lock().unwrap().fail_commit = Some(10);
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
            .get_pending_block_height_for_test()
            .unwrap(),
        None
    );
    assert!(!p.indexer.has_active_block_mutation_collection_for_test());
    p.history.lock().unwrap().fail_commit = None;
    p.sync(10, 10).await.unwrap();
    let parent = storage
        .get_pass_by_inscription_id(&old.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    let child = storage
        .get_pass_by_inscription_id(&new.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (parent.mint_version, parent.state),
        (1, MinerPassState::Consumed)
    );
    assert_eq!(
        (child.mint_version, child.state),
        (901, MinerPassState::Active)
    );
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
async fn tightened_state_admission_preserves_old_collab_and_rejected_prev_then_allows_inheritance()
{
    let catalog = conformance_catalog(&[(
        VersionFamily::PassStateMachineVersion,
        10,
        CONFORMANCE_STATE,
    )]);
    let leader = MintBlock::new(
        8,
        vec![MintSpec::standard(50, cold_recipient(50), 0, vec![])],
        false,
    );
    let mut old_spec = MintSpec::standard(51, source_script(SpendKind::Witness), 0, vec![]);
    old_spec.kind = MinerPassKind::Collab;
    old_spec.leader = Some(leader.mints[0].inscription_id);
    let old = MintBlock::new(9, vec![old_spec], false);
    let mut rejected_spec =
        MintSpec::standard(52, cold_recipient(52), 0, vec![old.mints[0].inscription_id]);
    rejected_spec.kind = MinerPassKind::Collab;
    rejected_spec.leader = Some(leader.mints[0].inscription_id);
    let rejected = MintBlock::new(10, vec![rejected_spec], false);
    let inherited = MintBlock::new(
        11,
        vec![MintSpec::standard(
            53,
            cold_recipient(53),
            0,
            vec![old.mints[0].inscription_id],
        )],
        false,
    );
    let p = Pipeline::with_catalog(
        "state-admission",
        &[&leader, &old, &rejected, &inherited],
        8,
        &catalog,
    )
    .await;
    p.sync(8, 9).await.unwrap();
    let before = p
        .indexer
        .pass_energy_manager()
        .get_pass_energy(&old.mints[0].inscription_id, 9)
        .await
        .unwrap()
        .unwrap()
        .energy;
    p.sync(10, 10).await.unwrap();
    let storage = p.indexer.miner_pass_storage();
    let old_id = old.mints[0].inscription_id;
    assert_eq!(
        storage
            .get_pass_by_inscription_id(&old_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    assert!(
        p.indexer
            .pass_energy_manager()
            .get_pass_energy(&old_id, 10)
            .await
            .unwrap()
            .unwrap()
            .energy
            > before
    );
    assert_eq!(
        storage
            .get_pass_by_inscription_id(&rejected.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Invalid
    );
    assert_eq!(
        storage
            .get_mint_audit(&rejected.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .error_code
            .as_deref(),
        Some("INVALID_USDB_COLLAB")
    );
    assert!(
        p.indexer
            .pass_energy_manager()
            .get_pass_energy_record_exact(&rejected.mints[0].inscription_id, 10)
            .unwrap()
            .is_none()
    );
    p.sync(11, 11).await.unwrap();
    assert_eq!(
        storage
            .get_pass_by_inscription_id(&old_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Consumed
    );
    assert_eq!(
        storage
            .get_pass_by_inscription_id(&inherited.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    // State selection is independent of the schema contract; every payload here remains v1.
    for height in 9..=11 {
        assert_eq!(
            p.indexer
                .active_version_set_at(height)
                .unwrap()
                .require_string(VersionFamily::InscriptionSchemaVersion)
                .unwrap(),
            usdb_util::INSCRIPTION_SCHEMA_VERSION_V1
        );
    }
    p.cleanup();
}

#[test]
fn conformance_support_is_scoped_and_does_not_bypass_other_rule_families() {
    let catalog = conformance_catalog(&[
        (
            VersionFamily::InscriptionSchemaVersion,
            10,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            11,
            CONFORMANCE_STATE,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            12,
            crate::index::test_miner_rules::CONFORMANCE_SCHEMA_STRUCTURED,
        ),
    ]);
    let catalog = usdb_util::BtcActivationRegistryCatalog::from_json(&catalog).unwrap();
    let registry = catalog.current_registry();
    // The normal library support registry never accepts conformance executors.
    assert!(
        BtcRuleTimeline::new(registry)
            .unwrap()
            .indexer_context_at(10)
            .is_err()
    );
    let timeline =
        BtcRuleTimeline::new_with_indexer_support(registry, validate_indexer_rules).unwrap();
    timeline.validate_indexer_range(8..=12).unwrap();
    for variation in 0..3 {
        let mut changed = registry.clone();
        match variation {
            0 => changed.scope.rules_scope = Some("another-scope".into()),
            1 => {
                changed.scope.network_id = "btc-mainnet".into();
                changed.scope.network_type = usdb_util::ActivationNetworkType::Mainnet;
            }
            _ => {
                changed
                    .records
                    .iter_mut()
                    .find(|r| r.version_family == VersionFamily::EnergyFormulaVersion)
                    .unwrap()
                    .version_value = usdb_util::VersionValue::String("unsupported-energy".into())
            }
        }
        let timeline =
            BtcRuleTimeline::new_with_indexer_support(&changed, validate_indexer_rules).unwrap();
        for height in [10, 12] {
            assert!(
                timeline.indexer_context_at(height).is_err(),
                "variation={variation}, height={height}"
            );
        }
    }
}

#[tokio::test]
async fn discovery_rejects_wrong_height_and_network_before_classification() {
    let batch = MintBlock::new(
        10,
        vec![MintSpec::standard(60, cold_recipient(60), 0, vec![])],
        false,
    );
    let source = BitcoindInscriptionSource::new(batch.core.client.clone());
    let raw = source.load_block_inscriptions(10, None).await.unwrap();
    let catalog = conformance_catalog(&[(
        VersionFamily::InscriptionSchemaVersion,
        10,
        CONFORMANCE_SCHEMA,
    )]);
    let wrong = conformance_context(&catalog, 9);
    let error = crate::inscription::classify_usdb_mints_from_inscriptions(
        raw.clone(),
        Network::Regtest,
        &wrong,
    )
    .unwrap_err();
    assert!(
        error.contains("event_height=10") && error.contains("context_height=9"),
        "{error}"
    );
    let right = conformance_context(&catalog, 10);
    assert!(
        crate::inscription::classify_usdb_mints_from_inscriptions(raw, Network::Bitcoin, &right)
            .unwrap_err()
            .contains("scope mismatch")
    );
}

// A stale external classifier must not make the indexer accept its schema selection.
struct StaleSchemaSource {
    inner: BitcoindInscriptionSource,
}
impl InscriptionSource for StaleSchemaSource {
    fn source_name(&self) -> &'static str {
        "stale-schema-fixture"
    }
    fn load_block_inscriptions<'a>(
        &'a self,
        height: u32,
        block: Option<Arc<bitcoincore_rpc::bitcoin::Block>>,
    ) -> crate::inscription::InscriptionSourceFuture<
        'a,
        Result<Vec<crate::inscription::DiscoveredInscription>, String>,
    > {
        self.inner.load_block_inscriptions(height, block)
    }
    fn load_block_mint_batch<'a>(
        &'a self,
        rules: &'a usdb_util::BtcRuleContext,
        block: Option<Arc<bitcoincore_rpc::bitcoin::Block>>,
        network: Network,
    ) -> crate::inscription::InscriptionSourceFuture<
        'a,
        Result<crate::inscription::DiscoveredMintBatch, String>,
    > {
        Box::pin(async move {
            let stale = crate::index::test_miner_rules::context(rules.btc_height());
            self.inner
                .load_block_mint_batch(&stale, block, network)
                .await
        })
    }
}

#[tokio::test]
async fn stale_external_schema_is_rejected_without_audit_or_state_and_retry_uses_current_rules() {
    let catalog = conformance_catalog(&[(
        VersionFamily::InscriptionSchemaVersion,
        10,
        CONFORMANCE_SCHEMA,
    )]);
    let mut spec = MintSpec::standard(70, cold_recipient(70), 0, vec![]);
    spec.version = 901;
    let batch = MintBlock::new(10, vec![spec], false);
    let mut p = Pipeline::with_catalog("stale-source", &[&batch], 9, &catalog).await;
    p.sync(9, 9).await.unwrap();
    Arc::get_mut(&mut p.indexer)
        .unwrap()
        .replace_inscription_source_for_test(Arc::new(StaleSchemaSource {
            inner: BitcoindInscriptionSource::new(p.core.client.clone()),
        }));
    let error = p.sync(10, 10).await.unwrap_err();
    assert!(
        error.contains("Mint discovery disagrees with canonical reveal set"),
        "{error}"
    );
    let id = batch.mints[0].inscription_id;
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
            .get_pass_by_inscription_id(&id)
            .unwrap()
            .is_none()
    );
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_mint_audit(&id)
            .unwrap()
            .is_none()
    );
    assert!(
        p.indexer
            .pass_energy_manager()
            .get_pass_energy_record_exact(&id, 10)
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
    Arc::get_mut(&mut p.indexer)
        .unwrap()
        .replace_inscription_source_for_test(Arc::new(BitcoindInscriptionSource::new(
            p.core.client.clone(),
        )));
    p.sync(10, 10).await.unwrap();
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&id)
            .unwrap()
            .unwrap()
            .state,
        MinerPassState::Active
    );
    p.cleanup();
}

#[test]
fn selected_schema_retains_strict_json_validation() {
    use crate::index::{InscriptionContentLoader, ParsedMintContent};
    let catalog = conformance_catalog(&[(
        VersionFamily::InscriptionSchemaVersion,
        10,
        CONFORMANCE_SCHEMA,
    )]);
    let rules = conformance_context(&catalog, 10);
    let id = crate::index::test_miner_state::id(71);
    for content in [
        r#"{"p":"usdb","op":"mint","v":901,"v":901,"usdb_main":"0x1111111111111111111111111111111111111111"}"#,
        r#"{"p":"usdb","op":"mint","v":901,"unknown":0,"usdb_main":"0x1111111111111111111111111111111111111111"}"#,
        r#"{"p":"usdb","op":"mint","v":"901","usdb_main":"0x1111111111111111111111111111111111111111"}"#,
        r#"{"p":"usdb","op":"mint","v":2,"usdb_main":"0x1111111111111111111111111111111111111111"}"#,
    ] {
        assert!(
            matches!(
                InscriptionContentLoader::classify_mint_content_str_with_rules(
                    &id,
                    content,
                    Network::Regtest,
                    &rules
                )
                .unwrap(),
                ParsedMintContent::Invalid(_)
            ),
            "{content}"
        );
    }
}

#[tokio::test]
async fn state_entry_rejects_context_from_another_height_or_rule_domain_without_writing() {
    use crate::index::test_miner_state::Harness;
    let h = Harness::new("wrong-context");
    let batch = MintBlock::new(
        10,
        vec![MintSpec::standard(72, cold_recipient(72), 0, vec![])],
        false,
    );
    let right = h.rules.indexer_context_at(10).unwrap();
    let mut other: usdb_util::BtcActivationRegistry = serde_json::from_value(serde_json::json!({
        "schema_version":usdb_util::SCOPED_ACTIVATION_REGISTRY_SCHEMA_VERSION,
        "scope":right.scope(),
        "records":usdb_util::BtcActivationRegistryCatalog::from_json(include_str!("fixtures/miner-pass-v2/catalog.json")).unwrap().current_registry().records
    })).unwrap();
    other.records[0].notes.push_str(" distinct exact revision");
    let foreign = BtcRuleTimeline::new(&other)
        .unwrap()
        .indexer_context_at(10)
        .unwrap();
    for rules in [h.rules.indexer_context_at(9).unwrap(), foreign] {
        let error = h
            .manager
            .on_mint_pass(&batch.mints[0], &batch.evidence, &batch.balances, &rules)
            .await
            .unwrap_err();
        assert!(
            error.contains("context height mismatch") || error.contains("rule domain mismatch"),
            "{error}"
        );
        assert!(
            h.storage
                .get_pass_by_inscription_id(&batch.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            h.storage
                .get_mint_audit(&batch.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(h.energy.get_pending_block_height_for_test().unwrap(), None);
        assert!(!h.manager.has_active_block_mutation_collection());
    }
    h.cleanup();
}

#[tokio::test]
async fn old_schema_passes_transfer_and_burn_under_new_rules_without_reactivation() {
    use crate::index::test_miner_evidence as chain;
    use bitcoincore_rpc::bitcoin::{ScriptBuf, opcodes::all::OP_RETURN, script::Builder};
    use std::collections::HashMap;
    use usdb_util::{SpentPrevout, ToBtcScriptHash};

    let catalog = conformance_catalog(&[
        (
            VersionFamily::InscriptionSchemaVersion,
            10,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            10,
            CONFORMANCE_STATE,
        ),
    ]);
    let witness = MintSpec::standard(80, source_script(SpendKind::Witness), 0, vec![]);
    let mut legacy = MintSpec::standard(81, source_script(SpendKind::Legacy), 0, vec![]);
    legacy.source = SpendKind::Legacy;
    let old = MintBlock::new(9, vec![witness, legacy], false);
    let events = MintBlock::new(10, vec![], false);
    let later = MintBlock::new(11, vec![], false);
    let target = cold_recipient(82);
    let burn: ScriptBuf = Builder::new().push_opcode(OP_RETURN).into_script();
    let mut coins = HashMap::new();
    let mut transactions = Vec::new();
    for (index, kind, destination) in [
        (0, SpendKind::Witness, target.clone()),
        (1, SpendKind::Legacy, burn),
    ] {
        let point = old.mints[index].satpoint.outpoint;
        let coin = chain::output(4900, source_script(kind));
        let mut tx = chain::transaction(vec![point], vec![chain::output(4000, destination)]);
        chain::sign(&mut tx, 0, std::slice::from_ref(&coin), kind, 1, false);
        coins.insert(
            point,
            SpentPrevout {
                txout: coin,
                height: 9,
                coinbase: false,
            },
        );
        transactions.push(tx);
    }
    let block = chain::block(transactions);
    events
        .core
        .state
        .lock()
        .unwrap()
        .blocks
        .insert(10, (block.clone(), chain::verbose(10, &block, &coins)));
    let p = Pipeline::with_catalog("old-lifecycle", &[&old, &events, &later], 9, &catalog).await;
    p.sync(9, 9).await.unwrap();
    for height in [10, 11] {
        p.sync(height, height).await.unwrap();
        let storage = p.indexer.miner_pass_storage();
        let transferred = storage
            .get_pass_by_inscription_id(&old.mints[0].inscription_id)
            .unwrap()
            .unwrap();
        let burned = storage
            .get_pass_by_inscription_id(&old.mints[1].inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                transferred.mint_version,
                transferred.state,
                transferred.owner
            ),
            (1, MinerPassState::Dormant, target.to_btc_script_hash())
        );
        assert_eq!(
            (burned.mint_version, burned.state),
            (1, MinerPassState::Burned)
        );
    }
    p.cleanup();
}

#[test]
fn structured_schema_has_an_independent_strict_grammar_and_normalizes_all_bindings() {
    use crate::index::test_miner_rules::CONFORMANCE_SCHEMA_STRUCTURED;
    use crate::index::{InscriptionContentLoader, ParsedMintContent, USDBInscription};
    let catalog = conformance_catalog(&[
        (
            VersionFamily::InscriptionSchemaVersion,
            10,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            20,
            CONFORMANCE_SCHEMA_STRUCTURED,
        ),
    ]);
    let rules = conformance_context(&catalog, 20);
    let id = crate::index::test_miner_state::id(71);
    let main = "0x1111111111111111111111111111111111111111";
    let address =
        bitcoincore_rpc::bitcoin::Address::from_script(&cold_recipient(72), Network::Regtest)
            .unwrap()
            .to_string();
    for (key, value, kind) in [
        ("usdb_main", main.to_string(), MinerPassKind::Standard),
        ("leader_pass_id", id.to_string(), MinerPassKind::Collab),
        ("leader_btc_addr", address, MinerPassKind::Collab),
    ] {
        let content = serde_json::json!({"p":"usdb", "op":"mint", "v":902, "binding": {key: value}, "prev":[id.to_string()]}).to_string();
        let ParsedMintContent::Valid(USDBInscription::Mint(mint)) =
            InscriptionContentLoader::classify_mint_content_str_with_rules(
                &id,
                &content,
                Network::Regtest,
                &rules,
            )
            .unwrap()
        else {
            panic!("valid nested binding rejected: {content}")
        };
        assert_eq!((mint.version, mint.pass_kind), (902, kind));
        assert_eq!(mint.prev, vec![id.to_string()]);
        assert_eq!(
            match key {
                "usdb_main" => mint.usdb_main,
                "leader_pass_id" => mint.leader_pass_id.unwrap(),
                _ => mint.leader_btc_addr.unwrap(),
            },
            value
        );
    }
    let valid = format!(r#"{{"p":"usdb","op":"mint","v":902,"binding":{{"usdb_main":"{main}"}}}}"#);
    for content in [
        valid.replace("\"v\":902", "\"v\":901"),
        valid.replace("\"v\":902", "\"v\":\"902\""),
        valid.replace("\"v\":902", "\"v\":902,\"v\":902"),
        valid.replace("\"binding\":{", "\"unknown\":0,\"binding\":{"),
        valid.replace("\"usdb_main\":", "\"unknown\":0,\"usdb_main\":"),
        valid.replace("\"usdb_main\":", "\"usdb_main\":null,\"usdb_main\":"),
        valid.replace("\"usdb_main\":", "\"leader_pass_id\":null,\"usdb_main\":"),
        valid.replace(&format!("\"{main}\""), "42"),
        valid.replace(&format!("{{\"usdb_main\":\"{main}\"}}"), "[]"),
        valid.replace(&format!("{{\"usdb_main\":\"{main}\"}}"), "{}"),
        valid.replace(
            &format!("\"binding\":{{\"usdb_main\":\"{main}\"}}"),
            &format!("\"usdb_main\":\"{main}\""),
        ),
        valid.replace("\"binding\":{", "\"prev\":null,\"binding\":{"),
        valid.replace(
            &format!("\"usdb_main\":\"{main}\""),
            "\"leader_btc_addr\":\"bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh\"",
        ),
    ] {
        assert!(
            matches!(
                InscriptionContentLoader::classify_mint_content_str_with_rules(
                    &id,
                    &content,
                    Network::Regtest,
                    &rules
                )
                .unwrap(),
                ParsedMintContent::Invalid(_)
            ),
            "{content}"
        );
    }
}

#[tokio::test]
async fn three_schema_epochs_classify_each_wire_grammar_at_both_reveal_boundaries() {
    use crate::index::test_miner_rules::CONFORMANCE_SCHEMA_STRUCTURED;
    let catalog = conformance_catalog(&[
        (
            VersionFamily::InscriptionSchemaVersion,
            10,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            20,
            CONFORMANCE_SCHEMA_STRUCTURED,
        ),
    ]);
    let heights = [9, 10, 11, 19, 20, 21];
    let batches: Vec<_> = heights
        .into_iter()
        .map(|height| {
            let specs = [1, 901, 902]
                .into_iter()
                .enumerate()
                .map(|(slot, version)| {
                    let tag = (height * 3 + slot as u32) as u8;
                    let mut spec = MintSpec::standard(tag, cold_recipient(tag), 0, vec![]);
                    spec.version = version;
                    spec
                })
                .collect();
            MintBlock::new(height, specs, false)
        })
        .collect();
    let empty: Vec<_> = (8..=21)
        .filter(|h| !heights.contains(h))
        .map(|h| MintBlock::new(h, vec![], false))
        .collect();
    let blocks: Vec<_> = batches.iter().chain(empty.iter()).collect();
    let mut p = Pipeline::with_catalog("three-wire-epochs", &blocks, 8, &catalog).await;
    let source: Arc<dyn InscriptionSource> =
        Arc::new(BitcoindInscriptionSource::new(p.core.client.clone()));
    Arc::get_mut(&mut p.indexer)
        .unwrap()
        .replace_inscription_source_for_test(Arc::new(CompareInscriptionSource::new_with_target(
            source.clone(),
            source,
            true,
            CompareTarget::UsdbMint,
        )));
    p.sync_to(21).await.unwrap();
    for (height, batch) in heights.into_iter().zip(&batches) {
        for (slot, version) in [1, 901, 902].into_iter().enumerate() {
            let expected_version = if height < 10 {
                1
            } else if height < 20 {
                901
            } else {
                902
            };
            let store = p.indexer.miner_pass_storage();
            let id = batch.mints[slot].inscription_id;
            let pass = store.get_pass_by_inscription_id(&id).unwrap().unwrap();
            assert_eq!(
                pass.state,
                if version == expected_version {
                    MinerPassState::Active
                } else {
                    MinerPassState::Invalid
                },
                "height={height}, version={version}"
            );
            assert_eq!(
                store
                    .get_mint_audit(&id)
                    .unwrap()
                    .unwrap()
                    .error_code
                    .as_deref(),
                if version == expected_version {
                    None
                } else {
                    Some("INVALID_SCHEMA")
                }
            );
        }
    }
    p.reopen().await.cleanup();
}
