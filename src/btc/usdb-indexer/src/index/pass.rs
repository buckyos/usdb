use super::content::MinerPassState;
use super::energy::PassEnergyManagerRef;
use crate::config::ConfigManagerRef;
use crate::storage::{MinerPassInfo, MinerPassStorageRef};
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

pub struct PassMintInscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,

    // The minting transaction info
    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: USDBScriptHash, // The owner address who minted the pass

    pub satpoint: SatPoint,

    // The inscription content
    pub eth_main: String,
    pub eth_collab: Option<String>,
    pub prev: Vec<InscriptionId>,
}

pub struct InvalidPassMintInscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: USDBScriptHash,
    pub satpoint: SatPoint,
    pub error_code: String,
    pub error_reason: String,
}

pub struct MinerPassManager {
    config: ConfigManagerRef,
    storage: MinerPassStorageRef,
    energy_manager: PassEnergyManagerRef,
}

impl MinerPassManager {
    pub fn new(
        config: ConfigManagerRef,
        miner_pass_storage: MinerPassStorageRef,
        energy_manager: PassEnergyManagerRef,
    ) -> Result<Self, String> {
        Ok(Self {
            config,
            storage: miner_pass_storage,
            energy_manager,
        })
    }

    pub fn miner_pass_storage(&self) -> &MinerPassStorageRef {
        &self.storage
    }

    pub async fn on_mint_pass(&self, mint_info: &PassMintInscriptionInfo) -> Result<(), String> {
        // First check if the owner already has an active pass
        self.dormant_last_pass(mint_info).await?;

        // Insert the new pass as active
        let info = MinerPassInfo {
            inscription_id: mint_info.inscription_id.clone(),
            inscription_number: mint_info.inscription_number,
            mint_txid: mint_info.mint_txid.clone(),
            mint_block_height: mint_info.mint_block_height,
            mint_owner: mint_info.mint_owner.clone(),

            satpoint: mint_info.satpoint.clone(),

            eth_main: mint_info.eth_main.clone(),
            eth_collab: mint_info.eth_collab.clone(),
            prev: mint_info.prev.clone(),
            invalid_code: None,
            invalid_reason: None,

            state: MinerPassState::Active,
            owner: mint_info.mint_owner.clone(),
        };
        // Persist current snapshot and append pass history event at mint height.
        self.storage
            .add_new_mint_pass_at_height(&info, mint_info.mint_block_height)?;

        info!(
            "New Miner Pass {} minted at block height {} for owner {}",
            mint_info.inscription_id, mint_info.mint_block_height, mint_info.mint_owner
        );

        // Try get all prev passes and mark them as consumed if they are not already dormant or consumed and on the same owner
        let mut inherited_energy = 0u64;
        for prev_inscription_id in &mint_info.prev {
            if let Some(prev_pass) = self
                .storage
                .get_pass_by_inscription_id(prev_inscription_id)?
            {
                // First check owner if same
                if prev_pass.owner != mint_info.mint_owner {
                    warn!(
                        "Previous Miner Pass {} owner {} is different from new mint pass {} owner {}, skip consuming",
                        prev_inscription_id,
                        prev_pass.owner,
                        mint_info.inscription_id,
                        mint_info.mint_owner
                    );
                    continue;
                }

                // There must be no active previous pass when minting new pass
                assert!(
                    prev_pass.state != MinerPassState::Active,
                    "Previous Miner Pass {} should not be active when minting new pass {}",
                    prev_inscription_id,
                    mint_info.inscription_id
                );

                // Only consume if previous pass is in active and dormant state
                if prev_pass.state != MinerPassState::Dormant {
                    warn!(
                        "Previous Miner Pass {} is in state {:?}, skip consuming",
                        prev_inscription_id, prev_pass.state
                    );
                    continue;
                }

                // Consume the previous pass and inherit energy
                let energy = self
                    .consume_pass(prev_inscription_id, mint_info.mint_block_height)
                    .await?;
                inherited_energy = inherited_energy.saturating_add(energy);
            } else {
                warn!(
                    "Previous Miner Pass {} not found for new mint pass {}",
                    prev_inscription_id, mint_info.inscription_id
                );
            }
        }

        // Update energy record for the new pass with inherited energy
        self.energy_manager
            .on_new_pass(
                &mint_info.inscription_id,
                &mint_info.mint_owner,
                mint_info.mint_block_height,
                inherited_energy,
            )
            .await?;

        Ok(())
    }

    pub async fn on_invalid_mint_pass(
        &self,
        invalid_info: &InvalidPassMintInscriptionInfo,
    ) -> Result<(), String> {
        let info = MinerPassInfo {
            inscription_id: invalid_info.inscription_id.clone(),
            inscription_number: invalid_info.inscription_number,
            mint_txid: invalid_info.mint_txid,
            mint_block_height: invalid_info.mint_block_height,
            mint_owner: invalid_info.mint_owner,
            satpoint: invalid_info.satpoint,
            eth_main: "".to_string(),
            eth_collab: None,
            prev: Vec::new(),
            invalid_code: Some(invalid_info.error_code.clone()),
            invalid_reason: Some(invalid_info.error_reason.clone()),
            owner: invalid_info.mint_owner,
            state: MinerPassState::Invalid,
        };
        // Invalid mint should also be visible in history timeline at inscription height.
        self.storage
            .add_invalid_mint_pass_at_height(&info, invalid_info.mint_block_height)?;

        warn!(
            "Invalid mint inscription recorded: module=pass_manager, inscription_id={}, block_height={}, owner={}, error_code={}, error_reason={}",
            invalid_info.inscription_id,
            invalid_info.mint_block_height,
            invalid_info.mint_owner,
            invalid_info.error_code,
            invalid_info.error_reason
        );
        Ok(())
    }

    // Dormant the last active pass for the same owner if exists, and update energy record for the dormant pass.
    // This is called when minting a new pass, to ensure there is only one active pass for each owner at any time.
    // The new minted pass will be active, and the existing active pass will be marked as dormant.
    // The dormant pass can still be consumed later to inherit energy to the new minted pass, but it cannot be consumed together with the new minted pass
    async fn dormant_last_pass(&self, mint_info: &PassMintInscriptionInfo) -> Result<(), String> {
        // First check the pass already exists on the same address
        let existing_pass = self
            .storage
            .get_last_active_mint_pass_by_owner(&mint_info.mint_owner)?;
        if let Some(last_pass) = existing_pass {
            assert!(
                last_pass.state == MinerPassState::Active,
                "Existing pass should be active {}",
                last_pass.inscription_id
            );
            assert!(
                last_pass.mint_block_height <= mint_info.mint_block_height,
                "Existing pass mint block height {} should be less than or equal to new mint pass block height {}",
                last_pass.mint_block_height,
                mint_info.mint_block_height
            );

            warn!(
                "Owner {} already has an active Miner Pass {} at block height {}, new mint pass {} at block height {} will be dormant",
                mint_info.mint_owner,
                last_pass.inscription_id,
                last_pass.mint_block_height,
                mint_info.inscription_id,
                mint_info.mint_block_height
            );

            // Mark the last pass as dormant
            // Use height-aware state transition so historical active-set reconstruction stays deterministic.
            self.storage.update_state_at_height(
                &last_pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                mint_info.mint_block_height,
            )?;

            info!(
                "Last Pass {} marked as Dormant due to new pass {} for owner {}",
                last_pass.inscription_id, mint_info.inscription_id, mint_info.mint_owner
            );

            // Update energy record for the pass
            self.energy_manager
                .on_pass_dormant(&last_pass.inscription_id, mint_info.mint_block_height)
                .await?;
        }

        Ok(())
    }

    // Consume the pass at the given block height, return the last energy balance
    async fn consume_pass(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<u64, String> {
        // Mark the pass as consumed
        let pass = self
            .storage
            .get_pass_by_inscription_id(inscription_id)?
            .ok_or_else(|| {
                let msg = format!(
                    "Miner Pass {} not found for consuming at block height {}",
                    inscription_id, block_height
                );
                error!("{}", msg);
                msg
            })?;

        // The pass must be dormant before consuming
        assert_eq!(
            pass.state,
            MinerPassState::Dormant,
            "Miner Pass {} must be dormant before consuming, but state is {:?}",
            inscription_id,
            pass.state
        );

        self.storage.update_state_at_height(
            inscription_id,
            MinerPassState::Consumed,
            pass.state,
            block_height,
        )?;

        // Get the latest energy record at or before block_height.
        // The pass may become dormant at an earlier height, so exact-height lookup is not reliable.
        let ret = self
            .energy_manager
            .get_pass_energy_at_or_before(inscription_id, block_height)
            .await?;
        if ret.is_none() {
            let msg = format!(
                "Miner Pass {} energy record not found at or before block height {} for consuming",
                inscription_id, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }
        let energy = ret.unwrap();
        assert_eq!(
            energy.state,
            MinerPassState::Dormant,
            "Miner Pass {} energy state must be Dormant before consuming, but state is {:?}",
            inscription_id,
            energy.state
        );

        self.energy_manager
            .on_pass_consumed(inscription_id, &pass.owner, block_height)?;

        info!(
            "Miner Pass {} consumed at block height {}, energy balance {}",
            inscription_id, block_height, energy.energy
        );

        Ok(energy.energy)
    }

    pub async fn on_pass_transfer(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &USDBScriptHash,
        satpoint: &SatPoint,
        block_height: u32,
    ) -> Result<(), String> {
        info!(
            "Miner Pass {} transferred to new owner {} at block height {}, new satpoint {}",
            inscription_id, new_owner, block_height, satpoint
        );

        // First lookup the pass by inscription id
        let pass = self.storage.get_pass_by_inscription_id(inscription_id)?;
        let pass = pass.ok_or_else(|| {
            let msg = format!(
                "Miner Pass {} not found for transfer at block height {}",
                inscription_id, block_height
            );
            error!("{}", msg);
            msg
        })?;

        // Update energy record for the pass before transfer if the pass is active
        if pass.state == MinerPassState::Active {
            self.energy_manager
                .update_pass_energy(inscription_id, block_height)
                .await?;
        }

        if pass.owner == *new_owner {
            warn!(
                "Miner Pass {} transferred to the same owner {}, skip updating owner",
                inscription_id, new_owner
            );
            self.storage.update_satpoint_at_height(
                inscription_id,
                &pass.satpoint,
                satpoint,
                block_height,
            )?;
        } else {
            if pass.state == MinerPassState::Active {
                // Freeze active energy at transfer height and mark pass state as Dormant first.
                self.energy_manager
                    .on_pass_dormant(inscription_id, block_height)
                    .await?;
                self.storage.update_state_at_height(
                    inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    block_height,
                )?;
            }

            // Transfer the ownership in storage
            self.storage.transfer_owner_at_height(
                inscription_id,
                new_owner,
                satpoint,
                block_height,
            )?;
        }

        Ok(())
    }

    pub async fn on_pass_burned(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<(), String> {
        info!(
            "Miner Pass {} burned at block height {}",
            inscription_id, block_height
        );

        // First lookup the pass by inscription id
        let pass = self.storage.get_pass_by_inscription_id(inscription_id)?;
        let pass = pass.ok_or_else(|| {
            let msg = format!(
                "Miner Pass {} not found for burning at block height {}",
                inscription_id, block_height
            );
            error!("{}", msg);
            msg
        })?;

        // Update energy record for the pass before burning if the pass is active
        if pass.state == MinerPassState::Active {
            self.energy_manager
                .update_pass_energy(inscription_id, block_height)
                .await?;
        }

        // Update the pass state to burned
        self.storage.update_state_at_height(
            inscription_id,
            MinerPassState::Burned,
            pass.state,
            block_height,
        )?;

        Ok(())
    }
}

pub type MinerPassManagerRef = Arc<MinerPassManager>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigManager;
    use crate::index::energy::PassEnergyManager;
    use crate::storage::MinerPassStorage;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    fn test_root_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("usdb_indexer_pass_test_{}_{}", test_name, nanos))
    }

    fn test_script_hash(tag: u8) -> USDBScriptHash {
        let script = ScriptBuf::from(vec![tag; 32]);
        script.to_usdb_script_hash()
    }

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        let txid = Txid::from_slice(&[tag; 32]).unwrap();
        InscriptionId { txid, index }
    }

    fn test_satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
        SatPoint {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout,
            },
            offset,
        }
    }

    fn setup_manager(
        test_name: &str,
    ) -> (
        PathBuf,
        MinerPassStorageRef,
        MinerPassManager,
        InscriptionId,
        USDBScriptHash,
        SatPoint,
    ) {
        let root_dir = test_root_dir(test_name);
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
        let energy_manager = Arc::new(PassEnergyManager::new(config.clone()).unwrap());
        let manager = MinerPassManager::new(config, storage.clone(), energy_manager).unwrap();

        let inscription_id = test_inscription_id(1, 0);
        let owner = test_script_hash(7);
        let satpoint = test_satpoint(2, 0, 0);

        let pass = MinerPassInfo {
            inscription_id: inscription_id.clone(),
            inscription_number: 1,
            mint_txid: Txid::from_slice(&[3; 32]).unwrap(),
            mint_block_height: 100,
            mint_owner: owner,
            satpoint: satpoint.clone(),
            eth_main: "0x1111111111111111111111111111111111111111".to_string(),
            eth_collab: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state: MinerPassState::Active,
        };
        storage.add_new_mint_pass(&pass).unwrap();
        storage
            .update_state(
                &inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
            )
            .unwrap();

        (root_dir, storage, manager, inscription_id, owner, satpoint)
    }

    #[tokio::test]
    async fn test_on_pass_transfer_same_owner_updates_satpoint() {
        let (root_dir, storage, manager, inscription_id, owner, old_satpoint) =
            setup_manager("transfer_same_owner");

        let new_satpoint = test_satpoint(9, 1, 42);
        manager
            .on_pass_transfer(&inscription_id, &owner, &new_satpoint, 101)
            .await
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.owner, owner);
        assert_eq!(updated.state, MinerPassState::Dormant);
        assert_eq!(updated.satpoint, new_satpoint);
        assert_ne!(updated.satpoint, old_satpoint);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_burned_from_dormant_updates_state() {
        let (root_dir, storage, manager, inscription_id, _owner, _satpoint) =
            setup_manager("burn_dormant");

        manager.on_pass_burned(&inscription_id, 101).await.unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Burned);

        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
