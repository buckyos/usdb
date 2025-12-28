use crate::btc::{BlockFileIndexer, BlockFileIndexerCallback, BlockFileReader};
use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDB, AddressDBRef};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::{Block, ScriptBuf, ScriptHash};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AddressIndexer {
    config: BalanceHistoryConfigRef,
    db: AddressDBRef,
    output: IndexOutputRef,
    all: Arc<Mutex<HashSet<ScriptHash>>>,
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

        Ok(Self {
            db: Arc::new(db),
            config,
            output,
            all: Arc::new(Mutex::new(HashSet::new())),
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
                if self.all.lock().unwrap().insert(script_hash) == false {
                    continue;
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
        //self.db.put_addresses(&records)?;

        let total = self.all.lock().unwrap().len();
        self.output
            .set_load_message(&format!("Indexed address file {}, new addresses: {}, total addresses: {}", block_file_index, records.len(), total));
        self.output.update_load_current_count(complete_count as u64);

        Ok(())
    }

    fn on_index_complete(&self) -> Result<(), String> {
        let count = self.all.lock().unwrap().len();
        self.output.set_load_message(&format!("Address index build complete, total addresses {}", count));
        self.output.finish_load();
        self.output.println(&format!("Address index build complete, total addresses {}", count));
        Ok(())
    }

    fn should_stop(&self) -> bool {
        self.should_stop.load(Ordering::Relaxed)
    }
}
