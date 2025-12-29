use bitcoincore_rpc::bitcoin::Block;
use bitcoincore_rpc::bitcoin::consensus::Decodable;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

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
    block_dir: PathBuf, // Bitcoind service blocks directory
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

pub struct BlockFileIndexer<UserData> {
    reader: BlockFileReaderRef,
    complete_count: Arc<AtomicUsize>,
    callback: BlockFileIndexerCallbackRef<UserData>,
}

impl<UserData> BlockFileIndexer<UserData> {
    pub fn new(
        reader: BlockFileReaderRef,
        callback: BlockFileIndexerCallbackRef<UserData>,
    ) -> Self {
        Self {
            reader,
            callback,
            complete_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    // Check if stop signal is set
    fn check_stop(&self) -> bool {
        self.callback.should_stop()
    }

    fn build_index_by_file(&self, file_index: usize) -> Result<(), String> {
        let mut ignore = false;
        let mut user_data = self.callback.on_file_index(file_index, &mut ignore)?;
        if ignore {
            info!("Ignoring blk file {} as per callback request", file_index);
        } else {
            let records = self.reader.read_blk_records_by_index(file_index)?;

            let mut offset = 0;
            for (record_index, record) in records.iter().enumerate() {
                let block: Block =
                    Block::consensus_decode(&mut record.as_slice()).map_err(|e| {
                        let msg = format!(
                            "Failed to deserialize block from blk file {}: {}",
                            file_index, e
                        );
                        error!("{}", msg);
                        msg
                    })?;

                if let Err(e) = self.callback.on_block_indexed(
                    &mut user_data,
                    file_index,
                    offset,
                    record_index,
                    &block,
                ) {
                    let msg = format!(
                        "Failed to process indexed block for blk file {}: {}",
                        file_index, e
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                offset += 8 + 4 + record.len(); // magic + size + data
            }
        }

        self.complete_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        if let Err(e) =
            self.callback
                .on_file_indexed(file_index, self.complete_count.clone(), user_data)
        {
            let msg = format!(
                "Failed to process indexed file for blk file {}: {}",
                file_index, e
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(())
    }

    pub fn build_index_by_file_range(
        self: &Arc<Self>,
        start_file_index: usize,
        end_file_index: usize,
    ) -> Result<(), String> {
        use rayon::prelude::*;

        let result: Vec<Result<(), String>> = (start_file_index..=end_file_index)
            .into_par_iter()
            .map(|file_index| {
                if self.check_stop() {
                    return Err("Stopped".to_string());
                }

                let result = self.build_index_by_file(file_index);
                match result {
                    Ok(()) => {
                        info!("Finished indexing blk file {}", file_index);
                        Ok(())
                    }
                    Err(e) => {
                        let msg =
                            format!("Failed to build index for blk file {}: {}", file_index, e);
                        error!("{}", msg);
                        Err(msg)
                    }
                }
            })
            .collect();

        for ret in result {
            if let Err(e) = ret {
                return Err(e);
            }
        }

        Ok(())
    }

    /*
    pub fn build_index_by_file_range_async(
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
    */

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

    pub fn build_index(self: &Arc<Self>) -> Result<(), String> {
        let latest_blk_file_index = self.find_latest_blk_file()?;

        self.complete_count
            .store(0, std::sync::atomic::Ordering::SeqCst);

        self.callback.on_index_begin(latest_blk_file_index)?;

        let latest_blk_file_index = latest_blk_file_index - 1; // Exclude the last file which may be incomplete

        self.build_index_by_file_range(0, latest_blk_file_index)?;

        self.callback.on_index_complete()?;

        Ok(())
    }
}

pub trait BlockFileIndexerCallback<UserData>: Send + Sync {
    fn on_index_begin(&self, total: usize) -> Result<(), String>;

    fn on_file_index(&self, block_file_index: usize, ignore: &mut bool)
    -> Result<UserData, String>;
    fn on_block_indexed(
        &self,
        user_data: &mut UserData,
        block_file_index: usize,
        block_file_offset: usize,
        block_record_index: usize,
        block: &Block,
    ) -> Result<(), String>;

    fn on_file_indexed(
        &self,
        block_file_index: usize,
        complete_count: Arc<AtomicUsize>,
        user_data: UserData,
    ) -> Result<(), String>;

    fn on_index_complete(&self) -> Result<(), String>;

    fn should_stop(&self) -> bool;
}

pub type BlockFileIndexerCallbackRef<UserData> = Arc<Box<dyn BlockFileIndexerCallback<UserData>>>;
