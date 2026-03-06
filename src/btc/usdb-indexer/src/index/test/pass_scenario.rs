use super::common::{
    MockBalanceProvider, cleanup_temp_dir, test_inscription_id, test_root_dir, test_satpoint,
    test_script_hash,
};
use crate::config::ConfigManager;
use crate::index::MinerPassState;
use crate::index::energy::{BalanceProvider, PassEnergyManager};
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
        .get_pass_energy_at_or_before(&pass_old, 110)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_dormant_110.state, MinerPassState::Dormant);
    assert!(old_dormant_110.energy > 0);

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
    assert!(new_energy_130.energy > 0);

    cleanup_temp_dir(&root_dir);
}
