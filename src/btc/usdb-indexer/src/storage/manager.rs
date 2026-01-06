use crate::config::ConfigManagerRef;
use super::inscriptions::{InscriptionStorage, InscriptionStorageRef};
use super::transfer::{InscriptionTransferStorageRef, InscriptionTransferStorage};
use std::sync::Arc;

pub struct InscriptionsManager {
    config: ConfigManagerRef,

    inscription_storage: InscriptionStorageRef,
    transfer_storage: InscriptionTransferStorageRef,
}

impl InscriptionsManager {
    pub fn new(
        config: ConfigManagerRef,
    ) -> Result<Self, String> {
        let data_dir = config.data_dir();
        
        let inscription_storage = Arc::new(InscriptionStorage::new(&data_dir)?);
        let transfer_storage = Arc::new(InscriptionTransferStorage::new(&data_dir)?);

        Ok(InscriptionsManager {
            config,
            inscription_storage,
            transfer_storage,
        })
    }

    pub fn inscription_storage(&self) -> &InscriptionStorageRef {
        &self.inscription_storage
    }

    pub fn transfer_storage(&self) -> &InscriptionTransferStorageRef {
        &self.transfer_storage
    }
}

pub type InscriptionsManagerRef = Arc<InscriptionsManager>;