use crate::config::ConfigManagerRef;
use crate::output::IndexOutputRef;
use balance_history::{
    ReadinessInfo as BalanceHistoryReadinessInfo, RpcClient as BalanceHistoryRpcClient,
    RpcClientRef as BalanceHistoryClientRef, SnapshotInfo as BalanceHistorySnapshotInfo,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use usdb_util::{BTCRpcClient, BTCRpcClientRef, ConsecutiveFailureTracker};

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

#[derive(Debug, Clone, Default)]
pub struct RuntimeReadinessStatus {
    /// True once the usdb-indexer RPC server has bound its listener.
    ///
    /// This is plain liveness and must not be interpreted as "consensus ready".
    pub rpc_alive: bool,
    /// True while the node is inside upstream reorg recovery or has detected a
    /// resumable recovery window that should block strict consumers.
    ///
    /// This flag is a live in-memory signal; the server also checks the durable
    /// pending marker in SQLite so restart paths still report not-ready.
    pub upstream_reorg_recovery_pending: bool,
    /// True after shutdown has been requested but before the process has fully exited.
    ///
    /// This lets readiness drop immediately during drain/teardown instead of
    /// waiting for the RPC listener to disappear.
    pub shutdown_requested: bool,
    /// A block is being processed or its last attempt failed; cleared only after durable commit.
    pub block_processing_pending_height: Option<u32>,
}

#[derive(Clone)]
pub struct StatusManager {
    btc_client: BTCRpcClientRef,
    balance_history_client: BalanceHistoryClientRef,
    output: IndexOutputRef,
    poll_interval: std::time::Duration,
    upstream_changed: tokio::sync::watch::Sender<u64>,
    shutdown: tokio::sync::watch::Sender<bool>,

    usdb_status: Arc<Mutex<USDBInscriptionIndexStatus>>,
    latest_balance_history_snapshot: Arc<Mutex<Option<BalanceHistorySnapshotInfo>>>,
    latest_balance_history_readiness: Arc<Mutex<Option<BalanceHistoryReadinessInfo>>>,
    runtime_readiness: Arc<Mutex<RuntimeReadinessStatus>>,
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
            poll_interval: std::time::Duration::from_millis(
                config.config().usdb.upstream_poll_interval_ms.min(1_000),
            ),
            upstream_changed: tokio::sync::watch::channel(0).0,
            shutdown: tokio::sync::watch::channel(false).0,
            latest_balance_history_snapshot: Arc::new(Mutex::new(None)),
            latest_balance_history_readiness: Arc::new(Mutex::new(None)),
            runtime_readiness: Arc::new(Mutex::new(RuntimeReadinessStatus::default())),
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

    pub fn set_balance_history_snapshot(&self, snapshot: Option<BalanceHistorySnapshotInfo>) {
        let mut current = self.latest_balance_history_snapshot.lock().unwrap();
        let changed = *current != snapshot;
        *current = snapshot;
        drop(current);
        if changed {
            self.notify_upstream();
        }
    }

    pub fn balance_history_readiness(&self) -> Option<BalanceHistoryReadinessInfo> {
        self.latest_balance_history_readiness
            .lock()
            .unwrap()
            .clone()
    }

    pub fn set_balance_history_readiness(&self, readiness: Option<BalanceHistoryReadinessInfo>) {
        let mut current = self.latest_balance_history_readiness.lock().unwrap();
        let key = |value: &BalanceHistoryReadinessInfo| {
            (
                value.rpc_alive,
                value.query_ready,
                value.consensus_ready,
                value.stable_height,
                value.stable_block_hash.clone(),
                value.latest_block_commit.clone(),
            )
        };
        let changed = current.as_ref().map(key) != readiness.as_ref().map(key);
        *current = readiness;
        drop(current);
        if changed {
            self.notify_upstream();
        }
    }

    /// Subscribe before reading cached state so a concurrent update cannot be missed.
    pub(crate) fn subscribe_upstream(&self) -> tokio::sync::watch::Receiver<u64> {
        self.upstream_changed.subscribe()
    }

    fn notify_upstream(&self) {
        self.upstream_changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    pub fn set_rpc_alive(&self, rpc_alive: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        if runtime.rpc_alive != rpc_alive {
            let msg = format!("RPC alive: {}", rpc_alive);
            info!("{}", msg);
            self.output.println(&msg);
        }
        runtime.rpc_alive = rpc_alive;
    }

    pub fn set_upstream_reorg_recovery_pending(&self, pending: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        if runtime.upstream_reorg_recovery_pending != pending {
            let msg = format!("Upstream reorg recovery pending: {}", pending);
            info!("{}", msg);
            self.output.println(&msg);
        }
        runtime.upstream_reorg_recovery_pending = pending;
    }

    /// Keep consensus readiness closed through data failures and retries, until the block commits.
    pub fn set_block_processing_pending_height(&self, height: Option<u32>) {
        self.runtime_readiness
            .lock()
            .unwrap()
            .block_processing_pending_height = height;
    }

    pub fn set_shutdown_requested(&self, shutdown_requested: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        if runtime.shutdown_requested != shutdown_requested {
            let msg = format!("Shutdown requested: {}", shutdown_requested);
            info!("{}", msg);
            self.output.println(&msg);
        }
        runtime.shutdown_requested = shutdown_requested;
        drop(runtime);
        self.shutdown.send_replace(shutdown_requested);
    }

    pub fn get_runtime_readiness(&self) -> RuntimeReadinessStatus {
        self.runtime_readiness.lock().unwrap().clone()
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

    /// Observe Core and BH independently: slow Core diagnostics cannot stall BH freshness.
    pub fn run_monitor(&self) {
        for bitcoin in [false, true] {
            let manager = self.clone();
            tokio::spawn(async move { manager.monitor_loop(bitcoin).await });
        }
    }

    async fn monitor_loop(&self, bitcoin: bool) {
        let mut shutdown = self.shutdown.subscribe();
        let mut failures = ConsecutiveFailureTracker::new(60);
        let source = if bitcoin {
            "bitcoin"
        } else {
            "balance-history"
        };
        while !*shutdown.borrow_and_update() {
            let observation = async {
                if bitcoin {
                    self.update_bitcoin_status().await
                } else {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        self.update_balance_history_status(),
                    )
                    .await
                    .map_err(|_| {
                        "Balance-history observation timed out after 10 seconds".to_string()
                    })?
                }
            };
            let result = tokio::select! {
                _ = shutdown.changed() => break,
                result = observation => result,
            };
            let delay = match result {
                Ok(()) => {
                    if let Some(recovery) = failures.record_success() {
                        info!(
                            "USDB status monitor recovered: source={source}, failed_attempts={}, outage_elapsed_ms={}",
                            recovery.failed_attempts,
                            recovery.elapsed.as_millis()
                        );
                    }
                    if bitcoin {
                        std::time::Duration::from_secs(1)
                    } else {
                        self.poll_interval
                    }
                }
                Err(error) => {
                    // Only a failed BH observation invalidates cached BH readiness. Core's
                    // optional progress query must neither freeze BH nor erase its fresh result.
                    if !bitcoin {
                        self.set_balance_history_snapshot(None);
                        self.set_balance_history_readiness(None);
                    }
                    let failure = failures.record_failure();
                    let delay = usdb_util::service_retry_delay(failure.attempt);
                    if failure.should_log {
                        warn!(
                            "USDB status monitor failed: source={source}, attempt={}, outage_elapsed_ms={}, retry_delay_ms={}, error={error}",
                            failure.attempt,
                            failure.elapsed.as_millis(),
                            delay.as_millis()
                        );
                    }
                    delay
                }
            };
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }

    async fn update_bitcoin_status(&self) -> Result<(), String> {
        // The Core client is synchronous. Keep it off the async executor, with at most
        // one outstanding diagnostic request, even during long transport timeouts.
        let client = self.btc_client.clone();
        let btc_height = tokio::task::spawn_blocking(move || client.poll_block_height())
            .await
            .map_err(|error| format!("Bitcoin status worker failed: {error}"))?
            .inspect_err(|error| self.output.btc_bar().set_message(error.clone()))?;

        // Update BTC bar
        let btc_bar = self.output.btc_bar();
        let current = btc_bar.length().unwrap_or(0);
        btc_bar.set_length(btc_height as u64);
        btc_bar.set_position(btc_height as u64);
        if current == 0 {
            btc_bar.reset_eta();
        }

        Ok(())
    }

    async fn update_balance_history_status(&self) -> Result<(), String> {
        let status = self
            .balance_history_client
            .get_sync_status()
            .await
            .map_err(|e| {
                let msg = format!("Failed to get Balance History sync status: {}", e);
                self.output.balance_history_bar().set_message(msg.clone());
                self.set_balance_history_snapshot(None);
                self.set_balance_history_readiness(None);
                msg
            })?;

        // self.output.println(&format!("Balance History sync status: {:?}", status));
        let balance_history_bar = self.output.balance_history_bar();
        let current = balance_history_bar.length().unwrap_or(0);
        balance_history_bar.set_length(status.total);
        balance_history_bar.set_position(status.current);
        if current == 0 {
            balance_history_bar.reset_eta();
        }

        if let Some(msg) = &status.message {
            balance_history_bar.set_message(msg.clone());
        }

        // Refresh the latest upstream stable snapshot from balance-history.
        let balance_history_snapshot = self
            .balance_history_client
            .get_snapshot_info()
            .await
            .map_err(|e| {
                let msg = format!("Failed to get Balance History snapshot info: {}", e);
                self.set_balance_history_snapshot(None);
                self.set_balance_history_readiness(None);
                msg
            })?;

        let balance_history_readiness =
            self.balance_history_client
                .get_readiness()
                .await
                .map_err(|e| {
                    let msg = format!("Failed to get Balance History readiness: {}", e);
                    self.set_balance_history_readiness(None);
                    msg
                })?;
        self.set_balance_history_readiness(Some(balance_history_readiness));
        self.set_balance_history_snapshot(Some(balance_history_snapshot));

        Ok(())
    }
}

pub type StatusManagerRef = Arc<StatusManager>;

#[cfg(test)]
#[path = "../../../../tests/indexer_upstream_observation.rs"]
mod tests;
