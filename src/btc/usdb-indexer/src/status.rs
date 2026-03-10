use crate::config::ConfigManagerRef;
use crate::output::IndexOutputRef;
use balance_history::{
    RpcClient as BalanceHistoryRpcClient, RpcClientRef as BalanceHistoryClientRef,
    SnapshotInfo as BalanceHistorySnapshotInfo,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use usdb_util::{BTCRpcClient, BTCRpcClientRef};

pub struct USDBInscriptionIndexStatus {
    pub genesis_block_height: u32,
    pub current: u32,
    pub total: u32,
    pub message: Option<String>,
}

impl USDBInscriptionIndexStatus {
    pub fn new(genesis_block_height: u32) -> Self {
        USDBInscriptionIndexStatus {
            genesis_block_height,
            current: 0,
            total: 0,
            message: None,
        }
    }
}

#[derive(Clone)]
pub struct StatusManager {
    btc_client: BTCRpcClientRef,
    balance_history_client: BalanceHistoryClientRef,
    output: IndexOutputRef,

    usdb_status: Arc<Mutex<USDBInscriptionIndexStatus>>,
    latest_balance_history_snapshot: Arc<Mutex<Option<BalanceHistorySnapshotInfo>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSyncStatusSnapshot {
    pub genesis_block_height: u32,
    pub current: u32,
    pub total: u32,
    pub message: Option<String>,
    pub balance_history_stable_height: Option<u32>,
}

impl StatusManager {
    pub fn new(config: ConfigManagerRef, output: IndexOutputRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        let balance_history_client =
            BalanceHistoryRpcClient::new(&config.config().balance_history.rpc_url)?;

        let usdb_status =
            USDBInscriptionIndexStatus::new(config.config().usdb.genesis_block_height);
        let usdb_status = Arc::new(Mutex::new(usdb_status));

        Ok(Self {
            btc_client: Arc::new(btc_client),
            balance_history_client: Arc::new(balance_history_client),
            output,
            latest_balance_history_snapshot: Arc::new(Mutex::new(None)),
            usdb_status,
        })
    }

    pub fn balance_history_stable_height(&self) -> Option<u32> {
        self.latest_balance_history_snapshot
            .lock()
            .unwrap()
            .as_ref()
            .map(|snapshot| snapshot.stable_height)
    }

    pub fn balance_history_snapshot(&self) -> Option<BalanceHistorySnapshotInfo> {
        self.latest_balance_history_snapshot.lock().unwrap().clone()
    }

    pub fn update_index_status(
        &self,
        current: Option<u32>,
        total: Option<u32>,
        message: Option<String>,
    ) {
        let mut status = self.usdb_status.lock().unwrap();

        if let Some(total) = total {
            status.total = total;
            self.output.index_bar().set_length(total as u64);
        }

        if let Some(current) = current {
            status.current = current;
            self.output.index_bar().set_position(current as u64);
        }

        if let Some(msg) = message {
            status.message = Some(msg.clone());
            self.output.index_bar().set_message(msg);
        }
    }

    pub fn get_index_status_snapshot(&self) -> IndexerSyncStatusSnapshot {
        let status = self.usdb_status.lock().unwrap();
        IndexerSyncStatusSnapshot {
            genesis_block_height: status.genesis_block_height,
            current: status.current,
            total: status.total,
            message: status.message.clone(),
            balance_history_stable_height: self.balance_history_stable_height(),
        }
    }

    pub fn run_monitor(&self) {
        tokio::spawn({
            let status_manager = self.clone();
            async move {
                loop {
                    if let Err(e) = status_manager.update_status().await {
                        error!("Failed to update status: {}", e);
                        // status_manager.output.println(&format!("Failed to update status: {}", e));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        });
    }

    async fn update_status(&self) -> Result<(), String> {
        let btc_height = self.btc_client.get_latest_block_height().map_err(|e| {
            let msg = format!("Failed to get BTC block height: {}", e);
            error!("{}", msg);
            self.output.btc_bar().set_message(msg.clone());
            msg
        })?;

        // Update BTC bar
        let btc_bar = self.output.btc_bar();
        let current = btc_bar.length().unwrap_or(0);
        btc_bar.set_length(btc_height as u64);
        btc_bar.set_position(btc_height as u64);
        if current == 0 {
            btc_bar.reset_eta();
        }

        let status = self
            .balance_history_client
            .get_sync_status()
            .await
            .map_err(|e| {
                let msg = format!("Failed to get Balance History sync status: {}", e);
                error!("{}", msg);
                self.output.balance_history_bar().set_message(msg.clone());
                msg
            })?;

        // self.output.println(&format!("Balance History sync status: {:?}", status));
        let balance_history_bar = self.output.balance_history_bar();
        let current = balance_history_bar.length().unwrap_or(0);
        balance_history_bar.set_length(status.total as u64);
        balance_history_bar.set_position(status.current as u64);
        if current == 0 {
            balance_history_bar.reset_eta();
        }

        if let Some(msg) = &status.message {
            balance_history_bar.set_message(msg.clone());
        }

        // Determine the latest synced block height among dependent services
        let balance_history_snapshot = self.balance_history_client.get_snapshot_info().await?;
        {
            let mut current_snapshot = self.latest_balance_history_snapshot.lock().unwrap();
            *current_snapshot = Some(balance_history_snapshot);
        }

        Ok(())
    }
}

pub type StatusManagerRef = Arc<StatusManager>;
