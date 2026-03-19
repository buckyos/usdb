use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SyncPhase {
    Initializing = 0,
    Loading = 1,
    Indexing = 2,
    Synced = 3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatus {
    pub phase: SyncPhase,
    pub current: u64,
    pub total: u64,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeReadinessStatus {
    /// True once the RPC server has finished binding/listening.
    ///
    /// This is pure liveness and must not be interpreted as "consensus ready".
    pub rpc_alive: bool,
    /// True while a local rollback or rollback-resume flow is actively mutating durable state.
    ///
    /// During this window the service must report itself as not query-ready for
    /// strict downstream consumers even if the RPC server is still reachable.
    pub rollback_in_progress: bool,
    /// True after shutdown has been requested but before the process has fully exited.
    ///
    /// This lets readiness drop immediately when the node enters drain/teardown,
    /// instead of waiting for the RPC listener to disappear.
    pub shutdown_requested: bool,
}

pub struct SyncStatusManager {
    status: Mutex<SyncStatus>,
    runtime_readiness: Mutex<RuntimeReadinessStatus>,
}

impl SyncStatusManager {
    pub fn new() -> Self {
        let status = SyncStatus {
            phase: SyncPhase::Initializing,
            current: 0,
            total: 0,
            message: None,
        };

        Self {
            status: Mutex::new(status),
            runtime_readiness: Mutex::new(RuntimeReadinessStatus::default()),
        }
    }

    pub fn update_phase(&self, phase: SyncPhase, msg: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.phase = phase;
        if msg.is_some() {
            status.message = msg;
        }
    }

    pub fn update_total(&self, total: u64, msg: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.total = total;
        if msg.is_some() {
            status.message = msg;
        }
    }

    pub fn update_current(&self, current: u64, msg: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.current = current;
        if msg.is_some() {
            status.message = msg;
        }
    }

    pub fn update_message(&self, message: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.message = message;
    }

    pub fn update_status(&self, current: u64, total: u64, message: Option<String>) {
        let mut status = self.status.lock().unwrap();
        status.current = current;
        status.total = total;
        status.message = message;
    }

    pub fn get_status(&self) -> SyncStatus {
        let status = self.status.lock().unwrap();
        status.clone()
    }

    pub fn set_rpc_alive(&self, rpc_alive: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        runtime.rpc_alive = rpc_alive;
    }

    pub fn set_rollback_in_progress(&self, rollback_in_progress: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        runtime.rollback_in_progress = rollback_in_progress;
    }

    pub fn set_shutdown_requested(&self, shutdown_requested: bool) {
        let mut runtime = self.runtime_readiness.lock().unwrap();
        runtime.shutdown_requested = shutdown_requested;
    }

    pub fn get_runtime_readiness(&self) -> RuntimeReadinessStatus {
        let runtime = self.runtime_readiness.lock().unwrap();
        runtime.clone()
    }
}

pub type SyncStatusManagerRef = std::sync::Arc<SyncStatusManager>;
