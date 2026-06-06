// UIP-0003 raw energy formula constants and pure helpers.
// Keep consensus-facing arithmetic integer-only and saturating at energy_uint.

pub type Energy = u128;

pub const UNIT_SATS: u64 = 100_000;
pub const ENERGY_PER_UNIT_BLOCK: Energy = 1;
pub const PENALTY_LAMBDA_NUM: Energy = 3;
pub const PENALTY_LAMBDA_DEN: Energy = 2;
pub const INHERIT_DISCOUNT_BPS: Energy = 500;
pub const BPS_DENOMINATOR: Energy = 10_000;
pub const COLLAB_WEIGHT_BPS: Energy = 5_000;
pub const ENERGY_MAX: Energy = Energy::MAX;

// Compatibility alias for current call sites. New UIP-0003 logic should use
// UNIT_SATS directly.
pub const ENERGY_BALANCE_THRESHOLD: u64 = UNIT_SATS;

const INHERIT_KEEP_BPS: Energy = BPS_DENOMINATOR - INHERIT_DISCOUNT_BPS;

/// Unit snapshots for one owner-balance change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitDelta {
    pub units_before: Energy,
    pub units_after: Energy,
    pub gained_units: Energy,
    pub lost_units: Energy,
}

/// Convert satoshi balance to UIP-0003 discrete balance units.
pub fn balance_units(balance_sats: u64) -> Energy {
    (balance_sats / UNIT_SATS) as Energy
}

/// Calculate gained/lost units from before/after balance snapshots.
pub fn calc_unit_delta(balance_before_sats: u64, balance_after_sats: u64) -> UnitDelta {
    let units_before = balance_units(balance_before_sats);
    let units_after = balance_units(balance_after_sats);

    UnitDelta {
        units_before,
        units_after,
        gained_units: units_after.saturating_sub(units_before),
        lost_units: units_before.saturating_sub(units_after),
    }
}

/// Clamp energy_uint to a u64 value for tests or transitional local callers.
pub fn saturating_energy_to_u64(value: Energy) -> u64 {
    value.min(u64::MAX as Energy) as u64
}

fn mul_div_floor_saturating(value: Energy, multiplier: Energy, denominator: Energy) -> Energy {
    assert!(denominator > 0, "energy denominator must be positive");

    let quotient = value / denominator;
    let remainder = value % denominator;

    let head = match quotient.checked_mul(multiplier) {
        Some(value) => value,
        None => return ENERGY_MAX,
    };
    let tail = remainder
        .checked_mul(multiplier)
        .map(|value| value / denominator)
        .unwrap_or(ENERGY_MAX);

    head.saturating_add(tail)
}

/// Calculate raw energy growth for a stable balance interval.
pub fn calc_growth_delta_energy(owner_balance_sats: u64, block_delta: u32) -> Energy {
    balance_units(owner_balance_sats)
        .saturating_mul(ENERGY_PER_UNIT_BLOCK)
        .saturating_mul(block_delta as Energy)
}

/// Calculate withdrawal penalty from lost units and active age.
pub fn calc_penalty_energy(lost_units: Energy, age_blocks: u32) -> Energy {
    let base = lost_units
        .saturating_mul(age_blocks as Energy)
        .saturating_mul(ENERGY_PER_UNIT_BLOCK);
    mul_div_floor_saturating(base, PENALTY_LAMBDA_NUM, PENALTY_LAMBDA_DEN)
}

/// Calculate withdrawal penalty from before/after balances and active height.
pub fn calc_balance_penalty_energy(
    balance_before_sats: u64,
    balance_after_sats: u64,
    active_block_height: u32,
    event_block_height: u32,
) -> Energy {
    let unit_delta = calc_unit_delta(balance_before_sats, balance_after_sats);
    if unit_delta.units_before == 0 || unit_delta.lost_units == 0 {
        return 0;
    }

    calc_penalty_energy(
        unit_delta.lost_units,
        event_block_height.saturating_sub(active_block_height),
    )
}

/// Calculate the active height after one owner-balance change.
pub fn calc_next_active_block_height(
    balance_before_sats: u64,
    balance_after_sats: u64,
    current_active_block_height: u32,
    event_block_height: u32,
) -> u32 {
    let unit_delta = calc_unit_delta(balance_before_sats, balance_after_sats);
    if (unit_delta.units_before == 0 && unit_delta.units_after > 0)
        || (unit_delta.lost_units > 0 && unit_delta.units_after == 0)
    {
        event_block_height
    } else {
        current_active_block_height
    }
}

/// Calculate raw energy inherited by one new pass from one prev pass.
pub fn calc_inheritable_energy(raw_energy: Energy) -> Energy {
    mul_div_floor_saturating(raw_energy, INHERIT_KEEP_BPS, BPS_DENOMINATOR)
}

/// Calculate UIP-0004 collab contribution from one active collab pass raw energy.
pub fn calc_collab_contribution(raw_energy: Energy) -> Energy {
    mul_div_floor_saturating(raw_energy, COLLAB_WEIGHT_BPS, BPS_DENOMINATOR)
}

/// Calculate UIP-0004 standard-pass effective energy from raw energy and aggregate collab contribution.
pub fn calc_standard_effective_energy(raw_energy: Energy, collab_contribution: Energy) -> Energy {
    raw_energy.saturating_add(collab_contribution)
}

/// Calculate raw energy growth.
pub fn calc_growth_delta(owner_balance_sats: u64, block_delta: u32) -> Energy {
    calc_growth_delta_energy(owner_balance_sats, block_delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_units_floor_at_threshold() {
        assert_eq!(balance_units(99_999), 0);
        assert_eq!(balance_units(100_000), 1);
        assert_eq!(balance_units(199_999), 1);
        assert_eq!(balance_units(200_000), 2);
    }

    #[test]
    fn test_unit_delta_uses_unit_snapshots_not_abs_delta_floor() {
        assert_eq!(
            calc_unit_delta(199_999, 100_000),
            UnitDelta {
                units_before: 1,
                units_after: 1,
                gained_units: 0,
                lost_units: 0,
            }
        );
        assert_eq!(
            calc_unit_delta(100_001, 99_999),
            UnitDelta {
                units_before: 1,
                units_after: 0,
                gained_units: 0,
                lost_units: 1,
            }
        );
        assert_eq!(
            calc_unit_delta(99_999, 100_000),
            UnitDelta {
                units_before: 0,
                units_after: 1,
                gained_units: 1,
                lost_units: 0,
            }
        );
    }

    #[test]
    fn test_growth_delta_uses_units_per_block() {
        assert_eq!(calc_growth_delta_energy(99_999, 144), 0);
        assert_eq!(calc_growth_delta_energy(100_000, 144), 144);
        assert_eq!(calc_growth_delta_energy(199_999, 144), 144);
        assert_eq!(calc_growth_delta_energy(200_000, 144), 288);
        assert_eq!(calc_growth_delta_energy(100_000_000, 1_008), 1_008_000);
    }

    #[test]
    fn test_growth_delta_u64_adapter_saturates() {
        assert_eq!(saturating_energy_to_u64(u64::MAX as Energy), u64::MAX);
        assert_eq!(saturating_energy_to_u64((u64::MAX as Energy) + 1), u64::MAX);
    }

    #[test]
    fn test_penalty_uses_lost_units_age_and_lambda() {
        assert_eq!(calc_penalty_energy(0, 100), 0);
        assert_eq!(calc_penalty_energy(1, 1), 1);
        assert_eq!(calc_penalty_energy(1, 2), 3);
        assert_eq!(calc_penalty_energy(2, 10), 30);
    }

    #[test]
    fn test_balance_penalty_ignores_non_unit_loss() {
        assert_eq!(calc_balance_penalty_energy(199_999, 100_000, 10, 20), 0);
        assert_eq!(calc_balance_penalty_energy(100_001, 99_999, 10, 20), 15);
        assert_eq!(calc_balance_penalty_energy(99_999, 0, 10, 20), 0);
    }

    #[test]
    fn test_active_height_changes_only_at_uip0003_boundaries() {
        assert_eq!(calc_next_active_block_height(100_000, 199_999, 7, 10), 7);
        assert_eq!(calc_next_active_block_height(99_999, 100_000, 7, 10), 10);
        assert_eq!(calc_next_active_block_height(250_000, 150_000, 7, 10), 7);
        assert_eq!(calc_next_active_block_height(100_001, 99_999, 7, 10), 10);
    }

    #[test]
    fn test_inheritable_energy_applies_five_percent_discount_floor() {
        assert_eq!(calc_inheritable_energy(0), 0);
        assert_eq!(calc_inheritable_energy(1), 0);
        assert_eq!(calc_inheritable_energy(20), 19);
        assert_eq!(calc_inheritable_energy(101), 95);
    }

    #[test]
    fn test_inheritable_energy_handles_u128_max_without_multiply_overflow() {
        let expected = (ENERGY_MAX / BPS_DENOMINATOR) * INHERIT_KEEP_BPS
            + ((ENERGY_MAX % BPS_DENOMINATOR) * INHERIT_KEEP_BPS) / BPS_DENOMINATOR;
        assert_eq!(calc_inheritable_energy(ENERGY_MAX), expected);
    }

    #[test]
    fn test_collab_contribution_applies_half_weight_floor() {
        assert_eq!(calc_collab_contribution(0), 0);
        assert_eq!(calc_collab_contribution(1), 0);
        assert_eq!(calc_collab_contribution(2), 1);
        assert_eq!(calc_collab_contribution(101), 50);
    }

    #[test]
    fn test_collab_contribution_handles_u128_max_without_multiply_overflow() {
        let expected = (ENERGY_MAX / BPS_DENOMINATOR) * COLLAB_WEIGHT_BPS
            + ((ENERGY_MAX % BPS_DENOMINATOR) * COLLAB_WEIGHT_BPS) / BPS_DENOMINATOR;
        assert_eq!(calc_collab_contribution(ENERGY_MAX), expected);
    }

    #[test]
    fn test_standard_effective_energy_saturates_raw_plus_contribution() {
        assert_eq!(calc_standard_effective_energy(10, 20), 30);
        assert_eq!(
            calc_standard_effective_energy(ENERGY_MAX - 1, 1),
            ENERGY_MAX
        );
        assert_eq!(
            calc_standard_effective_energy(ENERGY_MAX - 1, 2),
            ENERGY_MAX
        );
        assert_eq!(calc_standard_effective_energy(ENERGY_MAX, 1), ENERGY_MAX);
    }

    #[test]
    fn test_penalty_saturates_to_energy_max() {
        assert_eq!(calc_penalty_energy(ENERGY_MAX, u32::MAX), ENERGY_MAX);
    }
}
