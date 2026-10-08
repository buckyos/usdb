//! UIP-0017 economic RPC consistency, historical registry prefixes and cache isolation.
use crate::index::test_miner_queries::{
    QueryScenario, activation_golden, catalog, context, params,
};
use crate::index::test_miner_rules::{CONFORMANCE_EFFECTIVE, conformance_catalog};
use crate::index::test_miner_upgrade::fingerprint;
use crate::service::rpc::UsdbIndexerRpc;
use serde_json::{Value, json};
use usdb_util::{BtcActivationRegistryCatalog, USDB_ECONOMIC_STATE_VIEW_VERSION, VersionFamily};

const VIEW: &str = USDB_ECONOMIC_STATE_VIEW_VERSION;

#[tokio::test]
async fn every_economic_surface_uses_the_query_height_rules_without_writing_energy() {
    let scenario = QueryScenario::new(false);
    let catalog = catalog();
    let p = scenario.pipeline("economic-queries", &catalog).await;
    p.sync_to(22).await.unwrap();
    let before = fingerprint(&p, &scenario.ids, 22).await;
    let rpc = p.rpc();
    let mut profiles = Vec::new();
    let mut breakdowns = Vec::new();
    for height in [9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22] {
        let profile = rpc.get_pass_economic_profile(params(json!({"view_version":VIEW,"pass_id":scenario.ids[0].to_string(),"block_height":height}))).unwrap();
        let energy = rpc
            .get_pass_energy(params(
                json!({"inscription_id":scenario.ids[0].to_string(),"block_height":height}),
            ))
            .unwrap();
        let candidates = rpc
            .get_candidate_set_view(params(
                json!({"view_version":VIEW,"block_height":height,"limit":100}),
            ))
            .unwrap();
        let collabs = rpc.get_collab_breakdown(params(json!({"view_version":VIEW,"leader_pass_id":scenario.ids[0].to_string(),"block_height":height,"limit":100}))).unwrap();
        let miner = rpc.resolve_miner_candidate(params(json!({"view_version":VIEW,"usdb_main":profile.pass.usdb_main,"block_height":height}))).unwrap();
        let aggregate = rpc
            .get_miner_economic_aggregate(params(
                json!({"view_version":VIEW,"block_height":height}),
            ))
            .unwrap();
        let mut raw = Vec::new();
        for (&id, birth) in scenario.ids.iter().zip([8, 9, 11, 12]) {
            if height < birth {
                raw.push(0);
                continue;
            }
            let original = p
                .indexer
                .pass_energy_manager()
                .get_pass_energy_record_exact(&id, birth)
                .unwrap()
                .unwrap();
            let changes = p.history.lock().unwrap().values[&original.owner_address]
                .iter()
                .filter(|(h, _)| **h > birth && **h <= height)
                .map(|(&h, &b)| (h, b))
                .collect::<Vec<_>>();
            let expected =
                crate::index::test_energy_reference::reference(original, height, &changes).energy;
            let item = rpc
                .get_pass_energy(params(
                    json!({"inscription_id":id.to_string(),"block_height":height}),
                ))
                .unwrap();
            assert_eq!(item.raw_energy, expected.to_string(), "{height} {id}");
            raw.push(expected);
        }
        let weight = if height < 13 { 5_000u128 } else { 2_500 };
        let contribution = raw[1] * weight / 10_000 + raw[2] * weight / 10_000;
        let effective = raw[0] + contribution;
        // These fixtures stay below the first v1 level threshold. The synthetic contract
        // is independently specified: floor(E/1000), capped at 50; factor 10000-100*level.
        let level = if height < 16 {
            0
        } else {
            (effective / 1000).min(50) as u8
        };
        assert_eq!(profile.pass.raw_energy, raw[0].to_string());
        assert_eq!(profile.pass.collab_contribution, contribution.to_string());
        assert_eq!(profile.pass.effective_energy, effective.to_string());
        assert_eq!(profile.pass.level, level);
        assert_eq!(
            profile.pass.difficulty_factor_bps,
            10_000 - 100 * u64::from(level)
        );
        assert_eq!(energy.effective_energy, profile.pass.effective_energy);
        assert_eq!(energy.level, level);
        assert_eq!(
            energy.difficulty_factor_bps,
            profile.pass.difficulty_factor_bps
        );
        let row = candidates
            .items
            .iter()
            .find(|i| i.pass_id == profile.pass.pass_id)
            .unwrap();
        assert_eq!(row.effective_energy, profile.pass.effective_energy);
        assert_eq!(row.level, level);
        assert_eq!(
            row.difficulty_factor_bps,
            profile.pass.difficulty_factor_bps
        );
        assert_eq!(
            serde_json::to_value(&miner.pass).unwrap(),
            serde_json::to_value(&profile.pass).unwrap()
        );
        assert_eq!(aggregate.miner_aggregate, profile.miner_aggregate);
        assert_eq!(
            collabs.aggregate_collab_contribution,
            contribution.to_string()
        );
        for item in &collabs.items {
            assert_eq!(item.collab_weight_bps, weight as u64);
        }
        assert_eq!(collabs.external_state, profile.external_state);
        assert_eq!(candidates.external_state, profile.external_state);
        assert_eq!(aggregate.external_state, profile.external_state);
        let ranking = rpc
            .get_pass_energy_leaderboard(params(
                json!({"at_height":height,"page":0,"page_size":100}),
            ))
            .unwrap();
        for row in ranking.items {
            let index = scenario
                .ids
                .iter()
                .position(|id| id.to_string() == row.inscription_id)
                .unwrap();
            assert_eq!(row.energy, raw[index].to_string());
        }
        breakdowns.push(collabs);
        profiles.push(profile);
    }
    // Range/exact expose committed checkpoints; at-or-before performs projection. Empty
    // upgrade blocks do not materialize rows merely because one of these RPCs was called.
    let range=rpc.get_pass_energy_range(params(json!({"inscription_id":scenario.ids[0].to_string(),"from_height":8,"to_height":22,"page":0,"page_size":100}))).unwrap();
    for row in &range.items {
        let exact=rpc.get_pass_energy(params(json!({"inscription_id":scenario.ids[0].to_string(),"block_height":row.record_block_height,"mode":"exact"}))).unwrap();
        assert_eq!(exact.raw_energy, row.energy);
    }
    assert!(
        !range
            .items
            .iter()
            .any(|row| [13, 16, 20].contains(&row.record_block_height))
    );
    assert!(
        rpc.get_pass_energy(params(
            json!({"inscription_id":scenario.ids[0].to_string(),"block_height":20,"mode":"exact"})
        ))
        .is_err()
    );
    assert_eq!(fingerprint(&p, &scenario.ids, 22).await, before);
    let artifact = json!({"schema_version":"miner-pass-upgrade-economic-queries:v1","registry":activation_golden(&catalog),"profiles":profiles,"breakdowns":breakdowns});
    if let Some(path) = std::env::var_os("USDB_WRITE_UPGRADE_QUERY_FIXTURE") {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&artifact).unwrap() + "\n",
        )
        .unwrap();
    } else {
        let expected: Value = serde_json::from_str(include_str!(
            "fixtures/miner-pass-upgrade/economic-queries.json"
        ))
        .unwrap();
        assert_eq!(
            artifact, expected,
            "Regenerate and review paired Rust/Go query vectors when intentionally changing the fixture contract"
        );
    }
    drop(rpc);
    p.cleanup();
}

#[tokio::test]
async fn registry_queries_require_the_whole_prefix_even_when_the_last_versions_match() {
    let base = BtcActivationRegistryCatalog::from_json(&conformance_catalog(&[]))
        .unwrap()
        .current_registry()
        .clone();
    let next = BtcActivationRegistryCatalog::from_json(&conformance_catalog(&[
        (
            VersionFamily::EffectiveEnergyFormulaVersion,
            10,
            CONFORMANCE_EFFECTIVE,
        ),
        (
            VersionFamily::EffectiveEnergyFormulaVersion,
            20,
            usdb_util::EFFECTIVE_ENERGY_FORMULA_VERSION_V1,
        ),
    ]))
    .unwrap()
    .current_registry()
    .clone();
    let old_id = base.activation_registry_id();
    let catalog = BtcActivationRegistryCatalog::from_revisions(vec![base, next]).unwrap();
    let scenario = QueryScenario::new(false);
    let p = scenario
        .pipeline("registry-prefix", &catalog.to_json().unwrap())
        .await;
    p.sync_to(22).await.unwrap();
    let rpc = p.rpc();
    let before = fingerprint(&p, &scenario.ids, 22).await;
    let old=rpc.get_pass_economic_profile(params(json!({"view_version":VIEW,"pass_id":scenario.ids[0].to_string(),"block_height":9,"context":context(&old_id,9)}))).unwrap();
    assert_eq!(old.external_state.activation_registry_id, old_id);
    let current = rpc
        .get_pass_economic_profile(params(
            json!({"view_version":VIEW,"pass_id":scenario.ids[0].to_string(),"block_height":9}),
        ))
        .unwrap();
    assert_eq!(old.pass.raw_energy, current.pass.raw_energy);
    assert_eq!(
        old.external_state.system_state_id,
        current.external_state.system_state_id
    );
    assert_ne!(
        old.external_state.activation_registry_id,
        current.external_state.activation_registry_id
    );
    assert_eq!(
        p.indexer
            .active_version_set_at_with_registry(&old_id, 22)
            .unwrap(),
        p.indexer.active_version_set_at(22).unwrap()
    );
    for height in [10, 19, 20, 22] {
        let ctx = context(&old_id, height);
        let errors=vec![
            rpc.get_state_ref_at_height(params(json!({"block_height":height,"context":ctx}))).unwrap_err(),
            rpc.get_pass_economic_profile(params(json!({"view_version":VIEW,"pass_id":scenario.ids[0].to_string(),"block_height":height,"context":ctx}))).unwrap_err(),
            rpc.get_candidate_set_view(params(json!({"view_version":VIEW,"block_height":height,"context":ctx,"limit":1}))).unwrap_err(),
            rpc.get_collab_breakdown(params(json!({"view_version":VIEW,"leader_pass_id":scenario.ids[0].to_string(),"block_height":height,"context":ctx,"limit":1}))).unwrap_err(),
            rpc.get_pass_energy(params(json!({"inscription_id":scenario.ids[0].to_string(),"block_height":height,"context":ctx}))).unwrap_err(),
            rpc.get_pass_mint_audit(params(json!({"inscription_id":scenario.ids[0].to_string(),"at_height":height,"context":ctx}))).unwrap_err(),
            rpc.get_miner_economic_aggregate(params(json!({"view_version":VIEW,"block_height":height,"context":ctx}))).unwrap_err(),
            rpc.resolve_miner_candidate(params(json!({"view_version":VIEW,"usdb_main":current.pass.usdb_main,"block_height":height,"context":ctx}))).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                error.code,
                jsonrpc_core::ErrorCode::ServerError(
                    usdb_util::CONSENSUS_RPC_ERR_ACTIVE_VERSION_SET_MISMATCH
                )
            );
            assert!(
                error
                    .data
                    .unwrap()
                    .to_string()
                    .contains("first_difference_height=10")
            );
        }
    }
    assert_eq!(fingerprint(&p, &scenario.ids, 22).await, before);
    drop(rpc);
    p.cleanup();
}

#[tokio::test]
async fn same_height_reorg_invalidates_warm_caches_and_both_economic_cursors() {
    let old = QueryScenario::new(false);
    let new = QueryScenario::new(true);
    let p = old.pipeline("query-reorg", &catalog()).await;
    let clean = new.pipeline("query-reorg-clean", &catalog()).await;
    p.sync_to(22).await.unwrap();
    clean.sync_to(22).await.unwrap();
    let rpc = p.rpc();
    let ranking_request = json!({"page":0,"page_size":100});
    let candidate_request = json!({"view_version":VIEW,"limit":1});
    let collab_request =
        json!({"view_version":VIEW,"leader_pass_id":old.ids[0].to_string(),"limit":1});
    let ranking = rpc
        .get_pass_energy_leaderboard(params(ranking_request.clone()))
        .unwrap();
    let candidate = rpc
        .get_candidate_set_view(params(candidate_request.clone()))
        .unwrap();
    let collab = rpc
        .get_collab_breakdown(params(collab_request.clone()))
        .unwrap();
    assert!(candidate.next_cursor.is_some() && collab.next_cursor.is_some());
    // Warm all three caches before changing only upstream canonical history.
    rpc.get_pass_energy_leaderboard(params(ranking_request.clone()))
        .unwrap();
    rpc.get_candidate_set_view(params(candidate_request.clone()))
        .unwrap();
    rpc.get_collab_breakdown(params(collab_request.clone()))
        .unwrap();
    p.follow_chain(&clean);
    p.sync_to(22).await.unwrap();
    let mut continuation = candidate_request.clone();
    continuation["cursor"] = candidate.next_cursor.unwrap().into();
    assert!(rpc.get_candidate_set_view(params(continuation)).is_err());
    let mut continuation = collab_request.clone();
    continuation["cursor"] = collab.next_cursor.unwrap().into();
    assert!(rpc.get_collab_breakdown(params(continuation)).is_err());
    let ranking_after = rpc
        .get_pass_energy_leaderboard(params(ranking_request.clone()))
        .unwrap();
    assert_ne!(
        serde_json::to_value(&ranking).unwrap(),
        serde_json::to_value(&ranking_after).unwrap()
    );
    assert_eq!(
        serde_json::to_value(ranking_after).unwrap(),
        serde_json::to_value(
            clean
                .rpc()
                .get_pass_energy_leaderboard(params(ranking_request))
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        serde_json::to_value(
            rpc.get_candidate_set_view(params(candidate_request.clone()))
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(
            clean
                .rpc()
                .get_candidate_set_view(params(candidate_request))
                .unwrap()
        )
        .unwrap()
    );
    assert_eq!(
        serde_json::to_value(
            rpc.get_collab_breakdown(params(collab_request.clone()))
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(
            clean
                .rpc()
                .get_collab_breakdown(params(collab_request))
                .unwrap()
        )
        .unwrap()
    );
    drop(rpc);
    p.cleanup();
    clean.cleanup();
}

#[tokio::test]
async fn compatible_registry_cursors_pin_the_revision_and_height_across_head_advances() {
    let base = BtcActivationRegistryCatalog::from_json(&catalog())
        .unwrap()
        .current_registry()
        .clone();
    let mut next = base.clone();
    let mut future = next
        .records
        .iter()
        .find(|r| r.version_family == VersionFamily::PassStateMachineVersion)
        .unwrap()
        .clone();
    future.activation_height = 30;
    future.supersedes = Some(future.version_value.clone());
    future.version_value =
        usdb_util::VersionValue::String(crate::index::test_miner_rules::CONFORMANCE_STATE.into());
    next.records.push(future);
    let old_id = base.activation_registry_id();
    let catalog = BtcActivationRegistryCatalog::from_revisions(vec![base, next]).unwrap();
    let new_id = catalog.current_registry_id().to_string();
    let scenario = QueryScenario::new(false);
    let p = scenario
        .pipeline("registry-cursors", &catalog.to_json().unwrap())
        .await;
    p.sync_to(16).await.unwrap();
    let rpc = p.rpc();
    let candidate = rpc
        .get_candidate_set_view(params(
            json!({"view_version":VIEW,"limit":1,"context":context(&old_id,16)}),
        ))
        .unwrap();
    let collab = rpc.get_collab_breakdown(params(json!({"view_version":VIEW,"leader_pass_id":scenario.ids[0].to_string(),"limit":1,"context":context(&old_id,16)}))).unwrap();
    p.sync_to(22).await.unwrap();
    let mut candidate_request =
        json!({"view_version":VIEW,"limit":1,"cursor":candidate.next_cursor.unwrap()});
    let mut collab_request = json!({"view_version":VIEW,"leader_pass_id":scenario.ids[0].to_string(),"limit":1,"cursor":collab.next_cursor.unwrap()});
    let rest = rpc
        .get_candidate_set_view(params(candidate_request.clone()))
        .unwrap();
    assert_eq!(rest.external_state, candidate.external_state);
    assert_ne!(rest.items[0].pass_id, candidate.items[0].pass_id);
    assert!(rest.next_cursor.is_none());
    let rest = rpc
        .get_collab_breakdown(params(collab_request.clone()))
        .unwrap();
    assert_eq!(rest.external_state, collab.external_state);
    assert_ne!(rest.items[0].collab_pass_id, collab.items[0].collab_pass_id);
    assert!(rest.next_cursor.is_none());
    // Equal execution prefixes and equal state hashes do not allow changing a cursor's revision.
    for ctx in [context(&new_id, 16), context(&old_id, 22)] {
        candidate_request["context"] = ctx.clone();
        collab_request["context"] = ctx;
        assert!(
            rpc.get_candidate_set_view(params(candidate_request.clone()))
                .is_err()
        );
        assert!(
            rpc.get_collab_breakdown(params(collab_request.clone()))
                .is_err()
        );
    }
    drop(rpc);
    p.cleanup();
}

#[test]
fn derived_rules_reject_unknown_contracts_and_preserve_integer_boundaries() {
    use crate::index::economic_rules::EconomicRules;
    use crate::index::rules::validate_indexer_rules;
    use crate::index::test_miner_rules::conformance_context;
    use usdb_util::{BtcRuleTimeline, VersionValue};
    let set = conformance_context(&catalog(), 22)
        .active_version_set()
        .clone();
    let rules = EconomicRules::from_versions(&set).unwrap();
    for raw in [0, 1, 3, 4, 5, 999, u128::MAX] {
        assert_eq!(rules.collab(raw), raw / 4);
    }
    assert_eq!(rules.effective(u128::MAX, 1), u128::MAX);
    for (energy, level) in [
        (0, 0),
        (999, 0),
        (1000, 1),
        (49999, 49),
        (50000, 50),
        (u128::MAX, 50),
    ] {
        assert_eq!(
            rules.level_and_factor(energy),
            (level, 10_000 - 100 * u128::from(level))
        );
    }
    let base = BtcActivationRegistryCatalog::from_json(&catalog())
        .unwrap()
        .current_registry()
        .clone();
    for family in [
        VersionFamily::EffectiveEnergyFormulaVersion,
        VersionFamily::LevelFormulaVersion,
        VersionFamily::QuerySemanticsVersion,
    ] {
        let mut changed = base.clone();
        changed
            .records
            .iter_mut()
            .rev()
            .find(|r| r.version_family == family)
            .unwrap()
            .version_value = VersionValue::String("unknown-contract".into());
        let timeline =
            BtcRuleTimeline::new_with_indexer_support(&changed, validate_indexer_rules).unwrap();
        assert!(timeline.indexer_context_at(22).is_err(), "{family:?}");
    }
    for mainnet in [false, true] {
        let mut changed = base.clone();
        if mainnet {
            changed.scope.network_id = "btc-mainnet".into();
            changed.scope.network_type = usdb_util::ActivationNetworkType::Mainnet;
        } else {
            changed.scope.rules_scope = Some("other-scope".into());
        }
        let timeline =
            BtcRuleTimeline::new_with_indexer_support(&changed, validate_indexer_rules).unwrap();
        assert!(timeline.indexer_context_at(22).is_err());
    }
}

#[test]
fn live_service_catalog_is_explicit_and_matches_the_shared_contract() {
    use crate::index::test_miner_rules::*;
    let catalog = conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            160,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            180,
            CONFORMANCE_ENERGY_TRIPLE,
        ),
        (
            VersionFamily::EffectiveEnergyFormulaVersion,
            163,
            CONFORMANCE_EFFECTIVE,
        ),
        (VersionFamily::LevelFormulaVersion, 166, CONFORMANCE_LEVEL),
        (
            VersionFamily::InscriptionSchemaVersion,
            170,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            172,
            CONFORMANCE_STATE,
        ),
    ]);
    let source = conformance_catalog(&[]);
    let middle = crate::index::test_miner_epochs::append_revision(&source, &catalog);
    let mut last: Value = serde_json::from_str(&catalog).unwrap();
    let records = last["registries"][0]["records"].as_array_mut().unwrap();
    let mut schema = records
        .iter()
        .find(|r| r["version_value"] == CONFORMANCE_SCHEMA)
        .unwrap()
        .clone();
    schema["activation_height"] = 210.into();
    schema["supersedes"] = CONFORMANCE_SCHEMA.into();
    schema["version_value"] = CONFORMANCE_SCHEMA_STRUCTURED.into();
    records.push(schema);
    let registry: usdb_util::BtcActivationRegistry =
        serde_json::from_value(last["registries"][0].clone()).unwrap();
    last["current_registry_id"] = registry.activation_registry_id().into();
    let final_catalog =
        crate::index::test_miner_epochs::append_revision(&middle, &last.to_string());
    for (env, generated, checked) in [
        (
            "USDB_WRITE_LIVE_SOURCE_CATALOG",
            source,
            include_str!("fixtures/miner-pass-upgrade/live-source-catalog.json"),
        ),
        (
            "USDB_WRITE_LIVE_MIDDLE_CATALOG",
            middle,
            include_str!("fixtures/miner-pass-upgrade/live-middle-catalog.json"),
        ),
        (
            "USDB_WRITE_LIVE_UPGRADE_CATALOG",
            final_catalog,
            include_str!("fixtures/miner-pass-upgrade/live-catalog.json"),
        ),
    ] {
        let value: Value = serde_json::from_str(&generated).unwrap();
        if let Some(path) = std::env::var_os(env) {
            std::fs::write(path, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
        } else {
            assert_eq!(
                value,
                serde_json::from_str::<Value>(checked).unwrap(),
                "{env}"
            );
        }
    }
}
