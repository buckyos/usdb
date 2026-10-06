//! Independently selected, read-only economic formulas; no raw-energy ledger mutations.
use super::energy_formula::{Energy, calc_collab_contribution, calc_standard_effective_energy};
use super::{calc_difficulty_factor_bps, calc_level_from_effective_energy};
use usdb_util::{
    ActivationRegistryError, ActiveVersionSet, EFFECTIVE_ENERGY_FORMULA_VERSION_V1,
    LEVEL_FORMULA_VERSION_V1, VersionFamily,
};

#[derive(Clone, Copy)]
enum EffectiveRules {
    V1,
    #[cfg(test)]
    QuarterCollab,
}
#[derive(Clone, Copy)]
enum LevelRules {
    V1,
    #[cfg(test)]
    Thousands,
}

/// Raw settlement and derived economics have separate version families and heights.
#[derive(Clone, Copy)]
pub(crate) struct EconomicRules {
    effective: EffectiveRules,
    level: LevelRules,
}
impl EconomicRules {
    /// Require compiled implementations for both derived families; no default fallback.
    pub(crate) fn from_versions(set: &ActiveVersionSet) -> Result<Self, ActivationRegistryError> {
        let family = VersionFamily::EffectiveEnergyFormulaVersion;
        let value = set.require_string(family)?;
        let effective = match value {
            EFFECTIVE_ENERGY_FORMULA_VERSION_V1 => EffectiveRules::V1,
            #[cfg(test)]
            crate::index::test_miner_rules::CONFORMANCE_EFFECTIVE
                if super::rules::conformance_scope(set) =>
            {
                EffectiveRules::QuarterCollab
            }
            _ => {
                return Err(ActivationRegistryError::VersionNotSupported {
                    family,
                    value: value.into(),
                });
            }
        };
        let family = VersionFamily::LevelFormulaVersion;
        let value = set.require_string(family)?;
        let level = match value {
            LEVEL_FORMULA_VERSION_V1 => LevelRules::V1,
            #[cfg(test)]
            crate::index::test_miner_rules::CONFORMANCE_LEVEL
                if super::rules::conformance_scope(set) =>
            {
                LevelRules::Thousands
            }
            _ => {
                return Err(ActivationRegistryError::VersionNotSupported {
                    family,
                    value: value.into(),
                });
            }
        };
        Ok(Self { effective, level })
    }

    /// Expose the same weight used by the selected contribution formula.
    pub(crate) fn collab_weight_bps(self) -> u64 {
        match self.effective {
            EffectiveRules::V1 => super::energy_formula::COLLAB_WEIGHT_BPS as u64,
            #[cfg(test)]
            EffectiveRules::QuarterCollab => 2_500,
        }
    }

    /// Weight each collab separately before saturating summation.
    pub(crate) fn collab(self, raw: Energy) -> Energy {
        match self.effective {
            EffectiveRules::V1 => calc_collab_contribution(raw),
            #[cfg(test)]
            EffectiveRules::QuarterCollab => raw / 4,
        }
    }

    /// Combine a candidate's raw energy with already-weighted collab contributions.
    pub(crate) fn effective(self, raw: Energy, collab: Energy) -> Energy {
        match self.effective {
            EffectiveRules::V1 => calc_standard_effective_energy(raw, collab),
            #[cfg(test)]
            EffectiveRules::QuarterCollab => raw.saturating_add(collab),
        }
    }

    /// Resolve level and nominal difficulty factor under the same selected level contract.
    pub(crate) fn level_and_factor(self, effective: Energy) -> (u8, Energy) {
        match self.level {
            LevelRules::V1 => {
                let level = calc_level_from_effective_energy(effective);
                (level, calc_difficulty_factor_bps(level))
            }
            #[cfg(test)]
            LevelRules::Thousands => {
                let level = (effective / 1_000).min(50) as u8;
                (level, 10_000 - Energy::from(level) * 100)
            }
        }
    }
}
