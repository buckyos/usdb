use super::client::BTCClient;
use super::file_indexer::{
    BlockFileIndexer, BlockFileIndexerCallback, BlockFileReader, BlockFileReaderRef,
};
use crate::btc::rpc::BTCRpcClientRef;
use crate::db::BlockEntry;
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

const BLOCK_FILE_CACHE_MAX_CAPACITY: u64 = 8; // Max 8 blk files in cache

// Cache block records read from blk files
struct BlockFileCache {
    reader: BlockFileReaderRef,
    cache: moka::sync::Cache<usize, Arc<Vec<Block>>>, // file_index -> blocks
}

impl BlockFileCache {
    pub fn new(reader: BlockFileReaderRef) -> Self {
        let cache = moka::sync::Cache::builder()
            .time_to_live(std::time::Duration::from_secs(60 * 5)) // 5 minutes TTL
            .max_capacity(BLOCK_FILE_CACHE_MAX_CAPACITY) // Max 5 blk files cached
            .build();

        Self { reader, cache }
    }

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
}

struct BuildRecordResult {
    block_hash: BlockHash,
    prev_block_hash: BlockHash,
    block_file_index: usize,
    block_file_offset: u64,
    block_record_index: usize,
}

struct BlockRecordCache {
    block_hash_cache: HashMap<BlockHash, BlockEntry>,
    block_prev_hash_cache: HashMap<BlockHash, BlockHash>,
    sorted_blocks: Vec<(u64, BlockHash)>, // (height, block_hash)
}

impl BlockRecordCache {
    pub fn new() -> Self {
        Self {
            block_hash_cache: HashMap::new(),
            block_prev_hash_cache: HashMap::new(),
            sorted_blocks: Vec::new(),
        }
    }

    pub fn new_ref() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new()))
    }
}

#[derive(Clone)]
pub struct BlocksIndexer {
    reader: BlockFileReaderRef,
    cache: Arc<Mutex<BlockRecordCache>>,
    output: IndexOutputRef,
    should_stop: Arc<AtomicBool>,
}

impl BlocksIndexer {
    fn new(
        reader: BlockFileReaderRef,
        cache: Arc<Mutex<BlockRecordCache>>,
        output: IndexOutputRef,
        should_stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            reader,
            cache,
            output,
            should_stop,
        }
    }

    fn merge_build_result(&self, result: Vec<BuildRecordResult>) -> Result<(), String> {
        let mut cache = self.cache.lock().unwrap();
        for record in result {
            if let Some(prev_hash) = cache
                .block_prev_hash_cache
                .insert(record.prev_block_hash, record.block_hash)
            {
                let msg = format!(
                    "Duplicate prev_blockhash found in blk file {}: prev_hash = {}, block_hash = {}",
                    record.block_file_index, prev_hash, record.block_hash
                );
                error!("{}", msg);
                return Err(msg);
            }

            let entry = BlockEntry {
                block_file_index: record.block_file_index as u32,
                block_file_offset: record.block_file_offset,
                block_record_index: record.block_record_index,
            };

            if let Some(prev_entry) = cache
                .block_hash_cache
                .insert(record.block_hash, entry.clone())
            {
                let msg = format!(
                    "Duplicate block_hash found in blk file {}: block_hash = {}, prev_entry = {:?}, new_entry = {:?}",
                    record.block_file_index, record.block_hash, prev_entry, entry
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        Ok(())
    }

    pub fn build_index(&self) -> Result<(), String> {
        let file_indexer = BlockFileIndexer::new(
            self.reader.clone(),
            Arc::new(
                Box::new(self.clone()) as Box<dyn BlockFileIndexerCallback<Vec<BuildRecordResult>>>
            ),
        );

        let file_indexer = Arc::new(file_indexer);
        file_indexer.build_index()?;

        Ok(())
    }

    fn generate_sort_blocks(&self) -> Result<(), String> {
        let mut cache = self.cache.lock().unwrap();
        let mut prev_hash = BlockHash::all_zeros();
        let mut block_height = 0;
        let mut blocks = Vec::with_capacity(cache.block_hash_cache.len());
        loop {
            // Find block hash by prev_hash
            let block_hash = cache.block_prev_hash_cache.get(&prev_hash);
            if block_hash.is_none() {
                break;
            }

            // Get block entry by block_hash
            let block_hash = block_hash.unwrap();
            let entry = cache.block_hash_cache.get(block_hash);
            if entry.is_none() {
                let msg = format!("Block entry not found for block_hash {}", block_hash,);
                error!("{}", msg);
                return Err(msg);
            }

            prev_hash = *block_hash;
            blocks.push((block_height, block_hash.clone()));
            debug!("Loaded block {} with hash {}", block_height, block_hash);

            // For debug only: Verify block height by fetching block from rpc
            /* {
                let rpc_block = client.get_block(block_height).map_err(|e| {
                    let msg = format!(
                        "Failed to get block {} from rpc: {}",
                        block_hash,
                        e
                    );
                    error!("{}", msg);
                    msg
                })?;

                let rpc_block_hash = rpc_block.block_hash();
                if rpc_block_hash != *block_hash {
                    let msg = format!(
                        "Block hash mismatch for block {}: expected {}, got {}",
                        block_height,
                        block_hash,
                        rpc_block_hash
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
            }
            */

            block_height += 1;
        }

        // Save sorted blocks to cache
        cache.sorted_blocks = blocks;

        Ok(())
    }
}

impl BlockFileIndexerCallback<Vec<BuildRecordResult>> for BlocksIndexer {
    fn on_index_begin(&self, total: usize) -> Result<(), String> {
        self.output.start_load(total as u64);

        let latest_blk_file_index = total - 1; // Exclude the last file which may be incomplete
        let msg = format!(
            "Building block index from blk files 0 to {}...",
            latest_blk_file_index
        );
        self.output.println(&msg);

        Ok(())
    }

    fn on_file_index(&self, block_file_index: usize) -> Result<Vec<BuildRecordResult>, String> {
        let msg = format!("Indexing blk file {}", block_file_index);
        self.output.set_load_message(&msg);

        Ok(Vec::new())
    }

    fn on_block_indexed(
        &self,
        user_data: &mut Vec<BuildRecordResult>,
        block_file_index: usize,
        block_file_offset: usize,
        block_record_index: usize,
        block: &Block,
    ) -> Result<(), String> {
        let block_hash = block.block_hash();
        let item = BuildRecordResult {
            block_hash,
            prev_block_hash: block.header.prev_blockhash,
            block_file_index: block_file_index,
            block_file_offset: block_file_offset as u64,
            block_record_index: block_record_index,
        };
        user_data.push(item);

        Ok(())
    }

    fn on_file_indexed(
        &self,
        _block_file_index: usize,
        complete_count: usize,
        user_data: Vec<BuildRecordResult>,
    ) -> Result<(), String> {
        self.merge_build_result(user_data)?;

        // Update progress
        self.output.update_load_current_count(complete_count as u64);

        Ok(())
    }

    fn on_index_complete(&self) -> Result<(), String> {
        self.output
            .set_load_message("Generating sorted block list...");
        self.generate_sort_blocks()?;

        self.output.set_load_message("Block index build complete.");
        self.output.finish_load();

        Ok(())
    }

    fn should_stop(&self) -> bool {
        self.should_stop.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub struct BlockLocalLoader {
    block_reader: BlockFileReaderRef,
    btc_client: BTCRpcClientRef,
    block_index_cache: Arc<Mutex<BlockRecordCache>>,
    file_cache: BlockFileCache,
    output: IndexOutputRef,
    should_stop: Arc<AtomicBool>,
}

impl BlockLocalLoader {
    pub fn new(
        block_magic: u32,
        data_dir: &Path,
        btc_client: BTCRpcClientRef,
        output: IndexOutputRef,
    ) -> Result<Self, String> {
        let block_reader = Arc::new(BlockFileReader::new(block_magic, data_dir)?);
        let block_index_cache = BlockRecordCache::new_ref();
        let file_cache = BlockFileCache::new(block_reader.clone()); // Cache up to 3 blk files

        Ok(Self {
            block_reader,
            btc_client,
            block_index_cache,
            file_cache,
            output,
            should_stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn build_index(&self) -> Result<(), String> {
        let builder = BlocksIndexer::new(
            self.block_reader.clone(),
            self.block_index_cache.clone(),
            self.output.clone(),
            self.should_stop.clone(),
        );
        builder.build_index()
    }

    pub fn get_block_hash(&self, block_height: u64) -> Result<BlockHash, String> {
        let cache = self.block_index_cache.lock().unwrap();
        if cache.sorted_blocks.len() > block_height as usize {
            Ok(cache.sorted_blocks[block_height as usize].1.clone())
        } else {
            warn!(
                "Block height {} not found in local index cache, fetching from rpc",
                block_height
            );
            self.btc_client.get_block_hash(block_height)
        }
    }

    pub fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        // First try to load block from local blk files
        let cache = self.block_index_cache.lock().unwrap();
        let entry = cache.block_hash_cache.get(block_hash);
        if let Some(entry) = entry {
            let block = self.file_cache.get_block_by_file_index(
                entry.block_file_index as usize,
                entry.block_record_index,
            )?;
            return Ok(block);
        }

        // If not found in local blk files, load from rpc
        warn!(
            "Block {} not found in local blk files, fetching from rpc",
            block_hash
        );
        let block = self.btc_client.get_block_by_hash(block_hash).map_err(|e| {
            let msg = format!("Failed to get block {} from rpc: {}", block_hash, e);
            error!("{}", msg);
            msg
        })?;

        Ok(block)
    }

    pub fn get_block_by_height(&self, block_height: u64) -> Result<Block, String> {
        let block_hash = self.get_block_hash(block_height)?;
        self.get_block_by_hash(&block_hash)
    }

    pub async fn get_blocks(
        &self,
        start_height: u64,
        end_height: u64,
    ) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        for height in start_height..=end_height {
            let block = self.get_block_by_height(height)?;
            blocks.push(block);
        }

        Ok(blocks)
    }
}

#[async_trait::async_trait]
impl BTCClient for BlockLocalLoader {
    fn init(&self) -> Result<(), String> {
        self.build_index()?;
        info!("Block index built successfully");

        let cache = self.block_index_cache.lock().unwrap();
        let latest_height = (cache.sorted_blocks.len() as u64).saturating_sub(1);
        info!("Local file latest block height: {}", latest_height);

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.should_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.output.println("Stopping BlockLocalLoader...");

        Ok(())
    }

    fn get_latest_block_height(&self) -> Result<u64, String> {
        self.btc_client.get_latest_block_height()
    }

    fn get_block_hash(&self, block_height: u64) -> Result<BlockHash, String> {
        self.get_block_hash(block_height)
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        self.get_block_by_hash(block_hash)
    }

    fn get_block_by_height(&self, block_height: u64) -> Result<Block, String> {
        self.get_block_by_height(block_height)
    }

    async fn get_blocks(&self, start_height: u64, end_height: u64) -> Result<Vec<Block>, String> {
        self.get_blocks(start_height, end_height).await
    }

    fn get_utxo(
        &self,
        outpoint: &bitcoincore_rpc::bitcoin::OutPoint,
    ) -> Result<
        (
            bitcoincore_rpc::bitcoin::ScriptBuf,
            bitcoincore_rpc::bitcoin::Amount,
        ),
        String,
    > {
        self.btc_client.get_utxo(outpoint)
    }
}

#[cfg(test)]
mod tests {
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{BlockHash, bech32};

    use super::super::rpc::BTCRpcClient;
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use crate::db::BalanceHistoryDB;
    use std::path::PathBuf;

    #[test]
    fn test_read_blk_blocks() {
        let test_data_dir = std::env::temp_dir().join("bitcoin_test_data_loader");
        std::fs::create_dir_all(&test_data_dir).unwrap();

        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);

        let client = BTCRpcClient::new(config.btc.rpc_url(), config.btc.auth()).unwrap();
        let client = std::sync::Arc::new(client);

        let db = BalanceHistoryDB::new(&test_data_dir.join("db"), config.clone()).unwrap();
        let db = std::sync::Arc::new(db);

        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);

        let output = crate::output::IndexOutput::new();
        let output = Arc::new(output);

        /*
        let begin_tick = std::time::Instant::now();
        let mut file_index = 0;
        loop {
            let file = reader.get_blk_file_path(file_index);
            if !file.exists() {
                println!("No more blk files to read at index {:?}", file);
                break;
            }

            //let blocks = reader.load_blk_blocks_by_index(file_index).unwrap();
            let blocks = reader.read_blk_records2(&file).unwrap();
            println!(
                "Loaded {} blocks from blk file {}",
                blocks.len(),
                file_index
            );

            file_index += 1;
            if file_index == 100 {
                break;
            }
        }
        let end_tick = std::time::Instant::now();
        let duration = end_tick.duration_since(begin_tick);
        println!("Finished loading blk files in {:?}", duration);
        */

        /*
        let cache = BlockRecordCache::new_ref();
        let indexer = BlocksIndexer::new(reader.clone(), cache.clone(), output.clone());
        let indexer = Arc::new(indexer);
        let begin_tick = std::time::Instant::now();
        indexer.build_index_by_file_range(0, 5200).unwrap();
        let end_tick = std::time::Instant::now();
        let duration = end_tick.duration_since(begin_tick);
        println!("Finished building index in {:?}", duration);
        */

        let loader = BlockLocalLoader::new(
            config.btc.block_magic(),
            &config.btc.data_dir(),
            client.clone(),
            output.clone(),
        )
        .unwrap();

        loader.build_index().unwrap();

        let block_height = 400000;
        output.start_index(block_height);
        for height in 0..block_height {
            let block = loader.get_block_by_height(height).unwrap();
            //let _block_hash = block.block_hash();
            output.update_current_height(height as u64 + 1);
        }
    }

    #[test]
    fn test_latest_blk_file() {
        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);

        let client = BTCRpcClient::new(config.btc.rpc_url(), config.btc.auth()).unwrap();
        let client = std::sync::Arc::new(client);

        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);

        let output = crate::output::IndexOutput::new();
        let output = Arc::new(output);

        let cache = BlockRecordCache::new_ref();
        let indexer = BlocksIndexer::new(reader.clone(), cache.clone(), output.clone());
        let indexer = Arc::new(indexer);

        let latest_index = indexer.find_latest_blk_file().unwrap();
        println!("Latest blk file index: {}", latest_index);

        // Load latest blk file records
        let latest_block_height = client.get_latest_block_height().unwrap();
        println!("Latest block height from rpc: {}", latest_block_height);
        let latest_block_hash = client.get_block_hash(latest_block_height).unwrap();
        let records = reader.load_blk_blocks_by_index(latest_index).unwrap();
        let mut found = false;
        for record in records {
            let block_hash = record.block_hash();
            println!("Block hash: {}", block_hash);
            if block_hash == latest_block_hash {
                println!(
                    "Found latest block {} in blk file {}",
                    latest_block_height, latest_index
                );
                found = true;
                break;
            }
        }

        assert!(found, "Latest block not found in latest blk file");
    }

    #[test]
    fn test_block_file_cache() {
        let config = BalanceHistoryConfig::default();
        let config = std::sync::Arc::new(config);

        let reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let reader = Arc::new(reader);

        let cache = BlockFileCache::new(reader.clone());

        let block1 = cache.get_block_by_file_index(0, 0).unwrap();
        println!("Block 1 hash: {}", block1.block_hash());

        let block2 = cache.get_block_by_file_index(0, 1).unwrap();
        println!("Block 2 hash: {}", block2.block_hash());

        let block3 = cache.get_block_by_file_index(1, 0).unwrap();
        println!("Block 3 hash: {}", block3.block_hash());

        // Access block1 again to test cache hit
        let block1_again = cache.get_block_by_file_index(0, 0).unwrap();
        println!("Block 1 again hash: {}", block1_again.block_hash());

        // Test file 3
        let block4 = cache.get_block_by_file_index(2, 0).unwrap();
        println!("Block 4 hash: {}", block4.block_hash());

        for i in 0..100 {
            let block = cache.get_block_by_file_index(2, i).unwrap();
            println!("Block from file index {} hash: {}", i, block.block_hash());
        }

        for i in 0..100 {
            let block = cache.get_block_by_file_index(3, i).unwrap();
            println!("Block from file index {} hash: {}", i, block.block_hash());
        }

        assert_eq!(block1.block_hash(), block1_again.block_hash());
    }
}
