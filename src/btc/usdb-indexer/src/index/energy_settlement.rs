//! Pure raw-energy settlement over an immutable, exact registry history.
//! Checkpoints represent the state after their block; growth therefore covers (a, b].
use super::content::MinerPassState;
use super::energy_formula::{
    Energy, calc_balance_penalty_energy, calc_growth_delta, calc_inheritable_energy,
    calc_next_active_block_height,
};
use crate::storage::PassEnergyRecord;
use usdb_util::{
    ActivationRegistryError, ActiveVersionSet, BtcRuleTimeline, ENERGY_FORMULA_VERSION_V1,
    VersionFamily,
};

/// Only compiled formulas can execute; synthetic formulas never enter release binaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnergyRules {
    V1,
    #[cfg(test)]
    Double,
    #[cfg(test)]
    Triple,
}

impl EnergyRules {
    /// Select a compiled raw-energy contract without inferring support from its label.
    pub(crate) fn from_versions(
        versions: &ActiveVersionSet,
    ) -> Result<Self, ActivationRegistryError> {
        let family = VersionFamily::EnergyFormulaVersion;
        let value = versions.require_string(family)?;
        match value {
            ENERGY_FORMULA_VERSION_V1 => Ok(Self::V1),
            #[cfg(test)]
            crate::index::test_miner_rules::CONFORMANCE_ENERGY_DOUBLE
                if super::rules::conformance_scope(versions) =>
            {
                Ok(Self::Double)
            }
            #[cfg(test)]
            crate::index::test_miner_rules::CONFORMANCE_ENERGY_TRIPLE
                if super::rules::conformance_scope(versions) =>
            {
                Ok(Self::Triple)
            }
            _ => Err(ActivationRegistryError::VersionNotSupported {
                family,
                value: value.into(),
            }),
        }
    }

    fn growth(self, balance: u64, blocks: u32) -> Energy {
        let growth = calc_growth_delta(balance, blocks);
        match self {
            Self::V1 => growth,
            #[cfg(test)]
            Self::Double => growth.saturating_mul(2),
            #[cfg(test)]
            Self::Triple => growth.saturating_mul(3),
        }
    }

    fn balance_change(self, record: &PassEnergyRecord, height: u32, balance: u64) -> (Energy, u32) {
        let penalty = match self {
            Self::V1 => calc_balance_penalty_energy(
                record.owner_balance,
                balance,
                record.active_block_height,
                height,
            ),
            #[cfg(test)]
            Self::Double | Self::Triple => {
                let lost = (record.owner_balance / 100_000).saturating_sub(balance / 100_000);
                let rate = if self == Self::Double { 2 } else { 3 };
                // Test-only UIP: the event's rate applies to the complete retained age.
                lost as u128 * height.saturating_sub(record.active_block_height) as u128 * rate * 3
                    / 2
            }
        };
        (
            penalty,
            calc_next_active_block_height(
                record.owner_balance,
                balance,
                record.active_block_height,
                height,
            ),
        )
    }

    fn inherit(self, energy: Energy) -> Energy {
        match self {
            Self::V1 => calc_inheritable_energy(energy),
            #[cfg(test)]
            Self::Double => energy / 10 * 9 + energy % 10 * 9 / 10,
            #[cfg(test)]
            Self::Triple => energy / 4 * 3 + energy % 4 * 3 / 4,
        }
    }

    /// Every changed edge needs an explicit conversion or compatibility declaration.
    /// There is deliberately no inferred transitive or default identity conversion.
    pub(crate) fn transition_from(self, previous: Self) -> Result<EnergyTransition, String> {
        if self == previous {
            return Ok(EnergyTransition::Identity);
        }
        #[cfg(test)]
        match (previous, self) {
            (Self::V1, Self::Double) => return Ok(EnergyTransition::DoubleRepresentation),
            (Self::Double, Self::Triple) => return Ok(EnergyTransition::Identity),
            _ => {}
        }
        Err(format!(
            "Missing raw-energy transition: previous={previous:?}, next={self:?}"
        ))
    }
}

/// Registered pure conversions are derivable from the existing checkpoint and rule history.
/// No conversion changes pass state, balance or age, or requires a new persisted marker.
pub(crate) enum EnergyTransition {
    Identity,
    #[cfg(test)]
    DoubleRepresentation,
}
impl EnergyTransition {
    fn apply(self, energy: Energy) -> Energy {
        match self {
            Self::Identity => energy,
            #[cfg(test)]
            Self::DoubleRepresentation => energy.saturating_mul(2),
        }
    }
}

/// Shared arithmetic for projections, balance events and inheritance. It never writes state.
/// Formula changes may derive a new numeric representation, but cannot change pass rights.
pub(crate) struct EnergySettlement {
    timeline: BtcRuleTimeline,
}

impl EnergySettlement {
    /// Bind arithmetic to the same immutable current history used by the energy dataset.
    pub(crate) fn new(timeline: BtcRuleTimeline) -> Self {
        Self { timeline }
    }

    fn rules_at(&self, height: u32) -> Result<EnergyRules, String> {
        self.timeline
            .indexer_context_at(height)
            .and_then(|context| EnergyRules::from_versions(context.active_version_set()))
            .map_err(|error| {
                format!("Unsupported energy context: block_height={height}, error={error}")
            })
    }

    fn failure(&self, record: &PassEnergyRecord, target: u32, error: String) -> String {
        let msg = format!(
            "Raw-energy settlement failed: inscription_id={}, record_height={}, target_height={target}, state={}, registry_id={}, network_id={}, rules_scope={}, error={error}",
            record.inscription_id,
            record.block_height,
            record.state.as_str(),
            self.timeline.activation_registry_id(),
            self.timeline.scope().network_id,
            self.timeline.scope().rules_scope(),
        );
        error!("{msg}");
        msg
    }

    /// Apply each boundary conversion before that boundary block's growth. Existing
    /// v1 has no conversions. Test conversions also convert Dormant numeric units;
    /// terminal zero stays zero. No boundary resets the balance-age anchor.
    pub(crate) fn project(&self, record: &PassEnergyRecord, target: u32) -> Result<Energy, String> {
        let result = (|| {
            if target < record.block_height {
                return Err("Cannot project a checkpoint backwards".into());
            }
            let intervals = self
                .timeline
                .intervals(record.block_height..=target)
                .map_err(|error| error.to_string())?;
            let mut previous = self.rules_at(record.block_height)?;
            let mut energy = record.energy;
            for (index, interval) in intervals.iter().enumerate() {
                let rules = self.rules_at(interval.start_height)?;
                energy = rules
                    .transition_from(previous)
                    .map_err(|error| format!("boundary_height={}, {error}", interval.start_height))?
                    .apply(energy);
                // The first interval includes the checkpoint, whose block is already settled.
                let blocks = interval.end_height - interval.start_height + u32::from(index != 0);
                if record.state == MinerPassState::Active {
                    energy = energy.saturating_add(rules.growth(record.owner_balance, blocks));
                }
                previous = rules;
            }
            Ok(energy)
        })();
        result.map_err(|error| self.failure(record, target, error))
    }

    /// Settle old balance through the event block, then use that block's penalty rules.
    pub(crate) fn balance_change(
        &self,
        record: &PassEnergyRecord,
        height: u32,
        balance: u64,
    ) -> Result<(Energy, u32), String> {
        let energy = self.project(record, height)?;
        let rules = self
            .rules_at(height)
            .map_err(|error| self.failure(record, height, error))?;
        let (penalty, age_anchor) = rules.balance_change(record, height, balance);
        Ok((energy.saturating_sub(penalty), age_anchor))
    }

    /// Discount one already-settled prev, before summing contributions from other prevs.
    pub(crate) fn inherit(&self, energy: Energy, height: u32) -> Result<Energy, String> {
        self.rules_at(height).map(|rules| rules.inherit(energy)).map_err(|error| {
            let msg = format!("Energy inheritance rule selection failed: block_height={height}, registry_id={}, error={error}", self.timeline.activation_registry_id());
            error!("{msg}");
            msg
        })
    }
}
