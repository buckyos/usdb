use super::super::transfer::{
    InscriptionCreateInfo, InscriptionTransferTracker, TransferTrackSeed,
};
use crate::inscription::InscriptionTransferItem;
use crate::status::StatusManager;
use balance_history::{
    BlockCommitInfo as BalanceHistoryBlockCommitInfo,
    HistoricalSnapshotStateRef as BalanceHistoryHistoricalStateRef,
    RpcClient as BalanceHistoryRpcClient, SnapshotInfo as BalanceHistorySnapshotInfo,
};
use bitcoincore_rpc::bitcoin::Block;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use usdb_util::{BTCRpcClientRef, USDBScriptHash};

pub(crate) type TransferTrackerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(crate) type BalanceHistoryFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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

    fn reload_from_storage<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>>;

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

    fn reload_from_storage<'a>(&'a self) -> TransferTrackerFuture<'a, Result<(), String>> {
        Box::pin(async move { self.reload_from_storage().await })
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

pub(crate) trait BalanceHistoryCommitApi: Send + Sync {
    fn get_block_commit<'a>(
        &'a self,
        block_height: u32,
    ) -> BalanceHistoryFuture<'a, Result<Option<BalanceHistoryBlockCommitInfo>, String>>;

    fn get_state_ref_at_height<'a>(
        &'a self,
        block_height: u32,
    ) -> BalanceHistoryFuture<'a, Result<BalanceHistoryHistoricalStateRef, String>>;
}

impl BalanceHistoryCommitApi for BalanceHistoryRpcClient {
    fn get_block_commit<'a>(
        &'a self,
        block_height: u32,
    ) -> BalanceHistoryFuture<'a, Result<Option<BalanceHistoryBlockCommitInfo>, String>> {
        Box::pin(async move { self.get_block_commit(block_height).await })
    }

    fn get_state_ref_at_height<'a>(
        &'a self,
        block_height: u32,
    ) -> BalanceHistoryFuture<'a, Result<BalanceHistoryHistoricalStateRef, String>> {
        Box::pin(async move { self.get_state_ref_at_height(block_height).await })
    }
}

pub(crate) trait IndexStatusApi: Send + Sync {
    fn balance_history_stable_height(&self) -> Option<u32>;
    fn balance_history_snapshot(&self) -> Option<BalanceHistorySnapshotInfo>;
    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    );
    fn set_upstream_reorg_recovery_pending(&self, pending: bool);
}

impl IndexStatusApi for StatusManager {
    fn balance_history_stable_height(&self) -> Option<u32> {
        self.balance_history_stable_height()
    }

    fn balance_history_snapshot(&self) -> Option<BalanceHistorySnapshotInfo> {
        self.balance_history_snapshot()
    }

    fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        self.update_index_status(current, total, message);
    }

    fn set_upstream_reorg_recovery_pending(&self, pending: bool) {
        self.set_upstream_reorg_recovery_pending(pending);
    }
}
