use num_bigint::BigUint;
use sha3::{Digest, Keccak256};

/// UIP-0011 reward recipient and state-transition policy version.
pub const REWARD_RULE_VERSION_V1: u16 = 1;
/// UIP-0011 target-supply emission formula version.
pub const COINBASE_EMISSION_POLICY_VERSION_V1: u16 = 1;
/// UIP-0011 per-transaction fee split version.
pub const FEE_SPLIT_POLICY_VERSION_V1: u16 = 1;
/// UIP-0012 rolling collaboration coefficient version.
pub const COLLABORATION_EFFICIENCY_POLICY_VERSION_V1: u16 = 1;
/// UIP-0013 fixed-price policy version.
pub const PRICE_POLICY_VERSION_V1: u32 = 1;

/// Number of satoshis in one BTC.
pub const BTC_SATS_PER_BTC: u64 = 100_000_000;
/// UIP-0011 v1 release smoothing window.
pub const EMISSION_BLOCKS: u64 = 157_680;
/// Shared basis-point denominator.
pub const REWARD_BPS_DENOMINATOR: u64 = 10_000;
/// Miner transaction-fee share under UIP-0011 v1.
pub const MINER_FEE_BPS: u64 = 6_000;
/// Dividend transaction-fee share under UIP-0011 v1.
pub const DAO_FEE_BPS: u64 = 4_000;

/// Neutral UIP-0012 collaboration coefficient.
pub const K_BPS_BASE: u64 = 10_000;
/// Minimum integer coefficient satisfying K > 0.8.
pub const K_BPS_MIN: u64 = 8_001;
/// Maximum UIP-0012 collaboration coefficient.
pub const K_BPS_MAX: u64 = 20_000;
/// UIP-0012 v1 rolling-window length.
pub const K_WINDOW_BLOCKS: u64 = 50_400;

/// UIP-0013 v1 fixed price: 100,000 USDB native units per BTC.
pub const FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1: &str = "100000000000000000000000";
/// UIP-0013 source-kind identifier for a chain-owned fixed price.
pub const FIXED_PRICE_SOURCE_KIND_V1: u32 = 1;

const FIXED_PRICE_RANGE_DOMAIN_V1: &str = "usdb.price.policy.range:v1";

/// Auditable intermediate values produced by the UIP-0011 v1 emission formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmissionResult {
    pub target_supply_atoms: BigUint,
    pub remaining_target_atoms: BigUint,
    pub coinbase_emission_atoms: BigUint,
    pub issued_usdb_atoms_after: BigUint,
}

/// Complete per-transaction UIP-0011 v1 fee allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeSplit {
    pub miner_atoms: BigUint,
    pub dao_atoms: BigUint,
}

/// Parses the frozen UIP-0013 v1 fixed price.
pub fn fixed_price_atoms_per_btc_v1() -> BigUint {
    BigUint::parse_bytes(FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1.as_bytes(), 10)
        .expect("built-in UIP-0013 fixed price must be valid")
}

/// Derives the immutable UIP-0013 v1 activation-range identity.
pub fn fixed_price_range_id_v1(chain_id: &BigUint, start_block: u64) -> Result<[u8; 32], String> {
    validate_uint256("chain_id", chain_id, false)?;
    let mut encoded =
        Vec::with_capacity(FIXED_PRICE_RANGE_DOMAIN_V1.len() + 1 + 32 + 8 + 4 + 4 + 32);
    encoded.extend_from_slice(FIXED_PRICE_RANGE_DOMAIN_V1.as_bytes());
    encoded.push(0);
    let mut chain_id_word = [0u8; 32];
    let chain_id_bytes = chain_id.to_bytes_be();
    chain_id_word[32 - chain_id_bytes.len()..].copy_from_slice(&chain_id_bytes);
    encoded.extend_from_slice(&chain_id_word);
    encoded.extend_from_slice(&start_block.to_be_bytes());
    encoded.extend_from_slice(&PRICE_POLICY_VERSION_V1.to_be_bytes());
    encoded.extend_from_slice(&FIXED_PRICE_SOURCE_KIND_V1.to_be_bytes());
    let mut price_word = [0u8; 32];
    let price_bytes = fixed_price_atoms_per_btc_v1().to_bytes_be();
    price_word[32 - price_bytes.len()..].copy_from_slice(&price_bytes);
    encoded.extend_from_slice(&price_word);
    Ok(Keccak256::digest(encoded).into())
}

/// Applies the UIP-0011 v1 integer target-supply formula.
pub fn calculate_coinbase_emission_v1(
    total_miner_btc_sats: &BigUint,
    price_atoms_per_btc: &BigUint,
    issued_usdb_atoms: &BigUint,
    k_bps: u64,
) -> Result<EmissionResult, String> {
    if total_miner_btc_sats.bits() > 64 {
        return Err("total_miner_btc_sats must be uint64".to_string());
    }
    validate_uint256("price_atoms_per_btc", price_atoms_per_btc, false)?;
    validate_uint256("issued_usdb_atoms", issued_usdb_atoms, true)?;
    if !(K_BPS_MIN..=K_BPS_MAX).contains(&k_bps) {
        return Err(format!(
            "k_bps {k_bps} is outside [{K_BPS_MIN},{K_BPS_MAX}]"
        ));
    }

    let target_supply_atoms =
        total_miner_btc_sats * price_atoms_per_btc / BigUint::from(BTC_SATS_PER_BTC);
    if target_supply_atoms.bits() > 256 {
        return Err("target_supply_atoms overflows uint256".to_string());
    }
    let remaining_target_atoms = if target_supply_atoms > *issued_usdb_atoms {
        &target_supply_atoms - issued_usdb_atoms
    } else {
        BigUint::default()
    };
    let denominator = BigUint::from(EMISSION_BLOCKS * REWARD_BPS_DENOMINATOR);
    let mut coinbase_emission_atoms = &remaining_target_atoms * BigUint::from(k_bps) / denominator;
    if coinbase_emission_atoms > remaining_target_atoms {
        coinbase_emission_atoms = remaining_target_atoms.clone();
    }
    let issued_usdb_atoms_after = issued_usdb_atoms + &coinbase_emission_atoms;
    if issued_usdb_atoms_after.bits() > 256 {
        return Err("issued_usdb_atoms_after overflows uint256".to_string());
    }
    Ok(EmissionResult {
        target_supply_atoms,
        remaining_target_atoms,
        coinbase_emission_atoms,
        issued_usdb_atoms_after,
    })
}

/// Splits one refund-adjusted transaction fee, assigning rounding to the miner.
pub fn split_transaction_fee_v1(transaction_fee_atoms: &BigUint) -> Result<FeeSplit, String> {
    validate_uint256("tx_fee_atoms", transaction_fee_atoms, true)?;
    let dao_atoms =
        transaction_fee_atoms * BigUint::from(DAO_FEE_BPS) / BigUint::from(REWARD_BPS_DENOMINATOR);
    Ok(FeeSplit {
        miner_atoms: transaction_fee_atoms - &dao_atoms,
        dao_atoms,
    })
}

/// Computes the UIP-0012 v1 coefficient from current and average energy.
pub fn calculate_k_bps_v1(
    current_energy: &BigUint,
    average_energy: &BigUint,
) -> Result<u64, String> {
    validate_uint128("current_energy", current_energy)?;
    validate_uint128("average_energy", average_energy)?;
    if average_energy == &BigUint::default() {
        return Ok(K_BPS_BASE);
    }
    if current_energy < average_energy {
        let numerator = BigUint::from(60_000u64) * average_energy;
        let denominator = current_energy + BigUint::from(5u8) * average_energy;
        let penalty = ceil_divide(&numerator, &denominator);
        let penalty = penalty.to_u64_digits().first().copied().unwrap_or_default();
        return Ok(K_BPS_MAX.saturating_sub(penalty).max(K_BPS_MIN));
    }
    let k = BigUint::from(K_BPS_BASE) * current_energy / average_energy;
    if k >= BigUint::from(K_BPS_MAX) {
        return Ok(K_BPS_MAX);
    }
    Ok(k.to_u64_digits().first().copied().unwrap_or_default())
}

fn validate_uint256(name: &str, value: &BigUint, allow_zero: bool) -> Result<(), String> {
    if value.bits() > 256 {
        return Err(format!("{name} must be uint256"));
    }
    if !allow_zero && value == &BigUint::default() {
        return Err(format!("{name} must be positive"));
    }
    Ok(())
}

fn validate_uint128(name: &str, value: &BigUint) -> Result<(), String> {
    if value.bits() > 128 {
        return Err(format!("{name} must be uint128"));
    }
    Ok(())
}

fn ceil_divide(numerator: &BigUint, denominator: &BigUint) -> BigUint {
    (numerator + denominator - BigUint::from(1u8)) / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decimal(value: &str) -> BigUint {
        BigUint::parse_bytes(value.as_bytes(), 10).expect("test decimal must be valid")
    }

    #[test]
    fn coinbase_emission_v1_matches_go_golden_vectors() {
        let vectors = [
            (
                "zero-total",
                "0",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "0",
                10_000,
                "0",
                "0",
                "0",
                "0",
            ),
            (
                "issued-above-target",
                "100000000",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "200000000000000000000000",
                10_000,
                "100000000000000000000000",
                "0",
                "0",
                "200000000000000000000000",
            ),
            (
                "baseline-one-btc",
                "100000000",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "0",
                10_000,
                "100000000000000000000000",
                "100000000000000000000000",
                "634195839675291730",
                "634195839675291730",
            ),
            (
                "penalized-partial-target",
                "1234567890",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "234567890000000000000000",
                8_001,
                "1234567890000000000000000",
                "1000000000000000000000000",
                "5074200913242009132",
                "234572964200913242009132",
            ),
            (
                "single-sat-rounding",
                "1",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "0",
                10_000,
                "1000000000000000",
                "1000000000000000",
                "6341958396",
                "6341958396",
            ),
            (
                "remaining-one-atom",
                "2100000000000000",
                FIXED_PRICE_ATOMS_PER_BTC_DECIMAL_V1,
                "2099999999999999999999999999999",
                20_000,
                "2100000000000000000000000000000",
                "1",
                "0",
                "2099999999999999999999999999999",
            ),
        ];
        for (id, total, price, issued, k, target, remaining, emission, issued_after) in vectors {
            let result = calculate_coinbase_emission_v1(
                &decimal(total),
                &decimal(price),
                &decimal(issued),
                k,
            )
            .unwrap_or_else(|err| panic!("{id}: {err}"));
            assert_eq!(result.target_supply_atoms, decimal(target), "{id}: target");
            assert_eq!(
                result.remaining_target_atoms,
                decimal(remaining),
                "{id}: remaining"
            );
            assert_eq!(
                result.coinbase_emission_atoms,
                decimal(emission),
                "{id}: emission"
            );
            assert_eq!(
                result.issued_usdb_atoms_after,
                decimal(issued_after),
                "{id}: issued_after"
            );
        }
    }

    #[test]
    fn k_v1_matches_go_golden_vectors() {
        let vectors = [
            ("0", "0", 10_000),
            ("100", "0", 10_000),
            ("0", "100", 8_001),
            ("50", "100", 9_090),
            ("99", "100", 9_983),
            ("100", "100", 10_000),
            ("150", "100", 15_000),
            ("200", "100", 20_000),
            ("201", "100", 20_000),
        ];
        for (current, average, expected) in vectors {
            assert_eq!(
                calculate_k_bps_v1(&decimal(current), &decimal(average)).unwrap(),
                expected,
                "current={current} average={average}"
            );
        }
    }

    #[test]
    fn fee_v1_matches_go_golden_vectors() {
        let vectors = [
            ("0", "0", "0"),
            ("1", "1", "0"),
            ("2", "2", "0"),
            ("3", "2", "1"),
            ("10001", "6001", "4000"),
        ];
        for (fee, miner, dao) in vectors {
            let split = split_transaction_fee_v1(&decimal(fee)).unwrap();
            assert_eq!(split.miner_atoms, decimal(miner), "fee={fee}: miner");
            assert_eq!(split.dao_atoms, decimal(dao), "fee={fee}: dao");
        }
    }

    #[test]
    fn fixed_price_range_id_matches_go_golden_vector() {
        let range_id = fixed_price_range_id_v1(&BigUint::from(20_260_323u64), 0).unwrap();
        assert_eq!(
            range_id,
            [
                0x2a, 0xe4, 0x5c, 0xaf, 0xae, 0x84, 0xcc, 0x89, 0x2d, 0x1d, 0x43, 0x54, 0xf0, 0x2a,
                0x08, 0x69, 0xf9, 0x7d, 0xfd, 0x6c, 0xa2, 0xc7, 0x57, 0xba, 0x51, 0x1c, 0x57, 0x68,
                0x0b, 0x8b, 0xfa, 0xf4,
            ]
        );
    }

    #[test]
    fn formulas_reject_out_of_range_values() {
        let uint256_overflow = BigUint::from(1u8) << 256;
        let uint128_overflow = BigUint::from(1u8) << 128;
        assert!(
            calculate_coinbase_emission_v1(
                &(BigUint::from(1u8) << 64),
                &BigUint::from(1u8),
                &BigUint::default(),
                K_BPS_BASE,
            )
            .is_err()
        );
        assert!(
            calculate_coinbase_emission_v1(
                &BigUint::from(1u8),
                &BigUint::default(),
                &BigUint::default(),
                K_BPS_BASE,
            )
            .is_err()
        );
        assert!(split_transaction_fee_v1(&uint256_overflow).is_err());
        assert!(calculate_k_bps_v1(&uint128_overflow, &BigUint::from(1u8)).is_err());
    }
}
