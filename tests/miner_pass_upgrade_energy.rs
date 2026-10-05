//! C-batch conformance: independent per-block reference versus segmented settlement.
//! Test-only contract: at H1=10 convert raw E -> min(2E, MAX), then grow at 2/unit;
//! at H2=20 explicitly preserve units, then grow at 3/unit. Dormant is converted
//! without growth. Both edges preserve the age anchor. Apply H's balance events
//! after growth, penalty=floor(lost_units * retained_age * current_rate * 3/2).
//! Prev retains 95%, 90%, 75% in the three stages, with floor BEFORE summation.
use crate::index::MinerPassState;
use crate::index::energy_settlement::EnergySettlement;
use crate::index::pass::MintOperationPath;
use crate::index::rules::validate_indexer_rules;
use crate::index::test_miner_rules::{
    CONFORMANCE_ENERGY_DOUBLE, CONFORMANCE_ENERGY_TRIPLE, conformance_catalog,
};
use crate::index::test_miner_state::{
    Harness, MintBlock, MintSpec, SpendKind, id, recipient, source_script,
};
use crate::storage::PassEnergyRecord;
use usdb_util::{BtcActivationRegistryCatalog, BtcRuleTimeline, ToBtcScriptHash, VersionFamily};

fn catalog() -> String {
    conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            CONFORMANCE_ENERGY_TRIPLE,
        ),
    ])
}

fn kernel(json: &str) -> EnergySettlement {
    let catalog = BtcActivationRegistryCatalog::from_json(json).unwrap();
    EnergySettlement::new(
        BtcRuleTimeline::new_with_indexer_support(
            catalog.current_registry(),
            validate_indexer_rules,
        )
        .unwrap(),
    )
}

fn record(height: u32, balance: u64, energy: u128) -> PassEnergyRecord {
    PassEnergyRecord {
        inscription_id: id(1),
        block_height: height,
        state: MinerPassState::Active,
        active_block_height: 1,
        owner_address: source_script(SpendKind::Witness).to_btc_script_hash(),
        owner_balance: balance,
        owner_delta: 0,
        energy,
    }
}

// Independent block model: no production formula, version router or interval helper.
fn reference(mut r: PassEnergyRecord, target: u32, changes: &[(u32, u64)]) -> PassEnergyRecord {
    for height in r.block_height + 1..=target {
        if height == 10 {
            r.energy = r.energy.saturating_mul(2);
        }
        let rate = if height < 10 {
            1
        } else if height < 20 {
            2
        } else {
            3
        };
        if r.state == MinerPassState::Active {
            r.energy = r
                .energy
                .saturating_add((r.owner_balance / 100_000) as u128 * rate);
            for &(_, balance) in changes.iter().filter(|(h, _)| *h == height) {
                let before = r.owner_balance / 100_000;
                let after = balance / 100_000;
                let lost = before.saturating_sub(after);
                let penalty =
                    lost as u128 * (height - r.active_block_height) as u128 * rate * 3 / 2;
                r.energy = r.energy.saturating_sub(penalty);
                if (before == 0 && after > 0) || (lost > 0 && after == 0) {
                    r.active_block_height = height;
                }
                r.owner_balance = balance;
            }
        }
        r.block_height = height;
    }
    r
}

#[test]
fn boundary_vectors_do_not_use_the_age_anchor_as_growth_start() {
    let k = kernel(&catalog());
    let r = record(8, 200_000, 11);
    for (height, expected) in [
        (8, 11),
        (9, 13),
        (10, 30),
        (11, 34),
        (19, 66),
        (20, 72),
        (21, 78),
    ] {
        assert_eq!(k.project(&r, height).unwrap(), expected, "height={height}");
    }
    assert!(k.project(&r, 7).unwrap_err().contains("record_height=8"));
    // Unrelated family boundaries must not reapply the energy conversion.
    let mixed = conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            CONFORMANCE_ENERGY_TRIPLE,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            13,
            crate::index::test_miner_rules::CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            15,
            crate::index::test_miner_rules::CONFORMANCE_STATE,
        ),
    ]);
    assert_eq!(kernel(&mixed).project(&r, 21).unwrap(), 78);
}

#[test]
fn three_stages_match_per_block_model_for_sparse_and_dense_checkpoints() {
    let k = kernel(&catalog());
    for state in [
        MinerPassState::Active,
        MinerPassState::Dormant,
        MinerPassState::Consumed,
        MinerPassState::Burned,
        MinerPassState::Invalid,
    ] {
        for balance in [0, 99_999, 100_000, 199_999, 200_000, u64::MAX] {
            for start in [0, 8, 10, 19, 20] {
                for energy in [0, 1, 101, u128::MAX / 2 + 1, u128::MAX] {
                    let mut original = record(start, balance, energy);
                    original.active_block_height = start;
                    original.state = state.clone();
                    if !matches!(state, MinerPassState::Active | MinerPassState::Dormant) {
                        original.energy = 0;
                    }
                    let mut dense = original.clone();
                    for target in start..=30 {
                        let expected = reference(original.clone(), target, &[]);
                        let sparse = k.project(&original, target).unwrap();
                        dense.energy = k.project(&dense, target).unwrap();
                        dense.block_height = target;
                        assert_eq!(
                            sparse, expected.energy,
                            "state={state:?}, balance={balance}, start={start}, target={target}"
                        );
                        assert_eq!(dense.energy, sparse);
                    }
                }
            }
        }
    }
}

#[test]
fn boundary_balance_events_keep_rounding_and_age_through_zero_and_same_height_events() {
    let k = kernel(&catalog());
    let original = record(8, 400_000, 211);
    let changes = [
        (10, 600_000),
        (10, 500_000),
        (10, 400_000),
        (13, 0),
        (14, 99_999),
        (15, 100_001),
        (19, 199_999),
        (20, 99_999),
        (20, 100_000),
        (21, 300_000),
        (24, 200_000),
    ];
    let mut sparse = original.clone();
    for height in 9..=28 {
        for &(_, balance) in changes.iter().filter(|(h, _)| *h == height) {
            let (energy, anchor) = k.balance_change(&sparse, height, balance).unwrap();
            sparse.energy = energy;
            sparse.active_block_height = anchor;
            sparse.owner_balance = balance;
            sparse.block_height = height;
        }
        let expected = reference(original.clone(), height, &changes);
        assert_eq!(
            k.project(&sparse, height).unwrap(),
            expected.energy,
            "height={height}"
        );
        assert_eq!(sparse.active_block_height, expected.active_block_height);
    }
    assert_eq!(sparse.active_block_height, 20);
}

#[test]
fn settlement_rejects_missing_edges_unknown_intervals_and_wrong_scope() {
    let r = record(8, 100_000, 9);
    let missing = conformance_catalog(&[(
        VersionFamily::EnergyFormulaVersion,
        10,
        CONFORMANCE_ENERGY_TRIPLE,
    )]);
    let error = kernel(&missing).project(&r, 21).unwrap_err();
    assert!(
        error.contains("Missing raw-energy transition") && error.contains("boundary_height=10"),
        "{error}"
    );
    let missing_second = conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            usdb_util::ENERGY_FORMULA_VERSION_V1,
        ),
    ]);
    let error = kernel(&missing_second).project(&r, 21).unwrap_err();
    assert!(
        error.contains("Missing raw-energy transition") && error.contains("boundary_height=20"),
        "{error}"
    );
    let unknown = conformance_catalog(&[
        (VersionFamily::EnergyFormulaVersion, 10, "unknown-energy"),
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            usdb_util::ENERGY_FORMULA_VERSION_V1,
        ),
    ]);
    let k = kernel(&unknown);
    assert_eq!(k.project(&r, 9).unwrap(), 10);
    assert!(k.project(&r, 21).unwrap_err().contains("block_height=10"));
    let c = BtcActivationRegistryCatalog::from_json(&catalog()).unwrap();
    for network in [false, true] {
        let mut registry = c.current_registry().clone();
        if network {
            registry.scope.network_id = "btc-mainnet".into();
            registry.scope.network_type = usdb_util::ActivationNetworkType::Mainnet;
        } else {
            registry.scope.rules_scope = Some("another-scope".into());
        }
        let k = EnergySettlement::new(
            BtcRuleTimeline::new_with_indexer_support(&registry, validate_indexer_rules).unwrap(),
        );
        assert!(k.project(&r, 21).is_err());
    }
}

#[test]
fn maximum_height_and_energy_do_not_overflow_or_round_before_discount() {
    let k = kernel(&conformance_catalog(&[]));
    assert_eq!(
        k.project(&record(0, 100_000, 0), u32::MAX).unwrap(),
        u32::MAX as u128
    );
    assert_eq!(
        k.project(&record(u32::MAX, 100_000, 17), u32::MAX).unwrap(),
        17
    );
    let k = kernel(&catalog());
    assert_eq!(
        k.project(&record(8, u64::MAX, u128::MAX / 2 + 1), 30)
            .unwrap(),
        u128::MAX
    );
    assert_eq!(k.inherit(u128::MAX, 20).unwrap(), u128::MAX / 4 * 3 + 2);
    for (height, expected) in [(9, 19), (10, 18), (19, 18), (20, 15)] {
        assert_eq!(k.inherit(20, height).unwrap(), expected);
    }
}

#[tokio::test]
async fn lazy_balance_replay_and_event_writes_match_and_queries_never_write() {
    let catalog = catalog();
    let lazy = Harness::with_catalog("energy-lazy", &catalog);
    let event = Harness::with_catalog("energy-events", &catalog);
    let owner = source_script(SpendKind::Witness).to_btc_script_hash();
    let initial = record(8, 400_000, 211);
    let changes = [
        (10, 500_000),
        (13, 0),
        (14, 99_999),
        (15, 100_001),
        (19, 199_999),
        (20, 99_999),
        (21, 100_000),
        (24, 300_000),
    ];
    for h in [&lazy, &event] {
        h.energy
            .insert_pass_energy_record_for_test(&initial)
            .unwrap();
        h.timeline.set(owner, 8, initial.owner_balance);
        for &(height, balance) in &changes {
            h.timeline.set(owner, height, balance);
        }
    }
    let expected = reference(initial.clone(), 28, &changes);
    assert_eq!(
        lazy.energy
            .update_pass_energy(&id(1), 28)
            .await
            .unwrap()
            .energy,
        expected.energy
    );
    let mut before = initial.owner_balance;
    for &(height, balance) in &changes {
        let delta = balance as i64 - before as i64;
        assert!(
            event
                .energy
                .apply_active_balance_change(&id(1), &owner, height, balance, delta)
                .unwrap()
        );
        assert!(
            !event
                .energy
                .apply_active_balance_change(&id(1), &owner, height, balance, delta)
                .unwrap()
        );
        before = balance;
    }
    for height in 8..=28 {
        let expected = reference(initial.clone(), height, &changes);
        for h in [&lazy, &event] {
            let before = h
                .energy
                .get_pass_energy_record_at_or_before(&id(1), height)
                .unwrap()
                .unwrap();
            for _ in 0..2 {
                assert_eq!(
                    h.energy
                        .get_pass_energy(&id(1), height)
                        .await
                        .unwrap()
                        .unwrap()
                        .energy,
                    expected.energy
                );
            }
            let after = h
                .energy
                .get_pass_energy_record_at_or_before(&id(1), height)
                .unwrap()
                .unwrap();
            assert_eq!(
                (
                    before.block_height,
                    before.energy,
                    before.active_block_height
                ),
                (after.block_height, after.energy, after.active_block_height)
            );
            assert_eq!(after.active_block_height, expected.active_block_height);
            assert_eq!(
                h.energy
                    .get_pass_energy_record_exact(&id(1), height)
                    .unwrap()
                    .is_some(),
                height == 8 || changes.iter().any(|(h, _)| *h == height)
            );
        }
    }
    lazy.cleanup();
    event.cleanup();
}

#[tokio::test]
async fn dormant_burn_and_consumption_use_the_same_boundary_settlement() {
    let h = Harness::with_catalog("energy-lifecycle", &catalog());
    let r = record(8, 200_000, 11);
    h.energy.insert_pass_energy_record_for_test(&r).unwrap();
    h.energy.on_pass_dormant(&id(1), 9).await.unwrap();
    assert_eq!(
        h.energy
            .get_pass_energy(&id(1), 10)
            .await
            .unwrap()
            .unwrap()
            .energy,
        26
    );
    assert_eq!(
        h.energy
            .get_pass_energy(&id(1), 20)
            .await
            .unwrap()
            .unwrap()
            .energy,
        26
    );
    h.energy
        .on_pass_burned(&id(1), &r.owner_address, MinerPassState::Dormant, 20)
        .await
        .unwrap();
    assert_eq!(
        h.energy
            .get_pass_energy(&id(1), 21)
            .await
            .unwrap()
            .unwrap()
            .energy,
        0
    );
    let mut active = r.clone();
    active.inscription_id = id(2);
    h.energy
        .insert_pass_energy_record_for_test(&active)
        .unwrap();
    h.energy.on_pass_dormant(&id(2), 10).await.unwrap();
    assert_eq!(
        h.energy
            .get_pass_energy_record_exact(&id(2), 10)
            .unwrap()
            .unwrap()
            .energy,
        30
    );
    h.energy
        .on_pass_consumed(&id(2), &r.owner_address, 20)
        .unwrap();
    assert_eq!(
        h.energy
            .get_pass_energy(&id(2), 21)
            .await
            .unwrap()
            .unwrap()
            .energy,
        0
    );
    active.inscription_id = id(3);
    h.energy
        .insert_pass_energy_record_for_test(&active)
        .unwrap();
    h.energy
        .on_pass_burned(&id(3), &r.owner_address, MinerPassState::Active, 10)
        .await
        .unwrap();
    assert_eq!(
        h.energy
            .get_pass_energy(&id(3), 21)
            .await
            .unwrap()
            .unwrap()
            .energy,
        0
    );
    h.cleanup();
}

#[tokio::test]
async fn multi_epoch_prev_is_converted_then_discounted_individually_for_both_owner_paths() {
    for same_owner in [false, true] {
        for raw in [3, u128::MAX] {
            let h = Harness::with_catalog("energy-inheritance", &catalog());
            let source = source_script(SpendKind::Witness);
            let owner = source.to_btc_script_hash();
            for (tag, height) in [(1, 4), (2, 5), (3, 12)] {
                h.seed(tag, owner, height, vec![]).await;
            }
            for (tag, height, state) in [
                (1, 5, MinerPassState::Dormant),
                (2, 12, MinerPassState::Dormant),
                (3, 12, MinerPassState::Active),
            ] {
                let mut r = record(height, 0, raw);
                r.inscription_id = id(tag);
                r.state = state;
                h.energy.insert_pass_energy_record_for_test(&r).unwrap();
            }
            let dest = if same_owner { source } else { recipient(80) };
            let b = MintBlock::new(
                20,
                vec![MintSpec::standard(
                    80,
                    dest,
                    u64::from(same_owner),
                    vec![id(1), id(2), id(3)],
                )],
                false,
            );
            let guard = h.begin(20);
            let outcome = h
                .manager
                .on_mint_pass(
                    &b.mints[0],
                    &b.evidence,
                    &b.balances,
                    &h.rules.indexer_context_at(20).unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                outcome,
                Some(if same_owner {
                    MintOperationPath::SameOwner
                } else {
                    MintOperationPath::CrossOwner
                })
            );
            h.finish(20, guard);
            // floor(6*.75) + floor(3*.75) + floor(3*.75) = 8, not floor(12*.75)=9.
            let expected = if raw == 3 { 8 } else { u128::MAX };
            assert_eq!(
                h.energy
                    .get_pass_energy(&b.mints[0].inscription_id, 20)
                    .await
                    .unwrap()
                    .unwrap()
                    .energy,
                expected
            );
            for tag in 1..=3 {
                assert_eq!(h.state(&id(tag)), MinerPassState::Consumed);
                assert_eq!(
                    h.energy
                        .get_pass_energy_record_exact(&id(tag), 20)
                        .unwrap()
                        .unwrap()
                        .energy,
                    0
                );
            }
            h.cleanup();
        }
    }
}

#[tokio::test]
async fn unsupported_settlement_does_not_partially_write_earlier_balance_events() {
    let json = conformance_catalog(&[(
        VersionFamily::EnergyFormulaVersion,
        20,
        CONFORMANCE_ENERGY_TRIPLE,
    )]);
    let h = Harness::with_catalog("energy-missing-path", &json);
    let r = record(8, 100_000, 31);
    h.energy.insert_pass_energy_record_for_test(&r).unwrap();
    h.timeline.set(r.owner_address, 8, 100_000);
    h.timeline.set(r.owner_address, 9, 200_000);
    let error = h.energy.update_pass_energy(&id(1), 21).await.unwrap_err();
    assert!(error.contains("boundary_height=20"));
    assert!(
        h.energy
            .get_pass_energy_record_exact(&id(1), 9)
            .unwrap()
            .is_none()
    );
    assert!(
        h.energy
            .on_pass_burned(&id(1), &r.owner_address, MinerPassState::Active, 21)
            .await
            .is_err()
    );
    assert!(
        h.energy
            .get_pass_energy_record_exact(&id(1), 21)
            .unwrap()
            .is_none()
    );
    h.cleanup();
}

#[tokio::test]
async fn full_block_pipeline_inherits_at_second_boundary_and_retries_without_double_conversion() {
    use crate::index::test_miner_pipeline::{Pipeline, cold_recipient};
    let source = source_script(SpendKind::Witness);
    let owner = source.to_btc_script_hash();
    let old = MintBlock::new(
        8,
        vec![MintSpec::standard(90, source.clone(), 1, vec![])],
        false,
    );
    let middle = MintBlock::new(12, vec![MintSpec::standard(91, source, 1, vec![])], false);
    let new = MintBlock::new(
        20,
        vec![MintSpec::standard(
            92,
            cold_recipient(92),
            0,
            vec![old.mints[0].inscription_id, middle.mints[0].inscription_id],
        )],
        false,
    );
    let empty: Vec<_> = (9..20)
        .filter(|h| *h != 12)
        .map(|h| MintBlock::new(h, vec![], false))
        .collect();
    let mut blocks = vec![&old, &middle, &new];
    blocks.extend(empty.iter());
    let p = Pipeline::with_catalog("energy-full-block", &blocks, 8, &catalog()).await;
    let balances = p.history.lock().unwrap().values[&owner].clone();
    let changes: Vec<_> = balances.iter().map(|(&h, &b)| (h, b)).collect();
    let mut first = record(8, balances[&8], 0);
    first.active_block_height = 8;
    let first = reference(first, 12, &changes);
    let mut second = record(12, balances[&12], 0);
    second.active_block_height = 12;
    let second = reference(second, 20, &changes);
    let expected = first.energy * 3 / 4 + second.energy * 3 / 4;
    assert!(expected > 0);
    p.sync(8, 19).await.unwrap();
    let energy = p.indexer.pass_energy_manager();
    assert_eq!(
        energy
            .get_pass_energy(&old.mints[0].inscription_id, 19)
            .await
            .unwrap()
            .unwrap()
            .energy,
        first.energy
    );
    p.history.lock().unwrap().fail_commit = Some(20);
    assert!(p.sync(20, 20).await.is_err());
    for (mint, state) in [
        (&old.mints[0], MinerPassState::Dormant),
        (&middle.mints[0], MinerPassState::Active),
    ] {
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_pass_by_inscription_id(&mint.inscription_id)
                .unwrap()
                .unwrap()
                .state,
            state
        );
        assert!(
            energy
                .get_pass_energy_record_exact(&mint.inscription_id, 20)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_pass_by_inscription_id(&new.mints[0].inscription_id)
            .unwrap()
            .is_none()
    );
    assert!(
        energy
            .get_pass_energy_record_exact(&new.mints[0].inscription_id, 20)
            .unwrap()
            .is_none()
    );
    assert_eq!(energy.get_pending_block_height_for_test().unwrap(), None);
    p.history.lock().unwrap().fail_commit = None;
    p.sync(20, 20).await.unwrap();
    assert_eq!(
        energy
            .get_pass_energy(&new.mints[0].inscription_id, 20)
            .await
            .unwrap()
            .unwrap()
            .energy,
        expected
    );
    for parent in [&old.mints[0], &middle.mints[0]] {
        assert_eq!(
            energy
                .get_pass_energy(&parent.inscription_id, 20)
                .await
                .unwrap()
                .unwrap()
                .energy,
            0
        );
        assert_eq!(
            p.indexer
                .miner_pass_storage()
                .get_pass_by_inscription_id(&parent.inscription_id)
                .unwrap()
                .unwrap()
                .state,
            MinerPassState::Consumed
        );
    }
    assert_eq!(
        p.indexer
            .miner_pass_storage()
            .get_mint_audit(&new.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .operation_path
            .as_deref(),
        Some("cross_owner")
    );
    let p = p.reopen().await;
    assert_eq!(
        p.indexer
            .pass_energy_manager()
            .get_pass_energy(&new.mints[0].inscription_id, 20)
            .await
            .unwrap()
            .unwrap()
            .energy,
        expected
    );
    p.cleanup();
}
