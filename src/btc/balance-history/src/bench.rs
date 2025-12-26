
pub struct BlockBenchMark {
    pub vin_count: AtomicU32,
    pub vout_count: AtomicU32,

    // If hits load from memory cache, otherwise from db, last from bitcoind
    pub utxo_cache_hits: AtomicU32,
    pub utxo_cache_duration_micros: AtomicU64,

    pub utxo_db_hits: AtomicU32,
    pub utxo_db_duration_micros: AtomicU64,
    
    pub utxo_cache_misses: AtomicU32,
    pub utxo_miss_duration_micros: AtomicU64,


}

impl BlockBenchMark {
    pub fn new() -> Self {
        Self {
            vin_count: AtomicU32::new(0),
            vout_count: AtomicU32::new(0),
            utxo_cache_hits: AtomicU32::new(0),
            utxo_cache_duration_micros: AtomicU64::new(0),
            utxo_db_hits: AtomicU32::new(0),
            utxo_db_duration_micros: AtomicU64::new(0),
            utxo_cache_misses: AtomicU32::new(0),
            utxo_miss_duration_micros: AtomicU64::new(0),
        }
    }
}

pub struct BenchMarkManager {
    
}