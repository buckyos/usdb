//! Explicit fresh-network configuration for isolated tests, never a production fallback.
use crate::config::ConfigManager;
use std::path::PathBuf;

pub fn load(root: Option<PathBuf>) -> Result<ConfigManager, String> {
    let root = root.ok_or("Test configuration requires an explicit temporary root")?;
    let original = ConfigManager::load(Some(root.clone()))?;
    if original
        .config()
        .usdb
        .activation_registry_catalog_file
        .is_some()
    {
        return Ok(original);
    }
    let mut config = original.config().clone();
    let mut registry = usdb_util::embedded_btc_activation_registry(config.bitcoin.network())
        .map_err(|e| e.to_string())?
        .clone();
    registry.schema_version = usdb_util::SCOPED_ACTIVATION_REGISTRY_SCHEMA_VERSION.into();
    registry.scope.rules_scope = Some("miner-pass-test-fixture".into());
    for record in &mut registry.records {
        if record.version_family == usdb_util::VersionFamily::InscriptionSchemaVersion {
            record.version_value =
                serde_json::from_value(serde_json::json!(usdb_util::INSCRIPTION_SCHEMA_VERSION_V2))
                    .unwrap();
        }
        if record.version_family == usdb_util::VersionFamily::PassStateMachineVersion {
            record.version_value =
                serde_json::from_value(serde_json::json!(usdb_util::PASS_STATE_MACHINE_VERSION_V2))
                    .unwrap();
        }
    }
    let id = registry.activation_registry_id();
    let catalog = serde_json::json!({"schema_version":usdb_util::ACTIVATION_REGISTRY_CATALOG_SCHEMA_VERSION,
        "current_registry_id":id,"registries":[registry]});
    config.usdb.rules_scope = Some("miner-pass-test-fixture".into());
    config.usdb.activation_registry_id = Some(id);
    config.usdb.activation_registry_catalog_file = Some("miner-pass-test-catalog.json".into());
    std::fs::write(
        root.join("miner-pass-test-catalog.json"),
        catalog.to_string(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    ConfigManager::load(Some(root))
}

/// Adapt metadata-only scope/revision fixtures to the one executable mint rule.
pub fn current_catalog(json: &str) -> usdb_util::BtcActivationRegistryCatalog {
    let old = usdb_util::BtcActivationRegistryCatalog::from_json(json).unwrap();
    let registries = old
        .registry_ids()
        .iter()
        .map(|id| {
            let mut registry = old.registry_by_id(id).unwrap().clone();
            for record in &mut registry.records {
                match record.version_family {
                    usdb_util::VersionFamily::InscriptionSchemaVersion => {
                        record.version_value = usdb_util::VersionValue::String(
                            usdb_util::INSCRIPTION_SCHEMA_VERSION_V2.into(),
                        )
                    }
                    usdb_util::VersionFamily::PassStateMachineVersion => {
                        record.version_value = usdb_util::VersionValue::String(
                            usdb_util::PASS_STATE_MACHINE_VERSION_V2.into(),
                        )
                    }
                    _ => {}
                }
            }
            registry
        })
        .collect();
    usdb_util::BtcActivationRegistryCatalog::from_revisions(registries).unwrap()
}
