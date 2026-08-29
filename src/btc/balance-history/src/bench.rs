use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

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

impl Default for BatchBlockBenchMark {
    fn default() -> Self {
        Self::new()
    }
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

    /// Logs one completed block batch with its range, total latency, phase
    /// latencies, mutation counts, and resulting cache sizes.
    pub fn log(&self, block_range: &std::ops::Range<u32>, total_elapsed: Duration) {
        info!(
            "Balance-history batch metrics: module=batch_block_processor, start_height={}, end_height={}, block_count={}, total_elapsed_ms={}, balance_cache_count={}, utxo_cache_count={}, load_blocks_duration_micros={}, preprocess_utxos_duration_micros={}, preload_utxos_duration_micros={}, preload_utxos_count={}, preload_utxos_from_storage_count={}, preload_utxos_from_storage_duration_micros={}, preload_balances_duration_micros={}, preload_balances_count={}, preload_balances_from_db_count={}, process_balances_duration_micros={}, batch_put_utxo_count={}, batch_spent_utxo_count={}, batch_update_utxo_duration_micros={}, batch_update_balance_cache_count={}, batch_put_balance_count={}, batch_update_balances_duration_micros={}",
            block_range.start,
            block_range.end.saturating_sub(1),
            block_range.len(),
            total_elapsed.as_millis(),
            self.balance_cache_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.utxo_cache_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.load_blocks_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preprocess_utxos_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_from_none_memory_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_utxos_from_none_memory_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preload_balances_from_db_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.process_balances_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_utxo_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_spent_utxo_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_utxo_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balance_cache_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_put_balance_counts
                .load(std::sync::atomic::Ordering::Relaxed),
            self.batch_update_balances_duration_micros
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

pub type BatchBlockBenchMarkRef = Arc<BatchBlockBenchMark>;
