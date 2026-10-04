//! Real SQLite/RocksDB state harness and signed chain fixtures for UIP-0016 transitions.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use balance_history::AddressBalance;
use bitcoincore_rpc::bitcoin::{
    OutPoint, ScriptBuf, Txid,
    hashes::Hash,
    opcodes::all::{OP_ENDIF, OP_IF},
    script::{Builder, PushBytesBuf},
};
use ord::InscriptionId;
use ordinals::SatPoint;
use usdb_util::{BtcScriptHash, SpentPrevout, ToBtcScriptHash};

use crate::btc::{
    mint_evidence::{MintEvidenceContext, MintSatOutcome},
    transaction_balance::{BalanceBaseline, BalanceBaselineSide, TransactionBalanceContext},
};
use crate::index::energy::{BalanceProvider, PassEnergyManager};
use crate::index::pass::{MinerPassManager, PassMintInscriptionInfo};
use crate::index::test_miner_evidence as chain;
use crate::index::{MinerPassKind, MinerPassState, PassBlockMutationCollector};
use crate::storage::{MinePassStorageSavePointGuard, MinerPassStorage, PassEnergyStorage};

pub use chain::{SpendKind, source_script};

pub fn id(tag: u8) -> InscriptionId {
    InscriptionId {
        txid: Txid::from_byte_array([tag; 32]),
        index: 0,
    }
}
pub fn recipient(tag: u8) -> ScriptBuf {
    ScriptBuf::from(vec![0x51, tag])
}
pub fn satpoint(tag: u8) -> SatPoint {
    SatPoint {
        outpoint: OutPoint::new(id(tag).txid, 0),
        offset: 0,
    }
}

#[derive(Default)]
pub struct Timeline {
    balances: Mutex<HashMap<BtcScriptHash, BTreeMap<u32, u64>>>,
    pub fail: Mutex<Option<(BtcScriptHash, u32)>>,
}

impl Timeline {
    pub fn set(&self, owner: BtcScriptHash, height: u32, balance: u64) {
        self.balances
            .lock()
            .unwrap()
            .entry(owner)
            .or_default()
            .insert(height, balance);
    }
    fn records(&self, owner: BtcScriptHash, range: Range<u32>) -> Vec<AddressBalance> {
        let maps = self.balances.lock().unwrap();
        let Some(values) = maps.get(&owner) else {
            return Vec::new();
        };
        values
            .range(range)
            .map(|(&height, &balance)| {
                let old = values
                    .range(..height)
                    .next_back()
                    .map(|(_, v)| *v)
                    .unwrap_or(0);
                AddressBalance {
                    block_height: height,
                    balance,
                    delta: balance as i64 - old as i64,
                }
            })
            .collect()
    }
}

impl BalanceProvider for Timeline {
    fn get_balance_at_height<'a>(
        &'a self,
        owner: BtcScriptHash,
        height: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move {
            if *self.fail.lock().unwrap() == Some((owner, height)) {
                return Err("Injected balance read failure".into());
            }
            Ok(self
                .records(owner, 0..height + 1)
                .into_iter()
                .rev()
                .take(1)
                .collect())
        })
    }
    fn get_balance_at_range<'a>(
        &'a self,
        owner: BtcScriptHash,
        range: Range<u32>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
        Box::pin(async move { Ok(self.records(owner, range)) })
    }
}

pub struct Harness {
    pub root: PathBuf,
    pub data: PathBuf,
    pub storage: Arc<MinerPassStorage>,
    pub energy: Arc<PassEnergyManager>,
    pub manager: MinerPassManager,
    pub timeline: Arc<Timeline>,
}

impl Harness {
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "usdb-mint-v2-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Self::open(root, Arc::new(Timeline::default()))
    }
    pub fn open(root: PathBuf, timeline: Arc<Timeline>) -> Self {
        let config = Arc::new(crate::test_config::load(Some(root.clone())).unwrap());
        let data = config.data_dir();
        let storage = Arc::new(MinerPassStorage::new(&data).unwrap());
        let energy = Arc::new(PassEnergyManager::new_with_deps(
            config.clone(),
            PassEnergyStorage::new(&data).unwrap(),
            timeline.clone(),
        ));
        energy
            .reconcile_with_pass_synced_height(
                storage.get_synced_btc_block_height().unwrap().unwrap_or(0),
            )
            .unwrap();
        let manager = MinerPassManager::new(config, storage.clone(), energy.clone()).unwrap();
        Self {
            root,
            data,
            storage,
            energy,
            manager,
            timeline,
        }
    }
    pub fn begin(&self, height: u32) -> MinePassStorageSavePointGuard<'_> {
        let guard = MinePassStorageSavePointGuard::new(&self.storage).unwrap();
        self.energy.begin_block_sync(height).unwrap();
        self.manager
            .begin_block_mutation_collection(height)
            .unwrap();
        guard
    }
    pub fn finish(
        &self,
        height: u32,
        guard: MinePassStorageSavePointGuard<'_>,
    ) -> PassBlockMutationCollector {
        self.energy.finalize_block_sync(height).unwrap();
        let collector = self.manager.take_block_mutation_collector(height).unwrap();
        self.storage.update_synced_btc_block_height(height).unwrap();
        guard.commit().unwrap();
        collector
    }
    pub fn abort(&self, height: u32, guard: MinePassStorageSavePointGuard<'_>) {
        self.energy.abort_pending_block_sync(height).unwrap();
        self.manager.clear_block_mutation_collection();
        drop(guard);
    }
    /// Prepopulate already-validated history for transition tests; no inscription is executed here.
    pub async fn seed(&self, tag: u8, owner: BtcScriptHash, height: u32, prev: Vec<InscriptionId>) {
        let guard = self.begin(height);
        if let Some(old) = self
            .storage
            .get_last_active_mint_pass_by_owner(&owner)
            .unwrap()
        {
            self.energy
                .on_pass_dormant(&old.inscription_id, height)
                .await
                .unwrap();
            self.storage
                .update_state_at_height(
                    &old.inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    height,
                )
                .unwrap();
        }
        for previous in &prev {
            self.storage
                .update_state_at_height(
                    previous,
                    MinerPassState::Consumed,
                    MinerPassState::Dormant,
                    height,
                )
                .unwrap();
            self.energy
                .on_pass_consumed(previous, &owner, height)
                .unwrap();
        }
        let pass = crate::storage::MinerPassInfo {
            inscription_id: id(tag),
            inscription_number: i32::from(tag),
            mint_txid: id(tag).txid,
            mint_block_height: height,
            mint_owner: owner,
            owner,
            satpoint: satpoint(tag),
            mint_version: usdb_util::MINER_PASS_MINT_SCHEMA_VERSION,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "0x1111111111111111111111111111111111111111".into(),
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev,
            state: MinerPassState::Active,
            invalid_code: None,
            invalid_reason: None,
        };
        self.storage
            .add_new_mint_pass_at_height(&pass, height)
            .unwrap();
        self.energy
            .on_new_pass(&id(tag), &owner, height, 0)
            .await
            .unwrap();
        self.finish(height, guard);
    }
    pub fn state(&self, pass: &InscriptionId) -> MinerPassState {
        self.storage
            .get_pass_by_inscription_id(pass)
            .unwrap()
            .unwrap()
            .state
    }
    pub fn has_history(&self, owner: BtcScriptHash, height: u32) -> bool {
        self.storage.has_ever_valid_owner(&owner, height).unwrap()
    }
    pub fn cleanup(self) {
        let root = self.root.clone();
        drop(self);
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub struct MintSpec {
    pub tag: u8,
    pub version: u32,
    pub source: SpendKind,
    pub dest: ScriptBuf,
    pub balance_before: u64,
    pub prev: Vec<InscriptionId>,
    pub kind: MinerPassKind,
    pub leader: Option<InscriptionId>,
    pub leader_addr: Option<String>,
    pub flag: u8,
    pub main: String,
}
impl MintSpec {
    pub fn standard(
        tag: u8,
        dest: ScriptBuf,
        balance_before: u64,
        prev: Vec<InscriptionId>,
    ) -> Self {
        Self {
            tag,
            version: usdb_util::MINER_PASS_MINT_SCHEMA_VERSION,
            source: SpendKind::Witness,
            dest,
            balance_before,
            prev,
            kind: MinerPassKind::Standard,
            leader: None,
            leader_addr: None,
            flag: 1,
            main: "0x1111111111111111111111111111111111111111".into(),
        }
    }
}

pub struct MintBlock {
    pub core: chain::ChainCore,
    pub evidence: MintEvidenceContext,
    pub balances: TransactionBalanceContext,
    pub mints: Vec<PassMintInscriptionInfo>,
}

impl MintBlock {
    /// Build historical signed commits and one or several reveal transactions at `height`.
    pub fn new(height: u32, specs: Vec<MintSpec>, same_tx: bool) -> Self {
        let mut commits = Vec::new();
        let mut reveals = Vec::new();
        let mut coins = HashMap::new();
        let mut baselines = HashMap::new();
        for spec in &specs {
            let mut payload = serde_json::json!({"p":"usdb","op":"mint","v":spec.version,
                "usdb_main":spec.main,
                "prev":spec.prev.iter().map(ToString::to_string).collect::<Vec<_>>()});
            if spec.kind == MinerPassKind::Collab {
                payload.as_object_mut().unwrap().remove("usdb_main");
                if let Some(addr) = &spec.leader_addr {
                    payload["leader_btc_addr"] = serde_json::json!(addr);
                } else {
                    payload["leader_pass_id"] =
                        serde_json::json!(spec.leader.map(|id| id.to_string()));
                }
            }
            let script = Builder::new()
                .push_int(0)
                .push_opcode(OP_IF)
                .push_slice(b"ord")
                .push_slice([1])
                .push_slice(b"application/json")
                .push_int(0)
                .push_slice(PushBytesBuf::try_from(serde_json::to_vec(&payload).unwrap()).unwrap())
                .push_opcode(OP_ENDIF)
                .push_int(1)
                .into_script();
            let (commit_script, witness) = chain::tap_script(script);
            let input = OutPoint::new(id(spec.tag).txid, 7);
            let source_output = chain::output(6000, source_script(spec.source));
            let mut commit =
                chain::transaction(vec![input], vec![chain::output(5000, commit_script)]);
            chain::sign(
                &mut commit,
                0,
                std::slice::from_ref(&source_output),
                spec.source,
                spec.flag,
                false,
            );
            let point = OutPoint::new(commit.compute_txid(), 0);
            coins.insert(
                input,
                SpentPrevout {
                    txout: source_output,
                    height: height - 2,
                    coinbase: false,
                },
            );
            coins.insert(
                point,
                SpentPrevout {
                    txout: commit.output[0].clone(),
                    height: height - 1,
                    coinbase: false,
                },
            );
            let mut reveal =
                chain::transaction(vec![point], vec![chain::output(4900, spec.dest.clone())]);
            reveal.input[0].witness = witness;
            commits.push(commit);
            reveals.push(reveal);
            if let Some(previous) =
                baselines.insert(spec.dest.to_btc_script_hash(), spec.balance_before)
            {
                assert_eq!(previous, spec.balance_before);
            }
        }
        if same_tx {
            let mut joined = chain::transaction(Vec::new(), Vec::new());
            for reveal in reveals {
                joined.input.extend(reveal.input);
                joined.output.extend(reveal.output);
            }
            reveals = vec![joined];
        }
        let commit_block = chain::block(commits);
        let mut block = chain::block(reveals);
        block.header.prev_blockhash = commit_block.block_hash();
        let core = chain::ChainCore::new(vec![
            (
                height - 1,
                commit_block.clone(),
                chain::verbose(height - 1, &commit_block, &coins),
            ),
            (
                height,
                block.clone(),
                chain::verbose(height, &block, &coins),
            ),
        ]);
        let inputs = core.client.get_block_prevouts(height, &block).unwrap();
        let balances = TransactionBalanceContext::from_baseline(
            &inputs,
            BalanceBaseline {
                side: BalanceBaselineSide::BeforeBlock,
                height: height - 1,
                block_hash: commit_block.block_hash(),
                balances: baselines,
            },
        )
        .unwrap();
        let evidence =
            MintEvidenceContext::new(core.client.clone(), height, Arc::new(block.clone()));
        let mints = specs
            .into_iter()
            .enumerate()
            .map(|(i, spec)| {
                let id = InscriptionId {
                    txid: block.txdata[if same_tx { 1 } else { i + 1 }].compute_txid(),
                    index: if same_tx { i as u32 } else { 0 },
                };
                let MintSatOutcome::Located(sat) = evidence.locate_mint(id).unwrap() else {
                    panic!("expected supported reveal")
                };
                PassMintInscriptionInfo {
                    inscription_id: id,
                    inscription_number: i32::from(spec.tag),
                    mint_txid: id.txid,
                    mint_block_height: height,
                    mint_owner: sat.mint_owner,
                    satpoint: sat.satpoint,
                    mint_version: spec.version,
                    pass_kind: spec.kind,
                    usdb_main: if spec.kind == MinerPassKind::Collab {
                        String::new()
                    } else {
                        spec.main
                    },
                    leader_pass_id: spec.leader,
                    leader_btc_addr: spec.leader_addr,
                    prev: spec.prev,
                }
            })
            .collect();
        Self {
            core,
            evidence,
            balances,
            mints,
        }
    }
}

/// Copy only a closed isolated fixture directory to exercise restoring both persisted stores.
pub fn copy_closed_fixture(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let dest = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_closed_fixture(&entry.path(), &dest)
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}
