use super::content::MinerPassState;
use super::energy::PassEnergyManagerRef;
use crate::config::ConfigManagerRef;
use crate::storage::{MinerPassInfo, MinerPassStorage, MinerPassStorageRef};
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use std::sync::Arc;
use usdb_util::USDBScriptHash;
use ordinals::SatPoint;

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

pub struct MinerPassManager {
    config: ConfigManagerRef,
    storage: MinerPassStorageRef,
    energy_manager: PassEnergyManagerRef,
}

impl MinerPassManager {
    pub fn new(
        config: ConfigManagerRef,
        energy_manager: PassEnergyManagerRef,
    ) -> Result<Self, String> {
        let storage = MinerPassStorage::new(&config.data_dir())?;

        Ok(Self {
            config,
            storage: Arc::new(storage),
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

            state: MinerPassState::Active,
            owner: mint_info.mint_owner.clone(),
        };
        self.storage.add_new_mint_pass(&info)?;

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
            self.storage.update_state(
                &last_pass.inscription_id,
                MinerPassState::Active,
                MinerPassState::Dormant,
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

        self.storage
            .update_state(inscription_id, pass.state, MinerPassState::Consumed)?;

        // Get the energy record at this block height
        // The energy record must exist at this block height, which is updated when the pass is marked as dormant
        let ret = self
            .energy_manager
            .get_pass_energy(inscription_id, block_height)
            .await?;
        if ret.is_none() {
            let msg = format!(
                "Miner Pass {} energy record not found for consuming at block height {}",
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
            self.storage.update_satpoint(inscription_id, &pass.satpoint, &pass.satpoint)?;
        } else {
            // Transfer the ownership in storage
            self.storage.transfer_owner(inscription_id, new_owner, satpoint)?;
            if pass.state == MinerPassState::Active {
                self.storage.update_state(inscription_id, MinerPassState::Dormant, MinerPassState::Active)?;
            }
        }
        
        Ok(())
    }
}

pub type MinerPassManagerRef = Arc<MinerPassManager>;
