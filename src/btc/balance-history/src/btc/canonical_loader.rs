//! Optional local payload acceleration with canonical RPC ordering and automatic RPC fallback.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, ScriptBuf};

use super::canonical_index::{CanonicalBlockIndex, validate_payload};
use super::{BTCClient, BTCClientRef, BTCClientType};
use crate::config::BalanceHistoryConfigRef;

/// Observable counters for this client's optional local acceleration, excluding Core background IBD.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CanonicalLoaderStats {
    /// Successfully consumed, fully checked local block payloads.
    pub local_blocks: u64,
    /// Block payloads obtained through RPC, including near-tip requests and local misses.
    pub rpc_blocks: u64,
    /// Candidate physical records visited, including retried file tails.
    pub indexed_records: u64,
}

/// A BTC client whose height/hash authority is always RPC; blk files only accelerate payload reads.
/// Construction and near-tip use do not require local block files or a completed historical index.
pub struct CanonicalBlockLoader {
    rpc: BTCClientRef,
    data_dir: PathBuf,
    cache_dir: PathBuf,
    magic: u32,
    threshold: u32,
    max_height: u32,
    stable_lag: u32,
    index: Mutex<Option<CanonicalBlockIndex>>,
    stopped: AtomicBool,
    local_blocks: AtomicU64,
    rpc_blocks: AtomicU64,
    indexed_records: AtomicU64,
    local_active: AtomicBool,
}

impl CanonicalBlockLoader {
    /// Build a lazy loader bound to the configured node and a separate derived-index directory.
    pub fn new(rpc: BTCClientRef, config: &BalanceHistoryConfigRef) -> Result<Self, String> {
        let stable_lag = usdb_util::embedded_btc_stable_lag_blocks(config.btc.network())
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rpc,
            data_dir: config.btc.data_dir(),
            cache_dir: config.root_dir.join("local-block-index"),
            magic: config.btc.block_magic(),
            threshold: config.sync.local_loader_threshold as u32,
            max_height: config.sync.max_sync_block_height,
            stable_lag,
            index: Mutex::new(None),
            stopped: AtomicBool::new(false),
            local_active: AtomicBool::new(false),
            local_blocks: AtomicU64::new(0),
            rpc_blocks: AtomicU64::new(0),
            indexed_records: AtomicU64::new(0),
        })
    }

    /// Read counters without scanning files or changing the current source selection.
    pub fn stats(&self) -> CanonicalLoaderStats {
        CanonicalLoaderStats {
            local_blocks: self.local_blocks.load(Ordering::Relaxed),
            rpc_blocks: self.rpc_blocks.load(Ordering::Relaxed),
            indexed_records: self.indexed_records.load(Ordering::Relaxed),
        }
    }

    fn use_local(&self, start: u32) -> Result<bool, String> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err("Canonical block loader stopped".to_string());
        }
        let target = self.max_height.min(
            self.rpc
                .get_latest_block_height()?
                .saturating_sub(self.stable_lag),
        );
        let behind = target.saturating_sub(start.saturating_sub(1));
        let enabled = behind > self.threshold;
        if self.local_active.swap(enabled, Ordering::Relaxed) != enabled {
            log::info!(
                "Block payload source changed: module=canonical_local_loader, local_enabled={enabled}, blocks_behind={behind}, threshold={}",
                self.threshold
            );
        }
        Ok(enabled)
    }

    fn refresh(&self) -> bool {
        let begin = Instant::now();
        let result = (|| {
            let mut index = self.index.lock().unwrap();
            if index.is_none() {
                *index = Some(CanonicalBlockIndex::open(
                    &self.data_dir,
                    &self.cache_dir,
                    self.magic,
                )?);
            }
            index
                .as_ref()
                .unwrap()
                .refresh(&|| self.stopped.load(Ordering::Relaxed))
        })();
        match result {
            Ok(count) => {
                self.indexed_records
                    .fetch_add(count as u64, Ordering::Relaxed);
                if count > 0 {
                    log::info!(
                        "Local block index progressed: records={count}, elapsed_ms={}",
                        begin.elapsed().as_millis()
                    );
                }
                true
            }
            Err(error) => {
                log::warn!(
                    "Local block acceleration unavailable; using RPC: data_dir={}, error={error}",
                    self.data_dir.display()
                );
                self.index.lock().unwrap().take();
                false
            }
        }
    }

    fn payload(&self, hash: &BlockHash) -> Result<Block, String> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err("Canonical block loader stopped".to_string());
        }
        if let Some(index) = self.index.lock().unwrap().as_ref() {
            match index.block(hash) {
                Ok(Some(block)) => {
                    self.local_blocks.fetch_add(1, Ordering::Relaxed);
                    return Ok(block);
                }
                Ok(None) => log::debug!("Local block missing; using RPC: hash={hash}"),
                Err(error) => log::warn!(
                    "Local block candidate rejected; using RPC: hash={hash}, error={error}"
                ),
            }
        }
        let block = self.rpc.get_block_by_hash(hash)?;
        validate_payload(&block, hash)?;
        self.rpc_blocks.fetch_add(1, Ordering::Relaxed);
        Ok(block)
    }

    // Canonical hashes, parent continuity and a final tip-hash check prevent stale fork locations
    // or a reorg during retrieval from silently defining the block batch's logical order.
    fn local_range(&self, start: u32, end: u32) -> Result<Vec<Block>, String> {
        let mut blocks = Vec::new();
        let mut parent = start
            .checked_sub(1)
            .map(|h| self.rpc.get_block_hash(h))
            .transpose()?;
        for height in start..=end {
            let hash = self.rpc.get_block_hash(height)?;
            let block = self.payload(&hash)?;
            if parent.is_some_and(|p| block.header.prev_blockhash != p) {
                return Err(format!(
                    "Canonical local block batch parent changed at height {height}"
                ));
            }
            parent = Some(hash);
            blocks.push(block);
        }
        if parent != Some(self.rpc.get_block_hash(end)?) {
            return Err("Canonical local block batch changed during retrieval".to_string());
        }
        log::info!(
            "Block payload batch completed: module=canonical_local_loader, start={start}, end={end}, totals={:?}",
            self.stats()
        );
        Ok(blocks)
    }
}

/// Build the adaptive native client; no local scan runs until an actual backlog exceeds threshold.
pub fn create_canonical_btc_client(
    rpc: BTCClientRef,
    config: &BalanceHistoryConfigRef,
) -> Result<BTCClientRef, String> {
    Ok(Arc::new(Box::new(CanonicalBlockLoader::new(rpc, config)?)))
}

#[async_trait::async_trait]
impl BTCClient for CanonicalBlockLoader {
    fn get_type(&self) -> BTCClientType {
        BTCClientType::LocalLoader
    }
    fn init(&self) -> Result<(), String> {
        self.rpc.init()
    }
    fn stop(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn on_sync_complete(&self, _height: u32) -> Result<(), String> {
        log::info!(
            "Canonical block loader sync completed: totals={:?}",
            self.stats()
        );
        Ok(())
    }
    fn get_latest_block_height(&self) -> Result<u32, String> {
        self.rpc.get_latest_block_height()
    }
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, String> {
        self.rpc.get_block_hash(height)
    }
    fn get_block_by_hash(&self, hash: &BlockHash) -> Result<Block, String> {
        self.payload(hash)
    }
    fn get_block_by_height(&self, height: u32) -> Result<Block, String> {
        if self.use_local(height)? && self.refresh() {
            return self.local_range(height, height).map(|mut b| b.remove(0));
        }
        self.rpc_blocks.fetch_add(1, Ordering::Relaxed);
        self.rpc.get_block_by_height(height)
    }
    async fn get_blocks(&self, start: u32, end: u32) -> Result<Vec<Block>, String> {
        if start > end {
            return Err("Invalid canonical block range".to_string());
        }
        if self.use_local(start)? && self.refresh() {
            return self.local_range(start, end);
        }
        let blocks = self.rpc.get_blocks(start, end).await?;
        self.rpc_blocks
            .fetch_add(blocks.len() as u64, Ordering::Relaxed);
        Ok(blocks)
    }
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        self.rpc.get_utxo(outpoint)
    }
}
