use super::balance::AddressBalanceCacheRef;
use super::utxo::UTXOCacheRef;
use crate::config::BalanceHistoryConfigRef;

const MONITOR_INTERVAL_SECS: u64 = 10;
const SHRINK_PERCENT: usize = 10;

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
        let memory = usdb_util::get_memory_usage_snapshot();
        info!(
            "Starting cache memory monitor: source={}, limit_bytes={}, used_bytes={}, used_percent={}, max_percent={}",
            memory.source,
            memory.limit_bytes,
            memory.used_bytes,
            memory.used_percent(),
            self.config.sync.max_memory_percent
        );
        let monitor = self.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(MONITOR_INTERVAL_SECS));
                monitor.check();
            }
        });
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

pub type MemoryCacheMonitorRef = std::sync::Arc<MemoryCacheMonitor>;

#[cfg(test)]
mod tests {
    use super::*;

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
}
