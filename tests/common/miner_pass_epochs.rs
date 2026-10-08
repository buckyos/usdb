//! Three schema epochs, real state operations and immutable registry revisions.
use super::MinerPassKind;
use super::test_miner_evidence as chain;
use super::test_miner_pipeline::{Pipeline, append_transaction, cold_recipient};
use super::test_miner_rules::{
    CONFORMANCE_ENERGY_DOUBLE, CONFORMANCE_ENERGY_TRIPLE, CONFORMANCE_SCHEMA,
    CONFORMANCE_SCHEMA_STRUCTURED, CONFORMANCE_STATE, conformance_catalog,
};
use super::test_miner_state::{MintBlock, MintSpec, SpendKind, id, source_script};
use bitcoincore_rpc::bitcoin::OutPoint;
use ord::InscriptionId;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use usdb_util::VersionFamily;

/// Gap 0 activates all families together; gap 1 activates them at adjacent heights.
pub fn catalogs(gap: u32) -> [String; 3] {
    let first = [
        (
            VersionFamily::EnergyFormulaVersion,
            10,
            CONFORMANCE_ENERGY_DOUBLE,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            10 + gap,
            CONFORMANCE_SCHEMA,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            10 + 2 * gap,
            CONFORMANCE_STATE,
        ),
    ];
    let second = [
        (
            VersionFamily::EnergyFormulaVersion,
            20,
            CONFORMANCE_ENERGY_TRIPLE,
        ),
        (
            VersionFamily::InscriptionSchemaVersion,
            20 + gap,
            CONFORMANCE_SCHEMA_STRUCTURED,
        ),
        (
            VersionFamily::PassStateMachineVersion,
            20 + 2 * gap,
            usdb_util::PASS_STATE_MACHINE_VERSION_V2,
        ),
    ];
    let r1 = conformance_catalog(&[]);
    let r2 = append_revision(&r1, &conformance_catalog(&first));
    let r3 = append_revision(
        &r2,
        &conformance_catalog(&[first.as_slice(), second.as_slice()].concat()),
    );
    [r1, r2, r3]
}

/// Keep every prior immutable revision while appending the next complete schedule.
pub fn append_revision(previous: &str, next: &str) -> String {
    let previous: Value = serde_json::from_str(previous).unwrap();
    let mut next: Value = serde_json::from_str(next).unwrap();
    let mut revisions = previous["registries"].as_array().unwrap().clone();
    revisions.push(next["registries"][0].clone());
    next["registries"] = revisions.into();
    next.to_string()
}

/// A signed mint/transfer sequence with both old schemas inherited by schema 902.
pub struct Scenario {
    pub blocks: Vec<MintBlock>,
    pub ids: Vec<InscriptionId>,
    pub gap: u32,
}
impl Scenario {
    /// Change the middle commit on the alternate fork while preserving the prefix through H8.
    pub fn new(gap: u32, fork: bool) -> Self {
        assert!(gap <= 1);
        let source = source_script(SpendKind::Witness);
        let mut leader = MintBlock::new(
            8,
            vec![MintSpec::standard(130, source.clone(), 1, vec![])],
            false,
        );
        // Real upstream balance changes produce nonzero energy, including on replay.
        let coin = OutPoint::new(id(240).txid, 0);
        append_transaction(
            &mut leader,
            8,
            chain::transaction(vec![coin], vec![chain::output(400_000, source.clone())]),
            HashMap::from([(
                coin,
                usdb_util::SpentPrevout {
                    txout: chain::output(400_000, source_script(SpendKind::Legacy)),
                    height: 1,
                    coinbase: false,
                },
            )]),
        );
        let leader_id = leader.mints[0].inscription_id;
        let mut collab = MintSpec::standard(131, cold_recipient(131), 0, vec![]);
        collab.kind = MinerPassKind::Collab;
        collab.leader = Some(leader_id);
        let old_collab = MintBlock::new(9, vec![collab], false);
        let old_collab_id = old_collab.mints[0].inscription_id;
        let mut by_height = BTreeMap::<u32, Vec<MintSpec>>::new();
        let mut middle = MintSpec::standard(if fork { 132 } else { 133 }, source, 1, vec![]);
        middle.version = 901;
        by_height.entry(10 + gap).or_default().push(middle);
        let mut rejected = MintSpec::standard(134, cold_recipient(134), 0, vec![]);
        rejected.version = 901;
        rejected.kind = MinerPassKind::Collab;
        rejected.leader = Some(leader_id);
        by_height.entry(10 + 2 * gap).or_default().push(rejected);
        let mut blocks = vec![leader, old_collab];
        for (h, specs) in by_height {
            blocks.push(MintBlock::new(h, specs, false));
        }
        let middle_id = blocks[2].mints[0].inscription_id;
        let rejected_id = blocks.last().unwrap().mints.last().unwrap().inscription_id;
        let mut child = MintSpec::standard(135, cold_recipient(135), 0, vec![leader_id, middle_id]);
        child.version = 902;
        let mut new_collab = MintSpec::standard(136, cold_recipient(136), 0, vec![]);
        new_collab.version = 902;
        new_collab.kind = MinerPassKind::Collab;
        // Address binding can exist before the new leader is materialized in this block.
        new_collab.leader_addr = Some(
            bitcoincore_rpc::bitcoin::Address::from_script(
                &cold_recipient(135),
                bitcoincore_rpc::bitcoin::Network::Regtest,
            )
            .unwrap()
            .to_string(),
        );
        let mut second = BTreeMap::<u32, Vec<MintSpec>>::new();
        second.entry(20 + gap).or_default().push(child);
        second.entry(20 + 2 * gap).or_default().push(new_collab);
        let before = blocks.len();
        for (h, specs) in second {
            blocks.push(MintBlock::new(h, specs, false));
        }
        let child_id = blocks[before].mints[0].inscription_id;
        let new_collab_id = blocks.last().unwrap().mints.last().unwrap().inscription_id;
        let mut transfer = MintBlock::new(23, vec![], false);
        let coin = OutPoint::new(old_collab_id.txid, 0);
        let txout = blocks[1].core.state.lock().unwrap().blocks[&9].0.txdata[1].output[0].clone();
        append_transaction(
            &mut transfer,
            23,
            chain::transaction(vec![coin], vec![chain::output(4500, cold_recipient(137))]),
            HashMap::from([(
                coin,
                usdb_util::SpentPrevout {
                    txout,
                    height: 9,
                    coinbase: false,
                },
            )]),
        );
        blocks.push(transfer);
        for h in 10..=24 {
            if ![10 + gap, 10 + 2 * gap, 20 + gap, 20 + 2 * gap, 23].contains(&h) {
                blocks.push(MintBlock::new(h, vec![], false));
            }
        }
        Self {
            blocks,
            ids: vec![
                leader_id,
                old_collab_id,
                middle_id,
                rejected_id,
                child_id,
                new_collab_id,
            ],
            gap,
        }
    }
    /// Open isolated real stores using this chain and the chosen catalog revision.
    pub async fn pipeline(&self, name: &str, catalog: &str) -> Pipeline {
        Pipeline::with_catalog(name, &self.blocks.iter().collect::<Vec<_>>(), 8, catalog).await
    }
}
