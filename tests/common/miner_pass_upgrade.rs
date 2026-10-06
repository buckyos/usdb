//! Paired-store upgrade acceptance fixtures; all roots and processes belong to the test.
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bitcoincore_rpc::bitcoin::OutPoint;
use ord::InscriptionId;
use serde_json::{Value, json};
use usdb_util::VersionFamily;

use crate::index::MinerPassKind;
use crate::index::indexer::publication_faults::{CRASH_EXIT_CODE, FaultPoint};
use crate::index::test_miner_evidence as chain;
use crate::index::test_miner_pipeline::{Pipeline, append_transaction, cold_recipient};
use crate::index::test_miner_rules::{
    CONFORMANCE_ENERGY_DOUBLE, CONFORMANCE_ENERGY_TRIPLE, conformance_catalog,
};
use crate::index::test_miner_state::{MintBlock, MintSpec, SpendKind, source_script};
use crate::service::rpc::{GetStateRefAtHeightParams, UsdbIndexerRpc};

pub fn catalog() -> String {
    conformance_catalog(&[
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            CONFORMANCE_ENERGY_TRIPLE,
        ),
    ])
}

/// The forks agree through 8. Both cross two formula boundaries, remint, inherit,
/// and spend an old collab sat after H2 to expose stale transfer trackers on recovery.
pub struct Scenario {
    blocks: Vec<MintBlock>,
    pub ids: Vec<InscriptionId>,
}
impl Scenario {
    pub fn new(fork: bool) -> Self {
        let leader = MintBlock::new(
            8,
            vec![MintSpec::standard(
                90,
                source_script(SpendKind::Witness),
                1,
                vec![],
            )],
            false,
        );
        let mut spec =
            MintSpec::standard(if fork { 78 } else { 79 }, cold_recipient(79), 0, vec![]);
        spec.source = SpendKind::Legacy;
        spec.kind = MinerPassKind::Collab;
        spec.leader = Some(leader.mints[0].inscription_id);
        let collab = MintBlock::new(10, vec![spec], false);
        let middle = MintBlock::new(
            12,
            vec![MintSpec::standard(
                91,
                source_script(SpendKind::Witness),
                1,
                vec![],
            )],
            false,
        );
        let inherited = MintBlock::new(
            20,
            vec![MintSpec::standard(
                if fork { 92 } else { 93 },
                cold_recipient(92),
                0,
                vec![
                    leader.mints[0].inscription_id,
                    middle.mints[0].inscription_id,
                ],
            )],
            false,
        );
        let ids = vec![
            leader.mints[0].inscription_id,
            collab.mints[0].inscription_id,
            middle.mints[0].inscription_id,
            inherited.mints[0].inscription_id,
        ];
        let mut spend = MintBlock::new(21, vec![], false);
        let prevout = OutPoint::new(ids[1].txid, 0);
        let txout = collab.core.state.lock().unwrap().blocks[&10].0.txdata[1].output[0].clone();
        let tx = chain::transaction(vec![prevout], vec![chain::output(4500, cold_recipient(80))]);
        append_transaction(
            &mut spend,
            21,
            tx,
            HashMap::from([(
                prevout,
                usdb_util::SpentPrevout {
                    txout,
                    height: 10,
                    coinbase: false,
                },
            )]),
        );
        let mut blocks = vec![leader, collab, middle, inherited, spend];
        for height in 9..=22 {
            if ![10, 12, 20, 21].contains(&height) {
                blocks.push(MintBlock::new(height, vec![], false));
            }
        }
        Self { blocks, ids }
    }
    pub async fn pipeline(&self, name: &str) -> Pipeline {
        Pipeline::with_catalog(name, &self.blocks.iter().collect::<Vec<_>>(), 8, &catalog()).await
    }
}

/// Compare canonical state, not SQLite page layout, timestamps or local history row IDs.
/// Include exact sparse energy records as well as every historical projected result.
pub async fn fingerprint(p: &Pipeline, ids: &[InscriptionId], tip: u32) -> Value {
    let store = p.indexer.miner_pass_storage();
    let energy = p.indexer.pass_energy_manager();
    let mut passes = Vec::new();
    for id in ids {
        let pass = store.get_pass_by_inscription_id(id).unwrap().map(|r| json!({
            "id":r.inscription_id.to_string(), "number":r.inscription_number,
            "mint_txid":r.mint_txid.to_string(), "mint_height":r.mint_block_height,
            "mint_owner":r.mint_owner.to_string(), "owner":r.owner.to_string(), "state":r.state.as_str(),
            "satpoint":r.satpoint.to_string(), "version":r.mint_version, "kind":r.pass_kind.as_str(),
            "main":r.usdb_main, "leader":r.leader_pass_id.map(|v|v.to_string()), "leader_addr":r.leader_btc_addr,
            "prev":r.prev.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "invalid_code":r.invalid_code,"invalid_reason":r.invalid_reason,
        }));
        let history = store
            .get_pass_history_by_page_in_height_range(id, 0, tip, 0, 1000, false)
            .unwrap()
            .into_iter()
            .map(|r| {
                json!([
                    r.block_height,
                    r.event_type,
                    r.state.as_str(),
                    r.owner.to_string(),
                    r.satpoint.to_string()
                ])
            })
            .collect::<Vec<_>>();
        let mut ledger = Vec::new();
        for h in 8..=tip {
            let exact = energy
                .get_pass_energy_record_exact(id, h)
                .unwrap()
                .map(|r| {
                    json!([
                        r.block_height,
                        r.state.as_str(),
                        r.active_block_height,
                        r.owner_address.to_string(),
                        r.owner_balance,
                        r.owner_delta,
                        r.energy.to_string()
                    ])
                });
            let projected = energy
                .get_pass_energy(id, h)
                .await
                .unwrap()
                .map(|r| json!([r.state.as_str(), r.energy.to_string()]));
            ledger.push(json!([h, exact, projected]));
        }
        passes.push(json!({"pass":pass,"audit":store.get_mint_audit(id).unwrap(),"history":history,"energy":ledger}));
    }
    let mut blocks = Vec::new();
    for h in 8..=tip {
        blocks.push(json!({
            "height":h,"pass_commit":format!("{:?}",store.get_pass_block_commit(h).unwrap()),
            "balances":format!("{:?}",store.get_active_balance_snapshot(h).unwrap()),
            "anchor":format!("{:?}",store.get_balance_history_snapshot_anchor_at_height(h).unwrap()),
            "state_ref":p.rpc().get_state_ref_at_height(GetStateRefAtHeightParams { block_height:h, context:None }).unwrap(),
        }));
    }
    json!({"passes":passes,"blocks":blocks,"pass_height":store.get_committed_synced_btc_block_height().unwrap(),
        "energy_height":energy.get_synced_block_height_for_test().unwrap(),"pending":energy.get_pending_block_height_for_test().unwrap()})
}

/// Copy only isolated test data while the node is stopped; symlinks are never followed.
pub fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// Exit without unwinding the actual test indexer inside a bounded child process.
/// Upstream RPC servers stay alive in the parent while both parent stores are closed.
pub fn crash_child(root: &Path, height: u32, point: FaultPoint, reorg: bool) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "index::miner_pass_upgrade_recovery::upgrade_crash_child",
            "--ignored",
        ])
        .env("USDB_UPGRADE_TEST_ROOT", root)
        .env("USDB_UPGRADE_TEST_HEIGHT", height.to_string())
        .env("USDB_UPGRADE_TEST_POINT", format!("{point:?}"))
        .env("USDB_UPGRADE_TEST_REORG", if reorg { "yes" } else { "no" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!timed_out, "Crash child timed out: {point:?}");
    assert_eq!(
        output.status.code(),
        Some(CRASH_EXIT_CODE),
        "{point:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
