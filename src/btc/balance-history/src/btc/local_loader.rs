use crate::btc::client::BTCClientRef;
use crate::db::{BalanceHistoryDBRef, BlockEntry};
use bitcoincore_rpc::bitcoin::consensus::Decodable;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash, block};
use jsonrpsee::core::middleware::layer;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

struct CurrentBlockIterator {
    next_file_index: usize,
    current_blocks_in_file: Vec<Block>,
    current_block_index_in_file: usize,
    current_block_height: u64,
    current_block_hash: BlockHash,
}

impl Default for CurrentBlockIterator {
    fn default() -> Self {
        CurrentBlockIterator {
            next_file_index: 0,
            current_blocks_in_file: Vec::new(),
            current_block_index_in_file: 0,
            current_block_height: 0,
            current_block_hash: BlockHash::all_zeros(),
        }
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

pub type BlockFileReaderRef = Arc<BlockFileReader>;

struct BuildRecordResult {
    block_hash: BlockHash,
    prev_block_hash: BlockHash,
    block_file_index: usize,
    block_file_offset: u64,
}

struct BlockRecordCache {
    block_hash_cache: HashMap<BlockHash, BlockEntry>,
    block_prev_hash_cache: HashMap<BlockHash, BlockHash>,
}

pub struct BlocksIndexer {
    reader: BlockFileReaderRef,
    db: BalanceHistoryDBRef,
    cache: Mutex<BlockRecordCache>,
}

impl BlocksIndexer {
    pub fn new(reader: BlockFileReaderRef, db: BalanceHistoryDBRef) -> Self {
        Self {
            reader,
            db,
            cache: Mutex::new(BlockRecordCache {
                block_hash_cache: HashMap::new(),
                block_prev_hash_cache: HashMap::new(),
            }),
        }
    }

    fn build_index_by_file(&self, file_index: usize) -> Result<Vec<BuildRecordResult>, String> {
        let records = self.reader.read_blk_records_by_index(file_index)?;

        let mut offset = 0;
        let mut ret = Vec::new();
        for record in records {
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
        info!("Using {} max blocking threads for Tokio runtime to build index by file range", tc);

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
            let msg = format!("No blk files found in the data directory {}", self.reader.block_dir.display());
            error!("{}", msg);
            return Err(msg);
        }

        info!("Latest blk file index found: {}", file_index - 1);
        Ok(file_index - 1)
    }

    pub fn build_index(self: &Arc<Self>, client: BTCClientRef) -> Result<(), String> {
        let latest_blk_file_index = self.find_latest_blk_file()?;
        let latest_blk_file_index = 100;
        self.build_index_by_file_range(0, latest_blk_file_index)?;

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
                let msg = format!(
                    "Block entry not found for block_hash {}",
                    block_hash,
                );
                error!("{}", msg);
                return Err(msg);
            }

            prev_hash = *block_hash;
            blocks.push((block_height, entry.unwrap()));
            println!("Loaded block {} with hash {}", block_height, block_hash);
            // Verify block height by fetching block from rpc
            {
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

            block_height += 1;
        }

        // self.db.put_blocks(&blocks)?;
        Ok(())
    }
}

pub struct BlockLocalLoader {
    block_reader: BlockFileReaderRef,
    db: BalanceHistoryDBRef,
    btc_client: BTCClientRef,
    current_iterator: Mutex<CurrentBlockIterator>, // Use to load blocks sequentially from disk
}

impl BlockLocalLoader {
    pub fn new(
        block_magic: u32,
        data_dir: &Path,
        db: BalanceHistoryDBRef,
        btc_client: BTCClientRef,
    ) -> Result<Self, String> {
        let block_reader = Arc::new(BlockFileReader::new(block_magic, data_dir)?);

        Ok(Self {
            block_reader,
            db,
            btc_client,
            current_iterator: Mutex::new(CurrentBlockIterator::default()),
        })
    }

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

    fn get_blk_file_name(index: usize) -> String {
        format!("blk{:05}.dat", index)
    }

    pub fn build_index(&self) -> Result<(), String> {
        let builder = BlocksIndexer::new(self.block_reader.clone(), self.db.clone());
        let builder = Arc::new(builder);
        builder.build_index(self.btc_client.clone())
    }
}

#[cfg(test)]
mod tests {
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{BlockHash, bech32};

    use super::super::client::BTCClient;
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

        let client = BTCClient::new(config.btc.rpc_url(), config.btc.auth()).unwrap();
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

        
        let indexer = BlocksIndexer::new(reader.clone(), db.clone());
        let indexer = Arc::new(indexer);
        let begin_tick = std::time::Instant::now();
        indexer.build_index_by_file_range(0, 5200).unwrap();
        let end_tick = std::time::Instant::now();
        let duration = end_tick.duration_since(begin_tick);
        println!("Finished building index in {:?}", duration);
        

        let loader = BlockLocalLoader::new(
            config.btc.block_magic(),
            &config.btc.data_dir(),
            db.clone(),
            client.clone(),
        )
        .unwrap();

        //loader.build_index().unwrap();
    }
}
