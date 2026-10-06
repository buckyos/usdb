//! Explicit policies for crossing rule boundaries, including blocks without pass events.
//! Current policies preserve accepted state and derive raw energy from committed history.
//! A future materialized rights/representation change needs its own executor and commit review.
use super::energy_settlement::EnergyRules;
use super::rules::{MintSchemaRules, PassStateRules};
use usdb_util::{ActiveVersionSet, BtcRuleTimeline};

fn validate_transition(previous: &ActiveVersionSet, next: &ActiveVersionSet) -> Result<(), String> {
    let before_schema = MintSchemaRules::from_versions(previous).map_err(|e| e.to_string())?;
    let after_schema = MintSchemaRules::from_versions(next).map_err(|e| e.to_string())?;
    match (before_schema, after_schema) {
        (MintSchemaRules::V1, MintSchemaRules::V1) => {}
        #[cfg(test)]
        (MintSchemaRules::V1, MintSchemaRules::Conformance901)
        | (MintSchemaRules::Conformance901, MintSchemaRules::V1)
        | (MintSchemaRules::Conformance901, MintSchemaRules::Conformance901) => {}
    }
    // All registered schema edges affect new payloads only; stored mint facts stay accepted.
    let before_state = PassStateRules::from_versions(previous).map_err(|e| e.to_string())?;
    let after_state = PassStateRules::from_versions(next).map_err(|e| e.to_string())?;
    match (before_state, after_state) {
        (PassStateRules::V2, PassStateRules::V2) => {}
        #[cfg(test)]
        (PassStateRules::V2, PassStateRules::ConformanceNoNewCollab)
        | (PassStateRules::ConformanceNoNewCollab, PassStateRules::V2)
        | (PassStateRules::ConformanceNoNewCollab, PassStateRules::ConformanceNoNewCollab) => {}
    }
    // Admission changes preserve existing rights. Every energy edge explicitly declares
    // a pure conversion; the settlement kernel uses exactly the same declaration.
    // Never add a catch-all identity policy when registering another schema/state executor.
    let before_energy = EnergyRules::from_versions(previous).map_err(|e| e.to_string())?;
    EnergyRules::from_versions(next)
        .map_err(|e| e.to_string())?
        .transition_from(before_energy)?;
    Ok(())
}

/// Validate every compiled version and every adjacent transition in the indexed range.
/// On startup use origin..=durable tip; before H use max(origin,H-1)..=H. A fresh
/// origin has no earlier pass ledger to migrate. No writes or per-pass scans occur here.
pub(crate) fn validate_rule_history(
    timeline: &BtcRuleTimeline,
    start: u32,
    end: u32,
) -> Result<(), String> {
    let result = (|| {
        let intervals = timeline.intervals(start..=end).map_err(|e| e.to_string())?;
        let mut previous = None;
        for interval in intervals {
            let current = timeline
                .indexer_context_at(interval.start_height)
                .map_err(|e| e.to_string())?;
            if let Some(before) = previous {
                validate_transition(before, current.active_version_set())
                    .map_err(|e| format!("boundary_height={}, error={e}", interval.start_height))?;
            }
            previous = Some(interval.active_version_set);
        }
        Ok(())
    })();
    result.map_err(|error: String| {
        let msg = format!(
            "Unsupported indexer rule history: registry_id={}, network_id={}, rules_scope={}, start_height={start}, end_height={end}, error={error}",
            timeline.activation_registry_id(), timeline.scope().network_id, timeline.scope().rules_scope(),
        );
        error!("{msg}");
        msg
    })
}
