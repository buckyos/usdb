use crate::btc::{BlockFileIndexer, BlockFileIndexerCallback, BlockFileReader};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDB, AddressDBRef};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::{Block, ScriptBuf, ScriptHash};
use bloomfilter::Bloom;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AddressIndexer {
    config: BalanceHistoryConfigRef,
    db: AddressDBRef,
    output: IndexOutputRef,
    filter: Arc<Mutex<Bloom<ScriptHash>>>,
    total_addresses: Arc<AtomicU64>,
    should_stop: Arc<AtomicBool>,
}

impl AddressIndexer {
    pub fn new(
        root_dir: &Path,
        config: BalanceHistoryConfigRef,
        output: IndexOutputRef,
    ) -> Result<Self, String> {
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

        Ok(Self {
            db: Arc::new(db),
            config,
            output,
            filter: Arc::new(Mutex::new(bloom)),
            total_addresses: Arc::new(AtomicU64::new(0)),
            should_stop: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn build_index(&self) -> Result<(), String> {
        let block_reader = Arc::new(BlockFileReader::new(
            self.config.btc.block_magic(),
            &self.config.btc.data_dir(),
        )?);

        let file_indexer = BlockFileIndexer::new(
            block_reader,
            Arc::new(Box::new(self.clone())
                as Box<
                    dyn BlockFileIndexerCallback<Vec<(ScriptHash, ScriptBuf)>>,
                >),
        );

        self.filter.lock().unwrap().clear();
        self.total_addresses.store(0, Ordering::SeqCst);

        let file_indexer = Arc::new(file_indexer);
        file_indexer.build_index()?;

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.should_stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.output.println("Stopping AddressBuilder...");

        Ok(())
    }
}

impl BlockFileIndexerCallback<Vec<(ScriptHash, ScriptBuf)>> for AddressIndexer {
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
        _block_file_index: usize,
    ) -> Result<Vec<(ScriptHash, ScriptBuf)>, String> {
        Ok(Vec::new())
    }

    fn on_block_indexed(
        &self,
        records: &mut Vec<(ScriptHash, ScriptBuf)>,
        _block_file_index: usize,
        _block_file_offset: usize,
        _block_record_index: usize,
        block: &Block,
    ) -> Result<(), String> {
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

        Ok(())
    }

    fn on_file_indexed(
        &self,
        block_file_index: usize,
        complete_count: usize,
        records: Vec<(ScriptHash, ScriptBuf)>,
    ) -> Result<(), String> {
        self.db.put_addresses(&records)?;

        let total = self
            .total_addresses
            .fetch_add(records.len() as u64, Ordering::SeqCst)
            + records.len() as u64;

        self.output.set_load_message(&format!(
            "Indexed address file {}, new addresses: {}, total addresses (estimated): {}",
            block_file_index,
            records.len(),
            total
        ));
        self.output.update_load_current_count(complete_count as u64);

        Ok(())
    }

    fn on_index_complete(&self) -> Result<(), String> {
        self.db.flush()?;
        
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
