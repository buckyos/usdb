//! Rule-history inspection, execution capability and interval identity stay separate.
use usdb_util::{
    ActivationNetworkType, ActivationRegistryError, ActivationStatus, BtcActivationRecord,
    BtcActivationRegistry, BtcActivationRegistryCatalog, BtcRuleTimeline,
    ENERGY_FORMULA_VERSION_V1, INSCRIPTION_SCHEMA_VERSION_V1, LEVEL_FORMULA_VERSION_V1,
    VersionFamily, VersionValue,
};

fn registry() -> BtcActivationRegistry {
    BtcActivationRegistryCatalog::from_json(include_str!("fixtures/miner-pass-v2/catalog.json"))
        .unwrap()
        .current_registry()
        .clone()
}

fn activate(registry: &mut BtcActivationRegistry, family: VersionFamily, height: u32, value: &str) {
    let previous = registry
        .records
        .iter()
        .filter(|r| r.version_family == family && r.status == ActivationStatus::Active)
        .max_by_key(|r| r.activation_height)
        .unwrap();
    registry.records.push(BtcActivationRecord {
        uip: previous.uip.clone(),
        version_family: family,
        version_value: VersionValue::String(value.into()),
        activation_height: height.into(),
        status: ActivationStatus::Active,
        supersedes: Some(previous.version_value.clone()),
        notes: "Timeline inspection fixture; no executor is registered for synthetic versions."
            .into(),
    });
}

#[test]
fn existing_registry_and_active_set_identities_are_unchanged() {
    let catalog = BtcActivationRegistryCatalog::from_json(include_str!(
        "fixtures/miner-pass-v2/catalog-staged.json"
    ))
    .unwrap();
    for id in catalog.registry_ids() {
        let registry = catalog.registry_by_id(id).unwrap();
        let timeline = BtcRuleTimeline::new(registry).unwrap();
        assert_eq!(timeline.activation_registry_id(), id);
        assert_eq!(timeline.scope(), &registry.scope);
        for height in [0, 9, 10, 100, 10_000, u32::MAX] {
            let context = timeline.indexer_context_at(height).unwrap();
            assert_eq!(context.btc_height(), height);
            assert_eq!(context.activation_registry_id(), id);
            assert_eq!(context.scope(), &registry.scope);
            assert_eq!(
                context.active_version_set(),
                &registry.lookup_active_version_set(height).unwrap()
            );
            // Frozen schema-v1/state-v2 fixture vector, independent of the timeline implementation.
            assert_eq!(
                context.active_version_set_id(),
                "58a5bf5a3cfdba184c57a7ab13d6d7d6c19a359da467625ebc0392459a0f2a18"
            );
        }
    }
    let original =
        BtcRuleTimeline::new(catalog.registry_by_id(&catalog.registry_ids()[0]).unwrap()).unwrap();
    let staged =
        BtcRuleTimeline::new(catalog.registry_by_id(&catalog.registry_ids()[1]).unwrap()).unwrap();
    original.ensure_same_history(&staged, 0, u32::MAX).unwrap();
}

#[test]
fn independent_families_merge_boundaries_and_select_inclusive_activation_height() {
    let mut registry = registry();
    activate(
        &mut registry,
        VersionFamily::EnergyFormulaVersion,
        100,
        "test-energy:second",
    );
    activate(
        &mut registry,
        VersionFamily::LevelFormulaVersion,
        100,
        "test-level:second",
    );
    activate(
        &mut registry,
        VersionFamily::EnergyFormulaVersion,
        200,
        "test-energy:third",
    );
    // Input order must not affect either lookup or interval order.
    registry.records.reverse();
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    let intervals = timeline.intervals(90..=210).unwrap();
    assert_eq!(
        intervals
            .iter()
            .map(|i| (i.start_height, i.end_height))
            .collect::<Vec<_>>(),
        [(90, 99), (100, 199), (200, 210)]
    );
    for (height, energy, level) in [
        (99, ENERGY_FORMULA_VERSION_V1, LEVEL_FORMULA_VERSION_V1),
        (100, "test-energy:second", "test-level:second"),
        (101, "test-energy:second", "test-level:second"),
        (199, "test-energy:second", "test-level:second"),
        (200, "test-energy:third", "test-level:second"),
        (201, "test-energy:third", "test-level:second"),
    ] {
        let versions = timeline.version_set_at(height).unwrap();
        assert_eq!(
            versions
                .require_string(VersionFamily::EnergyFormulaVersion)
                .unwrap(),
            energy
        );
        assert_eq!(
            versions
                .require_string(VersionFamily::LevelFormulaVersion)
                .unwrap(),
            level
        );
        assert_eq!(
            versions
                .require_string(VersionFamily::InscriptionSchemaVersion)
                .unwrap(),
            INSCRIPTION_SCHEMA_VERSION_V1
        );
    }
    timeline.indexer_context_at(99).unwrap();
    assert!(matches!(
        timeline.indexer_context_at(100),
        Err(ActivationRegistryError::VersionNotSupported {
            family: VersionFamily::EnergyFormulaVersion,
            ..
        })
    ));
    let issues = timeline.indexer_support_issues(90..=210).unwrap();
    assert_eq!(
        issues
            .iter()
            .map(|i| (i.start_height, i.end_height))
            .collect::<Vec<_>>(),
        [(100, 199), (200, 210)]
    );
}

#[test]
fn audit_only_statuses_do_not_create_execution_boundaries() {
    let mut registry = registry();
    for (index, status) in [
        ActivationStatus::Planned,
        ActivationStatus::Deferred,
        ActivationStatus::Superseded,
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = registry.records[0].clone();
        record.activation_height = 100 + index as u64;
        record.version_value = VersionValue::String(format!("test-schema:{index}"));
        record.status = status;
        registry.records.push(record);
    }
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    let intervals = timeline.intervals(0..=u32::MAX).unwrap();
    assert_eq!(intervals.len(), 1);
    assert_eq!(
        (intervals[0].start_height, intervals[0].end_height),
        (0, u32::MAX)
    );
    assert!(
        timeline
            .indexer_support_issues(0..=u32::MAX)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn upper_height_and_single_block_intervals_do_not_overflow() {
    let mut registry = registry();
    activate(
        &mut registry,
        VersionFamily::EnergyFormulaVersion,
        u32::MAX,
        "test-energy:last",
    );
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    let intervals = timeline.intervals((u32::MAX - 1)..=u32::MAX).unwrap();
    assert_eq!(
        intervals
            .iter()
            .map(|i| (i.start_height, i.end_height))
            .collect::<Vec<_>>(),
        [(u32::MAX - 1, u32::MAX - 1), (u32::MAX, u32::MAX)]
    );
    assert_eq!(timeline.intervals(u32::MAX..=u32::MAX).unwrap().len(), 1);
    timeline.validate_indexer_range(0..=u32::MAX - 1).unwrap();
    assert!(
        timeline
            .validate_indexer_range(u32::MAX..=u32::MAX)
            .is_err()
    );
    let from = 11;
    let through = 10;
    assert!(timeline.intervals(from..=through).is_err());
}

#[test]
fn inactive_or_incomplete_history_never_produces_an_executable_context() {
    let mut registry = registry();
    for record in &mut registry.records {
        record.activation_height = 10;
    }
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    assert!(timeline.version_set_at(9).is_err());
    assert!(timeline.intervals(9..=10).is_err());
    assert!(timeline.ensure_same_history(&timeline, 0, 10).is_err());
    timeline.indexer_context_at(10).unwrap();
    for record in &mut registry.records {
        record.status = ActivationStatus::Planned;
    }
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    assert!(timeline.indexer_context_at(u32::MAX).is_err());
    let mut registry = self::registry();
    registry
        .records
        .retain(|r| r.version_family != VersionFamily::EnergyFormulaVersion);
    let timeline = BtcRuleTimeline::new(&registry).unwrap();
    assert!(timeline.version_set_at(0).is_ok()); // A declared but incomplete set is inspectable.
    assert!(timeline.indexer_context_at(0).is_err());
}

#[test]
fn invalid_registry_structure_is_rejected_before_caching() {
    let original = registry();
    let mut conflict = original.clone();
    let mut record = conflict.records[0].clone();
    record.version_value = VersionValue::String("conflicting-version".into());
    conflict.records.push(record);
    assert!(BtcRuleTimeline::new(&conflict).is_err());
    let mut overflow = original.clone();
    overflow.records[0].activation_height = u64::from(u32::MAX) + 1;
    assert!(BtcRuleTimeline::new(&overflow).is_err());
    let mut bad_chain = original;
    activate(
        &mut bad_chain,
        VersionFamily::EnergyFormulaVersion,
        100,
        "test-energy:second",
    );
    bad_chain.records.last_mut().unwrap().supersedes = None;
    assert!(BtcRuleTimeline::new(&bad_chain).is_err());
}

#[test]
fn future_extensions_preserve_only_the_shared_history_prefix() {
    let old = registry();
    let mut new = old.clone();
    activate(
        &mut new,
        VersionFamily::EnergyFormulaVersion,
        100,
        "test-energy:second",
    );
    let (old, new) = (
        BtcRuleTimeline::new(&old).unwrap(),
        BtcRuleTimeline::new(&new).unwrap(),
    );
    old.ensure_same_history(&new, 9, 99).unwrap();
    let error = old
        .ensure_same_history(&new, 9, 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("first_difference_height=100"), "{error}");
    assert!(error.contains(old.activation_registry_id()), "{error}");
    assert!(error.contains(new.activation_registry_id()), "{error}");
    assert!(new.ensure_same_history(&old, 9, 100).is_err());
}

#[test]
fn matching_final_versions_do_not_hide_different_intermediate_rules() {
    let old = registry();
    let mut new = old.clone();
    activate(
        &mut new,
        VersionFamily::EnergyFormulaVersion,
        100,
        "test-energy:intermediate",
    );
    activate(
        &mut new,
        VersionFamily::EnergyFormulaVersion,
        200,
        ENERGY_FORMULA_VERSION_V1,
    );
    let (old, new) = (
        BtcRuleTimeline::new(&old).unwrap(),
        BtcRuleTimeline::new(&new).unwrap(),
    );
    assert_eq!(
        old.version_set_at(210).unwrap(),
        new.version_set_at(210).unwrap()
    );
    new.indexer_context_at(210).unwrap();
    assert!(new.validate_indexer_range(9..=210).is_err());
    let error = old
        .ensure_same_history(&new, 9, 210)
        .unwrap_err()
        .to_string();
    assert!(error.contains("first_difference_height=100"), "{error}");
    // The caller must supply the real dataset origin. Only this suffix is equivalent.
    old.ensure_same_history(&new, 200, 210).unwrap();
}

#[test]
fn execution_history_equality_does_not_mean_registry_identity_equality() {
    let old = registry();
    let mut new = old.clone();
    new.records[0].notes.push_str(" Additional audit context.");
    // A redundant activation may split an otherwise identical rule interval.
    activate(
        &mut new,
        VersionFamily::EnergyFormulaVersion,
        100,
        ENERGY_FORMULA_VERSION_V1,
    );
    let (old, new) = (
        BtcRuleTimeline::new(&old).unwrap(),
        BtcRuleTimeline::new(&new).unwrap(),
    );
    assert_ne!(old.activation_registry_id(), new.activation_registry_id());
    old.ensure_same_history(&new, 0, u32::MAX).unwrap();
    new.ensure_same_history(&old, 0, u32::MAX).unwrap();
    assert_ne!(
        old.indexer_context_at(200).unwrap(),
        new.indexer_context_at(200).unwrap()
    );
}

#[test]
fn domain_changes_cannot_reuse_identical_rule_history() {
    let original = registry();
    let timeline = BtcRuleTimeline::new(&original).unwrap();
    for variation in 0..3 {
        let mut changed = original.clone();
        match variation {
            0 => changed.scope.rules_scope = Some("another-test-scope".into()),
            1 => changed.scope.stable_lag_blocks += 1,
            _ => {
                changed.scope.network_id = "btc-signet".into();
                changed.scope.network_type = ActivationNetworkType::Signet;
            }
        }
        let other = BtcRuleTimeline::new(&changed).unwrap();
        assert!(
            timeline
                .ensure_same_history(&other, 0, 99)
                .unwrap_err()
                .to_string()
                .contains("domain mismatch")
        );
    }
}

#[test]
fn timeline_and_context_are_immutable_snapshots_of_the_declared_registry() {
    let mut original = registry();
    let timeline = BtcRuleTimeline::new(&original).unwrap();
    let context = timeline.indexer_context_at(150).unwrap();
    activate(
        &mut original,
        VersionFamily::EnergyFormulaVersion,
        100,
        "test-energy:changed",
    );
    let changed = BtcRuleTimeline::new(&original).unwrap();
    assert_eq!(context, timeline.indexer_context_at(150).unwrap());
    assert_eq!(
        context
            .active_version_set()
            .require_string(VersionFamily::EnergyFormulaVersion)
            .unwrap(),
        ENERGY_FORMULA_VERSION_V1
    );
    assert!(changed.indexer_context_at(150).is_err());
}
