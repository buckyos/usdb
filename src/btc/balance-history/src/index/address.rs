use crate::btc::{
    self, BTCClientRef, BTCRpcClient, BlockFileIndexer, BlockFileIndexerCallback, BlockFileReader,
    BlockRecordCache,
};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDB, AddressDBRef, BlockEntry};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::{Block, ScriptBuf, ScriptHash};
use bloomfilter::Bloom;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AddressIndexer {
    config: BalanceHistoryConfigRef,
    db: AddressDBRef,
    output: IndexOutputRef,
    filter: Arc<Mutex<Bloom<ScriptHash>>>,
    block_cache: Arc<Mutex<BlockRecordCache>>,
    total_addresses: Arc<AtomicU64>,
    should_stop: Arc<AtomicBool>,
    btc_client: BTCClientRef,
}

impl AddressIndexer {
    pub fn new(
        root_dir: &Path,
        config: BalanceHistoryConfigRef,
        output: IndexOutputRef,
    ) -> Result<Self, String> {
        output.println("Initializing AddressIndexer...");
        let db = AddressDB::new(root_dir).map_err(|e| {
            let msg = format!("Failed to create AddressDB: {}", e);
            error!("{}", msg);
            msg
        })?;

        let bloom = Bloom::new_for_fp_rate(1_000_000_000, 0.01).map_err(|e| {
            let msg = format!("Failed to create bloom filter: {}", e);
            error!("{}", msg);
            msg
        })?;

        let rpc_url = config.btc.rpc_url();
        let auth = config.btc.auth();
        let btc_client = BTCRpcClient::new(rpc_url, auth).map_err(|e| {
            let msg = format!("Failed to create BTC client: {}", e);
            error!("{}", msg);
            msg
        })?;
        let btc_client = Arc::new(Box::new(btc_client) as Box<dyn btc::BTCClient>);

        Ok(Self {
            db: Arc::new(db),
            config,
            output,
            filter: Arc::new(Mutex::new(bloom)),
            block_cache: Arc::new(Mutex::new(BlockRecordCache::new())),
            total_addresses: Arc::new(AtomicU64::new(0)),
            should_stop: Arc::new(AtomicBool::new(false)),
            btc_client,
        })
    }

    pub fn build_index(&self) -> Result<(), String> {
        if !self.db.is_all_file_indexed()? {
            self.build_index_from_local()?;
        } else {
            self.output.println("All blk files have been indexed, skipping local index...");
        }

        self.build_index_from_rpc()?;

        Ok(())
    }

    fn build_index_from_local(&self) -> Result<(), String> {
        let block_reader = Arc::new(BlockFileReader::new(
            self.config.btc.block_magic(),
            &self.config.btc.data_dir(),
        )?);

        let file_indexer = BlockFileIndexer::new(
            block_reader,
            Arc::new(Box::new(self.clone())
                as Box<
                    dyn BlockFileIndexerCallback<Option<Vec<(ScriptHash, ScriptBuf)>>>,
                >),
        );

        self.filter.lock().unwrap().clear();
        self.block_cache.lock().unwrap().clear();
        self.total_addresses.store(0, Ordering::SeqCst);

        let file_indexer = Arc::new(file_indexer);
        file_indexer.build_index()?;

        Ok(())
    }

    fn build_index_from_rpc(&self) -> Result<(), String> {
        let latest_block_height = self.btc_client.get_latest_block_height()? as u32;
        let current_block_height = self.db.get_indexed_block_height()?;
        if current_block_height.is_none() {
            let msg = format!(
                "No indexed block height found, should run full index by local loader instead of RPC, latest block height: {}",
                latest_block_height
            );
            self.output.println(&msg);
            error!("{}", msg);
            return Err(msg);
        }
        let current_block_height = current_block_height.unwrap();
        if current_block_height >= latest_block_height {
            self.output.println(&format!(
                "Address index is already up to date at block height {} >= {}",
                current_block_height, latest_block_height
            ));
            return Ok(());
        }

        self.output.println(&format!(
            "Building address index from RPC from block height {} to {}...",
            current_block_height, latest_block_height
        ));

        for height in (current_block_height + 1)..=latest_block_height {
            let block = self.btc_client.get_block_by_height(height as u64)?;

            // Process block
            let mut records: Vec<(ScriptHash, ScriptBuf)> = Vec::new();
            for tx in &block.txdata {
                for output in &tx.output {
                    // Skip OP_RETURN outputs
                    if output.script_pubkey.is_op_return() {
                        continue;
                    }

                    let script_hash = output.script_pubkey.script_hash();
                    {
                        // If the blk had been index before, the bloom filter maybe incomplete, so this filer may let some duplicates pass
                        let mut filter = self.filter.lock().unwrap();
                        if filter.check(&script_hash) {
                            continue;
                        }
                        filter.set(&script_hash);
                    }
                    records.push((script_hash, output.script_pubkey.clone()));
                }
            }
            if !records.is_empty() {
                self.db.put_addresses(&records)?;
            }

            self.db.set_indexed_block_height(height)?;

            self.output.println(&format!(
                "Indexed block height {}, new addresses: {}",
                height,
                records.len(),
            ));
        }

        let msg = format!(
            "Address index build from RPC complete, latest block height reached {}",
            latest_block_height
        );
        self.output.println(&msg);
        info!("{}", msg);

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.should_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.output.println("Stopping AddressBuilder...");

        Ok(())
    }
}

impl BlockFileIndexerCallback<Option<Vec<(ScriptHash, ScriptBuf)>>> for AddressIndexer {
    fn on_index_begin(&self, total: usize) -> Result<(), String> {
        self.output.start_load(total as u64);

        let latest_blk_file_index = total - 1; // Exclude the last file which may be incomplete
        let msg = format!(
            "Building address from blk files 0 to {}...",
            latest_blk_file_index
        );
        self.output.println(&msg);

        Ok(())
    }

    fn on_file_index(
        &self,
        block_file_index: usize,
        _ignore: &mut bool,
    ) -> Result<Option<Vec<(ScriptHash, ScriptBuf)>>, String> {
        // Check if this blk file has already been indexed, then skip it
        match self.db.is_file_indexed(block_file_index as u32)? {
            true => Ok(None),
            false => Ok(Some(Vec::new())),
        }
    }

    fn on_block_indexed(
        &self,
        records: &mut Option<Vec<(ScriptHash, ScriptBuf)>>,
        block_file_index: usize,
        block_file_offset: usize,
        block_record_index: usize,
        block: &Block,
    ) -> Result<(), String> {
        let block_hash = block.block_hash();
        {
            let mut cache = self.block_cache.lock().unwrap();
            cache.add_new_block_entry(
                &block_hash,
                &block.header.prev_blockhash,
                BlockEntry {
                    block_file_index: block_file_index as u32,
                    block_file_offset: block_file_offset as u64,
                    block_record_index,
                },
            )?;
        }

        if let Some(records) = records.as_mut() {
            for tx in &block.txdata {
                for output in &tx.output {
                    let script_hash = output.script_pubkey.script_hash();

                    {
                        let mut filter = self.filter.lock().unwrap();
                        if filter.check(&script_hash) {
                            continue;
                        }
                        filter.set(&script_hash);
                    }

                    records.push((script_hash, output.script_pubkey.clone()));
                }
            }
        } else {
            // This block file has been indexed before, so skip
        }
        

        Ok(())
    }

    fn on_file_indexed(
        &self,
        block_file_index: usize,
        complete_count: Arc<std::sync::atomic::AtomicUsize>,
        records: Option<Vec<(ScriptHash, ScriptBuf)>>,
    ) -> Result<(), String> {
        if let Some(ref records) = records {
            if !records.is_empty() {
                self.db.put_addresses(&records)?;
            } else {
                // No new addresses found in this blk file or the file
            }
        } else {
            // This block file has been indexed before, so skip
        }

        self.db.set_file_indexed(block_file_index as u32)?;

        let new_records_len = if let Some(ref records) = records {
            records.len()
        } else {
            0
        };

        let total = self
            .total_addresses
            .fetch_add(new_records_len as u64, Ordering::SeqCst)
            + new_records_len as u64;
        self.output.set_load_message(&format!(
            "Indexed address file {}, new addresses: {}, total addresses (estimated): {}",
            block_file_index,
            new_records_len,
            total
        ));
        self.output.update_load_current_count(
            complete_count.load(std::sync::atomic::Ordering::Relaxed) as u64,
        );

        Ok(())
    }

    fn on_index_complete(&self) -> Result<(), String> {
        self.db.flush()?;

        self.block_cache.lock().unwrap().generate_sort_blocks()?;
        let latest_height = self.block_cache.lock().unwrap().get_latest_block_height();
        self.db.set_indexed_block_height(latest_height as u32)?;

        self.db.set_all_file_indexed()?;

        let count = self.total_addresses.load(Ordering::SeqCst);
        self.output.set_load_message(&format!(
            "Address index build complete, total addresses {}",
            count
        ));
        self.output.finish_load();
        self.output.println(&format!(
            "Address index build complete, total addresses (estimated) {}",
            count
        ));

        Ok(())
    }

    fn should_stop(&self) -> bool {
        self.should_stop.load(Ordering::Relaxed)
    }
}
