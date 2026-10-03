//! Supported rule-pair matrix and historical scope/state identity vectors.
use usdb_util::*;

#[test]
fn only_current_rules_execute_and_scope_identity_remains_separate() {
    let catalog = BtcActivationRegistryCatalog::from_json(include_str!(
        "fixtures/miner-pass-v2/catalog.json"
    ))
    .unwrap();
    let registry = catalog.current_registry();
    for height in [0, 9, 10, 11] {
        registry
            .lookup_active_version_set(height)
            .unwrap()
            .validate_btc_indexer()
            .unwrap();
    }
    let mut v1_json = serde_json::to_value(registry.lookup_active_version_set(9).unwrap()).unwrap();
    v1_json["inscription_schema_version"] = INSCRIPTION_SCHEMA_VERSION_V1.into();
    v1_json["pass_state_machine_version"] = PASS_STATE_MACHINE_VERSION_V1.into();
    let v1: ActiveVersionSet = serde_json::from_value(v1_json).unwrap();
    let v2 = registry.lookup_active_version_set(10).unwrap();
    for (schema, state, valid) in [
        (
            INSCRIPTION_SCHEMA_VERSION_V1,
            PASS_STATE_MACHINE_VERSION_V1,
            false,
        ),
        (
            INSCRIPTION_SCHEMA_VERSION_V2,
            PASS_STATE_MACHINE_VERSION_V2,
            true,
        ),
        (
            INSCRIPTION_SCHEMA_VERSION_V1,
            PASS_STATE_MACHINE_VERSION_V2,
            false,
        ),
        (
            INSCRIPTION_SCHEMA_VERSION_V2,
            PASS_STATE_MACHINE_VERSION_V1,
            false,
        ),
        ("", PASS_STATE_MACHINE_VERSION_V1, false),
        (INSCRIPTION_SCHEMA_VERSION_V2, "v999", false),
    ] {
        let mut json = serde_json::to_value(&v1).unwrap();
        json["inscription_schema_version"] = schema.into();
        json["pass_state_machine_version"] = state.into();
        let set = serde_json::from_value::<ActiveVersionSet>(json);
        assert_eq!(
            set.ok()
                .is_some_and(|set| set.validate_btc_indexer().is_ok()),
            valid,
            "{schema} {state}"
        );
    }
    assert!(v1.validate_btc_indexer().is_err());
    let mut other = registry.clone();
    other.scope.rules_scope = Some("miner-pass-other-fixture".into());
    other.records.retain(|r| r.activation_height == 0);
    other.validate().unwrap();
    let other_versions = other.lookup_active_version_set(10).unwrap();
    other_versions.validate_btc_indexer().unwrap();
    let identity = LocalStateCommitIdentity {
        commit_protocol_version: COMMIT_PROTOCOL_VERSION_V1.into(),
        upstream_snapshot_id: "11".repeat(32),
        active_version_set_id: v1.active_version_set_id(),
        local_synced_block_height: 10,
        latest_pass_block_commit: None,
        latest_active_balance_snapshot: None,
    };
    let legacy = build_local_state_commit(&identity);
    let mut activated = identity.clone();
    activated.active_version_set_id = v2.active_version_set_id();
    let mut separate = identity;
    separate.active_version_set_id = other_versions.active_version_set_id();
    assert_ne!(legacy, build_local_state_commit(&activated));
    assert_ne!(legacy, build_local_state_commit(&separate));
    assert_ne!(
        build_local_state_commit(&activated),
        build_local_state_commit(&separate)
    );
}
