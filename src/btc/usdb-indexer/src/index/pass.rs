use super::content::MinerPassState;
use super::energy::PassEnergyManagerRef;
use crate::config::ConfigManagerRef;
use crate::storage::{MinerPassStorageRef, MinerPassInfo};
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use usdb_util::USDBScriptHash;

pub struct PassMintInscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,

    // The minting transaction info
    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: USDBScriptHash, // The owner address who minted the pass

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
        Ok(Self {
            config,
            energy_manager,
        })
    }

    pub async fn on_mint_pass(
        &self,
        mint_info: &PassMintInscriptionInfo,
    ) -> Result<(), String> {
        // First check if the owner already has an active pass
        self.dormant_last_pass(mint_info).await?;

        // Insert the new pass as active
        let info = MinerPassInfo {
            inscription_id: mint_info.inscription_id.clone(),
            inscription_number: mint_info.inscription_number,
            mint_txid: mint_info.mint_txid.clone(),
            mint_block_height: mint_info.mint_block_height,
            mint_owner: mint_info.mint_owner.clone(),
            
            eth_main: mint_info.eth_main.clone(),
            eth_collab: mint_info.eth_collab.clone(),
            prev: mint_info.prev.clone(),

            state: MinerPassState::Active,
            owner: mint_info.mint_owner.clone(),
        };
        self.storage.add_new_mint_pass(
            &info,
        )?;

        info!(
            "New Miner Pass {} minted at block height {} for owner {}",
            mint_info.inscription_id, mint_info.mint_block_height, mint_info.mint_owner
        );

        // Try get all prev passes and mark them as consumed if they are not already dormant or consumed and on the same owner
        for prev_inscription_id in &mint_info.prev {
            if let Some(prev_pass) = self
                .storage
                .get_pass_by_inscription_id(prev_inscription_id)?
            {
                // First check owner if same
                if prev_pass.owner != mint_info.mint_owner {
                    warn!(
                        "Previous Miner Pass {} owner {} is different from new mint pass {} owner {}, skip consuming",
                        prev_inscription_id, prev_pass.owner, mint_info.inscription_id, mint_info.mint_owner
                    );
                    continue;
                }

                // There must be no active previous pass when minting new pass
                assert!(prev_pass.state != MinerPassState::Active,
                    "Previous Miner Pass {} should not be active when minting new pass {}",
                    prev_inscription_id,  mint_info.inscription_id
                );

                // Only consume if previous pass is in active and dormant state
                if prev_pass.state != MinerPassState::Dormant
                {
                    warn!(
                        "Previous Miner Pass {} is in state {:?}, skip consuming",
                        prev_inscription_id, prev_pass.state
                    );
                    continue;
                }
            } else {
                warn!(
                    "Previous Miner Pass {} not found for new mint pass {}",
                    prev_inscription_id, mint_info.inscription_id
                );
            }
        }

        // Update energy record as well
        self.energy_manager
            .update_pass_energy(&mint_info.inscription_id, mint_info.mint_block_height)
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
            
            // Update the energy record as well for the last time
            self.energy_manager.update_pass_energy(
                &last_pass.inscription_id,
                mint_info.mint_block_height,
            ).await?;
        }

        Ok(())
    }
}
