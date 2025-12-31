use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub struct BatchBlockBenchMark {
    // Global cache info
    pub balance_cache_counts: AtomicU64,
    pub utxo_cache_counts: AtomicU64,

    pub load_blocks_duration_micros: AtomicU64,

    // Utxo batch operations
    pub batch_put_utxo_counts: AtomicU64,
    pub batch_spent_utxo_counts: AtomicU64,
    pub batch_update_utxo_duration_micros: AtomicU64,

    // Balance batch operations
    pub batch_update_balance_cache_counts: AtomicU64,
    pub batch_put_balance_counts: AtomicU64,
    pub batch_update_balances_duration_micros: AtomicU64,

}

impl BatchBlockBenchMark {
    pub fn new() -> Self {
        Self {
            balance_cache_counts: AtomicU64::new(0),
            utxo_cache_counts: AtomicU64::new(0),
            load_blocks_duration_micros: AtomicU64::new(0),

            batch_put_utxo_counts: AtomicU64::new(0),
            batch_spent_utxo_counts: AtomicU64::new(0),
            batch_update_utxo_duration_micros: AtomicU64::new(0),

            batch_update_balance_cache_counts: AtomicU64::new(0),
            batch_put_balance_counts: AtomicU64::new(0),
            batch_update_balances_duration_micros: AtomicU64::new(0),
        }
    }

    pub fn log(&self) {
        info!(
            "BenchMark: load_blocks_duration_micros={}, utxo_cache_counts={}, balance_cache_counts={}, \
            batch_put_utxo_counts={}, batch_spent_utxo_counts={}, batch_update_utxo_duration_micros={}, \
            batch_update_balance_cache_counts={}, batch_put_balance_counts={}, batch_update_balances_duration_micros={}",
            self.load_blocks_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.utxo_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.balance_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_utxo_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_spent_utxo_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_utxo_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balance_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_balance_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balances_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
        );
    }
}

pub type BatchBlockBenchMarkRef = Arc<BatchBlockBenchMark>;