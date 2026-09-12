// Cross-crate P5 acceptance: real BH outputs feed production pass commitments, storage and RPC.
// The mutation stream is deterministic test input, not an end-to-end Ord replay.
use super::*;
use crate::index::{PassBlockMutation, PassBlockMutationCollector};

fn downstream_chain(
    source: &str,
    corrupt_commit: bool,
    corrupt_balance: bool,
) -> Vec<serde_json::Value> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/assumeutxo-p5/downstream-inputs.json"
    )))
    .unwrap();
    let (server, root) = build_server_with_genesis_and_network(
        "assumeutxo_p5_downstream",
        101,
        100,
        Network::Regtest,
    );
    let storage = server.indexer.miner_pass_storage();
    let mut previous = None;
    let mut results = Vec::new();
    for row in fixture["blocks"].as_array().unwrap() {
        let input = &row[source];
        let state: balance_history::HistoricalSnapshotStateRef =
            serde_json::from_value(input["state_ref"].clone()).unwrap();
        let height = state.block_height;
        let mut upstream: balance_history::BlockCommitInfo =
            serde_json::from_value(input["block_commit"].clone()).unwrap();
        if corrupt_commit && height == 102 {
            // Snapshot identity alone cannot detect a changed logical balance-history commit.
            upstream.block_commit = "ee".repeat(32);
        }
        let mut collector = PassBlockMutationCollector::new(height);
        collector.push(PassBlockMutation::StateTransition {
            inscription_id: format!("{}i0", "ab".repeat(32)),
            from_state: if height == 101 { "minted" } else { "active" }.to_string(),
            to_state: if height == 103 { "inactive" } else { "active" }.to_string(),
            owner: "p5-owner".to_string(),
            satpoint: format!("{}:0:0", "cd".repeat(32)),
        });
        let pass = collector
            .build_commit_entry(&upstream, previous.as_ref())
            .unwrap();
        let balances: Vec<u64> = serde_json::from_value(input["balances"].clone()).unwrap();
        let mut total: u64 = balances.iter().sum();
        if corrupt_balance && height == 102 {
            total += 1;
        }
        storage
            .upsert_balance_history_snapshot_anchor(&state.clone().into())
            .unwrap();
        storage.upsert_pass_block_commit(&pass).unwrap();
        storage
            .upsert_active_balance_snapshot(height, total, balances.len() as u32)
            .unwrap();
        storage.update_synced_btc_block_height(height).unwrap();
        let local = server.get_local_state_commit_info().unwrap().unwrap();
        let system = server.get_system_state_info().unwrap().unwrap();
        assert_eq!(local.upstream_snapshot_id, state.snapshot_id);
        assert_eq!(system.upstream_snapshot_id, state.snapshot_id);
        assert_eq!(
            local
                .latest_active_balance_snapshot
                .as_ref()
                .unwrap()
                .total_balance,
            total
        );
        assert_eq!(
            local
                .latest_pass_block_commit
                .as_ref()
                .unwrap()
                .block_commit,
            pass.block_commit
        );
        results.push(
            serde_json::json!({"height":height, "upstream_snapshot_id":state.snapshot_id,
            "pass_commit":pass.block_commit,"mutation_root":pass.mutation_root,
            "local":local,"system":system}),
        );
        previous = Some(pass);
    }
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
    results
}

#[test]
fn assumeutxo_p5_pass_local_system_contracts_match_full_replay() {
    let expected = downstream_chain("full_replay", false, false);
    let actual = downstream_chain("candidate", false, false);
    assert_eq!(actual, expected);
    let wrong_commit = downstream_chain("candidate", true, false);
    assert_eq!(wrong_commit[0], actual[0]);
    for index in [1, 2] {
        assert_eq!(
            wrong_commit[index]["upstream_snapshot_id"],
            actual[index]["upstream_snapshot_id"]
        );
        assert_ne!(
            wrong_commit[index]["pass_commit"],
            actual[index]["pass_commit"]
        );
        assert_ne!(
            wrong_commit[index]["local"]["local_state_commit"],
            actual[index]["local"]["local_state_commit"]
        );
        assert_ne!(
            wrong_commit[index]["system"]["system_state_id"],
            actual[index]["system"]["system_state_id"]
        );
    }
    let wrong_balance = downstream_chain("candidate", false, true);
    assert_eq!(wrong_balance[1]["pass_commit"], actual[1]["pass_commit"]);
    assert_ne!(
        wrong_balance[1]["local"]["local_state_commit"],
        actual[1]["local"]["local_state_commit"]
    );
    assert_ne!(
        wrong_balance[1]["system"]["system_state_id"],
        actual[1]["system"]["system_state_id"]
    );
}
