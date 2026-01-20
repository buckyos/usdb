use super::content::MinerPassState;
use crate::config::ConfigManagerRef;
use crate::index::pass;
use crate::storage::{PassEnergyRecord, PassEnergyStorage};
use balance_history::{AddressBalance, RpcClient as BalanceHistoryRpcClient};
use ord::InscriptionId;
use ord::subcommand::index::info;
use usdb_util::USDBScriptHash;

// 0.001 btc threshold = 100_000 Satoshi
const ENERGY_BALANCE_THRESHOLD: u64 = 100_000; // in Satoshi 0.001 BTC

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassEnergyResult {
    pub energy: u64,
    pub state: MinerPassState,
}
pub struct PassEnergyManager {
    config: ConfigManagerRef,
    storage: PassEnergyStorage,
    balance_history_client: BalanceHistoryRpcClient,
}

impl PassEnergyManager {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let storage = PassEnergyStorage::new(config.clone());
        let balance_history_client = BalanceHistoryRpcClient::new(
            &config.config().balance_history.rpc_url,
        )
        .map_err(|e| {
            let msg = format!("Failed to create Balance History RPC client: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            config,
            storage,
            balance_history_client,
        })
    }

    // Get the balance of an address at a specific block height, which may changed on or before that height
    async fn get_balance_at_height(
        &self,
        address: &USDBScriptHash,
        block_height: u32,
    ) -> Result<AddressBalance, String> {
        let mut balances = self
            .balance_history_client
            .get_address_balance(address.clone(), Some(block_height), None)
            .await?;

        assert!(
            balances.len() <= 1,
            "Expected at most one balance entry for address at specific block height {}",
            address
        );
        if let Some(balance) = balances.pop() {
            Ok(balance)
        } else {
            // Should not happen, but return zero balance if not found
            let msg = format!(
                "No balance entry found for address {} at block height {}",
                address, block_height
            );
            warn!("{}", msg);

            Ok(AddressBalance {
                block_height,
                balance: 0,
                delta: 0,
            })
        }
    }

    async fn get_balance_at_range(
        &self,
        address: &USDBScriptHash,
        block_range: std::ops::Range<u32>,
    ) -> Result<Vec<AddressBalance>, String> {
        let balances = self
            .balance_history_client
            .get_address_balance(address.clone(), None, Some(block_range))
            .await?;
        Ok(balances)
    }

    // When a new Miner Pass is created, initialize its energy record with zero energy at the block height
    pub async fn on_new_pass(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(), String> {
        let balance = self
            .get_balance_at_height(owner_address, block_height)
            .await?;

        // Should exactly match the block height when inscription is created
        // Because the utxos must changed at that block height, so we must have balance record at that height
        if balance.block_height != block_height {
            let msg = format!(
                "Balance history not found for owner {} at block height {} when creating new Miner Pass {}",
                owner_address, block_height, inscription_id
            );
            warn!("{}", msg);

            // Should not happen, but continue anyway?
            // return Err(msg);
        }

        let record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,

            state: MinerPassState::Active,
            active_block_height: block_height,
            owner_address: owner_address.clone(),
            owner_balance: balance.balance,
            owner_delta: balance.delta,
            energy: 0,
        };
        self.storage.insert_pass_energy_record(&record)?;

        info!(
            "New Miner Pass {} created at block height {} for owner {}, initial balance: {}, delta: {}",
            inscription_id, block_height, owner_address, balance.balance, balance.delta
        );
        Ok(())
    }

    // Get the energy and state of the pass at given block height
    // The record must exist for the pass at the block height
    pub async fn get_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyResult>, String> {
        let ret = self
            .storage
            .get_pass_energy_record(inscription_id, block_height)?;

        let value = ret.map(|v| PassEnergyResult {
            energy: v.energy,
            state: v.state,
        });

        Ok(value)
    }

    // Kernel function to update the energy of a Miner Pass at given block height
    pub async fn update_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<PassEnergyResult, String> {
        // First, get the last energy record for this pass at or before the given block height
        let mut last_record = self
            .storage
            .find_last_pass_energy_record(inscription_id, block_height)?
            .ok_or_else(|| {
                let msg = format!(
                    "No previous energy record found for inscription {} before block height {}",
                    inscription_id, block_height
                );
                error!("{}", msg);
                msg
            })?;
        assert!(
            last_record.block_height <= block_height,
            "Last record block height should be less than or equal to current block height"
        );

        // Check the pass state
        // If the pass is active, we need to update the energy based on owner's balance delta
        // If dormant or consumed, energy remains the same as last record, and we just record return the same energy
        if last_record.state != MinerPassState::Active {
            return Ok(PassEnergyResult {
                energy: last_record.energy,
                state: last_record.state,
            });
        }

        // For active passes, get the owner's balance records between last_record.block_height and block_height: [last_record.block_height + 1, block_height]
        let range = (last_record.block_height + 1)..(block_height + 1);
        let balances = self
            .get_balance_at_range(&last_record.owner_address, range)
            .await?;

        // Update energy based on balance changes records
        for balance_record in balances {
            // Calculate energy bonus between last_record.block_height and balance_record.block_height base on last_record.owner_balance
            // The R is related to the H, H = current block height - miner certificate's activation block height. The larger the H, the larger the R, but the R has an upper limit.
            let r: u32 = balance_record.block_height - last_record.active_block_height;
            assert!(r >= 1, "R should be at least 1");

            let energy_delta = if last_record.owner_balance >= ENERGY_BALANCE_THRESHOLD {
                last_record.owner_balance * 10000 * r as u64
            } else {
                0u64
            };

            let mut new_energy = last_record.energy.saturating_add(energy_delta);

            // The balance increased, so the active_block_height should not change,
            let active_block_height = if balance_record.delta > 0 {
                last_record.active_block_height
            } else {
                // The balance decreased, so we need to update active_block_height to this block height
                balance_record.block_height
            };

            // If the balance decreased, energy decreases by D * 10000 * 6 * 24 * 30 as punishment
            if balance_record.delta < 0 {
                let energy_delta = balance_record.delta.abs() as u64 * 10000 * 6 * 24 * 30;
                new_energy = new_energy.saturating_sub(energy_delta);
            }

            let new_energy = PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: balance_record.block_height,
                state: MinerPassState::Active,
                active_block_height,
                owner_address: last_record.owner_address.clone(),
                owner_balance: balance_record.balance,
                owner_delta: balance_record.delta,
                energy: new_energy,
            };
            self.storage.insert_pass_energy_record(&new_energy)?;
            last_record = new_energy;
        }

        let ret = if last_record.block_height < block_height {
            // No balance changes in between, just calculate energy up to block_height
            // This record should not save to storage, as there is no balance change record at this height
            let energy_delta = if last_record.owner_balance >= ENERGY_BALANCE_THRESHOLD {
                let r = block_height - last_record.active_block_height;
                last_record.owner_balance * 10000 * r as u64
            } else {
                0u64
            };
            let new_energy = last_record.energy.saturating_add(energy_delta);

            PassEnergyResult {
                energy: new_energy,
                state: last_record.state,
            }
        } else {
            PassEnergyResult {
                energy: last_record.energy,
                state: last_record.state,
            }
        };

        Ok(ret)
    }

    // Last update the pass energy when marking dormant
    pub async fn on_pass_dormant(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<(), String> {
        // Update the energy record as well for the last time
        let ret = self
            .update_pass_energy(&inscription_id, block_height)
            .await?;

        // Just for debugging, verify the last pass energy
        let last_pass_energy = self.get_pass_energy(&inscription_id, block_height).await?;

        assert_eq!(
            last_pass_energy.as_ref(),
            Some(&ret),
            "Last pass energy should match after update {} at {}",
            inscription_id,
            block_height
        );

        info!(
            "Miner Pass {} marked as Dormant at block height {}, final energy: {}",
            inscription_id,
            block_height,
            ret.energy
        );

        Ok(())
    }

    // Clear energy to zero on consumed
    pub fn on_pass_consumed(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(), String> {
        // Insert a new record with zero energy and state consumed
        let record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,
            state: MinerPassState::Consumed,
            active_block_height: block_height,
            owner_address: owner_address.clone(),
            owner_balance: 0,
            owner_delta: 0,
            energy: 0,
        };
        self.storage.insert_pass_energy_record(&record)?;

        Ok(())
    }
}

pub type PassEnergyManagerRef = std::sync::Arc<PassEnergyManager>;
