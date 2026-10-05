//! Shared, isolated rule contexts and conformance catalogs for indexer tests.
use usdb_util::{BtcActivationRegistryCatalog, BtcRuleContext, BtcRuleTimeline};

#[path = "miner_pass_conformance.rs"]
mod conformance;
pub use conformance::*;

const CATALOG: &str = include_str!("../fixtures/miner-pass-v2/catalog.json");

/// Existing schema-v1/state-v2 context for generic state and discovery fixtures.
pub fn context(height: u32) -> BtcRuleContext {
    let catalog = BtcActivationRegistryCatalog::from_json(CATALOG).unwrap();
    BtcRuleTimeline::new(catalog.current_registry())
        .unwrap()
        .indexer_context_at(height)
        .unwrap()
}

/// Resolve conformance contexts through the same compiled capability gate as the indexer.
pub fn conformance_context(catalog: &str, height: u32) -> BtcRuleContext {
    let catalog = BtcActivationRegistryCatalog::from_json(catalog).unwrap();
    BtcRuleTimeline::new_with_indexer_support(
        catalog.current_registry(),
        crate::index::rules::validate_indexer_rules,
    )
    .unwrap()
    .indexer_context_at(height)
    .unwrap()
}
