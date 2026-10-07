//! Forward-only adoption of a verified rule history at a stopped paired-store boundary.
//! This is metadata recovery, never a repair of a branch executed under obsolete rules.
use super::{MinerPassStorage, PassEnergyStorage};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use usdb_util::{
    BtcActivationRegistryCatalog, BtcRuleTimeline, INDEXER_RULES_BINDING_KEY, IndexerRulesBinding,
};

pub(crate) use usdb_util::INDEXER_REGISTRY_ADOPTION_KEY as JOURNAL_KEY;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Adoption {
    source: IndexerRulesBinding,
    target: IndexerRulesBinding,
    height: Option<u32>,
    energy_height: Option<u32>,
    block_commit: Option<String>,
    reorg_epoch: u64,
}

/// Named durable boundaries also exercised by restart/crash acceptance tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdoptionStage {
    Prepared,
    EnergyBound,
    Committed,
}

/// Validate and, when necessary, finish an authorized forward registry adoption.
/// Both database handles remain exclusively owned by the starting indexer.
pub(crate) fn adopt_registry(
    pass: &MinerPassStorage,
    energy: &PassEnergyStorage,
    catalog: &BtcActivationRegistryCatalog,
    timelines: &HashMap<String, BtcRuleTimeline>,
    target: &IndexerRulesBinding,
    mut boundary: impl FnMut(AdoptionStage) -> Result<(), String>,
) -> Result<(), String> {
    let result = (|| {
        pass.require_committed_writer()?;
        let raw_journal = pass.rules_metadata(JOURNAL_KEY)?;
        let journal: Option<Adoption> = raw_journal
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| {
                format!("Invalid registry adoption journal; preserve data and inspect: {e}")
            })?;
        let pass_binding = parse_binding(pass.rules_metadata(INDEXER_RULES_BINDING_KEY)?, "pass")?;
        let energy_binding = parse_binding(energy.rules_binding()?, "energy")?;
        if journal.is_none()
            && pass_binding.as_ref().is_none_or(|v| v == target)
            && energy_binding.as_ref().is_none_or(|v| v == target)
        {
            // Keep first-binding and ordinary block-crash recovery under their existing guards.
            pass.validate_paired_rules_binding(target, energy.has_indexed_state()?)?;
            energy.validate_paired_rules_binding(target, pass.has_indexed_state()?)?;
            return Ok(());
        }
        let source = journal.as_ref().map(|v| &v.source).or(pass_binding.as_ref())
            .ok_or("Registry adoption requires both original database bindings; preserve data and inspect")?;
        if pass_binding.as_ref() != Some(source)
            || !(energy_binding.as_ref() == Some(source)
                || journal.as_ref().is_some_and(|v| {
                    &v.target == target && energy_binding.as_ref() == Some(target)
                }))
        {
            return Err(format!(
                "Unjournaled or inconsistent registry bindings: pass={pass_binding:?}, energy={energy_binding:?}, target={target:?}; preserve data and inspect"
            ));
        }
        if journal.as_ref().is_some_and(|v| &v.target != target) {
            return Err(format!(
                "Pending registry adoption targets another configuration: journal={journal:?}, configured={target:?}; resume with the original target"
            ));
        }
        let old_registry = catalog
            .registry_by_id(&source.activation_registry_id)
            .map_err(|e| {
                format!(
                    "Cannot establish registry ancestry; restore the complete target catalog: {e}"
                )
            })?;
        if old_registry.schema_version != catalog.current_registry().schema_version
            || source != &IndexerRulesBinding::new(old_registry, target.index_origin_height)
            || target
                != &IndexerRulesBinding::new(catalog.current_registry(), target.index_origin_height)
        {
            return Err(format!(
                "Registry adoption domain mismatch: source={source:?}, target={target:?}, source_registry_schema={}, target_registry_schema={}; check source, scope, origin and identity schema",
                old_registry.schema_version,
                catalog.current_registry().schema_version
            ));
        }
        let ids = catalog.registry_ids();
        let before = ids
            .iter()
            .position(|id| id == &source.activation_registry_id)
            .unwrap();
        let after = ids
            .iter()
            .position(|id| id == &target.activation_registry_id)
            .unwrap();
        if before >= after {
            return Err(format!(
                "Registry adoption must move forward: source_revision={}, target_revision={}; use the correct release, no automatic downgrade",
                before + 1,
                after + 1
            ));
        }
        let height = pass.get_committed_synced_btc_block_height()?;
        let energy_height = energy.get_synced_block_height()?;
        let pending = energy.get_pending_block_height()?;
        let reorg = pass.get_upstream_reorg_recovery_pending_height()?;
        let max_record = energy.peek_max_record_block_height()?;
        let baseline = target.index_origin_height.saturating_sub(1);
        let aligned =
            height == energy_height || (height.is_none() && energy_height == Some(baseline));
        if !aligned
            || pending.is_some()
            || reorg.is_some()
            || max_record.is_some_and(|h| height.is_none_or(|tip| h > tip))
            || (height.is_none() && pass.has_ledger_records()?)
        {
            return Err(format!(
                "Registry adoption requires a completed paired block: pass_height={height:?}, energy_height={energy_height:?}, pending_energy={pending:?}, pending_reorg={reorg:?}, max_energy_record={max_record:?}; finish normal recovery with the source configuration first"
            ));
        }
        let old_timeline = timelines
            .get(&source.activation_registry_id)
            .ok_or("Source registry timeline is missing")?;
        let new_timeline = timelines
            .get(&target.activation_registry_id)
            .ok_or("Target registry timeline is missing")?;
        if old_timeline.activation_registry_id() != source.activation_registry_id
            || new_timeline.activation_registry_id() != target.activation_registry_id
        {
            return Err(
                "Registry adoption timeline identity mismatch; check the selected catalog".into(),
            );
        }
        if let Some(through) = height {
            pass.assert_no_data_after_block_height(through)?;
            pass.assert_balance_snapshot_consistency(through, target.index_origin_height)?;
            let origin = target.index_origin_height.min(through);
            old_timeline.ensure_same_history(new_timeline, origin, through).map_err(|e| {
                format!("Registry adoption requires rebuild: source={}, target={}, indexed_height={through}, error={e}; rebuild the affected derived dataset explicitly; no data was deleted", source.activation_registry_id, target.activation_registry_id)
            })?;
            crate::index::rule_transition::validate_rule_history(new_timeline, origin, through)?;
        } else {
            new_timeline
                .indexer_context_at(target.index_origin_height)
                .map_err(|e| e.to_string())?;
        }
        let intent = Adoption {
            source: source.clone(),
            target: target.clone(),
            height,
            energy_height,
            block_commit: height
                .map(|h| pass.get_pass_block_commit(h))
                .transpose()?
                .flatten()
                .map(|v| v.block_commit),
            reorg_epoch: pass.get_upstream_reorg_epoch()?,
        };
        if journal.as_ref().is_some_and(|v| v != &intent) {
            return Err(format!(
                "Registry adoption boundary changed: saved={journal:?}, actual={intent:?}; preserve data and inspect"
            ));
        }
        let encoded = match raw_journal {
            Some(raw) => raw,
            None => serde_json::to_string(&intent).map_err(|e| e.to_string())?,
        };
        if journal.is_none() {
            pass.prepare_rules_upgrade(JOURNAL_KEY, &encoded)?;
        }
        boundary(AdoptionStage::Prepared)?;
        if energy_binding.as_ref() == Some(source) {
            energy.replace_rules_binding(source, target)?;
        }
        boundary(AdoptionStage::EnergyBound)?;
        pass.finish_rules_upgrade(JOURNAL_KEY, &encoded, target)?;
        boundary(AdoptionStage::Committed)?;
        info!(
            "Registry adoption completed: source={}, target={}, indexed_height={height:?}; historical records preserved",
            source.activation_registry_id, target.activation_registry_id
        );
        for issue in new_timeline
            .indexer_support_issues(
                height.map_or(target.index_origin_height, |h| h.saturating_add(1))..=u32::MAX,
            )
            .map_err(|e| e.to_string())?
        {
            warn!(
                "Registry declares unsupported future rules: registry={}, first_height={}, error={}; update the client before this height",
                target.activation_registry_id, issue.start_height, issue.error
            );
        }
        Ok(())
    })();
    result.inspect_err(|e| {
        error!(
            "Registry adoption failed: target={}, error={e}",
            target.activation_registry_id
        )
    })
}

fn parse_binding(raw: Option<String>, store: &str) -> Result<Option<IndexerRulesBinding>, String> {
    raw.map(|value| {
        serde_json::from_str(&value).map_err(|e| {
            format!("Invalid {store} registry binding; preserve data and inspect: {e}")
        })
    })
    .transpose()
}
