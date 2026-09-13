//! Shared real-Core fixture and isolated database helpers for bootstrap integration tests.

use super::*;
use crate::CoreSnapshotMeta;
use crate::bootstrap::{NativeBootstrapConfig, NativeBootstrapIdentity};
use crate::btc::{BTCClient, BTCClientType};
use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint, ScriptBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use usdb_util::{ToBtcScriptHash, UTXOEntry};

pub struct Workspace(pub PathBuf);

impl Workspace {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "assumeutxo-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Clone)]
pub struct Fixture {
    pub blocks: Vec<Block>,
    pub fallback_calls: Arc<AtomicUsize>,
    pub fixture_dir: PathBuf,
    /// Optional advertised tip for stable-lag tests; only the retained real blocks may be fetched.
    pub advertised_tip: Option<u32>,
}

impl Fixture {
    /// Build an isolated native bootstrap configuration using this fixture's reviewed checkpoint.
    pub fn native_config(&self, work: &Path, origin: u32) -> Arc<BalanceHistoryConfig> {
        let source = work.join("source.dat");
        fs::copy(self.fixture_dir.join("snapshot.dat"), &source).unwrap();
        let mut cfg = config(&work.join("native"), Network::Regtest);
        cfg.bootstrap = Some(NativeBootstrapConfig {
            snapshot_file: source,
            identity: NativeBootstrapIdentity {
                snapshot: serde_json::from_slice(
                    &fs::read(self.fixture_dir.join("identity.json")).unwrap(),
                )
                .unwrap(),
                regtest_checkpoint: Some(
                    serde_json::from_slice(
                        &fs::read(self.fixture_dir.join("checkpoint.json")).unwrap(),
                    )
                    .unwrap(),
                ),
                origin_height: origin,
                origin_block_hash: self.blocks[origin as usize].block_hash(),
            },
            import_batch_size: 31,
            replay_batch_size: 1,
        });
        Arc::new(cfg)
    }

    pub fn load() -> Self {
        Self::load_at(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo"),
            "blocks",
        )
    }

    pub fn load_at(fixture_dir: PathBuf, branch: &str) -> Self {
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture_dir.join("chain.json")).unwrap()).unwrap();
        let blocks = json[branch]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| {
                bitcoincore_rpc::bitcoin::consensus::encode::deserialize_hex(
                    value.as_str().unwrap(),
                )
                .unwrap()
            })
            .collect();
        Self {
            blocks,
            fallback_calls: Arc::new(AtomicUsize::new(0)),
            fixture_dir,
            advertised_tip: None,
        }
    }

    pub fn client(&self) -> BTCClientRef {
        Arc::new(Box::new(self.clone()))
    }

    pub fn client_with_stable_tip(&self) -> BTCClientRef {
        let mut chain = self.clone();
        chain.advertised_tip = Some(
            self.blocks.len() as u32 - 1
                + usdb_util::embedded_btc_stable_lag_blocks(Network::Regtest).unwrap(),
        );
        Arc::new(Box::new(chain))
    }

    pub fn options(&self, work: &Path) -> AssumeUtxoImportOptions {
        let path = &self.fixture_dir;
        let reference = work.join("reference.db");
        self.reference(&work.join("full-replay"), &reference);
        AssumeUtxoImportOptions {
            snapshot: path.join("snapshot.dat"),
            identity: serde_json::from_slice(&fs::read(path.join("identity.json")).unwrap())
                .unwrap(),
            reference_sha256: format::hex(&Sha256::digest(fs::read(&reference).unwrap())),
            reference_core: reference,
        }
    }

    pub fn reference(&self, root: &Path, output: &Path) {
        let tip = self.blocks.len() as u32 - 1;
        let cfg = Arc::new(config(root, Network::Regtest));
        let db =
            Arc::new(BalanceHistoryDB::open(cfg.clone(), BalanceHistoryDBMode::Normal).unwrap());
        let processor = BatchBlockProcessor::new(
            self.client(),
            db.clone(),
            Arc::new(UTXOCache::new(cfg.clone(), CacheStrategy::Normal)),
            Arc::new(AddressBalanceCache::new(cfg, CacheStrategy::Normal)),
        );
        processor.process_blocks(1..tip + 1, tip, 288).unwrap();
        let mut reference = CoreSnapshotDb::create(output).unwrap();
        db.traverse_latest(None, 100, |rows| {
            reference.put_balance_history_entries(rows)
        })
        .unwrap();
        // Derive live outpoint membership independently from all real blocks, then read indexed values.
        let mut live = HashMap::new();
        for block in &self.blocks[1..] {
            for tx in &block.txdata {
                if !tx.is_coinbase() {
                    for input in &tx.input {
                        live.remove(&input.previous_output).unwrap();
                    }
                }
                for (vout, output) in tx.output.iter().enumerate() {
                    if output.script_pubkey.as_bytes().first() == Some(&0x6a)
                        || output.script_pubkey.len() > 10_000
                    {
                        continue;
                    }
                    live.insert(
                        OutPoint {
                            txid: tx.compute_txid(),
                            vout: vout as u32,
                        },
                        output.clone(),
                    );
                }
            }
        }
        let outputs: Vec<_> = live
            .into_iter()
            .map(|(outpoint, output)| {
                let indexed = db.get_utxo(&outpoint).unwrap().unwrap();
                assert_eq!(indexed.value, output.value.to_sat());
                assert_eq!(
                    indexed.script_hash,
                    output.script_pubkey.to_btc_script_hash()
                );
                UTXOEntry {
                    outpoint,
                    script_hash: indexed.script_hash,
                    value: indexed.value,
                }
            })
            .collect();
        reference.put_utxo_entries(&outputs).unwrap();
        reference
            .put_block_commit_entries(
                &(1..tip + 1)
                    .map(|height| db.get_block_commit(height).unwrap().unwrap())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        reference
            .write_meta(&CoreSnapshotMeta {
                block_height: tip,
                balance_history_count: reference.balance_history_count().unwrap(),
                utxo_count: outputs.len() as u64,
                block_commit_count: tip as u64,
                generated_at: 0,
                db_identity: BalanceHistoryDBIdentity::for_network(Network::Regtest),
                core_snapshot_id: crate::service::build_historical_state_ref_at_height(
                    &config(root, Network::Regtest),
                    &db,
                    tip,
                )
                .unwrap()
                .unwrap()
                .snapshot_id,
            })
            .unwrap();
        reference.finalize_for_distribution().unwrap();
    }
}

#[async_trait::async_trait]
impl BTCClient for Fixture {
    fn get_type(&self) -> BTCClientType {
        BTCClientType::RPC
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
        Ok(self.advertised_tip.unwrap_or(self.blocks.len() as u32 - 1))
    }
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, String> {
        Ok(self.get_block_by_height(height)?.block_hash())
    }
    fn get_block_by_hash(&self, hash: &BlockHash) -> Result<Block, String> {
        self.blocks
            .iter()
            .find(|block| block.block_hash() == *hash)
            .cloned()
            .ok_or("Unknown fixture block".to_string())
    }
    fn get_block_by_height(&self, height: u32) -> Result<Block, String> {
        self.blocks
            .get(height as usize)
            .cloned()
            .ok_or("Unknown fixture height".to_string())
    }
    async fn get_blocks(&self, start: u32, end: u32) -> Result<Vec<Block>, String> {
        (start..=end).map(|h| self.get_block_by_height(h)).collect()
    }
    fn get_utxo(&self, _: &OutPoint) -> Result<(ScriptBuf, Amount), String> {
        self.fallback_calls.fetch_add(1, Ordering::SeqCst);
        Err("Historical RPC fallback must never be called by these tests".to_string())
    }
}
