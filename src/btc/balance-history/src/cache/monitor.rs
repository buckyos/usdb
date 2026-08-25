use super::balance::AddressBalanceCacheRef;
use super::utxo::UTXOCacheRef;
use crate::config::BalanceHistoryConfigRef;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

const MONITOR_INTERVAL_SECS: u64 = 10;
const SHRINK_PERCENT: usize = 10;

#[derive(Clone)]
pub struct MemoryCacheMonitor {
    config: BalanceHistoryConfigRef,

    utxo_cache: UTXOCacheRef,
    address_balance_cache: AddressBalanceCacheRef,
    shutdown_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
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
            shutdown_tx: Arc::new(Mutex::new(None)),
            thread: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts one pressure-monitor thread; repeated calls are idempotent.
    pub fn start(&self) {
        let mut thread = self.thread.lock().unwrap();
        if thread.is_some() {
            debug!("Cache memory monitor is already running");
            return;
        }

        let memory = usdb_util::get_memory_usage_snapshot();
        info!(
            "Starting cache memory monitor: source={}, limit_bytes={}, used_bytes={}, used_percent={}, max_percent={}",
            memory.source,
            memory.limit_bytes,
            memory.used_bytes,
            memory.used_percent(),
            self.config.sync.max_memory_percent
        );
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        self.shutdown_tx.lock().unwrap().replace(shutdown_tx);
        let monitor = self.clone();
        thread.replace(std::thread::spawn(move || {
            loop {
                match shutdown_rx
                    .recv_timeout(std::time::Duration::from_secs(MONITOR_INTERVAL_SECS))
                {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => monitor.check(),
                }
            }
            info!("Cache memory monitor stopped");
        }));
    }

    /// Stops and joins the pressure-monitor thread; repeated calls are idempotent.
    pub fn stop(&self) {
        if let Some(shutdown_tx) = self.shutdown_tx.lock().unwrap().take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(thread) = self.thread.lock().unwrap().take()
            && thread.join().is_err()
        {
            error!("Cache memory monitor thread panicked while stopping");
        }
    }

    // Called when sync is complete to shrink caches
    // This maybe called multiple times
    pub fn on_sync_complete(&self) {
        self.utxo_cache
            .update_strategy(super::CacheStrategy::Normal);
        self.address_balance_cache
            .update_strategy(super::CacheStrategy::Normal);
    }

    fn check(&self) {
        let max_memory_percent = self.config.sync.max_memory_percent;
        let memory = usdb_util::get_memory_usage_snapshot();
        let used_percent = memory.used_percent();

        if memory.limit_bytes == 0 {
            error!("Unable to determine effective memory limit");
            return;
        }
        if used_percent <= max_memory_percent as u64 {
            return;
        }

        warn!(
            "High memory usage detected: source={}, used_bytes={}, limit_bytes={}, used_percent={}, max_percent={}; shrinking caches by {}%",
            memory.source,
            memory.used_bytes,
            memory.limit_bytes,
            used_percent,
            max_memory_percent,
            SHRINK_PERCENT
        );
        self.shrink_caches();
    }

    fn shrink_caches(&self) {
        let target_utxo_count = shrink_target(self.utxo_cache.get_count() as usize);
        self.utxo_cache.shrink(target_utxo_count);

        let target_balance_count = shrink_target(self.address_balance_cache.get_count() as usize);
        self.address_balance_cache.shrink(target_balance_count);
    }
}

fn shrink_target(current_count: usize) -> usize {
    let reduction = (current_count / (100 / SHRINK_PERCENT)).max(1);
    current_count.saturating_sub(reduction).max(1)
}

pub type MemoryCacheMonitorRef = Arc<MemoryCacheMonitor>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{AddressBalanceCache, CacheStrategy, UTXOCache};
    use crate::config::BalanceHistoryConfig;

    #[test]
    fn shrink_target_reduces_large_caches_by_ten_percent() {
        assert_eq!(shrink_target(1_000), 900);
        assert_eq!(shrink_target(101), 91);
    }

    #[test]
    fn shrink_target_never_creates_zero_capacity() {
        assert_eq!(shrink_target(0), 1);
        assert_eq!(shrink_target(1), 1);
        assert_eq!(shrink_target(9), 8);
    }

    #[test]
    fn monitor_start_and_stop_are_idempotent() {
        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = 1024 * 1024;
        config.sync.balance_max_cache_bytes = 1024 * 1024;
        let config = Arc::new(config);
        let utxo_cache = Arc::new(UTXOCache::new(config.clone(), CacheStrategy::Normal));
        let balance_cache = Arc::new(AddressBalanceCache::new(
            config.clone(),
            CacheStrategy::Normal,
        ));
        let monitor = MemoryCacheMonitor::new(config, utxo_cache, balance_cache);

        monitor.start();
        monitor.start();
        assert!(monitor.thread.lock().unwrap().is_some());

        monitor.stop();
        monitor.stop();
        assert!(monitor.thread.lock().unwrap().is_none());
    }
}
