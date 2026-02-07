use crate::btc::{OrdClient, OrdClientRef};
use crate::config::ConfigManagerRef;
use crate::output::IndexOutputRef;
use balance_history::{
    RpcClient as BalanceHistoryRpcClient, RpcClientRef as BalanceHistoryClientRef,
};
use std::sync::atomic::AtomicU32;
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
    ord_client: OrdClientRef,
    balance_history_client: BalanceHistoryClientRef,
    output: IndexOutputRef,

    usdb_status: Arc<Mutex<USDBInscriptionIndexStatus>>,

    // The latest block height that has been synced by all dependent services: BTC, Ordinals, Balance History
    latest_depend_synced_block_height: Arc<AtomicU32>,
}

impl StatusManager {
    pub fn new(config: ConfigManagerRef, output: IndexOutputRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;

        let balance_history_client =
            BalanceHistoryRpcClient::new(&config.config().balance_history.rpc_url)?;

        let usdb_status =
            USDBInscriptionIndexStatus::new(config.config().usdb.genesis_block_height);
        let usdb_status = Arc::new(Mutex::new(usdb_status));

        Ok(Self {
            btc_client: Arc::new(btc_client),
            ord_client: Arc::new(ord_client),
            balance_history_client: Arc::new(balance_history_client),
            output,
            latest_depend_synced_block_height: Arc::new(AtomicU32::new(0)),
            usdb_status,
        })
    }

    pub fn latest_depend_synced_block_height(&self) -> u32 {
        self.latest_depend_synced_block_height
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn update_index_status(&self, current: Option<u32>, total: Option<u32>, message: Option<String>) {
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

        let ord_height = self
            .ord_client
            .get_latest_block_height()
            .await
            .map_err(|e| {
                let msg = format!("Failed to get Ordinals block height: {}", e);
                error!("{}", msg);
                self.output.ord_bar().set_message(msg.clone());
                msg
            })?;

        // Update Ordinals bar
        let ord_bar = self.output.ord_bar();
        let current = ord_bar.length().unwrap_or(0);
        ord_bar.set_length(btc_height as u64);
        ord_bar.set_position(ord_height as u64);
        if current == 0 {
            ord_bar.reset_eta();
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
        let balance_history_height = self.balance_history_client.get_block_height().await? as u32;

        let latest_synced_height = *[btc_height, ord_height, balance_history_height]
            .iter()
            .min()
            .unwrap();
        self.latest_depend_synced_block_height
            .store(latest_synced_height, std::sync::atomic::Ordering::SeqCst);

        Ok(())
    }
}

pub type StatusManagerRef = Arc<StatusManager>;
