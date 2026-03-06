use super::common::{cleanup_temp_dir, test_inscription_id, test_root_dir, test_script_hash};
use crate::config::ConfigManager;
use crate::index::content::MinerPassState;
use crate::index::energy::{BalanceProvider, PassEnergyManager};
use crate::index::energy_formula::{calc_growth_delta, calc_penalty_from_delta};
use crate::storage::PassEnergyStorage;
use balance_history::AddressBalance;
use ord::InscriptionId;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

struct TimelineBalanceProvider {
    timelines: HashMap<USDBScriptHash, Vec<AddressBalance>>,
}

impl TimelineBalanceProvider {
    fn new(timelines: HashMap<USDBScriptHash, Vec<AddressBalance>>) -> Self {
        let mut normalized = HashMap::new();
        for (address, mut points) in timelines {
            points.sort_unstable_by_key(|v| v.block_height);
            normalized.insert(address, points);
        }

        Self {
            timelines: normalized,
        }
    }
}

impl BalanceProvider for TimelineBalanceProvider {
    fn get_balance_at_height<'a>(
        &'a self,
        address: USDBScriptHash,
        block_height: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move {
            let ret = self
                .timelines
                .get(&address)
                .and_then(|points| {
                    points
                        .iter()
                        .rev()
                        .find(|v| v.block_height <= block_height)
                        .cloned()
                })
                .map(|v| vec![v])
                .unwrap_or_default();
            Ok(ret)
        })
    }

    fn get_balance_at_range<'a>(
        &'a self,
        address: USDBScriptHash,
        block_range: Range<u32>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move {
            let ret = self
                .timelines
                .get(&address)
                .map(|points| {
                    points
                        .iter()
                        .filter(|v| block_range.contains(&v.block_height))
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(ret)
        })
    }
}

fn expected_energy_by_formula(
    initial_block_height: u32,
    initial_balance: u64,
    inherited_energy: u64,
    timeline_points: &[AddressBalance],
    target_height: u32,
) -> u64 {
    assert!(target_height >= initial_block_height);
    let mut energy = inherited_energy;
    let mut owner_balance = initial_balance;
    let mut active_block_height = initial_block_height;
    let mut last_processed_height = initial_block_height;

    for point in timeline_points {
        if point.block_height <= initial_block_height {
            continue;
        }
        if point.block_height > target_height {
            break;
        }

        let r = point.block_height - active_block_height;
        let delta = calc_growth_delta(owner_balance, r);
        energy = energy.saturating_add(delta);

        if point.delta < 0 {
            let penalty = calc_penalty_from_delta(point.delta);
            energy = energy.saturating_sub(penalty);
            active_block_height = point.block_height;
        }

        owner_balance = point.balance;
        last_processed_height = point.block_height;
    }

    if last_processed_height < target_height {
        let r = target_height - active_block_height;
        let delta = calc_growth_delta(owner_balance, r);
        energy = energy.saturating_add(delta);
    }

    energy
}

async fn setup_manager_with_timeline(
    test_name: &str,
    owner: USDBScriptHash,
    timeline_points: Vec<AddressBalance>,
    initial_height: u32,
    inherited_energy: u64,
) -> (PathBuf, PassEnergyManager, InscriptionId) {
    let root_dir = test_root_dir("energy_timeline", test_name);
    let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
    let storage = PassEnergyStorage::new(&config.data_dir()).unwrap();

    let mut timelines = HashMap::new();
    timelines.insert(owner, timeline_points.clone());
    let provider = Arc::new(TimelineBalanceProvider::new(timelines));
    let manager = PassEnergyManager::new_with_deps(config, storage, provider);

    let inscription_id = test_inscription_id(9, 0);
    manager
        .on_new_pass(&inscription_id, &owner, initial_height, inherited_energy)
        .await
        .unwrap();

    (root_dir, manager, inscription_id)
}

#[tokio::test]
async fn test_energy_timeline_no_change_growth_matches_formula() {
    let owner = test_script_hash(10);
    let timeline = vec![AddressBalance {
        block_height: 100,
        balance: 200_000,
        delta: 100,
    }];
    let (root_dir, manager, inscription_id) =
        setup_manager_with_timeline("no_change", owner, timeline.clone(), 100, 0).await;

    for height in 100..=105 {
        let got = manager
            .update_pass_energy(&inscription_id, height)
            .await
            .unwrap();
        let expected = expected_energy_by_formula(100, 200_000, 0, &timeline, height);
        assert_eq!(got.state, MinerPassState::Active);
        assert_eq!(got.energy, expected, "height={}", height);
    }

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_energy_timeline_below_threshold_has_no_growth() {
    let owner = test_script_hash(11);
    let timeline = vec![AddressBalance {
        block_height: 100,
        balance: 99_999,
        delta: 100,
    }];
    let (root_dir, manager, inscription_id) =
        setup_manager_with_timeline("below_threshold", owner, timeline.clone(), 100, 0).await;

    for height in 100..=110 {
        let got = manager
            .update_pass_energy(&inscription_id, height)
            .await
            .unwrap();
        let expected = expected_energy_by_formula(100, 99_999, 0, &timeline, height);
        assert_eq!(got.state, MinerPassState::Active);
        assert_eq!(got.energy, expected, "height={}", height);
    }

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_energy_timeline_mixed_deltas_matches_formula_and_is_idempotent() {
    let owner = test_script_hash(12);
    let timeline = vec![
        AddressBalance {
            block_height: 100,
            balance: 200_000,
            delta: 100,
        },
        AddressBalance {
            block_height: 103,
            balance: 210_000,
            delta: 10_000,
        },
        AddressBalance {
            block_height: 106,
            balance: 190_000,
            delta: -20_000,
        },
        AddressBalance {
            block_height: 108,
            balance: 220_000,
            delta: 30_000,
        },
    ];
    let (root_dir, manager, inscription_id) =
        setup_manager_with_timeline("mixed_deltas", owner, timeline.clone(), 100, 0).await;

    for height in 100..=110 {
        let got_first = manager
            .update_pass_energy(&inscription_id, height)
            .await
            .unwrap();
        let got_second = manager
            .update_pass_energy(&inscription_id, height)
            .await
            .unwrap();
        let expected = expected_energy_by_formula(100, 200_000, 0, &timeline, height);

        assert_eq!(got_first.state, MinerPassState::Active);
        assert_eq!(got_first.energy, expected, "first_call_height={}", height);
        assert_eq!(got_second.energy, expected, "second_call_height={}", height);
    }

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_energy_timeline_dormant_then_consumed_freezes_and_zeroes_energy() {
    let owner = test_script_hash(13);
    let timeline = vec![
        AddressBalance {
            block_height: 100,
            balance: 200_000,
            delta: 100,
        },
        AddressBalance {
            block_height: 103,
            balance: 210_000,
            delta: 10_000,
        },
    ];
    let (root_dir, manager, inscription_id) =
        setup_manager_with_timeline("dormant_consumed", owner, timeline.clone(), 100, 0).await;

    let expected_at_108 = expected_energy_by_formula(100, 200_000, 0, &timeline, 108);
    manager.on_pass_dormant(&inscription_id, 108).await.unwrap();

    for height in 108..=112 {
        let got = manager
            .update_pass_energy(&inscription_id, height)
            .await
            .unwrap();
        assert_eq!(got.state, MinerPassState::Dormant);
        assert_eq!(got.energy, expected_at_108, "height={}", height);
    }

    manager
        .on_pass_consumed(&inscription_id, &owner, 112)
        .unwrap();
    let consumed = manager
        .get_pass_energy(&inscription_id, 112)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.state, MinerPassState::Consumed);
    assert_eq!(consumed.energy, 0);

    cleanup_temp_dir(&root_dir);
}
