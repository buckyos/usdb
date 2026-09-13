//! Deterministic blk-file writes and an RPC fixture whose active height can advance during a test.

use super::*;
use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, ScriptBuf, consensus};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Encode Core's magic/length/payload framing, before applying file-position-dependent XOR.
pub fn frame(magic: u32, block: &Block) -> Vec<u8> {
    let bytes = consensus::serialize(block);
    let mut result = magic.to_le_bytes().to_vec();
    result.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    result.extend(bytes);
    result
}

/// Write complete or partial plaintext frames followed by raw preallocated zero padding.
pub fn write_file(path: &Path, plain: &[u8], xor: [u8; 8], allocated: usize) {
    let mut bytes: Vec<_> = plain
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ xor[i % 8])
        .collect();
    bytes.resize(allocated.max(bytes.len()), 0);
    fs::write(path, bytes).unwrap();
}

/// A changing active tip; historical block bodies at/before the snapshot are deliberately absent.
#[derive(Clone)]
pub struct GrowingChain {
    pub chain: Fixture,
    pub tip: Arc<AtomicU32>,
    pub payload_calls: Arc<AtomicUsize>,
}

impl GrowingChain {
    pub fn new(chain: Fixture, tip: u32) -> Self {
        Self {
            chain,
            tip: Arc::new(AtomicU32::new(tip)),
            payload_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn client(&self) -> BTCClientRef {
        Arc::new(Box::new(self.clone()))
    }
}

#[async_trait::async_trait]
impl crate::btc::BTCClient for GrowingChain {
    fn get_type(&self) -> crate::btc::BTCClientType {
        crate::btc::BTCClientType::RPC
    }
    fn init(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    fn on_sync_complete(&self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn get_latest_block_height(&self) -> Result<u32, String> {
        Ok(self.tip.load(Ordering::Relaxed))
    }
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, String> {
        if height > self.get_latest_block_height()? {
            return Err("Fixture height has not arrived".into());
        }
        self.chain.get_block_hash(height)
    }
    fn get_block_by_hash(&self, hash: &BlockHash) -> Result<Block, String> {
        let height = self
            .chain
            .blocks
            .iter()
            .position(|b| b.block_hash() == *hash)
            .ok_or("Unknown fixture hash")?;
        self.get_block_by_height(height as u32)
    }
    fn get_block_by_height(&self, height: u32) -> Result<Block, String> {
        if height <= 101 {
            return Err("Pre-snapshot block bodies are unavailable".into());
        }
        if height > self.get_latest_block_height()? {
            return Err("Fixture block has not arrived".into());
        }
        self.payload_calls.fetch_add(1, Ordering::Relaxed);
        self.chain.get_block_by_height(height)
    }
    async fn get_blocks(&self, start: u32, end: u32) -> Result<Vec<Block>, String> {
        (start..=end).map(|h| self.get_block_by_height(h)).collect()
    }
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        self.chain.get_utxo(outpoint)
    }
}
