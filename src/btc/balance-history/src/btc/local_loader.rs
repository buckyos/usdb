use crate::btc::rpc::BTCRpcClientRef;
use crate::db::{BalanceHistoryDBRef, BlockEntry};
use bitcoincore_rpc::bitcoin::address::error;
use bitcoincore_rpc::bitcoin::consensus::Decodable;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash, block};
use jsonrpsee::core::middleware::layer;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use super::client::BTCClient;

struct XorReader<R>
where
    R: Read,
{
    inner: R,
    key: [u8; 8],
    pos: usize, // 0..8
}

impl<R: Read> XorReader<R> {
    fn new(inner: R, key: [u8; 8]) -> Self {
        XorReader { inner, key, pos: 0 }
    }
}

impl<R: Read> Read for XorReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        for i in 0..n {
            buf[i] ^= self.key[self.pos];
            self.pos = (self.pos + 1) % 8;
        }
        Ok(n)
    }
}

pub struct BlockFileReader {
    block_dir: PathBuf, // Bitcoind blocks directory
    xor_key: Vec<u8>,
    block_magic: u32,
}

impl BlockFileReader {
    pub fn new(block_magic: u32, data_dir: &Path) -> Result<Self, String> {
        let block_dir = data_dir.join("blocks");
        assert!(
            block_dir.exists(),
            "Block directory does not exist: {}",
            block_dir.display()
        );

        // Try load XOR key from .bitcoin/blocks/xor.dat file
        let xor_file = block_dir.join("xor.dat");
        let xor_key = if xor_file.exists() {
            let mut file = File::open(&xor_file).map_err(|e| {
                let msg = format!("Failed to open xor.dat file {}: {}", xor_file.display(), e);
                log::error!("{}", msg);
                msg
            })?;

            let mut xor_key = [0u8; 8];
            file.read_exact(&mut xor_key).map_err(|e| {
                let msg = format!(
                    "Failed to read xor key from xor.dat file {}: {}",
                    xor_file.display(),
                    e
                );
                log::error!("{}", msg);
                msg
            })?;
            xor_key.to_vec()
        } else {
            vec![]
        };

        Ok(Self {
            block_magic,
            block_dir,
            xor_key,
        })
    }

    fn get_blk_file_name(index: usize) -> String {
        format!("blk{:05}.dat", index)
    }

    pub fn get_blk_file_path(&self, index: usize) -> PathBuf {
        self.block_dir.join(Self::get_blk_file_name(index))
    }

    pub fn read_blk_records2(&self, path: &Path) -> Result<Vec<Block>, String> {
        let mut data = std::fs::read(path).map_err(|e| {
            let msg = format!("Failed to read blk file {}: {}", path.display(), e);
            log::error!("{}", msg);
            msg
        })?;

        // Decrypt data in place if xor_key is set
        if !self.xor_key.is_empty() {
            let key: [u8; 8] = self.xor_key.as_slice().try_into().unwrap();
            for (i, byte) in data.iter_mut().enumerate() {
                *byte ^= key[i % 8];
            }
        }

        let mut records = Vec::new();
        let mut pos = 0usize;
        while pos + 8 <= data.len() {
            let magic =
                u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
            if magic != self.block_magic {
                let msg = format!(
                    "Invalid block magic in blk file {}: expected {:08X}, got {:08X}",
                    path.display(),
                    self.block_magic,
                    magic
                );
                error!("{}", msg);
                return Err(msg);
            }
            pos += 4;

            if pos + 4 > data.len() {
                let msg = format!(
                    "Failed to read size from blk file {}: {} > {}",
                    path.display(),
                    pos + 4,
                    data.len()
                );
                error!("{}", msg);
                return Err(msg);
            }

            let size = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            pos += 4;

            if pos + size > data.len() {
                let msg = format!(
                    "Failed to read block data from blk file {}: {} > {}",
                    path.display(),
                    pos + size,
                    data.len()
                );
                error!("{}", msg);
                return Err(msg);
            }

            use bitcoincore_rpc::bitcoin::consensus::deserialize;
            let block = deserialize::<Block>(&data[pos..pos + size]).map_err(|e| {
                let msg = format!(
                    "Failed to deserialize block from blk file {}: {}",
                    path.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;

            records.push(block);

            pos += size;
        }

        Ok(records)
    }

    pub fn read_blk_records(&self, path: &Path) -> Result<Vec<Vec<u8>>, String> {
        let file = File::open(path).map_err(|e| {
            let msg = format!("Failed to open blk file {}: {}", path.display(), e);
            log::error!("{}", msg);
            msg
        })?;
        let reader = BufReader::new(file);
        let mut reader = if self.xor_key.is_empty() {
            Box::new(reader) as Box<dyn Read>
        } else {
            let reader = BufReader::new(XorReader::new(
                reader,
                self.xor_key.as_slice().try_into().unwrap(),
            ));
            Box::new(reader) as Box<dyn Read>
        };

        let mut records = Vec::new();
        loop {
            let mut magic = [0u8; 4];
            if let Err(e) = reader.read_exact(&mut magic) {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    // Reached EOF
                    break;
                }

                let msg = format!(
                    "Failed to read magic from blk file {}: {}",
                    path.display(),
                    e
                );
                error!("{}", msg);
                return Err(msg);
            }

            let magic_u32 = u32::from_le_bytes(magic);
            if magic_u32 != self.block_magic {
                let msg = format!(
                    "Invalid block magic in blk file {}: expected {:08X}, got {:08X}",
                    path.display(),
                    self.block_magic,
                    magic_u32
                );
                error!("{}", msg);
                return Err(msg);
            }

            let mut size = [0u8; 4];
            reader.read_exact(&mut size).map_err(|e| {
                let msg = format!(
                    "Failed to read size from blk file {}: {}",
                    path.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;

            let size = u32::from_le_bytes(size) as usize;
            let mut data = vec![0u8; size];
            reader.read_exact(&mut data).map_err(|e| {
                let msg = format!(
                    "Failed to read block data from blk file {}: {}",
                    path.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;

            records.push(data);
        }

        Ok(records)
    }

    pub fn read_blk_records_by_index(&self, file_index: usize) -> Result<Vec<Vec<u8>>, String> {
        let file = self.get_blk_file_path(file_index);
        self.read_blk_records(&file)
    }

    pub fn read_blk_record(&self, path: &Path, offset: u64) -> Result<Vec<u8>, String> {
        let file = File::open(path).map_err(|e| {
            let msg = format!("Failed to open blk file {}: {}", path.display(), e);
            log::error!("{}", msg);
            msg
        })?;

        let mut reader = BufReader::new(file);
        reader.seek(std::io::SeekFrom::Start(offset)).map_err(|e| {
            let msg = format!(
                "Failed to seek to offset {} in blk file {}: {}",
                offset,
                path.display(),
                e
            );
            log::error!("{}", msg);
            msg
        })?;

        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic).map_err(|e| {
            let msg = format!(
                "Failed to read magic from blk file {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let mut size = [0u8; 4];
        reader.read_exact(&mut size).map_err(|e| {
            let msg = format!(
                "Failed to read size from blk file {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let size = u32::from_le_bytes(size) as usize;
        let mut data = vec![0u8; size];
        reader.read_exact(&mut data).map_err(|e| {
            let msg = format!(
                "Failed to read block data from blk file {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(data)
    }

    pub fn read_blk_record_by_index(
        &self,
        file_index: usize,
        offset: u64,
    ) -> Result<Vec<u8>, String> {
        let file = self.get_blk_file_path(file_index);
        self.read_blk_record(&file, offset)
    }

    pub fn load_blk_blocks(&self, path: &Path) -> Result<Vec<Block>, String> {
        let records = self.read_blk_records(path)?;

        let mut blocks = Vec::with_capacity(records.len());
        for record in records {
            let block: Block = Block::consensus_decode(&mut record.as_slice()).map_err(|e| {
                let msg = format!(
                    "Failed to deserialize block from blk file {}: {}",
                    path.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;

            blocks.push(block);
        }

        Ok(blocks)
    }

    pub fn load_blk_blocks_by_index(&self, file_index: usize) -> Result<Vec<Block>, String> {
        let file = self.get_blk_file_path(file_index);
        self.load_blk_blocks(&file)
    }

    pub fn load_blk_block(&self, path: &Path, offset: u64) -> Result<Block, String> {
        let record = self.read_blk_record(path, offset)?;

        let block: Block = Block::consensus_decode(&mut record.as_slice()).map_err(|e| {
            let msg = format!(
                "Failed to deserialize block from blk file {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(block)
    }

    pub fn load_blk_block_by_index(&self, file_index: usize, offset: u64) -> Result<Block, String> {
        let file_name = Self::get_blk_file_name(file_index);
        let path = Path::new(&file_name);
        self.load_blk_block(path, offset)
    }
}

// Cache block records read from blk files
struct BlockFileCache {
    reader: BlockFileReaderRef,
    cache: moka::sync::Cache<usize, Vec<Block>>, // file_index -> blocks
}

impl BlockFileCache {
    pub fn new(reader: BlockFileReaderRef, max_capacity: u64) -> Self {
        let cache = moka::sync::Cache::builder()
            .max_capacity(max_capacity)
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

        // Cache the blocks
        self.cache.insert(file_index, blocks);

        Ok(record)
    }
}

pub type BlockFileReaderRef = Arc<BlockFileReader>;

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

pub struct BlocksIndexer {
    reader: BlockFileReaderRef,
    cache: Arc<Mutex<BlockRecordCache>>,
}

impl BlocksIndexer {
    pub fn new(reader: BlockFileReaderRef, cache: Arc<Mutex<BlockRecordCache>>) -> Self {
        Self { reader, cache }
    }

    pub fn cache(&self) -> Arc<Mutex<BlockRecordCache>> {
        self.cache.clone()
    }

    fn build_index_by_file(&self, file_index: usize) -> Result<Vec<BuildRecordResult>, String> {
        let records = self.reader.read_blk_records_by_index(file_index)?;

        let mut offset = 0;
        let mut ret = Vec::new();
        for (record_index, record) in records.iter().enumerate() {
            let block: Block = Block::consensus_decode(&mut record.as_slice()).map_err(|e| {
                let msg = format!(
                    "Failed to deserialize block from blk file {}: {}",
                    file_index, e
                );
                error!("{}", msg);
                msg
            })?;

            let block_hash = block.block_hash();
            let item = BuildRecordResult {
                block_hash,
                prev_block_hash: block.header.prev_blockhash,
                block_file_index: file_index,
                block_file_offset: offset as u64,
                block_record_index: record_index,
            };
            ret.push(item);

            offset += 8 + 4 + record.len(); // magic + size + data
        }

        Ok(ret)
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

    pub async fn build_index_by_file_range_async(
        self: &Arc<Self>,
        start_file_index: usize,
        end_file_index: usize,
    ) -> Result<(), String> {
        // Use tokio to parallelize the indexing of blk files

        let count = (end_file_index - start_file_index + 1) as usize;
        let mut handles = Vec::with_capacity(count);

        for file_index in start_file_index..=end_file_index {
            let handle = tokio::task::spawn_blocking({
                let this = self.clone();
                move || match this.build_index_by_file(file_index) {
                    Ok(ret) => {
                        this.merge_build_result(ret)?;

                        println!("Finished indexing blk file {}", file_index);
                        Ok(())
                    }
                    Err(e) => {
                        let msg =
                            format!("Failed to build index for blk file {}: {}", file_index, e);
                        error!("{}", msg);
                        Err(msg)
                    }
                }
            });
            handles.push(handle);
        }

        let results_of_handles = futures::future::join_all(handles).await;
        for result in results_of_handles {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    return Err(e);
                }
                Err(e) => {
                    let msg = format!("Tokio task failed: {}", e);
                    error!("{}", msg);
                    return Err(msg);
                }
            }
        }

        Ok(())
    }

    pub fn build_index_by_file_range(
        self: &Arc<Self>,
        start_file_index: usize,
        end_file_index: usize,
    ) -> Result<(), String> {
        let tc = (num_cpus::get_physical() * 2).clamp(16, 96);
        info!(
            "Using {} max blocking threads for Tokio runtime to build index by file range",
            tc
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(tc)
            .build()
            .unwrap();

        rt.block_on(self.build_index_by_file_range_async(start_file_index, end_file_index))?;

        Ok(())
    }

    fn find_latest_blk_file(&self) -> Result<usize, String> {
        let mut file_index = 0;
        loop {
            let file = self.reader.get_blk_file_path(file_index);
            if !file.exists() {
                break;
            }
            file_index += 1;
        }

        if file_index == 0 {
            let msg = format!(
                "No blk files found in the data directory {}",
                self.reader.block_dir.display()
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!("Latest blk file index found: {}", file_index - 1);
        Ok(file_index - 1)
    }

    pub fn build_index(self: &Arc<Self>, client: BTCRpcClientRef) -> Result<(), String> {
        let latest_blk_file_index = self.find_latest_blk_file()?;
        self.build_index_by_file_range(0, latest_blk_file_index)?;

        self.generate_sort_blocks()?;

        Ok(())
    }

    fn generate_sort_blocks(&self) -> Result<(), String> {
        let cache = self.cache.lock().unwrap();
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
            println!("Loaded block {} with hash {}", block_height, block_hash);

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
        let mut cache = self.cache.lock().unwrap();
        cache.sorted_blocks = blocks;

        Ok(())
    }
}

pub struct BlockLocalLoader {
    block_reader: BlockFileReaderRef,
    btc_client: BTCRpcClientRef,
    block_index_cache: Arc<Mutex<BlockRecordCache>>,
    file_cache: BlockFileCache,
}

impl BlockLocalLoader {
    pub fn new(
        block_magic: u32,
        data_dir: &Path,
        btc_client: BTCRpcClientRef,
    ) -> Result<Self, String> {
        let block_reader = Arc::new(BlockFileReader::new(block_magic, data_dir)?);
        let block_index_cache = BlockRecordCache::new_ref();
        let file_cache = BlockFileCache::new(block_reader.clone(), 3); // Cache up to 3 blk files

        Ok(Self {
            block_reader,
            btc_client,
            block_index_cache,
            file_cache,
        })
    }

    /*
    // Load missing blocks from rpc
    fn process_missing_blocks(
        &self,
        prev_block_hash: BlockHash,
        expected_prev_hash: BlockHash,
    ) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        let mut prev_block_hash = prev_block_hash;
        while prev_block_hash != expected_prev_hash {
            // Load block from rpc
            println!("Loading missing block {} from rpc", prev_block_hash);
            let block = self
                .btc_client
                .get_block_by_hash(&prev_block_hash)
                .map_err(|e| {
                    let msg = format!("Failed to get block {} from rpc: {}", prev_block_hash, e);
                    error!("{}", msg);
                    msg
                })?;

            prev_block_hash = block.header.prev_blockhash;

            // Prepend block to list
            blocks.insert(0, block);
        }

        Ok(blocks)
    }
    */

    pub fn build_index(&self) -> Result<(), String> {
        let builder = BlocksIndexer::new(self.block_reader.clone(), self.block_index_cache.clone());
        let builder = Arc::new(builder);
        builder.build_index(self.btc_client.clone())
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
    async fn init(&self) -> Result<(), String> {
        self.build_index()?;
        info!("Block index built successfully");

        let cache = self.block_index_cache.lock().unwrap();
        let latest_height = (cache.sorted_blocks.len() as u64).saturating_sub(1);
        info!("Local file latest block height: {}", latest_height);

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

    fn get_utxo(&self, outpoint: &bitcoincore_rpc::bitcoin::OutPoint) -> Result<(bitcoincore_rpc::bitcoin::ScriptBuf, bitcoincore_rpc::bitcoin::Amount), String> {
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

        let cache = BlockRecordCache::new_ref();
        let indexer = BlocksIndexer::new(reader.clone(), cache.clone());
        let indexer = Arc::new(indexer);
        let begin_tick = std::time::Instant::now();
        indexer.build_index_by_file_range(0, 5200).unwrap();
        let end_tick = std::time::Instant::now();
        let duration = end_tick.duration_since(begin_tick);
        println!("Finished building index in {:?}", duration);

        let loader = BlockLocalLoader::new(
            config.btc.block_magic(),
            &config.btc.data_dir(),
            client.clone(),
        )
        .unwrap();

        //loader.build_index().unwrap();
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

        let cache = BlockRecordCache::new_ref();
        let indexer = BlocksIndexer::new(
            reader.clone(),
            cache.clone(),
        );
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
}
