use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};
use crate::config::ConfigManager;
use crate::index::MinerPassState;
use crate::index::energy::{BalanceProvider, PassEnergyManager};
use crate::index::energy_formula::{calc_growth_delta, calc_penalty_from_delta};
use crate::index::pass::{MinerPassManager, PassMintInscriptionInfo};
use crate::storage::{MinerPassStorage, MinerPassStorageRef, PassEnergyStorage};
use balance_history::AddressBalance;
use bitcoincore_rpc::bitcoin::Txid;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

enum ScenarioOp {
    Mint {
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        height: u32,
        prev: Vec<InscriptionId>,
    },
    Transfer {
        inscription_id: InscriptionId,
        new_owner: USDBScriptHash,
        height: u32,
        satpoint: SatPoint,
    },
}

struct ScenarioRunner {
    manager: MinerPassManager,
    tx_seed: u8,
}

impl ScenarioRunner {
    async fn run(&mut self, ops: Vec<ScenarioOp>) -> Result<(), String> {
        for op in ops {
            match op {
                ScenarioOp::Mint {
                    inscription_id,
                    owner,
                    height,
                    prev,
                } => {
                    let mint_info = PassMintInscriptionInfo {
                        inscription_number: inscription_id.index as i32,
                        mint_txid: Txid::from_slice(&[self.tx_seed; 32]).unwrap(),
                        mint_block_height: height,
                        mint_owner: owner,
                        satpoint: test_satpoint(self.tx_seed, 0, 0),
                        eth_main: "0x1111111111111111111111111111111111111111".to_string(),
                        eth_collab: None,
                        prev,
                        inscription_id,
                    };
                    self.tx_seed = self.tx_seed.wrapping_add(1);
                    self.manager.on_mint_pass(&mint_info).await?;
                }
                ScenarioOp::Transfer {
                    inscription_id,
                    new_owner,
                    height,
                    satpoint,
                } => {
                    self.manager
                        .on_pass_transfer(&inscription_id, &new_owner, &satpoint, height)
                        .await?;
                }
            }
        }

        Ok(())
    }
}

fn setup_manager_with_mock(
    test_name: &str,
    mock_provider: Arc<dyn BalanceProvider>,
) -> (
    PathBuf,
    MinerPassStorageRef,
    Arc<PassEnergyManager>,
    MinerPassManager,
) {
    let root_dir = test_root_dir("pass_scenario", test_name);
    let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
    let pass_storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
    let energy_storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
    let energy_manager = Arc::new(PassEnergyManager::new_with_deps(
        config.clone(),
        energy_storage,
        mock_provider,
    ));
    let manager =
        MinerPassManager::new(config, pass_storage.clone(), energy_manager.clone()).unwrap();

    (root_dir, pass_storage, energy_manager, manager)
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

#[tokio::test]
async fn test_scenario_transfer_then_remint_prev_inherits_and_consumes() {
    let owner_a = test_script_hash(40);
    let owner_b = test_script_hash(41);
    let pass_old = test_inscription_id(50, 0);
    let pass_new = test_inscription_id(51, 0);

    let mock_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 100, 200_000, 100)
            .with_height(owner_b, 120, 150_000, 10)
            .with_range(
                owner_a,
                101..111,
                vec![AddressBalance {
                    block_height: 105,
                    balance: 201_000,
                    delta: 1_000,
                }],
            ),
    );
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("transfer_remint", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 80,
    };
    runner
        .run(vec![
            ScenarioOp::Mint {
                inscription_id: pass_old.clone(),
                owner: owner_a,
                height: 100,
                prev: vec![],
            },
            ScenarioOp::Transfer {
                inscription_id: pass_old.clone(),
                new_owner: owner_b,
                height: 110,
                satpoint: test_satpoint(90, 1, 1),
            },
            ScenarioOp::Mint {
                inscription_id: pass_new.clone(),
                owner: owner_b,
                height: 120,
                prev: vec![pass_old.clone()],
            },
        ])
        .await
        .unwrap();

    let old_pass = storage
        .get_pass_by_inscription_id(&pass_old)
        .unwrap()
        .unwrap();
    assert_eq!(old_pass.owner, owner_b);
    assert_eq!(old_pass.state, MinerPassState::Consumed);

    let new_pass = storage
        .get_pass_by_inscription_id(&pass_new)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_b);
    assert_eq!(new_pass.state, MinerPassState::Active);

    let old_dormant_110 = energy_manager
        .get_pass_energy(&pass_old, 110)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_dormant_110.state, MinerPassState::Dormant);
    let expected_old_dormant_110 = expected_energy_by_formula(
        100,
        200_000,
        0,
        &[AddressBalance {
            block_height: 105,
            balance: 201_000,
            delta: 1_000,
        }],
        110,
    );
    assert_eq!(old_dormant_110.energy, expected_old_dormant_110);

    let old_consumed_120 = energy_manager
        .get_pass_energy(&pass_old, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_consumed_120.state, MinerPassState::Consumed);
    assert_eq!(old_consumed_120.energy, 0);

    let new_energy_120 = energy_manager
        .get_pass_energy(&pass_new, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy_120.state, MinerPassState::Active);
    assert_eq!(new_energy_120.energy, old_dormant_110.energy);

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_scenario_same_owner_remint_prev_consumed_and_single_active() {
    let owner_a = test_script_hash(60);
    let pass_old = test_inscription_id(61, 0);
    let pass_new = test_inscription_id(62, 0);

    let mock_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 100, 200_000, 100)
            .with_height(owner_a, 130, 210_000, 500)
            .with_range(
                owner_a,
                101..131,
                vec![AddressBalance {
                    block_height: 120,
                    balance: 205_000,
                    delta: 500,
                }],
            ),
    );
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("same_owner_remint", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 100,
    };
    runner
        .run(vec![
            ScenarioOp::Mint {
                inscription_id: pass_old.clone(),
                owner: owner_a,
                height: 100,
                prev: vec![],
            },
            ScenarioOp::Mint {
                inscription_id: pass_new.clone(),
                owner: owner_a,
                height: 130,
                prev: vec![pass_old.clone()],
            },
        ])
        .await
        .unwrap();

    let old_pass = storage
        .get_pass_by_inscription_id(&pass_old)
        .unwrap()
        .unwrap();
    assert_eq!(old_pass.owner, owner_a);
    assert_eq!(old_pass.state, MinerPassState::Consumed);

    let new_pass = storage
        .get_pass_by_inscription_id(&pass_new)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_a);
    assert_eq!(new_pass.state, MinerPassState::Active);

    let active = storage.get_all_active_pass_by_page(0, 10).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].inscription_id, pass_new);

    let old_energy_130 = energy_manager
        .get_pass_energy(&pass_old, 130)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_energy_130.state, MinerPassState::Consumed);
    assert_eq!(old_energy_130.energy, 0);

    let new_energy_130 = energy_manager
        .get_pass_energy(&pass_new, 130)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy_130.state, MinerPassState::Active);
    let expected_new_energy_130 = expected_energy_by_formula(
        100,
        200_000,
        0,
        &[AddressBalance {
            block_height: 120,
            balance: 205_000,
            delta: 500,
        }],
        130,
    );
    assert_eq!(new_energy_130.energy, expected_new_energy_130);

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_scenario_transfer_to_owner_with_existing_active_keeps_existing_active() {
    let owner_a = test_script_hash(70);
    let owner_b = test_script_hash(71);
    let pass_a = test_inscription_id(72, 0);
    let pass_b = test_inscription_id(73, 0);

    let mock_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 100, 200_000, 100)
            .with_height(owner_b, 105, 210_000, 100)
            .with_range(
                owner_a,
                101..111,
                vec![AddressBalance {
                    block_height: 108,
                    balance: 205_000,
                    delta: 5_000,
                }],
            ),
    );
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("transfer_to_owner_with_active", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 120,
    };
    runner
        .run(vec![
            ScenarioOp::Mint {
                inscription_id: pass_a.clone(),
                owner: owner_a,
                height: 100,
                prev: vec![],
            },
            ScenarioOp::Mint {
                inscription_id: pass_b.clone(),
                owner: owner_b,
                height: 105,
                prev: vec![],
            },
            ScenarioOp::Transfer {
                inscription_id: pass_a.clone(),
                new_owner: owner_b,
                height: 110,
                satpoint: test_satpoint(121, 1, 1),
            },
        ])
        .await
        .unwrap();

    let transferred = storage
        .get_pass_by_inscription_id(&pass_a)
        .unwrap()
        .unwrap();
    assert_eq!(transferred.owner, owner_b);
    assert_eq!(transferred.state, MinerPassState::Dormant);

    let existing = storage
        .get_pass_by_inscription_id(&pass_b)
        .unwrap()
        .unwrap();
    assert_eq!(existing.owner, owner_b);
    assert_eq!(existing.state, MinerPassState::Active);

    let active = storage.get_all_active_pass_by_page(0, 10).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].inscription_id, pass_b);

    let transferred_energy = energy_manager
        .get_pass_energy(&pass_a, 110)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transferred_energy.state, MinerPassState::Dormant);
    let expected_transferred_energy = expected_energy_by_formula(
        100,
        200_000,
        0,
        &[AddressBalance {
            block_height: 108,
            balance: 205_000,
            delta: 5_000,
        }],
        110,
    );
    assert_eq!(transferred_energy.energy, expected_transferred_energy);

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_scenario_mint_with_multiple_prev_inherits_sum_and_consumes_all() {
    let owner_a = test_script_hash(80);
    let owner_b = test_script_hash(81);
    let pass_prev_1 = test_inscription_id(82, 0);
    let pass_prev_2 = test_inscription_id(83, 0);
    let pass_new = test_inscription_id(84, 0);

    let mock_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 100, 200_000, 100)
            .with_height(owner_a, 120, 220_000, 100)
            .with_height(owner_b, 140, 260_000, 100)
            .with_range(
                owner_a,
                101..111,
                vec![AddressBalance {
                    block_height: 106,
                    balance: 210_000,
                    delta: 10_000,
                }],
            )
            .with_range(
                owner_a,
                121..131,
                vec![AddressBalance {
                    block_height: 126,
                    balance: 230_000,
                    delta: 10_000,
                }],
            ),
    );
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("multiple_prev_inherit", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 140,
    };
    runner
        .run(vec![
            ScenarioOp::Mint {
                inscription_id: pass_prev_1.clone(),
                owner: owner_a,
                height: 100,
                prev: vec![],
            },
            ScenarioOp::Transfer {
                inscription_id: pass_prev_1.clone(),
                new_owner: owner_b,
                height: 110,
                satpoint: test_satpoint(141, 1, 1),
            },
            ScenarioOp::Mint {
                inscription_id: pass_prev_2.clone(),
                owner: owner_a,
                height: 120,
                prev: vec![],
            },
            ScenarioOp::Transfer {
                inscription_id: pass_prev_2.clone(),
                new_owner: owner_b,
                height: 130,
                satpoint: test_satpoint(142, 1, 1),
            },
            ScenarioOp::Mint {
                inscription_id: pass_new.clone(),
                owner: owner_b,
                height: 140,
                prev: vec![pass_prev_1.clone(), pass_prev_2.clone()],
            },
        ])
        .await
        .unwrap();

    let prev_1_dormant = energy_manager
        .get_pass_energy(&pass_prev_1, 110)
        .await
        .unwrap()
        .unwrap();
    let prev_2_dormant = energy_manager
        .get_pass_energy(&pass_prev_2, 130)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev_1_dormant.state, MinerPassState::Dormant);
    assert_eq!(prev_2_dormant.state, MinerPassState::Dormant);
    let expected_prev_1_dormant = expected_energy_by_formula(
        100,
        200_000,
        0,
        &[AddressBalance {
            block_height: 106,
            balance: 210_000,
            delta: 10_000,
        }],
        110,
    );
    let expected_prev_2_dormant = expected_energy_by_formula(
        120,
        220_000,
        0,
        &[AddressBalance {
            block_height: 126,
            balance: 230_000,
            delta: 10_000,
        }],
        130,
    );
    assert_eq!(prev_1_dormant.energy, expected_prev_1_dormant);
    assert_eq!(prev_2_dormant.energy, expected_prev_2_dormant);

    let prev_1_consumed = energy_manager
        .get_pass_energy(&pass_prev_1, 140)
        .await
        .unwrap()
        .unwrap();
    let prev_2_consumed = energy_manager
        .get_pass_energy(&pass_prev_2, 140)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev_1_consumed.state, MinerPassState::Consumed);
    assert_eq!(prev_1_consumed.energy, 0);
    assert_eq!(prev_2_consumed.state, MinerPassState::Consumed);
    assert_eq!(prev_2_consumed.energy, 0);

    let new_pass = storage
        .get_pass_by_inscription_id(&pass_new)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_b);
    assert_eq!(new_pass.state, MinerPassState::Active);

    let new_energy = energy_manager
        .get_pass_energy(&pass_new, 140)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy.state, MinerPassState::Active);
    assert_eq!(
        new_energy.energy,
        prev_1_dormant.energy.saturating_add(prev_2_dormant.energy)
    );

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_scenario_missing_prev_is_ignored_and_new_pass_stays_active() {
    let owner_a = test_script_hash(90);
    let pass_new = test_inscription_id(91, 0);
    let missing_prev = test_inscription_id(92, 0);

    let mock_provider =
        Arc::new(MockBalanceProvider::default().with_height(owner_a, 100, 200_000, 100));
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("missing_prev_ignored", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 160,
    };
    runner
        .run(vec![ScenarioOp::Mint {
            inscription_id: pass_new.clone(),
            owner: owner_a,
            height: 100,
            prev: vec![missing_prev],
        }])
        .await
        .unwrap();

    let new_pass = storage
        .get_pass_by_inscription_id(&pass_new)
        .unwrap()
        .unwrap();
    assert_eq!(new_pass.owner, owner_a);
    assert_eq!(new_pass.state, MinerPassState::Active);

    let new_energy = energy_manager
        .get_pass_energy(&pass_new, 100)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(new_energy.state, MinerPassState::Active);
    assert_eq!(new_energy.energy, 0);

    cleanup_temp_dir(&root_dir);
}

#[tokio::test]
async fn test_scenario_double_inherit_same_prev_only_first_gets_energy() {
    let owner_a = test_script_hash(100);
    let owner_b = test_script_hash(101);
    let pass_prev = test_inscription_id(102, 0);
    let pass_new_1 = test_inscription_id(103, 0);
    let pass_new_2 = test_inscription_id(104, 0);

    let mock_provider = Arc::new(
        MockBalanceProvider::default()
            .with_height(owner_a, 100, 200_000, 100)
            .with_height(owner_b, 120, 220_000, 100)
            .with_height(owner_b, 130, 230_000, 100)
            .with_range(
                owner_a,
                101..111,
                vec![AddressBalance {
                    block_height: 106,
                    balance: 210_000,
                    delta: 10_000,
                }],
            )
            .with_range(
                owner_b,
                121..131,
                vec![AddressBalance {
                    block_height: 125,
                    balance: 225_000,
                    delta: 5_000,
                }],
            ),
    );
    let (root_dir, storage, energy_manager, manager) =
        setup_manager_with_mock("double_inherit_same_prev", mock_provider);

    let mut runner = ScenarioRunner {
        manager,
        tx_seed: 180,
    };
    runner
        .run(vec![
            ScenarioOp::Mint {
                inscription_id: pass_prev.clone(),
                owner: owner_a,
                height: 100,
                prev: vec![],
            },
            ScenarioOp::Transfer {
                inscription_id: pass_prev.clone(),
                new_owner: owner_b,
                height: 110,
                satpoint: test_satpoint(181, 1, 1),
            },
            ScenarioOp::Mint {
                inscription_id: pass_new_1.clone(),
                owner: owner_b,
                height: 120,
                prev: vec![pass_prev.clone()],
            },
            ScenarioOp::Mint {
                inscription_id: pass_new_2.clone(),
                owner: owner_b,
                height: 130,
                prev: vec![pass_prev.clone()],
            },
        ])
        .await
        .unwrap();

    let prev_dormant = energy_manager
        .get_pass_energy(&pass_prev, 110)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev_dormant.state, MinerPassState::Dormant);
    let expected_prev_dormant = expected_energy_by_formula(
        100,
        200_000,
        0,
        &[AddressBalance {
            block_height: 106,
            balance: 210_000,
            delta: 10_000,
        }],
        110,
    );
    assert_eq!(prev_dormant.energy, expected_prev_dormant);

    let prev_consumed = energy_manager
        .get_pass_energy(&pass_prev, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prev_consumed.state, MinerPassState::Consumed);
    assert_eq!(prev_consumed.energy, 0);

    let first_new_energy = energy_manager
        .get_pass_energy(&pass_new_1, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_new_energy.state, MinerPassState::Active);
    assert_eq!(first_new_energy.energy, prev_dormant.energy);

    let second_new_energy = energy_manager
        .get_pass_energy(&pass_new_2, 130)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_new_energy.state, MinerPassState::Active);
    assert_eq!(second_new_energy.energy, 0);

    let second_new = storage
        .get_pass_by_inscription_id(&pass_new_2)
        .unwrap()
        .unwrap();
    assert_eq!(second_new.owner, owner_b);
    assert_eq!(second_new.state, MinerPassState::Active);

    cleanup_temp_dir(&root_dir);
}
