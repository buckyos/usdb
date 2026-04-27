use super::client::{BTCClient, BTCClientRef, BTCClientType};
use super::file_indexer::{
    BlockFileIndexer, BlockFileIndexerCallback, BlockFileReader, BlockFileReaderRef,
};
use crate::cache::BlockFileCache;
use crate::db::{BalanceHistoryDB, BalanceHistoryDBRef, BlockEntry};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Block, BlockHash};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

struct BuildRecordResult {
    block_hash: BlockHash,
    prev_block_hash: BlockHash,
    block_file_index: usize,
    block_file_offset: u64,
    block_record_index: usize,
}

pub struct BlockRecordCache {
    btc_client: BTCClientRef,
    block_hash_cache: HashMap<BlockHash, BlockEntry>,

    // Mapping from prev_block_hash -> block_hash
    // There maybe multiple blocks with the same prev_block_hash (e.g. forks),
    block_prev_hash_cache: HashMap<BlockHash, Vec<BlockHash>>,
    sorted_blocks: Vec<(u32, BlockHash)>, // (height, block_hash)
}

impl BlockRecordCache {
    pub fn new(btc_client: BTCClientRef) -> Self {
        Self {
            btc_client,
            block_hash_cache: HashMap::new(),
            block_prev_hash_cache: HashMap::new(),
            sorted_blocks: Vec::new(),
        }
    }

    pub fn new_ref(btc_client: BTCClientRef) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new(btc_client)))
    }

    // Try load block records from db if exists
    pub fn load_from_db(&mut self, db: &BalanceHistoryDB) -> Result<(), String> {
        let blocks = db.get_all_blocks()?;
        let block_heights = db.get_all_block_heights()?;

        self.block_hash_cache = blocks;
        self.sorted_blocks = block_heights;

        Ok(())
    }

    pub fn save_to_db(
        &self,
        last_block_file_index: u32,
        db: &BalanceHistoryDB,
    ) -> Result<(), String> {
        // Expand block_hash_cache to vec and sort
        let mut blocks = Vec::with_capacity(self.block_hash_cache.len());
        for (block_hash, entry) in self.block_hash_cache.iter() {
            blocks.push((block_hash.clone(), entry.clone()));
        }

        use rayon::prelude::*;
        blocks.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));

        db.put_blocks_sync(last_block_file_index, &blocks, &self.sorted_blocks)?;

        Ok(())
    }

    pub fn calc_latest_block_file_index(&self) -> Option<u32> {
        self.block_hash_cache
            .values()
            .map(|entry| entry.block_file_index)
            .max()
    }

    pub fn get_latest_block_height(&self) -> u32 {
        if self.sorted_blocks.is_empty() {
            0
        } else {
            self.sorted_blocks.last().unwrap().0
        }
    }

    pub fn clear(&mut self) {
        self.block_hash_cache.clear();
        self.block_prev_hash_cache.clear();
        self.sorted_blocks.clear();
    }

    pub fn add_new_block_entry(
        &mut self,
        block_hash: &BlockHash,
        prev_block_hash: &BlockHash,
        entry: BlockEntry,
    ) -> Result<(), String> {
        match self.block_prev_hash_cache.entry(*prev_block_hash) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(vec![*block_hash]);
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                warn!(
                    "Multiple blocks with the same prev_block_hash {}: adding block_hash {}",
                    prev_block_hash, block_hash
                );
                e.get_mut().push(*block_hash);
            }
        }

        if let Some(prev_entry) = self.block_hash_cache.insert(*block_hash, entry.clone()) {
            let msg = format!(
                "Duplicate block_hash found in blk file {}: block_hash = {}, prev_entry = {:?}, new_entry = {:?}",
                entry.block_file_index, block_hash, prev_entry, entry
            );
            error!("{}", msg);
            return Err(msg);
        }

        Ok(())
    }

    pub fn generate_sort_blocks(&mut self) -> Result<(), String> {
        let mut prev_hash = BlockHash::all_zeros();
        let mut block_height = 0;
        let mut blocks = Vec::with_capacity(self.block_hash_cache.len());
        loop {
            // Find block hash by prev_hash
            let ret = self.block_prev_hash_cache.get(&prev_hash);
            if ret.is_none() {
                break;
            }
            let block_hashes = ret.unwrap();
            assert!(
                !block_hashes.is_empty(),
                "Block hashes list should not be empty for prev_hash {}",
                prev_hash
            );

            // If there are multiple blocks with the same prev_hash (forks), use btc rpc to find the main chain block
            let block_hash = if block_hashes.len() > 1 {
                warn!(
                    "Multiple blocks found with the same prev_hash {}: {:?}, querying rpc for main chain block",
                    prev_hash, block_hashes
                );

                let block = self
                    .btc_client
                    .get_block_by_height(block_height)
                    .map_err(|e| {
                        let msg = format!(
                            "Failed to get block at height {} from rpc: {}",
                            block_height, e
                        );
                        error!("{}", msg);
                        msg
                    })?;
                let block_hash = block.block_hash();

                if !block_hashes.contains(&block_hash) {
                    let msg = format!(
                        "Rpc returned block hash {} at height {}, which is not in the list of block hashes {:?} with prev_hash {}",
                        block_hash, block_height, block_hashes, prev_hash
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                block_hash
            } else {
                block_hashes[0]
            };

            // Get block entry by block_hash
            let entry = self.block_hash_cache.get(&block_hash);
            if entry.is_none() {
                let msg = format!("Block entry not found for block_hash {}", block_hash,);
                error!("{}", msg);
                return Err(msg);
            }

            prev_hash = block_hash;
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
        self.sorted_blocks = blocks;

        Ok(())
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
            let entry = BlockEntry {
                block_file_index: record.block_file_index as u32,
                block_file_offset: record.block_file_offset,
                block_record_index: record.block_record_index as u32,
            };

            cache.add_new_block_entry(&record.block_hash, &record.prev_block_hash, entry)?;
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
}

impl BlockFileIndexerCallback<Vec<BuildRecordResult>> for BlocksIndexer {
    fn on_index_begin(&self, total: usize) -> Result<(), String> {
        self.output.start_load(total as u64);

        if total == 0 {
            self.output.println(
                "No complete blk files available for LocalLoader yet; falling back to RPC reads during this sync.",
            );
            return Ok(());
        }

        let latest_blk_file_index = total - 1; // Exclude the last file which may be incomplete
        let msg = format!(
            "Building block index from blk files 0 to {}...",
            latest_blk_file_index
        );
        self.output.println(&msg);

        Ok(())
    }

    fn on_file_index(
        &self,
        block_file_index: usize,
        _ignore: &mut bool,
    ) -> Result<Vec<BuildRecordResult>, String> {
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
        complete_count: Arc<AtomicUsize>,
        user_data: Vec<BuildRecordResult>,
    ) -> Result<(), String> {
        self.merge_build_result(user_data)?;

        // Update progress
        self.output.update_load_current_count(
            complete_count.load(std::sync::atomic::Ordering::Relaxed) as u64,
        );

        Ok(())
    }

    fn on_index_complete(&self) -> Result<(), String> {
        self.output
            .set_load_message("Generating sorted block list...");
        self.cache.lock().unwrap().generate_sort_blocks()?;

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
    btc_client: BTCClientRef,
    block_index_cache: Arc<Mutex<BlockRecordCache>>,
    file_cache: BlockFileCache,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
    should_stop: Arc<AtomicBool>,
}

impl BlockLocalLoader {
    pub fn new(
        block_magic: u32,
        data_dir: &Path,
        btc_client: BTCClientRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Result<Self, String> {
        let block_reader = Arc::new(BlockFileReader::new(block_magic, data_dir)?);
        let block_index_cache = BlockRecordCache::new_ref(btc_client.clone());
        let file_cache = BlockFileCache::new(block_reader.clone())?;

        Ok(Self {
            block_reader,
            btc_client,
            block_index_cache,
            file_cache,
            db,
            output,
            should_stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn build_index(&self) -> Result<(), String> {
        // First try load from db
        if let Some(last_block_file_index) = self.db.get_last_block_file_index()? {
            info!(
                "Loading block index from db, last indexed blk file index: {}",
                last_block_file_index
            );

            let current_last_blk_file = self.block_reader.find_latest_blk_file()? as u32;
            if last_block_file_index > current_last_blk_file {
                warn!(
                    "Persisted block index is ahead of local blk files: module=local_loader, action=clear_and_rebuild, db_last_block_file_index={}, current_last_blk_file={}",
                    last_block_file_index, current_last_blk_file
                );
                self.db.clear_blocks()?;
            } else if Self::should_try_restore_from_db(last_block_file_index, current_last_blk_file)
            {
                if self
                    .try_restore_block_index_from_db(last_block_file_index, current_last_blk_file)?
                {
                    return Ok(());
                }
            } else {
                warn!(
                    "Block index in db is outdated (last indexed blk file index: {}, current last blk file index: {}), rebuilding index from blk files...",
                    last_block_file_index, current_last_blk_file
                );
            }
        }

        self.rebuild_block_index()
    }

    fn should_try_restore_from_db(last_block_file_index: u32, current_last_blk_file: u32) -> bool {
        last_block_file_index >= current_last_blk_file.saturating_sub(10)
    }

    fn validation_tip_height(len: usize) -> Option<u32> {
        if len == 0 {
            None
        } else {
            Some((len - 1) as u32)
        }
    }

    fn validate_loaded_block_index(
        cache: &BlockRecordCache,
        btc_client: &BTCClientRef,
        last_block_file_index: u32,
        current_last_blk_file: u32,
    ) -> Result<(), String> {
        if cache.sorted_blocks.is_empty() {
            return Err("Persisted block index is empty".to_string());
        }

        if last_block_file_index > current_last_blk_file {
            return Err(format!(
                "Persisted last block file index {} exceeds current local blk file {}",
                last_block_file_index, current_last_blk_file
            ));
        }

        if let Some(max_cached_file_index) = cache.calc_latest_block_file_index() {
            if max_cached_file_index > last_block_file_index {
                return Err(format!(
                    "Cached block file index {} exceeds persisted last block file index {}",
                    max_cached_file_index, last_block_file_index
                ));
            }
        }

        for (expected_height, (height, block_hash)) in cache.sorted_blocks.iter().enumerate() {
            if *height != expected_height as u32 {
                return Err(format!(
                    "Persisted block heights are not contiguous: expected height {}, got {}",
                    expected_height, height
                ));
            }

            if !cache.block_hash_cache.contains_key(block_hash) {
                return Err(format!(
                    "Persisted block hash {} at height {} is missing from block cache",
                    block_hash, height
                ));
            }
        }

        // Keep the restore validation cheap: rely on in-memory continuity checks,
        // then use one RPC tip hash as the external chain anchor.
        if let Some(height) = Self::validation_tip_height(cache.sorted_blocks.len()) {
            let expected_hash = cache.sorted_blocks[height as usize].1;
            let rpc_hash = btc_client.get_block_hash(height).map_err(|e| {
                format!(
                    "Failed to validate persisted block index against RPC at height {}: {}",
                    height, e
                )
            })?;

            if rpc_hash != expected_hash {
                return Err(format!(
                    "Persisted block hash mismatch at height {}: cached={}, rpc={}",
                    height, expected_hash, rpc_hash
                ));
            }
        }

        Ok(())
    }

    fn try_restore_block_index_from_db(
        &self,
        last_block_file_index: u32,
        current_last_blk_file: u32,
    ) -> Result<bool, String> {
        self.output.println("Loading block index from db...");

        match Self::restore_or_clear_persisted_block_index(
            &self.block_index_cache,
            &self.db,
            &self.btc_client,
            last_block_file_index,
            current_last_blk_file,
        ) {
            Ok(()) => {
                self.output.println("Block index loaded from db.");
                info!(
                    "Loaded persisted block index successfully: module=local_loader, db_last_block_file_index={}, current_last_blk_file={}",
                    last_block_file_index, current_last_blk_file
                );
                Ok(true)
            }
            Err(reason) => {
                warn!(
                    "Persisted block index validation failed: module=local_loader, reason={}, action=clear_and_rebuild, db_last_block_file_index={}, current_last_blk_file={}",
                    reason, last_block_file_index, current_last_blk_file
                );
                Ok(false)
            }
        }
    }

    fn restore_or_clear_persisted_block_index(
        block_index_cache: &Arc<Mutex<BlockRecordCache>>,
        db: &BalanceHistoryDBRef,
        btc_client: &BTCClientRef,
        last_block_file_index: u32,
        current_last_blk_file: u32,
    ) -> Result<(), String> {
        let validation_result = {
            let mut cache = block_index_cache.lock().unwrap();
            cache.clear();
            cache.load_from_db(db)?;
            Self::validate_loaded_block_index(
                &cache,
                btc_client,
                last_block_file_index,
                current_last_blk_file,
            )
        };

        match validation_result {
            Ok(()) => Ok(()),
            Err(reason) => {
                block_index_cache.lock().unwrap().clear();
                db.clear_blocks().map_err(|e| {
                    format!(
                        "{}; additionally failed to clear persisted block index: {}",
                        reason, e
                    )
                })?;
                Err(reason)
            }
        }
    }

    fn rebuild_block_index(&self) -> Result<(), String> {
        let builder = BlocksIndexer::new(
            self.block_reader.clone(),
            self.block_index_cache.clone(),
            self.output.clone(),
            self.should_stop.clone(),
        );
        builder.build_index()?;

        // Save to db
        self.output.println("Saving block index to db...");

        let cache = self.block_index_cache.lock().unwrap();
        let Some(last_block_file_index) = cache.calc_latest_block_file_index() else {
            self.output.println(
                "Local block index is empty after excluding the last open blk file; skipping persisted block index save.",
            );
            return Ok(());
        };
        cache.save_to_db(last_block_file_index, &self.db)?;

        info!("Block index saved to db {}", last_block_file_index);
        self.output.println(&format!(
            "Block index saved to db {}",
            last_block_file_index
        ));

        Ok(())
    }

    pub fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
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
                entry.block_record_index as usize,
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

    pub fn get_block_by_height(&self, block_height: u32) -> Result<Block, String> {
        let block_hash = self.get_block_hash(block_height)?;
        self.get_block_by_hash(&block_hash)
    }

    pub async fn get_blocks(
        &self,
        start_height: u32,
        end_height: u32,
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
    fn get_type(&self) -> BTCClientType {
        BTCClientType::LocalLoader
    }

    fn init(&self) -> Result<(), String> {
        self.build_index()?;
        info!("Block index built successfully");

        let cache = self.block_index_cache.lock().unwrap();
        let latest_height = (cache.sorted_blocks.len() as u32).saturating_sub(1);
        info!("Local file latest block height: {}", latest_height);

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.should_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.output.println("Stopping BlockLocalLoader...");

        Ok(())
    }

    fn on_sync_complete(&self, block_height: u32) -> Result<(), String> {
        // Clear caches
        {
            let mut cache = self.block_index_cache.lock().unwrap();
            cache.clear();
        }
        self.file_cache.clear();

        debug!(
            "BlockLocalLoader sync complete at block height {}: caches cleared",
            block_height
        );

        Ok(())
    }

    fn get_latest_block_height(&self) -> Result<u32, String> {
        self.btc_client.get_latest_block_height()
    }

    fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
        self.get_block_hash(block_height)
    }

    fn get_block_by_hash(&self, block_hash: &BlockHash) -> Result<Block, String> {
        self.get_block_by_hash(block_hash)
    }

    fn get_block_by_height(&self, block_height: u32) -> Result<Block, String> {
        self.get_block_by_height(block_height)
    }

    async fn get_blocks(&self, start_height: u32, end_height: u32) -> Result<Vec<Block>, String> {
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
    use super::*;
    use crate::btc::BTCClient;
    use crate::config::BalanceHistoryConfig;
    use crate::db::{BalanceHistoryDB, BalanceHistoryDBMode};
    use crate::output::IndexOutput;
    use crate::status::SyncStatusManager;
    #[cfg(usdb_bh_real_btc)]
    use bitcoincore_rpc::bitcoin::Network;
    use bitcoincore_rpc::bitcoin::{Amount, BlockHash, OutPoint, ScriptBuf};
    use std::collections::BTreeMap;
    #[cfg(usdb_bh_real_btc)]
    use std::time::{SystemTime, UNIX_EPOCH};
    #[cfg(usdb_bh_real_btc)]
    use usdb_util::{BTCAuth, BTCRpcClient};

    #[cfg(usdb_bh_real_btc)]
    const TEST_SUBSET_BLK_FILE_COUNT: usize = 4;

    struct MockBTCClient {
        hashes: BTreeMap<u32, BlockHash>,
    }

    #[async_trait::async_trait]
    impl BTCClient for MockBTCClient {
        fn get_type(&self) -> BTCClientType {
            BTCClientType::RPC
        }

        fn init(&self) -> Result<(), String> {
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            Ok(())
        }

        fn on_sync_complete(&self, _block_height: u32) -> Result<(), String> {
            Ok(())
        }

        fn get_latest_block_height(&self) -> Result<u32, String> {
            self.hashes
                .keys()
                .next_back()
                .copied()
                .ok_or_else(|| "No mock block heights configured".to_string())
        }

        fn get_block_hash(&self, block_height: u32) -> Result<BlockHash, String> {
            self.hashes
                .get(&block_height)
                .copied()
                .ok_or_else(|| format!("Missing mock block hash at height {}", block_height))
        }

        fn get_block_by_hash(&self, _block_hash: &BlockHash) -> Result<Block, String> {
            Err("MockBTCClient::get_block_by_hash is not used in this test".to_string())
        }

        fn get_block_by_height(&self, _block_height: u32) -> Result<Block, String> {
            Err("MockBTCClient::get_block_by_height is not used in this test".to_string())
        }

        async fn get_blocks(
            &self,
            _start_height: u32,
            _end_height: u32,
        ) -> Result<Vec<Block>, String> {
            Err("MockBTCClient::get_blocks is not used in this test".to_string())
        }

        fn get_utxo(&self, _outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
            Err("MockBTCClient::get_utxo is not used in this test".to_string())
        }
    }

    fn make_mock_client(hashes: &[(u32, [u8; 32])]) -> BTCClientRef {
        let hashes = hashes
            .iter()
            .map(|(height, hash)| (*height, BlockHash::from_slice(hash).unwrap()))
            .collect();
        Arc::new(Box::new(MockBTCClient { hashes }) as Box<dyn BTCClient>)
    }

    fn make_cache(entries: &[(u32, [u8; 32], u32)]) -> BlockRecordCache {
        let client = make_mock_client(&[]);
        let mut cache = BlockRecordCache::new(client);
        for (height, hash, file_index) in entries {
            let block_hash = BlockHash::from_slice(hash).unwrap();
            cache.sorted_blocks.push((*height, block_hash));
            cache.block_hash_cache.insert(
                block_hash,
                BlockEntry {
                    block_file_index: *file_index,
                    block_file_offset: 0,
                    block_record_index: 0,
                },
            );
        }
        cache
    }

    #[test]
    fn test_should_try_restore_from_db_uses_saturating_threshold() {
        assert!(BlockLocalLoader::should_try_restore_from_db(0, 0));
        assert!(BlockLocalLoader::should_try_restore_from_db(0, 5));
        assert!(BlockLocalLoader::should_try_restore_from_db(3, 9));
        assert!(BlockLocalLoader::should_try_restore_from_db(5, 15));
        assert!(!BlockLocalLoader::should_try_restore_from_db(3, 15));
    }

    #[test]
    fn test_validation_tip_height_only_anchors_tip() {
        assert_eq!(BlockLocalLoader::validation_tip_height(0), None);
        assert_eq!(BlockLocalLoader::validation_tip_height(1), Some(0));
        assert_eq!(BlockLocalLoader::validation_tip_height(3), Some(2));
    }

    #[test]
    fn test_validate_loaded_block_index_rejects_non_contiguous_heights() {
        let cache = make_cache(&[(0, [1u8; 32], 0), (2, [2u8; 32], 0)]);
        let client = make_mock_client(&[(0, [1u8; 32]), (2, [2u8; 32])]);

        let err = BlockLocalLoader::validate_loaded_block_index(&cache, &client, 0, 0).unwrap_err();
        assert!(err.contains("not contiguous"));
    }

    #[test]
    fn test_validate_loaded_block_index_rejects_tip_hash_mismatch() {
        let cache = make_cache(&[(0, [1u8; 32], 0), (1, [2u8; 32], 0), (2, [3u8; 32], 0)]);
        let client = make_mock_client(&[(0, [1u8; 32]), (1, [2u8; 32]), (2, [9u8; 32])]);

        let err = BlockLocalLoader::validate_loaded_block_index(&cache, &client, 0, 0).unwrap_err();
        assert!(err.contains("mismatch"));
    }

    #[test]
    fn test_validate_loaded_block_index_accepts_consistent_cache() {
        let cache = make_cache(&[(0, [1u8; 32], 0), (1, [2u8; 32], 0), (2, [3u8; 32], 0)]);
        let client = make_mock_client(&[(0, [1u8; 32]), (1, [2u8; 32]), (2, [3u8; 32])]);

        BlockLocalLoader::validate_loaded_block_index(&cache, &client, 0, 0).unwrap();
    }

    #[test]
    fn test_restore_or_clear_persisted_block_index_clears_invalid_db_state() {
        let mut config = BalanceHistoryConfig::default();
        let temp_dir = std::env::temp_dir().join("balance_history_restore_or_clear_test");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        config.root_dir = temp_dir;
        let config = Arc::new(config);

        let db = Arc::new(BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap());

        let block_hash_1 = BlockHash::from_slice(&[1u8; 32]).unwrap();
        let block_hash_2 = BlockHash::from_slice(&[2u8; 32]).unwrap();
        let blocks = vec![
            (
                block_hash_1,
                BlockEntry {
                    block_file_index: 0,
                    block_file_offset: 0,
                    block_record_index: 0,
                },
            ),
            (
                block_hash_2,
                BlockEntry {
                    block_file_index: 0,
                    block_file_offset: 1,
                    block_record_index: 1,
                },
            ),
        ];
        let broken_heights = vec![(0, block_hash_1), (2, block_hash_2)];
        db.put_blocks_sync(0, &blocks, &broken_heights).unwrap();

        let cache = BlockRecordCache::new_ref(make_mock_client(&[(0, [1u8; 32]), (2, [2u8; 32])]));
        let btc_client = make_mock_client(&[(0, [1u8; 32]), (2, [2u8; 32])]);

        let err = BlockLocalLoader::restore_or_clear_persisted_block_index(
            &cache,
            &db,
            &btc_client,
            0,
            0,
        )
        .unwrap_err();

        assert!(err.contains("not contiguous"));
        assert_eq!(db.get_last_block_file_index().unwrap(), None);
        assert!(db.get_all_blocks().unwrap().is_empty());
        assert!(db.get_all_block_heights().unwrap().is_empty());
        assert!(cache.lock().unwrap().sorted_blocks.is_empty());
    }

    #[cfg(usdb_bh_real_btc)]
    fn real_test_root(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("balance_history_local_loader_{}_{}", tag, nanos))
    }

    #[cfg(usdb_bh_real_btc)]
    fn required_real_btc_env(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| {
            panic!(
                "{} must be set when USDB_BH_REAL_BTC=1 real-data tests are compiled",
                name
            )
        })
    }

    #[cfg(usdb_bh_real_btc)]
    fn parse_real_btc_network() -> Network {
        match std::env::var("BTC_NETWORK")
            .unwrap_or_else(|_| "bitcoin".to_string())
            .as_str()
        {
            "bitcoin" | "mainnet" => Network::Bitcoin,
            "testnet" | "testnet3" => Network::Testnet,
            "regtest" => Network::Regtest,
            "signet" => Network::Signet,
            "testnet4" => Network::Testnet4,
            other => panic!(
                "unsupported BTC_NETWORK='{}'; expected bitcoin, testnet, regtest, signet, or testnet4",
                other
            ),
        }
    }

    #[cfg(usdb_bh_real_btc)]
    fn load_real_test_base_config() -> BalanceHistoryConfig {
        assert_eq!(
            std::env::var("USDB_BH_REAL_BTC").as_deref(),
            Ok("1"),
            "real BTC tests require USDB_BH_REAL_BTC=1"
        );

        let data_dir = std::path::PathBuf::from(required_real_btc_env("BTC_DATA_DIR"));
        let rpc_url = required_real_btc_env("BTC_RPC_URL");
        let mut config = BalanceHistoryConfig::default();
        config.btc.network = parse_real_btc_network();
        config.btc.data_dir = Some(data_dir.clone());
        config.btc.rpc_url = Some(rpc_url);
        config.btc.auth = if let Ok(cookie_file) = std::env::var("BTC_COOKIE_FILE") {
            Some(BTCAuth::CookieFile(std::path::PathBuf::from(cookie_file)))
        } else if let Ok(user) = std::env::var("BTC_RPC_USER") {
            Some(BTCAuth::UserPass(
                user,
                std::env::var("BTC_RPC_PASSWORD").unwrap_or_default(),
            ))
        } else {
            Some(BTCAuth::CookieFile(data_dir.join(".cookie")))
        };
        if let Ok(block_magic) = std::env::var("BTC_BLOCK_MAGIC") {
            let parsed = if let Some(hex) = block_magic.strip_prefix("0x") {
                u32::from_str_radix(hex, 16)
            } else {
                block_magic.parse()
            }
            .expect("BTC_BLOCK_MAGIC must be a hex or decimal u32");
            config.btc.block_magic = Some(parsed);
        }
        config
    }

    #[cfg(usdb_bh_real_btc)]
    fn prepare_subset_block_data_dir(tag: &str, file_count: usize) -> std::path::PathBuf {
        let source_config = load_real_test_base_config();
        let source_data_dir = source_config.btc.data_dir();
        let source_blocks_dir = source_data_dir.join("blocks");
        let target_data_dir = real_test_root(&format!("{}_btc", tag));
        let target_blocks_dir = target_data_dir.join("blocks");
        std::fs::create_dir_all(&target_blocks_dir).unwrap();

        let xor_file = source_blocks_dir.join("xor.dat");
        if xor_file.exists() {
            std::fs::copy(&xor_file, target_blocks_dir.join("xor.dat")).unwrap();
        }

        for index in 0..file_count {
            let file_name = format!("blk{:05}.dat", index);
            let source_file = source_blocks_dir.join(&file_name);
            assert!(
                source_file.exists(),
                "expected blk file to exist for real local loader test: {}",
                source_file.display()
            );
            std::fs::copy(&source_file, target_blocks_dir.join(file_name)).unwrap();
        }

        target_data_dir
    }

    #[cfg(usdb_bh_real_btc)]
    fn make_real_test_env(
        tag: &str,
    ) -> (
        Arc<BalanceHistoryConfig>,
        BTCClientRef,
        Arc<BalanceHistoryDB>,
        Arc<IndexOutput>,
    ) {
        let mut config = load_real_test_base_config();
        let rpc_url = config.btc.rpc_url();
        let auth = config.btc.auth();
        let root_dir = real_test_root(tag);
        let _ = std::fs::remove_dir_all(&root_dir);
        std::fs::create_dir_all(&root_dir).unwrap();
        config.root_dir = root_dir;
        let subset_data_dir = prepare_subset_block_data_dir(tag, TEST_SUBSET_BLK_FILE_COUNT);
        config.btc.data_dir = Some(subset_data_dir);
        let config = Arc::new(config);

        let client = BTCRpcClient::new(rpc_url, auth).unwrap();
        let client: BTCClientRef = Arc::new(Box::new(client) as Box<dyn BTCClient>);

        let db =
            Arc::new(BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap());

        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));

        (config, client, db, output)
    }

    #[cfg(usdb_bh_real_btc)]
    fn build_real_loader(
        config: Arc<BalanceHistoryConfig>,
        client: BTCClientRef,
        db: Arc<BalanceHistoryDB>,
        output: Arc<IndexOutput>,
    ) -> BlockLocalLoader {
        BlockLocalLoader::new(
            config.btc.block_magic(),
            &config.btc.data_dir(),
            client,
            db,
            output,
        )
        .unwrap()
    }

    #[cfg(usdb_bh_real_btc)]
    fn assert_real_rpc_available(client: &BTCClientRef) {
        match client.get_latest_block_height() {
            Ok(height) => {
                println!(
                    "real local-loader test connected to bitcoind RPC, latest height={}",
                    height
                );
            }
            Err(err) => {
                panic!(
                    "real BTC tests require a reachable bitcoind RPC configured by BTC_RPC_URL/auth env: {}",
                    err
                );
            }
        }
    }

    #[cfg(usdb_bh_real_btc)]
    fn sample_heights(latest_indexed_height: u32) -> Vec<u32> {
        let mut heights = vec![0, latest_indexed_height];
        if latest_indexed_height >= 2 {
            heights.push(latest_indexed_height / 2);
        }
        heights.sort_unstable();
        heights.dedup();
        heights
    }

    #[cfg(usdb_bh_real_btc)]
    fn make_subset_reader(
        tag: &str,
        file_count: usize,
    ) -> (Arc<BalanceHistoryConfig>, Arc<BlockFileReader>) {
        let mut config = BalanceHistoryConfig::default();
        config.btc.data_dir = Some(prepare_subset_block_data_dir(tag, file_count));
        let config = Arc::new(config);
        let reader = Arc::new(
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap(),
        );
        (config, reader)
    }

    #[cfg(usdb_bh_real_btc)]
    fn make_default_subset_reader(tag: &str) -> (Arc<BalanceHistoryConfig>, Arc<BlockFileReader>) {
        make_subset_reader(tag, TEST_SUBSET_BLK_FILE_COUNT)
    }

    #[cfg(usdb_bh_real_btc)]
    fn make_live_reader_and_client() -> (
        Arc<BalanceHistoryConfig>,
        BTCClientRef,
        Arc<BlockFileReader>,
    ) {
        let config = Arc::new(load_real_test_base_config());
        let client = BTCRpcClient::new(config.btc.rpc_url(), config.btc.auth()).unwrap();
        let client: BTCClientRef = Arc::new(Box::new(client) as Box<dyn BTCClient>);
        let reader = Arc::new(
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap(),
        );
        (config, client, reader)
    }

    #[cfg(usdb_bh_real_btc)]
    fn non_empty_file_indices(reader: &BlockFileReader, max_file_index: usize) -> Vec<usize> {
        let mut result = Vec::new();
        for file_index in 0..=max_file_index {
            let blocks = reader.load_blk_blocks_by_index(file_index).unwrap();
            if !blocks.is_empty() {
                result.push(file_index);
            }
        }
        result
    }

    #[cfg(usdb_bh_real_btc)]
    fn assert_subset_latest_blk_file(reader: &BlockFileReader) -> usize {
        let latest_index = reader.find_latest_blk_file().unwrap();
        assert_eq!(
            latest_index,
            TEST_SUBSET_BLK_FILE_COUNT - 1,
            "expected subset blk dir to contain exactly {} blk files",
            TEST_SUBSET_BLK_FILE_COUNT
        );
        latest_index
    }

    #[cfg(usdb_bh_real_btc)]
    fn first_non_empty_file_with_min_blocks(
        reader: &BlockFileReader,
        min_block_count: usize,
    ) -> usize {
        let latest_index = reader.find_latest_blk_file().unwrap();
        for file_index in non_empty_file_indices(reader, latest_index) {
            let block_count = reader.load_blk_blocks_by_index(file_index).unwrap().len();
            if block_count >= min_block_count {
                return file_index;
            }
        }

        panic!(
            "expected at least one blk file with {} or more blocks in subset data dir",
            min_block_count
        );
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_local_loader_build_index_matches_rpc_on_sample_heights() {
        let (config, client, db, output) = make_real_test_env("build_match_rpc");
        assert_real_rpc_available(&client);
        let subset_reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        let subset_latest = assert_subset_latest_blk_file(&subset_reader);
        println!("subset data dir: {}", config.btc.data_dir().display());
        println!("subset latest blk file index: {}", subset_latest);

        let loader = build_real_loader(config, client.clone(), db, output);

        loader.build_index().unwrap();

        let latest_indexed_height = {
            let cache = loader.block_index_cache.lock().unwrap();
            assert!(
                !cache.sorted_blocks.is_empty(),
                "local loader cache should not be empty"
            );
            cache.sorted_blocks.last().unwrap().0
        };

        for height in sample_heights(latest_indexed_height) {
            let loader_hash = loader.get_block_hash(height).unwrap();
            let rpc_hash = client.get_block_hash(height).unwrap();
            assert_eq!(
                loader_hash, rpc_hash,
                "block hash mismatch at height {}",
                height
            );

            let block = loader.get_block_by_height(height).unwrap();
            assert_eq!(
                block.block_hash(),
                rpc_hash,
                "block body/hash mismatch at height {}",
                height
            );
        }
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_restore_block_index_from_db() {
        let (config, client, db, output) = make_real_test_env("restore_valid");
        assert_real_rpc_available(&client);
        let subset_reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        assert_subset_latest_blk_file(&subset_reader);
        let loader = build_real_loader(config.clone(), client.clone(), db.clone(), output.clone());
        loader.build_index().unwrap();

        let last_block_file_index = db.get_last_block_file_index().unwrap().unwrap();
        let current_last_blk_file = loader.block_reader.find_latest_blk_file().unwrap() as u32;

        let restore_loader = build_real_loader(config, client.clone(), db, output);
        let restored = restore_loader
            .try_restore_block_index_from_db(last_block_file_index, current_last_blk_file)
            .unwrap();
        assert!(
            restored,
            "expected persisted block index restore to succeed"
        );

        let latest_indexed_height = {
            let cache = restore_loader.block_index_cache.lock().unwrap();
            assert!(
                !cache.sorted_blocks.is_empty(),
                "restored cache should not be empty"
            );
            cache.sorted_blocks.last().unwrap().0
        };

        let restored_hash = restore_loader
            .get_block_hash(latest_indexed_height)
            .unwrap();
        let rpc_hash = client.get_block_hash(latest_indexed_height).unwrap();
        assert_eq!(restored_hash, rpc_hash);
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_build_index_rebuilds_after_corrupted_persisted_state() {
        let (config, client, db, output) = make_real_test_env("rebuild_after_corrupt");
        assert_real_rpc_available(&client);
        let subset_reader =
            BlockFileReader::new(config.btc.block_magic(), &config.btc.data_dir()).unwrap();
        assert_subset_latest_blk_file(&subset_reader);
        let loader = build_real_loader(config.clone(), client.clone(), db.clone(), output.clone());
        loader.build_index().unwrap();

        let block_hash_0 = client.get_block_hash(0).unwrap();
        let block_hash_2 = client.get_block_hash(2).unwrap();
        db.clear_blocks().unwrap();
        db.put_blocks_sync(
            0,
            &vec![
                (
                    block_hash_0,
                    BlockEntry {
                        block_file_index: 0,
                        block_file_offset: 0,
                        block_record_index: 0,
                    },
                ),
                (
                    block_hash_2,
                    BlockEntry {
                        block_file_index: 0,
                        block_file_offset: 1,
                        block_record_index: 1,
                    },
                ),
            ],
            &vec![(0, block_hash_0), (2, block_hash_2)],
        )
        .unwrap();

        let rebuild_loader = build_real_loader(config, client.clone(), db.clone(), output);
        rebuild_loader.build_index().unwrap();

        let latest_indexed_height = {
            let cache = rebuild_loader.block_index_cache.lock().unwrap();
            assert!(
                !cache.sorted_blocks.is_empty(),
                "rebuilt cache should not be empty"
            );
            for (expected_height, (height, _)) in cache.sorted_blocks.iter().enumerate() {
                assert_eq!(
                    *height, expected_height as u32,
                    "rebuilt cache should be contiguous"
                );
            }
            cache.sorted_blocks.last().unwrap().0
        };

        assert!(db.get_last_block_file_index().unwrap().is_some());
        assert!(!db.get_all_block_heights().unwrap().is_empty());
        let rebuilt_hash = rebuild_loader
            .get_block_hash(latest_indexed_height)
            .unwrap();
        let rpc_hash = client.get_block_hash(latest_indexed_height).unwrap();
        assert_eq!(rebuilt_hash, rpc_hash);
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_read_blk_blocks_matches_direct_reader_on_subset_files() {
        let (_config, reader) = make_default_subset_reader("read_blk_blocks");
        let latest_index = assert_subset_latest_blk_file(&reader);

        let non_empty_files = non_empty_file_indices(&reader, latest_index);
        assert!(
            !non_empty_files.is_empty(),
            "expected at least one non-empty blk file in subset data dir"
        );

        for file_index in non_empty_files {
            let path = reader.get_blk_file_path(file_index);
            let blocks_via_record_reader = reader.read_blk_records2(&path).unwrap();
            let blocks_via_loader = reader.load_blk_blocks_by_index(file_index).unwrap();
            assert_eq!(
                blocks_via_record_reader.len(),
                blocks_via_loader.len(),
                "block count mismatch for blk file {}",
                file_index
            );
            assert_eq!(
                blocks_via_record_reader.first().unwrap().block_hash(),
                blocks_via_loader.first().unwrap().block_hash(),
                "first block hash mismatch for blk file {}",
                file_index
            );
            assert_eq!(
                blocks_via_record_reader.last().unwrap().block_hash(),
                blocks_via_loader.last().unwrap().block_hash(),
                "last block hash mismatch for blk file {}",
                file_index
            );
        }
    }

    #[cfg(usdb_bh_real_btc)]
    fn measure_blk_file_memory_usage(
        reader: &BlockFileReader,
        start_index: usize,
        count: usize,
    ) -> (usize, u64) {
        let mut sys = sysinfo::System::new_all();
        sys.refresh_memory();
        let available_memory = sys.available_memory();

        let mut file_index = start_index;
        let mut result = Vec::new();
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
            result.push(blocks);

            file_index += 1;
            if file_index >= start_index + count {
                break;
            }
        }

        sys.refresh_memory();
        let available_memory_after = sys.available_memory();
        let used_memory = available_memory - available_memory_after;
        let item_memory = used_memory / count as u64;
        println!(
            "Used memory after loading {} blk files: {} bytes",
            count, used_memory
        );
        println!("Estimated memory per item: {} bytes", item_memory);

        (result.len(), used_memory)
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_profile_blk_file_reader_memory_usage() {
        let (_config, reader) = make_default_subset_reader("memory_profile");
        let (loaded_file_count, used_memory) =
            measure_blk_file_memory_usage(&reader, 0, TEST_SUBSET_BLK_FILE_COUNT);
        assert!(
            loaded_file_count > 0,
            "expected memory profiling to load at least one blk file"
        );
        println!(
            "profiled blk reader memory usage: loaded_files={}, used_memory={} bytes",
            loaded_file_count, used_memory
        );
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_latest_complete_blk_file_blocks_are_available_via_rpc() {
        let (_config, client, reader) = make_live_reader_and_client();
        assert_real_rpc_available(&client);
        let latest_index = reader.find_latest_blk_file().unwrap();
        println!("Latest blk file index: {}", latest_index);

        let latest_complete_index = latest_index.saturating_sub(1);
        let records = reader
            .load_blk_blocks_by_index(latest_complete_index)
            .unwrap();
        assert!(
            !records.is_empty(),
            "expected latest complete blk file {} to contain blocks",
            latest_complete_index
        );

        let sample_indices = if records.len() == 1 {
            vec![0]
        } else {
            vec![0, records.len() - 1]
        };

        for record_index in sample_indices {
            let block_hash = records[record_index].block_hash();
            let rpc_block = client.get_block_by_hash(&block_hash).unwrap();
            assert_eq!(
                rpc_block.block_hash(),
                block_hash,
                "block at record {} from latest complete blk file {} did not match RPC",
                record_index,
                latest_complete_index
            );
        }
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_block_file_cache_returns_consistent_block_on_repeat_access() {
        let (_config, reader) = make_default_subset_reader("cache_repeat_access");
        let cache = BlockFileCache::new(reader.clone()).unwrap();
        let file_index = first_non_empty_file_with_min_blocks(&reader, 2);
        let expected_blocks = reader.load_blk_blocks_by_index(file_index).unwrap();

        let first_block = cache.get_block_by_file_index(file_index, 0).unwrap();
        let second_block = cache.get_block_by_file_index(file_index, 1).unwrap();
        let first_block_again = cache.get_block_by_file_index(file_index, 0).unwrap();

        assert_eq!(first_block.block_hash(), expected_blocks[0].block_hash());
        assert_eq!(second_block.block_hash(), expected_blocks[1].block_hash());
        assert_eq!(first_block.block_hash(), first_block_again.block_hash());
    }

    #[cfg(usdb_bh_real_btc)]
    #[test]
    fn real_btc_correctness_block_file_cache_matches_reader_across_multiple_files() {
        let (_config, reader) = make_default_subset_reader("cache_multiple_files");
        let cache = BlockFileCache::new(reader.clone()).unwrap();
        let non_empty_files =
            non_empty_file_indices(&reader, assert_subset_latest_blk_file(&reader));

        assert!(
            !non_empty_files.is_empty(),
            "expected at least one non-empty blk file in subset data dir"
        );

        for file_index in non_empty_files {
            let expected_blocks = reader.load_blk_blocks_by_index(file_index).unwrap();
            let first_block = cache.get_block_by_file_index(file_index, 0).unwrap();
            let last_record_index = expected_blocks.len() - 1;
            let last_block = cache
                .get_block_by_file_index(file_index, last_record_index)
                .unwrap();

            assert_eq!(
                first_block.block_hash(),
                expected_blocks.first().unwrap().block_hash()
            );
            assert_eq!(
                last_block.block_hash(),
                expected_blocks.last().unwrap().block_hash()
            );
        }
    }
}
