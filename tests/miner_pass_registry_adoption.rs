//! Real paired-store adoption, interrupted metadata writes and independent replay comparison.
use crate::config::ConfigManager;
use crate::index::test_miner_pipeline::Pipeline;
use crate::index::test_miner_queries::{context, params};
use crate::index::test_miner_rules::{CONFORMANCE_ENERGY_DOUBLE, conformance_catalog};
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use crate::index::test_miner_upgrade::fingerprint;
use crate::index::{InscriptionIndexer, rules};
use crate::output::IndexOutput;
use crate::service::rpc::UsdbIndexerRpc;
use crate::status::StatusManager;
use crate::storage::rules_upgrade::{AdoptionStage, JOURNAL_KEY, adopt_registry};
use crate::storage::{MinerPassStorage, PassEnergyStorage};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use usdb_util::{
    BtcActivationRegistryCatalog, BtcRuleTimeline, INDEXER_RULES_BINDING_KEY, IndexerRulesBinding,
    VersionFamily,
};

fn catalogs() -> (String, String) {
    let old = conformance_catalog(&[]);
    let next = conformance_catalog(&[(
        VersionFamily::EnergyFormulaVersion,
        10,
        CONFORMANCE_ENERGY_DOUBLE,
    )]);
    let mut doc: Value = serde_json::from_str(&next).unwrap();
    let previous: Value = serde_json::from_str(&old).unwrap();
    doc["registries"]
        .as_array_mut()
        .unwrap()
        .insert(0, previous["registries"][0].clone());
    (old, doc.to_string())
}

fn select(root: &Path, catalog: &str) {
    let parsed = BtcActivationRegistryCatalog::from_json(catalog).unwrap();
    let path = root.join("config.json");
    let mut cfg: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    cfg["usdb"]["activation_registry_id"] = parsed.current_registry_id().into();
    std::fs::write(path, cfg.to_string()).unwrap();
    std::fs::write(root.join("catalog.json"), catalog).unwrap();
}

fn attempt(root: &Path) -> Result<(), String> {
    let cfg = Arc::new(ConfigManager::load(Some(root.to_owned()))?);
    let status = Arc::new(StatusManager::new(
        cfg.clone(),
        Arc::new(IndexOutput::new()),
    )?);
    InscriptionIndexer::new(cfg, status).map(|_| ())
}

fn coordinate(
    root: &Path,
    hook: impl FnMut(AdoptionStage) -> Result<(), String>,
) -> Result<(), String> {
    let cfg = ConfigManager::load(Some(root.to_owned()))?;
    let catalog = cfg.activation_registry_catalog()?;
    let timelines: HashMap<_, _> = catalog
        .registry_ids()
        .iter()
        .map(|id| {
            (
                id.clone(),
                BtcRuleTimeline::new_with_indexer_support(
                    catalog.registry_by_id(id).unwrap(),
                    rules::validate_indexer_rules,
                )
                .unwrap(),
            )
        })
        .collect();
    let pass = MinerPassStorage::new(&cfg.data_dir())?;
    let energy = PassEnergyStorage::new(&cfg.data_dir())?;
    let target = IndexerRulesBinding::new(
        catalog.current_registry(),
        cfg.config().usdb.genesis_block_height,
    );
    adopt_registry(&pass, &energy, &catalog, &timelines, &target, hook)
}

fn without_query_identity(mut value: Value) -> Value {
    for block in value["blocks"].as_array_mut().unwrap() {
        // Unpinned RPC identities select the current revision; persisted commitments may not change.
        block.as_object_mut().unwrap().remove("state_ref");
    }
    value
}

#[tokio::test]
async fn adopts_before_activation_preserves_history_and_matches_fresh_replay() {
    let (old, next) = catalogs();
    let mint = MintBlock::new(
        8,
        vec![MintSpec::standard(
            90,
            source_script(SpendKind::Witness),
            1,
            vec![],
        )],
        false,
    );
    let empty: Vec<_> = (9..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
    let blocks: Vec<_> = std::iter::once(&mint).chain(empty.iter()).collect();
    let ids = vec![mint.mints[0].inscription_id];
    let p = Pipeline::with_catalog("adopt-prefix", &blocks, 8, &old).await;
    p.sync_to(9).await.unwrap();
    let before = without_query_identity(fingerprint(&p, &ids, 9).await);
    let old_id = BtcActivationRegistryCatalog::from_json(&old)
        .unwrap()
        .current_registry_id()
        .to_owned();
    let historical = serde_json::to_value(
        p.rpc()
            .get_state_ref_at_height(params(
                json!({"block_height":9,"context":context(&old_id,9)}),
            ))
            .unwrap(),
    )
    .unwrap();
    let p = p.while_stopped(|root| select(root, &next)).await;
    assert_eq!(
        before,
        without_query_identity(fingerprint(&p, &ids, 9).await)
    );
    assert!(
        p.indexer
            .miner_pass_storage()
            .rules_metadata(JOURNAL_KEY)
            .unwrap()
            .is_none()
    );
    p.sync_to(12).await.unwrap();
    assert_eq!(
        historical,
        serde_json::to_value(
            p.rpc()
                .get_state_ref_at_height(params(
                    json!({"block_height":9,"context":context(&old_id,9)})
                ))
                .unwrap()
        )
        .unwrap()
    );
    assert!(
        p.rpc()
            .get_state_ref_at_height(params(
                json!({"block_height":12,"context":context(&old_id,12)})
            ))
            .is_err()
    );
    let fresh = Pipeline::with_catalog("adopt-replay", &blocks, 8, &next).await;
    fresh.sync_to(12).await.unwrap();
    assert_eq!(
        fingerprint(&p, &ids, 12).await,
        fingerprint(&fresh, &ids, 12).await
    );
    p.cleanup();
    fresh.cleanup();
}

#[tokio::test]
async fn obsolete_history_at_or_after_activation_is_rejected_without_metadata_writes() {
    for tip in [10, 12] {
        let (old, next) = catalogs();
        let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
        let p = Pipeline::with_catalog(
            "adopt-obsolete",
            &blocks.iter().collect::<Vec<_>>(),
            8,
            &old,
        )
        .await;
        p.sync_to(tip).await.unwrap();
        let p = p
            .while_stopped(|root| {
                select(root, &next);
                let error = attempt(root).unwrap_err();
                assert!(
                    error.contains("requires rebuild")
                        && error.contains("first_difference_height=10"),
                    "{error}"
                );
                let cfg = ConfigManager::load(Some(root.to_owned())).unwrap();
                let pass = MinerPassStorage::new(&cfg.data_dir()).unwrap();
                assert!(pass.rules_metadata(JOURNAL_KEY).unwrap().is_none());
                let expected = IndexerRulesBinding::new(
                    BtcActivationRegistryCatalog::from_json(&old)
                        .unwrap()
                        .current_registry(),
                    8,
                );
                pass.validate_rules_binding(&expected).unwrap();
                PassEnergyStorage::new(&cfg.data_dir())
                    .unwrap()
                    .validate_rules_binding(&expected)
                    .unwrap();
                select(root, &old);
            })
            .await;
        p.cleanup();
    }
}

#[tokio::test]
async fn restarts_complete_each_durable_adoption_boundary() {
    for point in [
        AdoptionStage::Prepared,
        AdoptionStage::EnergyBound,
        AdoptionStage::Committed,
    ] {
        let (old, next) = catalogs();
        let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
        let p = Pipeline::with_catalog(
            "adopt-interrupted",
            &blocks.iter().collect::<Vec<_>>(),
            8,
            &old,
        )
        .await;
        p.sync_to(9).await.unwrap();
        let p = p
            .while_stopped(|root| {
                select(root, &next);
                let error = coordinate(root, |stage| {
                    if stage == point {
                        Err("injected interruption".into())
                    } else {
                        Ok(())
                    }
                })
                .unwrap_err();
                assert_eq!(error, "injected interruption");
            })
            .await;
        p.sync_to(12).await.unwrap();
        assert!(
            p.indexer
                .miner_pass_storage()
                .rules_metadata(JOURNAL_KEY)
                .unwrap()
                .is_none()
        );
        p.reopen().await.cleanup();
    }
}

#[tokio::test]
async fn rejects_downgrade_unknown_source_and_unjournaled_mixed_stores() {
    let (old, next) = catalogs();
    let blocks: Vec<_> = (8..=9).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p =
        Pipeline::with_catalog("adopt-reject", &blocks.iter().collect::<Vec<_>>(), 8, &old).await;
    p.sync_to(9).await.unwrap();
    let p = p
        .while_stopped(|root| {
            select(root, &next);
            coordinate(root, |_| Ok(())).unwrap();
            let mut doc: Value = serde_json::from_str(&next).unwrap();
            doc["current_registry_id"] =
                serde_json::from_str::<Value>(&old).unwrap()["current_registry_id"].clone();
            select(root, &doc.to_string());
            assert!(attempt(root).unwrap_err().contains("must move forward"));
            select(root, &old);
            assert!(attempt(root).unwrap_err().contains("ancestry"));
            select(root, &next);
            let cfg = ConfigManager::load(Some(root.to_owned())).unwrap();
            let db = rusqlite::Connection::open(cfg.data_dir().join("miner_pass.db")).unwrap();
            let original: String = db
                .query_row(
                    "SELECT value FROM state_text WHERE name=?1",
                    [INDEXER_RULES_BINDING_KEY],
                    |r| r.get(0),
                )
                .unwrap();
            let source = IndexerRulesBinding::new(
                BtcActivationRegistryCatalog::from_json(&old)
                    .unwrap()
                    .current_registry(),
                8,
            );
            db.execute(
                "UPDATE state_text SET value=?1 WHERE name=?2",
                [source.to_json().unwrap(), INDEXER_RULES_BINDING_KEY.into()],
            )
            .unwrap();
            assert!(
                attempt(root)
                    .unwrap_err()
                    .contains("inconsistent registry bindings")
            );
            db.execute(
                "UPDATE state_text SET value=?1 WHERE name=?2",
                [original, INDEXER_RULES_BINDING_KEY.into()],
            )
            .unwrap();
        })
        .await;
    p.cleanup();
}

#[test]
#[ignore = "child process for abrupt adoption crashes"]
fn adoption_crash_child() {
    let root = std::env::var("USDB_ADOPTION_TEST_ROOT").unwrap();
    let point = std::env::var("USDB_ADOPTION_TEST_POINT").unwrap();
    coordinate(Path::new(&root), |stage| {
        if format!("{stage:?}") == point {
            std::process::exit(86);
        }
        Ok(())
    })
    .unwrap();
    panic!("crash boundary was not reached");
}

#[tokio::test]
async fn abrupt_process_exit_at_each_boundary_recovers_without_unwinding() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    for point in [
        AdoptionStage::Prepared,
        AdoptionStage::EnergyBound,
        AdoptionStage::Committed,
    ] {
        let (old, next) = catalogs();
        let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
        let p = Pipeline::with_catalog("adopt-crash", &blocks.iter().collect::<Vec<_>>(), 8, &old)
            .await;
        p.sync_to(9).await.unwrap();
        let p = p
            .while_stopped(|root| {
                select(root, &next);
                let mut child = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "index::miner_pass_registry_adoption::adoption_crash_child",
                        "--ignored",
                    ])
                    .env("USDB_ADOPTION_TEST_ROOT", root)
                    .env("USDB_ADOPTION_TEST_POINT", format!("{point:?}"))
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                let start = Instant::now();
                while child.try_wait().unwrap().is_none() {
                    if start.elapsed() > Duration::from_secs(30) {
                        child.kill().unwrap();
                        panic!(
                            "adoption child timed out at {point:?}: {:?}",
                            child.wait_with_output().unwrap()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let output = child.wait_with_output().unwrap();
                assert_eq!(output.status.code(), Some(86), "{point:?}: {output:?}");
            })
            .await;
        p.sync_to(12).await.unwrap();
        p.cleanup();
    }
}

#[tokio::test]
async fn refuses_dirty_boundary_and_changed_recovery_intent_without_switching_bindings() {
    let (old, next) = catalogs();
    let blocks: Vec<_> = (8..=9).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p =
        Pipeline::with_catalog("adopt-dirty", &blocks.iter().collect::<Vec<_>>(), 8, &old).await;
    p.sync_to(9).await.unwrap();
    let p = p
        .while_stopped(|root| {
            select(root, &next);
            let cfg = ConfigManager::load(Some(root.to_owned())).unwrap();
            let energy = PassEnergyStorage::new(&cfg.data_dir()).unwrap();
            energy.set_pending_block_height(10).unwrap();
            drop(energy);
            assert!(
                attempt(root)
                    .unwrap_err()
                    .contains("pending_energy=Some(10)")
            );
            let energy = PassEnergyStorage::new(&cfg.data_dir()).unwrap();
            energy.clear_pending_block_height().unwrap();
            energy.set_synced_block_height(8).unwrap();
            drop(energy);
            assert!(attempt(root).unwrap_err().contains("energy_height=Some(8)"));
            let energy = PassEnergyStorage::new(&cfg.data_dir()).unwrap();
            energy.set_synced_block_height(9).unwrap();
            drop(energy);
            let pass = MinerPassStorage::new(&cfg.data_dir()).unwrap();
            assert!(pass.rules_metadata(JOURNAL_KEY).unwrap().is_none());
            drop(pass);
            coordinate(root, |stage| {
                if stage == AdoptionStage::Prepared {
                    Err("stop".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            let db = rusqlite::Connection::open(cfg.data_dir().join("miner_pass.db")).unwrap();
            let original: String = db
                .query_row(
                    "SELECT value FROM state_text WHERE name=?1",
                    [JOURNAL_KEY],
                    |r| r.get(0),
                )
                .unwrap();
            let mut intent: Value = serde_json::from_str(&original).unwrap();
            intent["height"] = 8.into();
            db.execute(
                "UPDATE state_text SET value=?1 WHERE name=?2",
                [intent.to_string(), JOURNAL_KEY.into()],
            )
            .unwrap();
            assert!(attempt(root).unwrap_err().contains("boundary changed"));
            db.execute(
                "UPDATE state_text SET value=?1 WHERE name=?2",
                [original, JOURNAL_KEY.into()],
            )
            .unwrap();
            select(root, &old);
            assert!(attempt(root).unwrap_err().contains("another configuration"));
            select(root, &next);
        })
        .await;
    p.cleanup();
}

#[tokio::test]
async fn initialized_empty_dataset_can_adopt_before_its_first_block() {
    let (old, next) = catalogs();
    let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p =
        Pipeline::with_catalog("adopt-empty", &blocks.iter().collect::<Vec<_>>(), 8, &old).await;
    assert!(
        p.indexer
            .miner_pass_storage()
            .get_committed_synced_btc_block_height()
            .unwrap()
            .is_none()
    );
    let p = p
        .while_stopped(|root| {
            select(root, &next);
            coordinate(root, |stage| {
                if stage == AdoptionStage::EnergyBound {
                    Err("stop empty adoption".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        })
        .await;
    p.sync_to(12).await.unwrap();
    p.cleanup();
}

#[tokio::test]
async fn unknown_future_rules_allow_prefix_adoption_but_stop_before_activation_writes() {
    let (old, next) = catalogs();
    let mut doc: Value = serde_json::from_str(&next).unwrap();
    let records = doc["registries"][1]["records"].as_array_mut().unwrap();
    records.last_mut().unwrap()["version_value"] = "unsupported-future-energy:v99".into();
    let registry: usdb_util::BtcActivationRegistry =
        serde_json::from_value(doc["registries"][1].clone()).unwrap();
    doc["current_registry_id"] = registry.activation_registry_id().into();
    let future = doc.to_string();
    let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p = Pipeline::with_catalog(
        "adopt-unknown-future",
        &blocks.iter().collect::<Vec<_>>(),
        8,
        &old,
    )
    .await;
    p.sync_to(9).await.unwrap();
    let p = p.while_stopped(|root| select(root, &future)).await;
    let error = p.sync_to(10).await.unwrap_err();
    assert!(error.contains("unsupported-future-energy:v99"), "{error}");
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
            .get_pass_block_commit(10)
            .unwrap()
            .is_none()
    );
    assert!(
        p.indexer
            .pass_energy_manager()
            .get_pending_block_height_for_test()
            .unwrap()
            .is_none()
    );
    p.cleanup();
}

#[tokio::test]
async fn intermediate_divergence_is_rejected_even_if_tip_versions_match() {
    let old = conformance_catalog(&[]);
    let fork = conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            12,
            "uip-0003-pass-energy-formula:v1",
        ),
    ]);
    let mut doc: Value = serde_json::from_str(&fork).unwrap();
    doc["registries"].as_array_mut().unwrap().insert(
        0,
        serde_json::from_str::<Value>(&old).unwrap()["registries"][0].clone(),
    );
    let blocks: Vec<_> = (8..=12).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p = Pipeline::with_catalog(
        "adopt-middle-divergence",
        &blocks.iter().collect::<Vec<_>>(),
        8,
        &old,
    )
    .await;
    p.sync_to(12).await.unwrap();
    let p = p
        .while_stopped(|root| {
            select(root, &doc.to_string());
            let error = attempt(root).unwrap_err();
            assert!(
                error.contains("requires rebuild") && error.contains("first_difference_height=10"),
                "{error}"
            );
            select(root, &old);
        })
        .await;
    p.cleanup();
}

#[tokio::test]
async fn rejects_cross_domain_metadata_and_malformed_journals_without_adopting() {
    let (old, next) = catalogs();
    let blocks: Vec<_> = (8..=9).map(|h| MintBlock::new(h, vec![], false)).collect();
    let p =
        Pipeline::with_catalog("adopt-domains", &blocks.iter().collect::<Vec<_>>(), 8, &old).await;
    p.sync_to(9).await.unwrap();
    let p = p
        .while_stopped(|root| {
            select(root, &next);
            let cfg = ConfigManager::load(Some(root.to_owned())).unwrap();
            let source = IndexerRulesBinding::new(
                BtcActivationRegistryCatalog::from_json(&old)
                    .unwrap()
                    .current_registry(),
                8,
            );
            let db = rusqlite::Connection::open(cfg.data_dir().join("miner_pass.db")).unwrap();
            for (field, value) in [
                ("btc_network_id", json!("btc-mainnet")),
                ("rules_scope", json!("another-scope")),
                ("index_origin_height", json!(7)),
                ("schema_version", json!("unknown:v99")),
            ] {
                let mut value_binding = serde_json::to_value(&source).unwrap();
                value_binding[field] = value;
                let corrupt: IndexerRulesBinding = serde_json::from_value(value_binding).unwrap();
                db.execute(
                    "UPDATE state_text SET value=?1 WHERE name=?2",
                    [corrupt.to_json().unwrap(), INDEXER_RULES_BINDING_KEY.into()],
                )
                .unwrap();
                let energy = PassEnergyStorage::new(&cfg.data_dir()).unwrap();
                energy.replace_rules_binding(&source, &corrupt).unwrap();
                drop(energy);
                let error = attempt(root).unwrap_err();
                assert!(error.contains("domain mismatch"), "{field}: {error}");
                let pass = MinerPassStorage::new(&cfg.data_dir()).unwrap();
                assert!(pass.rules_metadata(JOURNAL_KEY).unwrap().is_none());
                assert_eq!(
                    pass.rules_metadata(INDEXER_RULES_BINDING_KEY).unwrap(),
                    Some(corrupt.to_json().unwrap())
                );
                drop(pass);
                let energy = PassEnergyStorage::new(&cfg.data_dir()).unwrap();
                energy.replace_rules_binding(&corrupt, &source).unwrap();
                drop(energy);
                db.execute(
                    "UPDATE state_text SET value=?1 WHERE name=?2",
                    [source.to_json().unwrap(), INDEXER_RULES_BINDING_KEY.into()],
                )
                .unwrap();
            }
            db.execute(
                "INSERT INTO state_text(name,value) VALUES (?1,?2)",
                [JOURNAL_KEY, "{invalid}"],
            )
            .unwrap();
            assert!(
                attempt(root)
                    .unwrap_err()
                    .contains("Invalid registry adoption journal")
            );
            db.execute("DELETE FROM state_text WHERE name=?1", [JOURNAL_KEY])
                .unwrap();
            db.execute(
                "INSERT INTO state(name,value) VALUES (?1,9)",
                ["upstream_reorg_recovery_pending_height"],
            )
            .unwrap();
            assert!(attempt(root).unwrap_err().contains("pending_reorg=Some(9)"));
            db.execute(
                "DELETE FROM state WHERE name=?1",
                ["upstream_reorg_recovery_pending_height"],
            )
            .unwrap();
            select(root, &old);
        })
        .await;
    p.cleanup();
}
