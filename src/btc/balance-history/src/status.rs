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

pub struct SyncStatusManager {
    status: Mutex<SyncStatus>,
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
}

pub type SyncStatusManagerRef = std::sync::Arc<SyncStatusManager>;
