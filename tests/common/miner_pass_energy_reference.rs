//! Independent three-stage block model shared by settlement and recovery acceptance.
use crate::index::MinerPassState;
use crate::storage::PassEnergyRecord;

// Independent block model: no production formula, version router or interval helper.
pub fn reference(mut r: PassEnergyRecord, target: u32, changes: &[(u32, u64)]) -> PassEnergyRecord {
    for height in r.block_height + 1..=target {
        if height == 10 {
            r.energy = r.energy.saturating_mul(2);
        }
        let rate = if height < 10 {
            1
        } else if height < 20 {
            2
        } else {
            3
        };
        if r.state == MinerPassState::Active {
            r.energy = r
                .energy
                .saturating_add((r.owner_balance / 100_000) as u128 * rate);
            for &(_, balance) in changes.iter().filter(|(h, _)| *h == height) {
                let before = r.owner_balance / 100_000;
                let after = balance / 100_000;
                let lost = before.saturating_sub(after);
                let penalty =
                    lost as u128 * (height - r.active_block_height) as u128 * rate * 3 / 2;
                r.energy = r.energy.saturating_sub(penalty);
                if (before == 0 && after > 0) || (lost > 0 && after == 0) {
                    r.active_block_height = height;
                }
                r.owner_balance = balance;
            }
        }
        r.block_height = height;
    }
    r
}
