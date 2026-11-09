use super::balance::AddressBalanceCacheRef;
use super::utxo::UTXOCacheRef;
use crate::config::BalanceHistoryConfigRef;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MONITOR_INTERVAL_SECS: u64 = 10;
const SHRINK_PERCENT: usize = 10;
const SHRINK_COOLDOWN: Duration = Duration::from_secs(60);

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
            let mut pressure = CachePressure::default();
            loop {
                match shutdown_rx
                    .recv_timeout(std::time::Duration::from_secs(MONITOR_INTERVAL_SECS))
                {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => monitor.check(&mut pressure),
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

    fn check(&self, pressure: &mut CachePressure) {
        self.check_memory(
            usdb_util::get_memory_usage_snapshot(),
            pressure,
            Instant::now(),
        );
    }

    // Separate sampling from decisions so pressure behavior can be tested without
    // allocating gigabytes or depending on the test host's current memory load.
    fn check_memory(
        &self,
        memory: usdb_util::MemoryUsageSnapshot,
        pressure: &mut CachePressure,
        now: Instant,
    ) {
        let max_memory_percent = self.config.sync.max_memory_percent;
        let utxos = self.utxo_cache.get_count() as usize;
        let balances = self.address_balance_cache.get_count() as usize;
        if !pressure.should_shrink(
            memory.limit_bytes > 0 && memory.pressure_percent() > max_memory_percent as u64,
            utxos > 1 || balances > 1,
            now,
        ) {
            return;
        }

        warn!(
            "High memory usage detected: source={}, used_bytes={}, reclaimable_file_bytes={}, pressure_bytes={}, limit_bytes={}, pressure_percent={}, max_percent={}; shrinking caches by {}%",
            memory.source,
            memory.used_bytes,
            memory.reclaimable_file_bytes,
            memory.pressure_bytes(),
            memory.limit_bytes,
            memory.pressure_percent(),
            max_memory_percent,
            SHRINK_PERCENT
        );
        // Empty/minimum caches cannot release useful memory. Do not resize them
        // to one entry merely because another allocation is under pressure.
        if utxos > 1 {
            self.utxo_cache.shrink(shrink_target(utxos));
        }
        if balances > 1 {
            self.address_balance_cache.shrink(shrink_target(balances));
        }
    }
}

/// Require sustained pressure and allow reclaim to settle between evictions.
#[derive(Default)]
struct CachePressure {
    previous_high: bool,
    last_shrink: Option<Instant>,
}

impl CachePressure {
    fn should_shrink(&mut self, high: bool, useful: bool, now: Instant) -> bool {
        let sustained = self.previous_high && high;
        self.previous_high = high;
        if !sustained
            || !useful
            || self
                .last_shrink
                .is_some_and(|last| now.duration_since(last) < SHRINK_COOLDOWN)
        {
            return false;
        }
        self.last_shrink = Some(now);
        true
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
    fn pressure_requires_two_samples_cooldown_and_useful_cache_entries() {
        let mut pressure = CachePressure::default();
        let now = Instant::now();
        assert!(!pressure.should_shrink(true, true, now));
        assert!(!pressure.should_shrink(false, true, now + Duration::from_secs(10)));
        assert!(!pressure.should_shrink(true, true, now + Duration::from_secs(20)));
        assert!(pressure.should_shrink(true, true, now + Duration::from_secs(30)));
        assert!(!pressure.should_shrink(true, true, now + Duration::from_secs(40)));
        assert!(!pressure.should_shrink(true, false, now + Duration::from_secs(90)));
        assert!(pressure.should_shrink(true, true, now + Duration::from_secs(100)));
    }

    #[test]
    fn full_page_cache_does_not_evict_business_cache_but_real_pressure_does() {
        use bitcoincore_rpc::bitcoin::{OutPoint, Txid, hashes::Hash};
        use usdb_util::{BtcScriptHash, MemoryAccountingSource, MemoryUsageSnapshot, UTXOValue};

        let mut config = BalanceHistoryConfig::default();
        config.sync.utxo_max_cache_bytes = 1024 * 1024;
        config.sync.balance_max_cache_bytes = 1024 * 1024;
        config.sync.max_memory_percent = 80;
        let config = Arc::new(config);
        let utxos = Arc::new(UTXOCache::new(config.clone(), CacheStrategy::Normal));
        let balances = Arc::new(AddressBalanceCache::new(
            config.clone(),
            CacheStrategy::Normal,
        ));
        let value = Arc::new(UTXOValue {
            script_hash: BtcScriptHash::from_slice(&[0u8; 32]).unwrap(),
            value: 1000,
        });
        for vout in 0..100 {
            utxos.put(
                Arc::new(OutPoint {
                    txid: Txid::all_zeros(),
                    vout,
                }),
                value.clone(),
            );
        }
        let monitor = MemoryCacheMonitor::new(config, utxos.clone(), balances);
        let mut pressure = CachePressure::default();
        let now = Instant::now();
        let mut memory = MemoryUsageSnapshot {
            source: MemoryAccountingSource::CgroupV2,
            limit_bytes: 1000,
            used_bytes: 990,
            reclaimable_file_bytes: 800,
        };
        for second in [0, 10, 20, 30] {
            monitor.check_memory(memory, &mut pressure, now + Duration::from_secs(second));
        }
        assert_eq!(utxos.get_count(), 100);
        memory.reclaimable_file_bytes = 0;
        monitor.check_memory(memory, &mut pressure, now + Duration::from_secs(40));
        assert_eq!(utxos.get_count(), 100);
        monitor.check_memory(memory, &mut pressure, now + Duration::from_secs(50));
        assert_eq!(utxos.get_count(), 90);
        monitor.check_memory(memory, &mut pressure, now + Duration::from_secs(60));
        assert_eq!(utxos.get_count(), 90);
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
