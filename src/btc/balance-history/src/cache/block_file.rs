use crate::btc::BlockFileReaderRef;
use bitcoincore_rpc::bitcoin::Block;
use lru::LruCache;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

// Each blk file cache will take 200-250MB memory
const BLOCK_FILE_CACHE_MAX_CAPACITY: u64 = 10; // Max 10 blk files in cache

const BLOCK_FILE_PREFETCH_QUEUE_CAPACITY: usize = 5; // Max 5 blk files in prefetch queue

// Cache block records read from blk files
pub struct BlockFileCache {
    reader: BlockFileReaderRef,
    cache: Mutex<LruCache<usize, Arc<Vec<Block>>>>, // file_index -> blocks
    prefetch_manager: PrefetchManager,
}

impl Drop for BlockFileCache {
    fn drop(&mut self) {
        self.prefetch_manager.stop();
    }
}

impl BlockFileCache {
    pub fn new(reader: BlockFileReaderRef) -> Result<Self, String> {
        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(BLOCK_FILE_CACHE_MAX_CAPACITY as usize).unwrap(),
        ));

        let prefetch_manager = PrefetchManager::new(reader.clone());
        prefetch_manager.start()?;

        Ok(Self {
            reader,
            cache,
            prefetch_manager,
        })
    }

    pub fn get_block_by_file_index(
        &self,
        file_index: usize,
        record_index: usize,
    ) -> Result<Block, String> {
        let blocks = {
            let mut cache = self.cache.lock().unwrap();
            cache
                .try_get_or_insert(file_index, || {
                    // Try to get from prefetch first
                    if let Some(blocks) = self.prefetch_manager.fetch_by_index(file_index) {
                        // println!("Prefetch hit for blk file index {}, record {}", file_index, record_index);
                        return Ok::<Arc<Vec<Block>>, String>(blocks);
                    }

                    info!(
                        "Cache miss for blk file index {}, record {}",
                        file_index, record_index
                    );

                    let blocks = self.reader.load_blk_blocks_by_index(file_index)?;
                    let blocks = Arc::new(blocks);
                    Ok::<Arc<Vec<Block>>, String>(blocks)
                })?
                .clone()
        };

        if let Some(block) = blocks.get(record_index) {
            // println!("Cache hit for blk file index {}, record {}", file_index, record_index);
            Ok(block.clone())
        } else {
            let msg = format!(
                "Record index {} out of bounds for file index {}",
                record_index, file_index
            );
            error!("{}", msg);
            Err(msg)
        }
    }

    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        info!("Clearing BlockFileCache, current count: {}", cache.len());
        cache.clear();
    }

    /*
    pub fn get_block_by_file_index(
        &self,
        file_index: usize,
        record_index: usize,
    ) -> Result<Block, String> {
        if let Some(blocks) = self.cache.get(&file_index) {
            if let Some(block) = blocks.get(record_index) {
                // println!("Cache hit for blk file index {}, record {}", file_index, record_index);
                return Ok(block.clone());
            } else {
                let msg = format!(
                    "Record index {} out of bounds for file index {}",
                    record_index, file_index
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        info!(
            "Cache miss for blk file index {}, record {}",
            file_index, record_index
        );
        let blocks = self.reader.load_blk_blocks_by_index(file_index)?;
        let record = blocks.get(record_index);
        if record.is_none() {
            let msg = format!(
                "Record index {} out of bounds for file index {}",
                record_index, file_index
            );
            error!("{}", msg);
            return Err(msg);
        }
        let record = record.unwrap().clone();

        // Remove file_index - BLOCK_FILE_CACHE_MAX_CAPACITY from cache to limit memory usage
        if file_index >= BLOCK_FILE_CACHE_MAX_CAPACITY as usize {
            self.cache
                .invalidate(&(file_index - BLOCK_FILE_CACHE_MAX_CAPACITY as usize));
        }

        // Cache the blocks
        let blocks = Arc::new(blocks);
        self.cache.insert(file_index, blocks);

        Ok(record)
    }
    */
}

pub type BlockFileCacheRef = std::sync::Arc<BlockFileCache>;

type PrefetchQueue = Arc<Mutex<VecDeque<(usize, Arc<Vec<Block>>)>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefetchStep {
    QueueFull,
    ReachedEnd,
    Load(usize),
}

fn plan_prefetch_step(
    queue_len: usize,
    last_file_index: Option<usize>,
    requested_file_index: usize,
    latest_file_index: usize,
) -> PrefetchStep {
    if queue_len >= BLOCK_FILE_PREFETCH_QUEUE_CAPACITY {
        return PrefetchStep::QueueFull;
    }
    let next_file_index = last_file_index
        .map(|index| index.saturating_add(1))
        .unwrap_or(requested_file_index);
    if next_file_index > latest_file_index {
        PrefetchStep::ReachedEnd
    } else {
        PrefetchStep::Load(next_file_index)
    }
}

#[derive(Clone)]
struct PrefetchManager {
    queue: PrefetchQueue,
    reader: BlockFileReaderRef,
    sender: Arc<Mutex<Option<mpsc::Sender<usize>>>>,
    latest_blk_file_index: Arc<AtomicUsize>,
}

impl PrefetchManager {
    pub fn new(reader: BlockFileReaderRef) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            reader,
            sender: Arc::new(Mutex::new(None)),
            latest_blk_file_index: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn notify_prefetch(&self, file_index: usize) {
        let sender_lock = self.sender.lock().unwrap();
        if let Some(sender) = sender_lock.as_ref()
            && let Err(err) = sender.send(file_index)
        {
            error!(
                "Failed to notify prefetch for blk file index {}: {}",
                file_index, err
            );
        }
    }

    pub fn start(&self) -> Result<(), String> {
        let (tx, rx) = mpsc::channel::<usize>();
        {
            let mut sender_lock = self.sender.lock().unwrap();
            *sender_lock = Some(tx);
        }

        // Find the latest file index in local blk files
        // And we should not prefetch beyond that
        let latest_blk_file_index = self.reader.find_latest_blk_file()?;
        self.latest_blk_file_index
            .store(latest_blk_file_index, Ordering::SeqCst);

        let manager = self.clone();
        std::thread::Builder::new()
            .name("block-file-prefetch".to_string())
            .spawn(move || {
                while let Ok(file_index) = rx.recv() {
                    // Wait for prefetch requests instead of polling shared state.
                    if file_index == 0 {
                        info!("Stopping block file prefetch manager");
                        break;
                    }

                    let mut reach_end = false;
                    loop {
                        let (count, last_file_index) = {
                            let queue = manager.queue.lock().unwrap();
                            (queue.len(), queue.back().map(|(index, _)| *index))
                        };
                        let prefetch_index = match plan_prefetch_step(
                            count,
                            last_file_index,
                            file_index,
                            manager.latest_blk_file_index.load(Ordering::SeqCst),
                        ) {
                            PrefetchStep::QueueFull => {
                                // The consumer sends another request before its next fetch. Return
                                // to recv() instead of spinning while sync is paused.
                                break;
                            }
                            PrefetchStep::ReachedEnd => {
                                info!(
                                    "Reached latest blk file index {}, stop prefetching",
                                    manager.latest_blk_file_index.load(Ordering::SeqCst)
                                );
                                reach_end = true;
                                break;
                            }
                            PrefetchStep::Load(index) => index,
                        };

                        let blocks = match manager.reader.load_blk_blocks_by_index(prefetch_index) {
                            Ok(blocks) => {
                                info!("Prefetched blk file index {}", prefetch_index);
                                Arc::new(blocks)
                            }
                            Err(err) => {
                                error!("Failed to load blk file index {}: {}", prefetch_index, err);
                                break;
                            }
                        };

                        manager.add_prefetch(prefetch_index, blocks);
                    }

                    if reach_end {
                        break;
                    }
                }
                manager.sender.lock().unwrap().take();
            })
            .map_err(|error| format!("Failed to start block file prefetch thread: {}", error))?;

        Ok(())
    }

    pub fn stop(&self) {
        // Use index 0 as stop signal
        if let Some(sender) = self.sender.lock().unwrap().take()
            && let Err(error) = sender.send(0)
        {
            debug!("Block file prefetch worker already stopped: {}", error);
        }
    }

    pub fn add_prefetch(&self, file_index: usize, blocks: Arc<Vec<Block>>) {
        let mut queue = self.queue.lock().unwrap();
        queue.push_back((file_index, blocks));
    }

    pub fn get_next_prefetch(&self, file_index: usize) -> Option<(usize, Arc<Vec<Block>>)> {
        let mut queue = self.queue.lock().unwrap();

        // If the requested file_index is less than the front of the queue, return None
        if file_index < queue.front().map(|(index, _)| *index).unwrap_or(usize::MAX) {
            return None;
        }

        queue.pop_front()
    }

    pub fn fetch_by_index(&self, file_index: usize) -> Option<Arc<Vec<Block>>> {
        // First notify prefetch
        self.notify_prefetch(file_index + 1);

        // Then check prefetch queue by order
        while let Some((index, blocks)) = self.get_next_prefetch(file_index) {
            if index == file_index {
                return Some(blocks);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_plan_pauses_when_queue_is_full() {
        assert_eq!(
            plan_prefetch_step(BLOCK_FILE_PREFETCH_QUEUE_CAPACITY, Some(9), 10, 20),
            PrefetchStep::QueueFull
        );
    }

    #[test]
    fn prefetch_plan_loads_requested_or_next_file_and_stops_at_end() {
        assert_eq!(plan_prefetch_step(0, None, 10, 20), PrefetchStep::Load(10));
        assert_eq!(
            plan_prefetch_step(2, Some(12), 10, 20),
            PrefetchStep::Load(13)
        );
        assert_eq!(
            plan_prefetch_step(2, Some(20), 10, 20),
            PrefetchStep::ReachedEnd
        );
    }
}

#[cfg(all(test, usdb_bh_real_btc))]
mod real_btc_tests {
    use super::*;
    use crate::btc::BlockFileReader;
    use crate::config::BalanceHistoryConfig;

    fn real_btc_config() -> Arc<BalanceHistoryConfig> {
        assert_eq!(
            std::env::var("USDB_BH_REAL_BTC").as_deref(),
            Ok("1"),
            "real BTC tests require USDB_BH_REAL_BTC=1"
        );

        let mut config = BalanceHistoryConfig::default();
        let btc_data_dir = std::env::var("BTC_DATA_DIR")
            .expect("BTC_DATA_DIR must be set when USDB_BH_REAL_BTC=1");
        config.btc.data_dir = Some(std::path::PathBuf::from(btc_data_dir));
        if let Ok(block_magic) = std::env::var("BTC_BLOCK_MAGIC") {
            let parsed = if let Some(hex) = block_magic.strip_prefix("0x") {
                u32::from_str_radix(hex, 16)
            } else {
                block_magic.parse()
            }
            .expect("BTC_BLOCK_MAGIC must be a hex or decimal u32");
            config.btc.block_magic = Some(parsed);
        }
        std::sync::Arc::new(config)
    }

    fn env_usize(names: &[&str], default: usize) -> usize {
        names
            .iter()
            .find_map(|name| std::env::var(name).ok().map(|value| (*name, value)))
            .map(|(name, value)| {
                value
                    .parse()
                    .unwrap_or_else(|_| panic!("{} must be a usize", name))
            })
            .unwrap_or(default)
    }

    fn append_real_btc_metric(metric: serde_json::Value) {
        let Ok(path) = std::env::var("USDB_BH_REAL_BTC_METRICS_FILE") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write as _;
        writeln!(file, "{}", metric).unwrap();
    }

    #[test]
    fn real_btc_profile_block_file_cache_prefetch_sample_range() {
        let config = real_btc_config();
        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);

        let cache = BlockFileCache::new(reader.clone()).unwrap();
        let start = env_usize(
            &[
                "USDB_BH_REAL_BTC_PROFILE_START_FILE",
                "USDB_BH_REAL_BTC_CACHE_START_FILE",
            ],
            0,
        );
        let count = env_usize(
            &[
                "USDB_BH_REAL_BTC_PROFILE_FILE_COUNT",
                "USDB_BH_REAL_BTC_CACHE_FILE_COUNT",
            ],
            4,
        );
        let sleep_ms = env_usize(&["USDB_BH_REAL_BTC_CACHE_SLEEP_MS"], 0) as u64;
        let started = std::time::Instant::now();
        let mut successful_reads = 0usize;
        for i in start..start + count {
            let block = cache.get_block_by_file_index(i, 0).unwrap();
            println!(
                "Got block at file index {}, record 0: {}",
                i,
                block.block_hash()
            );
            successful_reads += 1;

            if sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
            }
        }
        append_real_btc_metric(serde_json::json!({
            "component": "balance-history-real-btc-test",
                "metric_type": "block_file_cache_prefetch",
                "test": "real_btc_profile_block_file_cache_prefetch_sample_range",
                "btc_data_dir": config.btc.data_dir().display().to_string(),
                "profile_segment": std::env::var("USDB_BH_REAL_BTC_PROFILE_SEGMENT").unwrap_or_else(|_| "custom".to_string()),
                "start_file": start,
            "requested_file_count": count,
            "successful_reads": successful_reads,
            "sleep_ms": sleep_ms,
            "duration_ms": started.elapsed().as_millis(),
        }));
    }

    #[test]
    fn real_btc_correctness_block_file_cache_live_range_matches_direct_reader() {
        let config = real_btc_config();
        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);
        let cache = BlockFileCache::new(reader.clone()).unwrap();
        let start = env_usize(
            &[
                "USDB_BH_REAL_BTC_PROFILE_START_FILE",
                "USDB_BH_REAL_BTC_CACHE_START_FILE",
            ],
            0,
        );
        let count = env_usize(
            &[
                "USDB_BH_REAL_BTC_PROFILE_FILE_COUNT",
                "USDB_BH_REAL_BTC_CACHE_FILE_COUNT",
            ],
            4,
        );

        for file_index in start..start + count {
            let path = reader.get_blk_file_path(file_index);
            let expected_blocks = reader.read_blk_records2(&path).unwrap();
            assert!(
                !expected_blocks.is_empty(),
                "expected blk file {} to contain at least one block",
                path.display()
            );

            let first = cache.get_block_by_file_index(file_index, 0).unwrap();
            let last_record_index = expected_blocks.len() - 1;
            let last = cache
                .get_block_by_file_index(file_index, last_record_index)
                .unwrap();

            assert_eq!(
                first.block_hash(),
                expected_blocks.first().unwrap().block_hash(),
                "first block hash mismatch for blk file {}",
                file_index
            );
            assert_eq!(
                last.block_hash(),
                expected_blocks.last().unwrap().block_hash(),
                "last block hash mismatch for blk file {}",
                file_index
            );
        }
    }
}
