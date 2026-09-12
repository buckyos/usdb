//! P6 origin commitment acceptance across real replay/import and independent encoding vectors.

use super::test_common::{Fixture, Workspace};
use super::*;
use crate::bootstrap::inspect_bootstrap_origin;
use crate::{BalanceHistoryEntry, ScriptRegistryEntry};
use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid, hashes::Hash};
use usdb_util::{BtcScriptHash, ToBtcScriptHash, UTXOEntry};

fn p5_fixture() -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        "blocks",
    )
}

#[test]
fn origin_commit_real_replay_matches_import_without_reference_files() {
    let work = Workspace::new();
    let chain = p5_fixture();
    let options = chain.options(&work.0);
    let root = work.0.join("candidate");
    let _lock = AssumeUtxoWorkspaceLock::acquire(&root, true).unwrap();
    import_snapshot(&root, &options, 31).unwrap();
    replay_snapshot(&root, chain.client(), 103, 2).unwrap();
    // The inspector must not use the old core file or the P4 workspace input request.
    fs::remove_file(&options.reference_core).unwrap();
    fs::remove_file(root.join("input.json")).unwrap();
    let hash = chain.blocks[103].block_hash();
    let actual =
        inspect_bootstrap_origin(&root.join("state"), Network::Regtest, 103, hash).unwrap();
    let full =
        inspect_bootstrap_origin(&work.0.join("full-replay"), Network::Regtest, 103, hash).unwrap();
    assert_eq!(actual.identity, full.identity);
    assert_eq!(actual.origin_commit, full.origin_commit);
    assert!(!actual.activated);
    {
        let db = BalanceHistoryDB::open(
            Arc::new(config(&root.join("state"), Network::Regtest)),
            BalanceHistoryDBMode::Normal,
        )
        .unwrap();
        let mut old = db.get_block_commit(103).unwrap().unwrap();
        old.block_commit = [0xee; 32];
        old.balance_delta_root = [0xdd; 32];
        db.put_block_commits_async(&[old]).unwrap();
        let script = ScriptBuf::from_bytes(vec![0x51, 0x51]);
        let hash = script.to_btc_script_hash();
        let current = db.get_latest_balance(&hash).unwrap();
        db.put_address_history_async(&vec![
            BalanceHistoryEntry {
                script_hash: hash,
                block_height: 103,
                balance: current.balance,
                delta: -123,
            },
            BalanceHistoryEntry {
                script_hash: BtcScriptHash::from_byte_array([0x99; 32]),
                block_height: 103,
                balance: 0,
                delta: 0,
            },
        ])
        .unwrap();
        let extra = ScriptBuf::from_bytes(vec![0x54]);
        db.put_script_registry_entries(&[ScriptRegistryEntry {
            script_hash: extra.to_btc_script_hash(),
            script_pubkey: extra,
        }])
        .unwrap();
        db.flush_all().unwrap();
    }
    let again = inspect_bootstrap_origin(&root.join("state"), Network::Regtest, 103, hash).unwrap();
    assert_eq!(again.identity, actual.identity);
    assert_eq!(again.origin_commit, actual.origin_commit);
    assert!(inspect_bootstrap_origin(&root.join("state"), Network::Regtest, 102, hash).is_err());
    assert!(
        inspect_bootstrap_origin(
            &root.join("state"),
            Network::Regtest,
            103,
            BlockHash::all_zeros()
        )
        .is_err()
    );
    assert!(inspect_bootstrap_origin(&root.join("state"), Network::Bitcoin, 103, hash).is_err());
}

#[test]
fn origin_commit_database_projection_matches_python_and_binds_actual_state() {
    let work = Workspace::new();
    let root = work.0.join("synthetic");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/bootstrap-origin-v1.json")).unwrap();
    let block_hash: BlockHash = golden["identity"]["origin_block_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let a = BtcScriptHash::from_byte_array([0xab; 32]);
    let b = BtcScriptHash::from_byte_array([0x01; 32]);
    let point = OutPoint {
        txid: Txid::from_byte_array([0x22; 32]),
        vout: 256,
    };
    {
        let db = BalanceHistoryDB::open(
            Arc::new(config(&root, Network::Regtest)),
            BalanceHistoryDBMode::Normal,
        )
        .unwrap();
        db.put_utxos(&[
            UTXOEntry {
                outpoint: point,
                script_hash: a,
                value: 7,
            },
            UTXOEntry {
                outpoint: OutPoint {
                    txid: Txid::from_byte_array([0x11; 32]),
                    vout: 1,
                },
                script_hash: b,
                value: 5,
            },
            UTXOEntry {
                outpoint: OutPoint {
                    txid: Txid::from_byte_array([0x11; 32]),
                    vout: 0,
                },
                script_hash: b,
                value: 0,
            },
        ])
        .unwrap();
        db.update_address_history_with_block_commits_async(
            &vec![
                BalanceHistoryEntry {
                    script_hash: a,
                    block_height: 103,
                    balance: 7,
                    delta: 7,
                },
                BalanceHistoryEntry {
                    script_hash: b,
                    block_height: 103,
                    balance: 5,
                    delta: 5,
                },
            ],
            103,
            &[BlockCommitEntry {
                block_height: 103,
                btc_block_hash: block_hash,
                balance_delta_root: [0; 32],
                block_commit: [0; 32],
            }],
        )
        .unwrap();
        db.flush_all().unwrap();
    }
    let original = inspect_bootstrap_origin(&root, Network::Regtest, 103, block_hash).unwrap();
    assert_eq!(
        serde_json::to_value(&original.identity).unwrap(),
        golden["identity"]
    );
    assert_eq!(original.origin_commit, golden["origin_commit"]);
    assert_eq!(original.identity.utxos.rows, 3); // Keep the zero-valued output.
    {
        let db = BalanceHistoryDB::open(
            Arc::new(config(&root, Network::Regtest)),
            BalanceHistoryDBMode::Normal,
        )
        .unwrap();
        db.put_address_history_async(&vec![BalanceHistoryEntry {
            script_hash: a,
            block_height: 103,
            balance: 8,
            delta: 8,
        }])
        .unwrap();
        db.flush_all().unwrap();
    }
    assert!(
        inspect_bootstrap_origin(&root, Network::Regtest, 103, block_hash)
            .unwrap_err()
            .contains("totals differ")
    );
    {
        let db = BalanceHistoryDB::open(
            Arc::new(config(&root, Network::Regtest)),
            BalanceHistoryDBMode::Normal,
        )
        .unwrap();
        db.put_utxos(&[UTXOEntry {
            outpoint: point,
            script_hash: a,
            value: 8,
        }])
        .unwrap();
        db.flush_all().unwrap();
    }
    let changed = inspect_bootstrap_origin(&root, Network::Regtest, 103, block_hash).unwrap();
    assert_ne!(changed.origin_commit, original.origin_commit);
    {
        let db = BalanceHistoryDB::open(
            Arc::new(config(&root, Network::Regtest)),
            BalanceHistoryDBMode::Normal,
        )
        .unwrap();
        db.put_address_history_async(&vec![BalanceHistoryEntry {
            script_hash: a,
            block_height: 104,
            balance: 8,
            delta: 0,
        }])
        .unwrap();
        db.flush_all().unwrap();
    }
    assert!(
        inspect_bootstrap_origin(&root, Network::Regtest, 103, block_hash)
            .unwrap_err()
            .contains("above origin height")
    );
}
