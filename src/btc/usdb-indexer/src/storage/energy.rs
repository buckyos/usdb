use crate::index::MinerPassState;
use ord::{InscriptionId, templates::inscription};
use usdb_util::USDBScriptHash;

#[derive(Clone, Debug)]
pub struct PassEnergyKey {
    pub inscription_id: InscriptionId,
    pub block_height: u32,
}   

#[derive(Clone, Debug)]
pub struct PassEnergyValue {
    pub state: MinerPassState,
    pub active_block_height: u32,   // The block height when the pass mint or balance decreased(for active passes)
    pub owner_address: String,
    pub owner_balance: u64, // In Satoshi at block height
    pub owner_delta: i64,  // In Satoshi at block height
    pub energy: u64,       // Energy balance associated with the pass at block height
}

#[derive(Clone, Debug)]
pub struct PassEnergyRecord {
    pub inscription_id: InscriptionId,
    pub block_height: u32,

    pub state: MinerPassState,
    pub active_block_height: u32,   // The block height when the pass mint or balance decreased(for active passes)
    pub owner_address: USDBScriptHash,
    pub owner_balance: u64, // in Satoshi at block height
    pub owner_delta: i64, // in Satoshi at block height
    pub energy: u64, // Energy balance associated with the pass at block height
}

pub struct PassEnergyStorage {

}

impl PassEnergyStorage {
    pub fn new(
        
    ) -> Self {
        Self {
            
        }
    }

    pub fn insert_pass_energy_record(
        &self,
        record: &PassEnergyRecord,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn get_pass_energy_record(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyValue>, String> {
        Ok(None)
    }

    pub fn find_last_pass_energy_record(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
    ) -> Result<Option<PassEnergyRecord>, String> {
        Ok(None)
    }
}