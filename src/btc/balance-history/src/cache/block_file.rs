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

#[derive(Clone)]
struct PrefetchManager {
    queue: Arc<Mutex<VecDeque<(usize, Arc<Vec<Block>>)>>>,
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
        if let Some(sender) = sender_lock.as_ref() {
            if let Err(err) = sender.send(file_index) {
                error!(
                    "Failed to notify prefetch for blk file index {}: {}",
                    file_index, err
                );
            }
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
        self.latest_blk_file_index.store(latest_blk_file_index, Ordering::SeqCst);

        let manager = self.clone();
        std::thread::spawn(move || {
            loop {
                // Wait for prefetch requests
                if let Ok(file_index) = rx.recv() {
                    if file_index == 0 {
                        // Stop signal
                        info!("Stopping block file prefetch manager");
                        break;
                    }

                    let mut reach_end = false;
                    loop {
                        let (count, last_file_index) = {
                            let queue = manager.queue.lock().unwrap();
                            (queue.len(), queue.back().map(|(index, _)| *index))
                        };
                        if count >= BLOCK_FILE_PREFETCH_QUEUE_CAPACITY {
                            // Prefetch queue is full, skip this request
                            continue;
                        }

                        let prefetch_index = if let Some(last_index) = last_file_index {
                            last_index + 1
                        } else {
                            file_index
                        };

                        if prefetch_index > manager.latest_blk_file_index.load(Ordering::SeqCst) {
                            // Reached the latest blk file, stop prefetching
                            info!(
                                "Reached latest blk file index {}, stop prefetching",
                                manager.latest_blk_file_index.load(Ordering::SeqCst)
                            );
                            reach_end = true;
                            break;
                        }

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
            }
        });

        Ok(())
    }

    pub fn stop(&self) {
        // Use index 0 as stop signal
        self.notify_prefetch(0);
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
    use crate::config::BalanceHistoryConfig;
    use crate::btc::BlockFileReader;
    use usdb_util::LogConfig;

    #[test]
    fn test_block_file_cache() {
        let config = LogConfig::new("block_file_cache_test").enable_console(true);
        usdb_util::init_log(config);

        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);

        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);

        let cache = BlockFileCache::new(reader.clone());
        for i in 1000..1100 {
            let block = cache.get_block_by_file_index(i, 0).unwrap();
            println!(
                "Got block at file index {}, record 0: {}",
                i,
                block.block_hash()
            );

            // Sleep a bit to allow prefetching
            std::thread::sleep(std::time::Duration::from_millis(3000));
        }
    }
}