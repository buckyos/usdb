//! Economic-query fixtures: real indexing, two Leaders, fixed/address collabs, independent upgrades.
use crate::index::MinerPassKind;
use crate::index::test_miner_pipeline::{Pipeline, cold_recipient};
use crate::index::test_miner_rules::{
    CONFORMANCE_EFFECTIVE, CONFORMANCE_ENERGY_DOUBLE, CONFORMANCE_ENERGY_TRIPLE, CONFORMANCE_LEVEL,
    conformance_catalog,
};
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use bitcoincore_rpc::bitcoin::{Address, Network};
use ord::InscriptionId;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use usdb_util::{BtcActivationRegistryCatalog, VersionFamily};

pub fn params<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
pub fn context(registry: &str, height: u32) -> Value {
    json!({"requested_height":height,"expected_state":{"activation_registry_id":registry}})
}
pub fn catalog() -> String {
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
        (
            VersionFamily::EffectiveEnergyFormulaVersion,
            13,
            CONFORMANCE_EFFECTIVE,
        ),
        (VersionFamily::LevelFormulaVersion, 16, CONFORMANCE_LEVEL),
    ])
}
pub struct QueryScenario {
    blocks: Vec<MintBlock>,
    pub ids: Vec<InscriptionId>,
}
impl QueryScenario {
    pub fn new(fork: bool) -> Self {
        let leader = MintBlock::new(
            8,
            vec![MintSpec::standard(
                111,
                source_script(SpendKind::Witness),
                1,
                vec![],
            )],
            false,
        );
        let mut fixed = MintSpec::standard(
            if fork { 112 } else { 113 },
            source_script(SpendKind::Legacy),
            1,
            vec![],
        );
        fixed.source = SpendKind::Legacy;
        fixed.kind = MinerPassKind::Collab;
        fixed.leader = Some(leader.mints[0].inscription_id);
        let fixed = MintBlock::new(9, vec![fixed], false);
        let mut address = MintSpec::standard(114, source_script(SpendKind::Taproot), 1, vec![]);
        address.source = SpendKind::Taproot;
        address.kind = MinerPassKind::Collab;
        address.leader_addr = Some(
            Address::from_script(&source_script(SpendKind::Witness), Network::Regtest)
                .unwrap()
                .to_string(),
        );
        let address = MintBlock::new(11, vec![address], false);
        let other = MintBlock::new(
            12,
            vec![MintSpec::standard(
                if fork { 115 } else { 116 },
                cold_recipient(115),
                0,
                vec![],
            )],
            false,
        );
        let ids = vec![
            leader.mints[0].inscription_id,
            fixed.mints[0].inscription_id,
            address.mints[0].inscription_id,
            other.mints[0].inscription_id,
        ];
        let mut blocks = vec![leader, fixed, address, other];
        for height in 8..=22 {
            if ![8, 9, 11, 12].contains(&height) {
                blocks.push(MintBlock::new(height, vec![], false));
            }
        }
        Self { blocks, ids }
    }
    pub async fn pipeline(&self, name: &str, catalog: &str) -> Pipeline {
        Pipeline::with_catalog(name, &self.blocks.iter().collect::<Vec<_>>(), 8, catalog).await
    }
}

/// Canonical wire contract supplied to the independent Go validator; no runtime test hooks.
pub fn activation_golden(catalog: &str) -> Value {
    let catalog = BtcActivationRegistryCatalog::from_json(catalog).unwrap();
    let registry = catalog.current_registry();
    let mut heights = registry
        .records
        .iter()
        .map(|r| r.activation_height as u32)
        .collect::<Vec<_>>();
    heights.sort();
    heights.dedup();
    json!({
        "network_id":"btc-regtest", "rules_scope":registry.scope.rules_scope(), "revision":1,
        "current":true, "stable_lag_blocks":registry.stable_lag_blocks(),
        "activation_registry_id":registry.activation_registry_id(),
        "activations":heights.into_iter().map(|height| {
            let set = registry.lookup_active_version_set(height).unwrap();
            json!({"btc_height":height,"active_version_set_id":set.active_version_set_id(),"active_version_set":set})
        }).collect::<Vec<_>>()
    })
}
