// Energy formula constants are centralized here for consistency between
// production logic and tests. We intentionally keep them as code constants
// for now (not runtime config) to reduce protocol drift risk.

// 0.001 BTC threshold in satoshi.
pub const ENERGY_BALANCE_THRESHOLD: u64 = 100_000;

// Growth formula: balance * ENERGY_GROWTH_MULTIPLIER * r.
pub const ENERGY_GROWTH_MULTIPLIER: u64 = 10_000;

// Penalty formula: abs(delta) * ENERGY_PENALTY_MULTIPLIER.
// Current protocol multiplier = 10_000 * 6 * 24 * 30.
pub const ENERGY_PENALTY_MULTIPLIER: u64 = 43_200_000;

fn saturating_u128_to_u64(value: u128) -> u64 {
    if value > u64::MAX as u128 {
        u64::MAX
    } else {
        value as u64
    }
}

// Calculate growth delta with saturating arithmetic.
pub fn calc_growth_delta(owner_balance: u64, r: u32) -> u64 {
    if owner_balance < ENERGY_BALANCE_THRESHOLD {
        return 0;
    }

    let raw = (owner_balance as u128)
        .saturating_mul(ENERGY_GROWTH_MULTIPLIER as u128)
        .saturating_mul(r as u128);
    saturating_u128_to_u64(raw)
}

// Calculate penalty from signed balance delta. Positive/zero delta has no penalty.
pub fn calc_penalty_from_delta(delta: i64) -> u64 {
    if delta >= 0 {
        return 0;
    }

    let loss_sats = delta.unsigned_abs();
    let raw = (loss_sats as u128).saturating_mul(ENERGY_PENALTY_MULTIPLIER as u128);
    saturating_u128_to_u64(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_growth_delta_below_threshold_is_zero() {
        assert_eq!(calc_growth_delta(99_999, 100), 0);
    }

    #[test]
    fn test_growth_delta_saturates_to_u64_max() {
        let value = calc_growth_delta(u64::MAX, u32::MAX);
        assert_eq!(value, u64::MAX);
    }

    #[test]
    fn test_penalty_delta_non_negative_is_zero() {
        assert_eq!(calc_penalty_from_delta(0), 0);
        assert_eq!(calc_penalty_from_delta(10), 0);
    }

    #[test]
    fn test_penalty_delta_handles_i64_min_and_saturates() {
        let value = calc_penalty_from_delta(i64::MIN);
        assert_eq!(value, u64::MAX);
    }
}
