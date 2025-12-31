use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub struct BatchBlockBenchMark {
    // Global cache info
    pub balance_cache_counts: AtomicU64,
    pub utxo_cache_counts: AtomicU64,

    pub load_blocks_duration_micros: AtomicU64,

    // Preload utxos
    pub preprocess_utxos_duration_micros: AtomicU64,
    pub preload_utxos_duration_micros: AtomicU64,
    pub preload_utxos_counts: AtomicU64,
    pub preload_utxos_from_none_memory_counts: AtomicU64,
    pub preload_utxos_from_none_memory_duration_micros: AtomicU64,

    // Preload balances
    pub preload_balances_duration_micros: AtomicU64,
    pub preload_balances_counts: AtomicU64,
    pub preload_balances_from_db_counts: AtomicU64,

    // Balance processing
    pub process_balances_duration_micros: AtomicU64,

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
            process_balances_duration_micros: AtomicU64::new(0),

            preprocess_utxos_duration_micros: AtomicU64::new(0),
            preload_utxos_duration_micros: AtomicU64::new(0),
            preload_utxos_counts: AtomicU64::new(0),
            preload_utxos_from_none_memory_counts: AtomicU64::new(0),
            preload_utxos_from_none_memory_duration_micros: AtomicU64::new(0),

            preload_balances_duration_micros: AtomicU64::new(0),
            preload_balances_counts: AtomicU64::new(0),
            preload_balances_from_db_counts: AtomicU64::new(0),
            
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
            "BatchBlockBenchMark: balance_cache_counts={}, utxo_cache_counts={}, load_blocks_duration_micros={}, preprocess_utxos_duration_micros={}, preload_utxos_duration_micros={}, preload_utxos_counts={}, preload_utxos_from_none_memory_counts={}, preload_utxos_from_none_memory_duration_micros={}, preload_balances_duration_micros={}, preload_balances_counts={}, preload_balances_from_db_counts={}, process_balances_duration_micros={}, batch_put_utxo_counts={}, batch_spent_utxo_counts={}, batch_update_utxo_duration_micros={}, batch_update_balance_cache_counts={}, batch_put_balance_counts={}, batch_update_balances_duration_micros={}",
            self.balance_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.utxo_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.load_blocks_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.preprocess_utxos_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_from_none_memory_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_from_none_memory_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_from_db_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.process_balances_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_utxo_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_spent_utxo_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_utxo_duration_micros.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balance_cache_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_balance_counts.load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balances_duration_micros.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

pub type BatchBlockBenchMarkRef = Arc<BatchBlockBenchMark>;