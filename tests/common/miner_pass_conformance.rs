//! Isolated catalog declarations shared by unit and ordinary-binary integration tests.
use usdb_util::{BtcActivationRegistry, VersionFamily};

const CATALOG: &str = include_str!("../fixtures/miner-pass-v2/catalog.json");

pub const CONFORMANCE_SCOPE: &str = "miner-pass-upgrade-conformance";
pub const CONFORMANCE_SCHEMA: &str = "conformance-miner-pass-schema:901";
pub const CONFORMANCE_STATE: &str = "conformance-miner-pass-state:no-new-collab";

/// Add independent schema/state boundaries in a scope accepted only by indexer test builds.
pub fn conformance_catalog(activations: &[(VersionFamily, u32, &str)]) -> String {
    let mut doc: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
    doc["registries"][0]["scope"]["rules_scope"] = CONFORMANCE_SCOPE.into();
    let records = doc["registries"][0]["records"].as_array_mut().unwrap();
    for &(family, height, version) in activations {
        let mut record = records
            .iter()
            .rev()
            .find(|r| r["version_family"] == family.as_str())
            .unwrap()
            .clone();
        record["activation_height"] = height.into();
        record["supersedes"] = record["version_value"].clone();
        record["version_value"] = version.into();
        record["notes"] =
            "Isolated compiled conformance executor; never a production activation.".into();
        records.push(record);
    }
    let registry: BtcActivationRegistry =
        serde_json::from_value(doc["registries"][0].clone()).unwrap();
    doc["current_registry_id"] = registry.activation_registry_id().into();
    doc.to_string()
}
