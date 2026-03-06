use super::super::transfer::{
    InscriptionCreateInfo, InscriptionTransferTracker, TransferTrackSeed,
};
use crate::inscription::InscriptionTransferItem;
use crate::status::StatusManager;
use bitcoincore_rpc::bitcoin::Block;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use usdb_util::{BTCRpcClientRef, USDBScriptHash};

pub(crate) type TransferTrackerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) trait BlockHintProvider: Send + Sync {
    fn load_block_hint(&self, block_height: u32) -> Result<Option<Arc<Block>>, String>;
}

pub(super) struct RpcBlockHintProvider {
    btc_client: BTCRpcClientRef,
}

impl RpcBlockHintProvider {
    pub(super) fn new(btc_client: BTCRpcClientRef) -> Self {
        Self { btc_client }
    }
}

impl BlockHintProvider for RpcBlockHintProvider {
    fn load_block_hint(&self, block_height: u32) -> Result<Option<Arc<Block>>, String> {
        let block = self.btc_client.get_block(block_height)?;
        Ok(Some(Arc::new(block)))
    }
}

pub(crate) trait TransferTrackerApi: Send + Sync {
    fn init<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>>;

    fn calc_create_satpoint<'a>(
        &'a self,
        inscription_id: &'a InscriptionId,
    ) -> TransferTrackerFuture<'a, Result<InscriptionCreateInfo, String>>;

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    ) -> TransferTrackerFuture<'a, Result<(), String>>;

    fn process_block_with_hint<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        extra_tracked_inscriptions: Vec<TransferTrackSeed>,
    ) -> TransferTrackerFuture<'a, Result<Vec<InscriptionTransferItem>, String>>;

    fn commit_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> TransferTrackerFuture<'a, Result<(), String>>;

    fn rollback_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> TransferTrackerFuture<'a, Result<(), String>>;
}

impl TransferTrackerApi for InscriptionTransferTracker {
    fn init<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move { self.init().await })
    }

    fn calc_create_satpoint<'a>(
        &'a self,
        inscription_id: &'a InscriptionId,
    ) -> TransferTrackerFuture<'a, Result<InscriptionCreateInfo, String>> {
        Box::pin(async move { self.calc_create_satpoint(inscription_id).await })
    }

    fn add_new_inscription<'a>(
        &'a self,
        inscription_id: InscriptionId,
        owner: USDBScriptHash,
        satpoint: SatPoint,
    ) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.add_new_inscription(inscription_id, owner, satpoint)
                .await
        })
    }

    fn process_block_with_hint<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
        extra_tracked_inscriptions: Vec<TransferTrackSeed>,
    ) -> TransferTrackerFuture<'a, Result<Vec<InscriptionTransferItem>, String>> {
        Box::pin(async move {
            self.process_block_with_hint(block_height, block_hint, extra_tracked_inscriptions)
                .await
        })
    }

    fn commit_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move { self.commit_staged_block(block_height) })
    }

    fn rollback_staged_block<'a>(
        &'a self,
        block_height: u32,
    ) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move { self.rollback_staged_block(block_height) })
    }
}

pub(crate) trait IndexStatusApi: Send + Sync {
    fn latest_depend_synced_block_height(&self) -> u32;
    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    );
}

impl IndexStatusApi for StatusManager {
    fn latest_depend_synced_block_height(&self) -> u32 {
        self.latest_depend_synced_block_height()
    }

    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        self.update_index_status(current, total, message);
    }
}
