use super::balance::AddressBalanceCacheRef;
use super::utxo::UTXOCacheRef;
use crate::config::BalanceHistoryConfigRef;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

#[derive(Clone)]
pub struct MemoryCacheMonitor {
    config: BalanceHistoryConfigRef,

    utxo_cache: UTXOCacheRef,
    address_balance_cache: AddressBalanceCacheRef,
}

impl MemoryCacheMonitor {
    pub fn new(
        config: BalanceHistoryConfigRef,
        utxo_cache: UTXOCacheRef,
        address_balance_cache: AddressBalanceCacheRef,
    ) -> Self {
        Self {
            config,
            utxo_cache,
            address_balance_cache,
        }
    }

    pub fn start(&self) {
        let monitor = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            monitor.check();
        });
    }

    // Called when sync is complete to shrink caches
    // This maybe called multiple times
    pub fn on_sync_complete(&self) {
        self.utxo_cache.clear();
    }

    fn check(&self) {
        let max_memory_percent = self.config.sync.max_memory_percent;

        let mut info = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
        );
        info.refresh_memory();

        if info.total_memory() == 0 {
            // Unable to get memory info
            error!("Unable to get system memory info");
            return;
        }

        let used_percent = info.used_memory() * 100 / info.total_memory();
        if used_percent <= self.config.sync.max_memory_percent as u64 {
            return;
        }

        // Memory usage is high, need shrink caches
        info!(
            "High memory usage detected: {}% used, max allowed {}%, shrinking caches",
            used_percent, max_memory_percent
        );
        self.shrink_caches();
    }

    fn shrink_caches(&self) {
        // Reduce 1% of UTXO cache each time
        let target_utxo_count = (self.utxo_cache.get_count() as usize * 99) / 100;
        self.utxo_cache.shrink(target_utxo_count);

        // Reduce 1% of Address Balance cache each time
        let target_balance_count = (self.address_balance_cache.get_count() as usize * 99) / 100;
        self.address_balance_cache.shrink(target_balance_count);
    }
}


pub type MemoryCacheMonitorRef = std::sync::Arc<MemoryCacheMonitor>;