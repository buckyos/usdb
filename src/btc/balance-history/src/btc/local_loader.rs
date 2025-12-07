use crate::db::BalanceHistoryDBRef;
use bitcoincore_rpc::bitcoin::consensus::Decodable;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash};
use crate::btc::client::BTCClientRef;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

pub struct BlockLocalLoader {
    block_dir: PathBuf, // Bitcoind blocks directory
    xor_key: Vec<u8>,
    db: BalanceHistoryDBRef,
    btc_client: BTCClientRef,
    block_magic: u32,
    current_iterator: Mutex<CurrentBlockIterator>,  // Use to load blocks sequentially from disk
}

impl BlockLocalLoader {
    pub fn new(block_magic: u32, data_dir: &Path,  db: BalanceHistoryDBRef, btc_client: BTCClientRef) -> Result<Self, String> {
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
            db,
            btc_client,
            current_iterator: Mutex::new(CurrentBlockIterator::default()),
        })
    }

    fn read_blk_records(&self, path: &Path) -> Result<Vec<Vec<u8>>, String> {
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

    fn load_blk_blocks(&self, path: &Path, mut prev_block_hash: BlockHash) -> Result<Vec<Block>, String> {
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

            // Check prev_block_hash if mismatch, try to load missing blocks from rpc
            if block.header.prev_blockhash != prev_block_hash {
                //println!("Block prev hash mismatch: got {}, expected {}", block.header.prev_blockhash, prev_block_hash);
                // Try to load missing blocks from rpc
                //let mut missing_blocks = self.process_missing_blocks(block.header.prev_blockhash, prev_block_hash)?;
                //blocks.append(&mut missing_blocks);
            }
        
            prev_block_hash = block.block_hash();
            blocks.push(block);
        }

        Ok(blocks)
    }

    // Load missing blocks from rpc
    fn process_missing_blocks(&self, prev_block_hash: BlockHash, expected_prev_hash: BlockHash) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        let mut prev_block_hash = prev_block_hash;  
        while prev_block_hash != expected_prev_hash {
            // Load block from rpc
            println!("Loading missing block {} from rpc", prev_block_hash);
            let block = self.btc_client.get_block_by_hash(&prev_block_hash).map_err(|e| {
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

    fn load_blk_blocks_by_index(&self, index: usize, prev_block_hash: BlockHash) -> Result<Vec<Block>, String> {
        let data_file_name = format!("blk{:05}.dat", index);
        let file_path = self.block_dir.join(data_file_name);
        if !file_path.exists() {
            let msg = format!("Block data file does not exist: {}", file_path.display());
            warn!("{}", msg);
            return Ok(vec![]);
        }

        self.load_blk_blocks(&file_path, prev_block_hash)
    }

    // Sequentially load blocks from disk
    pub fn get_next_block(&self) -> Result<(u64, Block), String> {
        let mut current = self.current_iterator.lock().unwrap();

        if current.next_file_index == 0 && current.current_blocks_in_file.is_empty() {
            // First time load
            let blocks = self.load_blk_blocks_by_index(0, BlockHash::all_zeros())?;
            current.current_blocks_in_file = blocks;
            current.next_file_index = 1;
        } else if current.current_block_index_in_file >= current.current_blocks_in_file.len() {
            // Load next blk file
            let blocks = self.load_blk_blocks_by_index(current.next_file_index, current.current_block_hash)?;
            if blocks.is_empty() {
                let msg = format!(
                    "No more blocks available at height {}",
                    current.current_block_height
                );
                error!("{}", msg);
                return Err(msg);
            }

            current.current_blocks_in_file = blocks;
            current.current_block_index_in_file = 0;
            current.next_file_index += 1;
        }

        let block = current.current_blocks_in_file[current.current_block_index_in_file].clone();
        let height = current.current_block_height;
        current.current_block_index_in_file += 1;
        current.current_block_height += 1;
        current.current_block_hash = block.block_hash();

        Ok((height, block))
    }
}


#[cfg(test)]
mod tests {
    use bitcoincore_rpc::bitcoin::BlockHash;
    use bitcoincore_rpc::bitcoin::hashes::Hash;

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

        let loader =
            BlockLocalLoader::new(config.btc.block_magic(), &config.btc.data_dir(), db.clone(), client.clone()).unwrap();

        for i in 0..550 {
            let (height, block) = loader.get_next_block().unwrap();
            println!("Read block at height {}: {} with {} txs", height, block.block_hash(), block.txdata.len());
            assert_eq!(height, i, "Block height mismatch at index {} got {}", i, height);

            println!("Loaded block {} from disk, prev {}, height {}", block.block_hash(), block.header.prev_blockhash, height);

            // Verify by fetching block hash from rpc
            //let rpc_block_hash = client.get_block_hash(height as u64).unwrap();
            //assert_eq!(block.block_hash(), rpc_block_hash, "Block hash mismatch at height {}", height);
        }

        //let blocks = loader.read_blk_blocks_by_index(0).unwrap();
        //assert!(!blocks.is_empty());
        //println!("Read {} blocks from blk00000.dat", blocks.len());
    }
}
