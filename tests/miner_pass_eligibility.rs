//! UIP-0016 state transitions using chain evidence and real SQLite/RocksDB block recovery.

use crate::index::test_miner_state as support;

use crate::index::energy_formula::calc_inheritable_energy;
use crate::index::pass::{InvalidPassMintInscriptionInfo, MintOperationPath};
use crate::index::{MinerPassKind, MinerPassState, PassBlockMutation};
use support::*;
use usdb_util::ToBtcScriptHash;

async fn apply(
    h: &Harness,
    b: &MintBlock,
    index: usize,
) -> Result<Option<MintOperationPath>, String> {
    h.manager
        .on_mint_pass(
            &b.mints[index],
            &b.evidence,
            &b.balances,
            &h.rules
                .indexer_context_at(b.mints[index].mint_block_height)
                .unwrap(),
        )
        .await
}

#[tokio::test]
async fn first_opening_is_exact_zero_and_can_skip_unavailable_source_history() {
    for balance in [0, 1] {
        let h = Harness::new("opening");
        let dest = recipient(20);
        let owner = dest.to_btc_script_hash();
        let b = MintBlock::new(
            10,
            vec![MintSpec::standard(20, dest, balance, vec![])],
            false,
        );
        let guard = h.begin(10);
        if balance == 0 {
            b.core.state.lock().unwrap().blocks.remove(&9); // Opening does not need historical source authorization.
            assert_eq!(
                apply(&h, &b, 0).await.unwrap(),
                Some(MintOperationPath::FirstOpening)
            );
            assert!(h.has_history(owner, 10));
        } else {
            assert_eq!(apply(&h, &b, 0).await.unwrap(), None);
            assert!(!h.has_history(owner, 10));
            assert_eq!(
                h.storage
                    .get_pass_by_inscription_id(&b.mints[0].inscription_id)
                    .unwrap()
                    .unwrap()
                    .invalid_code
                    .as_deref(),
                Some("INELIGIBLE_RECIPIENT")
            );
        }
        // The RPC reader must never see provisional audit, for either Active or Invalid mints.
        assert!(
            h.storage
                .get_mint_audit(&b.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        h.finish(10, guard);
        assert!(
            h.storage
                .get_mint_audit(&b.mints[0].inscription_id)
                .unwrap()
                .is_some()
        );
        h.cleanup();
    }
}

#[tokio::test]
async fn invalid_history_and_terminal_transfers_do_not_occupy_the_receiver() {
    let h = Harness::new("invalid_history");
    let dest = recipient(21);
    let owner = dest.to_btc_script_hash();
    let guard = h.begin(3);
    h.manager
        .on_invalid_mint_pass(&InvalidPassMintInscriptionInfo {
            inscription_id: id(1),
            inscription_number: 1,
            mint_txid: id(1).txid,
            mint_block_height: 3,
            mint_owner: owner,
            satpoint: satpoint(1),
            error_code: "INVALID_SCHEMA".into(),
            error_reason: "fixture".into(),
        })
        .await
        .unwrap();
    h.finish(3, guard);
    let guard = h.begin(4);
    h.manager
        .on_pass_transfer(&id(1), &recipient(22).to_btc_script_hash(), &satpoint(4), 4)
        .await
        .unwrap();
    h.finish(4, guard);
    assert!(!h.has_history(owner, 4));
    assert!(!h.has_history(recipient(22).to_btc_script_hash(), 4));
    let b = MintBlock::new(10, vec![MintSpec::standard(21, dest, 0, vec![])], false);
    let guard = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::FirstOpening)
    );
    h.finish(10, guard);
    h.cleanup();
}

#[tokio::test]
async fn burn_consumption_and_transfer_never_restore_old_owner_opening() {
    for terminal in ["burn", "consume", "transfer"] {
        let h = Harness::new(terminal);
        let dest = recipient(23);
        let owner = dest.to_btc_script_hash();
        h.seed(1, owner, 3, vec![]).await;
        if terminal == "consume" {
            h.seed(2, owner, 4, vec![id(1)]).await;
        } else {
            let guard = h.begin(4);
            if terminal == "burn" {
                h.manager.on_pass_burned(&id(1), 4).await.unwrap();
            } else {
                h.manager
                    .on_pass_transfer(&id(1), &recipient(24).to_btc_script_hash(), &satpoint(4), 4)
                    .await
                    .unwrap();
            }
            h.finish(4, guard);
        }
        assert!(h.has_history(owner, 4));
        let b = MintBlock::new(10, vec![MintSpec::standard(23, dest, 0, vec![])], false);
        let guard = h.begin(10);
        assert_eq!(apply(&h, &b, 0).await.unwrap(), None);
        h.finish(10, guard);
        h.cleanup();
    }
}

#[tokio::test]
async fn unsolicited_mint_cannot_replace_existing_active_or_consume_its_prev() {
    let h = Harness::new("forced_gift");
    let dest = recipient(25);
    let owner = dest.to_btc_script_hash();
    h.timeline.set(owner, 3, 100_000_000);
    h.seed(1, owner, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![
            MintSpec::standard(25, dest.clone(), 100_000_000, vec![]),
            MintSpec::standard(26, dest, 100_000_000, vec![id(1)]),
        ],
        false,
    );
    let guard = h.begin(10);
    for i in 0..2 {
        assert_eq!(apply(&h, &b, i).await.unwrap(), None);
        assert_eq!(h.state(&id(1)), MinerPassState::Active);
    }
    let mutations = h.finish(10, guard);
    assert!(
        mutations
            .mutations()
            .iter()
            .all(|m| matches!(m, PassBlockMutation::InvalidMint { .. }))
    );
    assert!(
        h.energy
            .get_pass_energy_record_exact(&id(1), 10)
            .unwrap()
            .is_none()
    );
    h.cleanup();
}

#[tokio::test]
async fn same_owner_payment_can_change_beneficiary_with_or_without_prev() {
    for inherit in [false, true] {
        let h = Harness::new("same_owner");
        let dest = source_script(SpendKind::Witness);
        let owner = dest.to_btc_script_hash();
        h.timeline.set(owner, 3, 100_000_000);
        h.seed(1, owner, 3, vec![]).await;
        let mut spec = MintSpec::standard(
            27,
            dest,
            100_000_000,
            if inherit { vec![id(1)] } else { vec![] },
        );
        spec.main = "0x2222222222222222222222222222222222222222".into();
        let b = MintBlock::new(10, vec![spec], false);
        let guard = h.begin(10);
        assert_eq!(
            apply(&h, &b, 0).await.unwrap(),
            Some(MintOperationPath::SameOwner)
        );
        h.finish(10, guard);
        assert_eq!(
            h.state(&id(1)),
            if inherit {
                MinerPassState::Consumed
            } else {
                MinerPassState::Dormant
            }
        );
        assert_eq!(
            h.storage
                .get_pass_by_inscription_id(&b.mints[0].inscription_id)
                .unwrap()
                .unwrap()
                .usdb_main,
            "0x2222222222222222222222222222222222222222"
        );
        h.cleanup();
    }
}

#[tokio::test]
async fn cross_owner_consumes_only_listed_prev_and_inherits_each_after_loss() {
    for include_active in [false, true] {
        let h = Harness::new("cross_prev");
        let source = source_script(SpendKind::Witness).to_btc_script_hash();
        h.timeline.set(source, 3, 100_000_000);
        h.seed(1, source, 3, vec![]).await;
        h.seed(2, source, 4, vec![]).await;
        h.seed(3, source, 5, vec![]).await;
        let mut prev = vec![id(1)];
        if include_active {
            prev.push(id(3));
        }
        let expected = prev
            .iter()
            .map(|id| {
                h.energy
                    .get_pass_energy_record_at_or_before(id, 10)
                    .unwrap()
                    .unwrap()
            })
            .map(|r| {
                if r.state == MinerPassState::Active {
                    calc_inheritable_energy(
                        r.energy
                            + crate::index::energy_formula::calc_growth_delta(
                                r.owner_balance,
                                10 - r.block_height,
                            ),
                    )
                } else {
                    calc_inheritable_energy(r.energy)
                }
            })
            .sum::<u128>();
        let b = MintBlock::new(
            10,
            vec![MintSpec::standard(28, recipient(28), 0, prev)],
            false,
        );
        let guard = h.begin(10);
        assert_eq!(
            apply(&h, &b, 0).await.unwrap(),
            Some(MintOperationPath::CrossOwner)
        );
        h.finish(10, guard);
        assert_eq!(h.state(&id(1)), MinerPassState::Consumed);
        assert_eq!(h.state(&id(2)), MinerPassState::Dormant);
        assert_eq!(
            h.state(&id(3)),
            if include_active {
                MinerPassState::Consumed
            } else {
                MinerPassState::Active
            }
        );
        assert_eq!(
            h.energy
                .get_pass_energy(&b.mints[0].inscription_id, 10)
                .await
                .unwrap()
                .unwrap()
                .energy,
            expected
        );
        assert!(h.has_history(source, 10));
        h.cleanup();
    }
}

#[tokio::test]
async fn all_prev_and_leader_checks_finish_before_any_supersession_or_consumption() {
    for invalid in [
        "foreign",
        "duplicate",
        "missing",
        "burned",
        "consumed",
        "leader",
        "source_signature",
    ] {
        let h = Harness::new(invalid);
        let source = source_script(SpendKind::Witness).to_btc_script_hash();
        h.seed(1, source, 3, vec![]).await;
        h.seed(2, source, 4, vec![]).await;
        h.seed(3, recipient(29).to_btc_script_hash(), 5, vec![])
            .await;
        if invalid == "burned" {
            let g = h.begin(6);
            h.manager.on_pass_burned(&id(1), 6).await.unwrap();
            h.finish(6, g);
        }
        if invalid == "consumed" {
            h.seed(4, source, 6, vec![id(1)]).await;
        }
        let prev = match invalid {
            "foreign" => vec![id(1), id(3)],
            "duplicate" => vec![id(1), id(1)],
            "missing" => vec![id(1), id(99)],
            _ => vec![id(1), id(2)],
        };
        let before = [h.state(&id(1)), h.state(&id(2)), h.state(&id(3))];
        let mut spec = MintSpec::standard(30, recipient(30), 0, prev);
        if invalid == "leader" {
            spec.kind = MinerPassKind::Collab;
            spec.leader = Some(id(99));
        }
        if invalid == "source_signature" {
            spec.flag = 0x81;
        }
        let b = MintBlock::new(10, vec![spec], false);
        let g = h.begin(10);
        assert_eq!(apply(&h, &b, 0).await.unwrap(), None, "{invalid}");
        let mutations = h.finish(10, g);
        assert_eq!(
            [h.state(&id(1)), h.state(&id(2)), h.state(&id(3))],
            before,
            "{invalid}"
        );
        assert!(
            mutations
                .mutations()
                .iter()
                .all(|m| matches!(m, PassBlockMutation::InvalidMint { .. }))
        );
        h.cleanup();
    }
}

#[tokio::test]
async fn cross_owner_requires_unoccupied_zero_destination_and_actual_source_of_prev() {
    for case in ["balance", "history", "wrong_source"] {
        let h = Harness::new(case);
        let source = source_script(SpendKind::Witness).to_btc_script_hash();
        let dest = recipient(31);
        let owner = dest.to_btc_script_hash();
        h.seed(1, source, 3, vec![]).await;
        if case == "history" {
            h.seed(2, owner, 4, vec![]).await;
            let g = h.begin(5);
            h.manager.on_pass_burned(&id(2), 5).await.unwrap();
            h.finish(5, g);
        }
        let mut spec =
            MintSpec::standard(31, dest, if case == "balance" { 1 } else { 0 }, vec![id(1)]);
        if case == "wrong_source" {
            spec.source = SpendKind::Legacy;
        }
        let b = MintBlock::new(10, vec![spec], false);
        let g = h.begin(10);
        assert_eq!(apply(&h, &b, 0).await.unwrap(), None);
        h.finish(10, g);
        assert_eq!(h.state(&id(1)), MinerPassState::Active);
        h.cleanup();
    }
}

#[tokio::test]
async fn same_reveal_uses_one_pre_balance_but_updates_history_between_mints() {
    for second_authorized in [false, true] {
        let h = Harness::new("ordered_same_tx");
        let dest = if second_authorized {
            source_script(SpendKind::Witness)
        } else {
            recipient(32)
        };
        let mut first = MintSpec::standard(32, dest.clone(), 0, vec![]);
        first.source = SpendKind::Legacy;
        let second = MintSpec::standard(33, dest, 0, vec![]);
        let b = MintBlock::new(10, vec![first, second], true);
        let g = h.begin(10);
        assert_eq!(b.mints[0].mint_txid, b.mints[1].mint_txid);
        assert_eq!(
            apply(&h, &b, 0).await.unwrap(),
            Some(MintOperationPath::FirstOpening)
        );
        assert_eq!(
            apply(&h, &b, 1).await.unwrap(),
            if second_authorized {
                Some(MintOperationPath::SameOwner)
            } else {
                None
            }
        );
        h.finish(10, g);
        assert_eq!(
            h.state(&b.mints[0].inscription_id),
            if second_authorized {
                MinerPassState::Dormant
            } else {
                MinerPassState::Active
            }
        );
        h.cleanup();
    }
}

#[tokio::test]
async fn prior_transfer_occupies_target_and_invalid_mint_preserves_the_transfer() {
    let h = Harness::new("transfer_first");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    let dest = recipient(34);
    let owner = dest.to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![MintSpec::standard(34, dest, 0, vec![id(1)])],
        false,
    );
    let g = h.begin(10);
    h.manager
        .on_pass_transfer(&id(1), &owner, &satpoint(34), 10)
        .await
        .unwrap();
    assert!(h.has_history(owner, 10));
    assert_eq!(apply(&h, &b, 0).await.unwrap(), None);
    h.finish(10, g);
    let old = h
        .storage
        .get_pass_by_inscription_id(&id(1))
        .unwrap()
        .unwrap();
    assert_eq!(old.owner, owner);
    assert_eq!(old.state, MinerPassState::Dormant);
    h.cleanup();
}

#[tokio::test]
async fn unavailable_source_aborts_block_without_recording_invalid_and_can_retry() {
    let h = Harness::new("missing_source");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![MintSpec::standard(35, recipient(35), 0, vec![id(1)])],
        false,
    );
    let removed = b.core.state.lock().unwrap().blocks.remove(&9).unwrap();
    let g = h.begin(10);
    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(
        err.contains(&format!("inscription_id={}", b.mints[0].inscription_id)),
        "{err}"
    );
    assert!(err.contains("block_height=10"), "{err}");
    h.abort(10, g);
    assert!(
        h.storage
            .get_pass_by_inscription_id(&b.mints[0].inscription_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(h.state(&id(1)), MinerPassState::Active);
    b.core.state.lock().unwrap().blocks.insert(9, removed);
    let g = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    h.finish(10, g);
    h.cleanup();
}

#[tokio::test]
async fn withdrawn_schema_cannot_select_behavior_through_v2_entrypoint() {
    let h = Harness::new("version_guard");
    let mut b = MintBlock::new(
        10,
        vec![MintSpec::standard(36, recipient(36), 0, vec![])],
        false,
    );
    b.mints[0].mint_version = 2;
    let g = h.begin(10);
    assert_eq!(apply(&h, &b, 0).await.unwrap(), None);
    h.finish(10, g);
    assert_eq!(
        h.storage
            .get_pass_by_inscription_id(&b.mints[0].inscription_id)
            .unwrap()
            .unwrap()
            .invalid_code
            .as_deref(),
        Some("INVALID_SCHEMA")
    );
    h.cleanup();
}

#[tokio::test]
async fn partial_consumption_and_new_pass_energy_failures_roll_back_both_stores() {
    for fail_sql in [false, true] {
        let h = Harness::new("atomic_failure");
        let source = source_script(SpendKind::Witness).to_btc_script_hash();
        let dest = recipient(40);
        let owner = dest.to_btc_script_hash();
        h.timeline.set(source, 3, 100_000_000);
        h.seed(1, source, 3, vec![]).await;
        h.seed(2, source, 4, vec![]).await;
        let b = MintBlock::new(
            10,
            vec![MintSpec::standard(40, dest, 0, vec![id(1), id(2)])],
            false,
        );
        let db = h.data.join(crate::constants::MINER_PASS_DB_FILE);
        let connection = rusqlite::Connection::open(&db).unwrap();
        if fail_sql {
            connection.execute_batch(&format!("CREATE TRIGGER fail_second_consume BEFORE UPDATE OF state ON miner_passes WHEN NEW.inscription_id = '{}' AND NEW.state = 'consumed' BEGIN SELECT RAISE(ABORT, 'injected second consume'); END;",id(2))).unwrap();
        } else {
            *h.timeline.fail.lock().unwrap() = Some((owner, 10));
        }
        let g = h.begin(10);
        let err = apply(&h, &b, 0).await.unwrap_err();
        assert!(
            err.contains(if fail_sql {
                "injected second consume"
            } else {
                "Injected balance read failure"
            }),
            "{err}"
        );
        assert_eq!(h.state(&id(1)), MinerPassState::Consumed); // Failure is after at least one mutation.
        let committed: String = connection
            .query_row(
                "SELECT state FROM miner_passes WHERE inscription_id=?1",
                [id(1).to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(committed, "dormant"); // Uncommitted partial state never reaches other SQLite readers.
        h.abort(10, g);
        assert_eq!(h.state(&id(1)), MinerPassState::Dormant);
        assert_eq!(h.state(&id(2)), MinerPassState::Active);
        assert!(!h.has_history(owner, 10));
        assert!(
            h.storage
                .get_pass_by_inscription_id(&b.mints[0].inscription_id)
                .unwrap()
                .is_none()
        );
        assert!(
            h.energy
                .get_pass_energy_record_exact(&id(1), 10)
                .unwrap()
                .is_none()
        );
        assert!(
            h.energy
                .get_pass_energy_record_exact(&id(2), 10)
                .unwrap()
                .is_none()
        );
        if fail_sql {
            connection
                .execute_batch("DROP TRIGGER fail_second_consume")
                .unwrap();
        } else {
            *h.timeline.fail.lock().unwrap() = None;
        }
        let g = h.begin(10);
        assert_eq!(
            apply(&h, &b, 0).await.unwrap(),
            Some(MintOperationPath::CrossOwner)
        );
        h.finish(10, g);
        assert_eq!(h.state(&id(1)), MinerPassState::Consumed);
        assert_eq!(h.state(&id(2)), MinerPassState::Consumed);
        drop(connection);
        h.cleanup();
    }
}

#[tokio::test]
async fn finalized_energy_with_uncommitted_sqlite_recovers_on_reopen_and_retries() {
    let h = Harness::new("finalize_window");
    let root = h.root.clone();
    let timeline = h.timeline.clone();
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![MintSpec::standard(41, recipient(41), 0, vec![id(1)])],
        false,
    );
    let g = h.begin(10);
    apply(&h, &b, 0).await.unwrap();
    h.energy.finalize_block_sync(10).unwrap();
    assert!(
        h.energy
            .get_pass_energy_record_exact(&b.mints[0].inscription_id, 10)
            .unwrap()
            .is_some()
    );
    drop(g);
    drop(h); // SQLite rolls back while RocksDB has already finalized this height.
    let h = Harness::open(root, timeline);
    assert_eq!(h.state(&id(1)), MinerPassState::Active);
    assert!(!h.has_history(b.mints[0].mint_owner, 10));
    assert!(
        h.energy
            .get_pass_energy_record_exact(&b.mints[0].inscription_id, 10)
            .unwrap()
            .is_none()
    );
    let g = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    h.finish(10, g);
    h.cleanup();
}

#[tokio::test]
async fn history_survives_closed_store_restore_and_reorg_replay_matches_mutations() {
    let h = Harness::new("restore_replay");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    let root = h.root.clone();
    let timeline = h.timeline.clone();
    h.timeline.set(source, 3, 100_000_000);
    h.seed(1, source, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![MintSpec::standard(42, recipient(42), 0, vec![id(1)])],
        false,
    );
    let g = h.begin(10);
    apply(&h, &b, 0).await.unwrap();
    let first = h.finish(10, g);
    let expected_energy = h
        .energy
        .get_pass_energy(&b.mints[0].inscription_id, 10)
        .await
        .unwrap()
        .unwrap();
    let restored_root = root.with_extension("restored");
    drop(h);
    copy_closed_fixture(&root, &restored_root);
    let h = Harness::open(restored_root, timeline);
    assert!(h.has_history(source, 10));
    assert!(h.has_history(b.mints[0].mint_owner, 10));
    assert_eq!(h.state(&id(1)), MinerPassState::Consumed);
    assert_eq!(
        h.energy
            .get_pass_energy(&b.mints[0].inscription_id, 10)
            .await
            .unwrap()
            .unwrap(),
        expected_energy
    );
    h.storage.rollback_to_block_height(3, None).unwrap();
    h.energy.rollback_to_pass_synced_height(3).unwrap();
    assert!(h.has_history(source, 10));
    assert!(!h.has_history(b.mints[0].mint_owner, 10));
    assert_eq!(h.state(&id(1)), MinerPassState::Active);
    let g = h.begin(10);
    apply(&h, &b, 0).await.unwrap();
    let replay = h.finish(10, g);
    assert_eq!(replay.mutations(), first.mutations());
    assert_eq!(
        replay.mutation_root().unwrap(),
        first.mutation_root().unwrap()
    );
    assert_eq!(
        h.energy
            .get_pass_energy(&b.mints[0].inscription_id, 10)
            .await
            .unwrap()
            .unwrap(),
        expected_energy
    );
    h.cleanup();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn collab_uses_same_eligibility_and_neither_leader_reference_follows_rotation() {
    use bitcoincore_rpc::bitcoin::{Address, Network};
    let h = Harness::new("collab");
    let leader_script = source_script(SpendKind::Witness);
    let leader_owner = leader_script.to_btc_script_hash();
    h.seed(1, leader_owner, 3, vec![]).await;
    let collaborator = source_script(SpendKind::Legacy);
    let mut fixed = MintSpec::standard(43, collaborator.clone(), 0, vec![]);
    fixed.kind = MinerPassKind::Collab;
    fixed.leader = Some(id(1));
    let mut dynamic = MintSpec::standard(44, recipient(44), 0, vec![]);
    dynamic.kind = MinerPassKind::Collab;
    dynamic.leader_addr = Some(
        Address::from_script(&leader_script, Network::Bitcoin)
            .unwrap()
            .to_string(),
    );
    let b = MintBlock::new(10, vec![fixed, dynamic], false);
    let g = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::FirstOpening)
    );
    assert_eq!(
        apply(&h, &b, 1).await.unwrap(),
        Some(MintOperationPath::FirstOpening)
    );
    h.finish(10, g);
    let mut attack = MintSpec::standard(45, collaborator, 1, vec![b.mints[0].inscription_id]);
    attack.kind = MinerPassKind::Collab;
    attack.leader = Some(id(1));
    let attack = MintBlock::new(11, vec![attack], false);
    let g = h.begin(11);
    assert_eq!(apply(&h, &attack, 0).await.unwrap(), None);
    h.finish(11, g);
    assert_eq!(h.state(&b.mints[0].inscription_id), MinerPassState::Active);
    let mut inherit = MintSpec::standard(46, recipient(46), 0, vec![b.mints[0].inscription_id]);
    inherit.kind = MinerPassKind::Collab;
    inherit.leader = Some(id(1));
    inherit.source = SpendKind::Legacy;
    let inherited = MintBlock::new(12, vec![inherit], false);
    let g = h.begin(12);
    assert_eq!(
        apply(&h, &inherited, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    h.finish(12, g);
    let fixed = h
        .storage
        .get_pass_by_inscription_id(&inherited.mints[0].inscription_id)
        .unwrap()
        .unwrap();
    let dynamic = h
        .storage
        .get_pass_by_inscription_id(&b.mints[1].inscription_id)
        .unwrap()
        .unwrap();
    assert!(
        h.manager
            .resolve_collab_leader_at_height(&fixed, 12)
            .unwrap()
            .is_some()
    );
    assert!(
        h.manager
            .resolve_collab_leader_at_height(&dynamic, 12)
            .unwrap()
            .is_some()
    );
    let leader = MintBlock::new(
        20,
        vec![MintSpec::standard(47, recipient(47), 0, vec![id(1)])],
        false,
    );
    let g = h.begin(20);
    apply(&h, &leader, 0).await.unwrap();
    h.finish(20, g);
    assert!(
        h.manager
            .resolve_collab_leader_at_height(&fixed, 20)
            .unwrap()
            .is_none()
    );
    assert!(
        h.manager
            .resolve_collab_leader_at_height(&dynamic, 20)
            .unwrap()
            .is_none()
    );
    h.cleanup();
}

#[tokio::test]
async fn cross_owner_settles_whole_block_balance_loss_before_five_percent_inheritance() {
    let mut inherited = Vec::new();
    for transfer_at_mint_height in [false, true] {
        let h = Harness::new("migration_energy");
        let source = source_script(SpendKind::Witness).to_btc_script_hash();
        let dest = recipient(48);
        let owner = dest.to_btc_script_hash();
        h.timeline.set(source, 3, 100_000_000);
        h.seed(1, source, 3, vec![]).await;
        if transfer_at_mint_height {
            h.timeline.set(source, 10, 0);
            h.timeline.set(owner, 10, 100_000_000);
        } else {
            h.timeline.set(source, 11, 0);
            h.timeline.set(owner, 11, 100_000_000);
        }
        let b = MintBlock::new(
            10,
            vec![MintSpec::standard(48, dest, 0, vec![id(1)])],
            false,
        );
        let g = h.begin(10);
        apply(&h, &b, 0).await.unwrap();
        h.finish(10, g);
        inherited.push(
            h.energy
                .get_pass_energy(&b.mints[0].inscription_id, 10)
                .await
                .unwrap()
                .unwrap()
                .energy,
        );
        h.cleanup();
    }
    assert!(inherited[0] > 0);
    assert_eq!(inherited[1], 0); // Emptying D in the same block incurs the existing balance penalty first.
}

#[tokio::test]
async fn repeated_cross_owner_prev_is_consumed_once_even_with_two_fresh_targets() {
    let h = Harness::new("prev_competition");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    let b = MintBlock::new(
        10,
        vec![
            MintSpec::standard(49, recipient(49), 0, vec![id(1)]),
            MintSpec::standard(50, recipient(50), 0, vec![id(1)]),
        ],
        false,
    );
    let g = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    assert_eq!(apply(&h, &b, 1).await.unwrap(), None);
    h.finish(10, g);
    assert!(h.has_history(b.mints[0].mint_owner, 10));
    assert!(!h.has_history(b.mints[1].mint_owner, 10));
    assert_eq!(h.state(&id(1)), MinerPassState::Consumed);
    h.cleanup();
}

#[tokio::test]
async fn v2_rejects_missing_block_guards_and_mismatched_balance_context_without_writes() {
    let h = Harness::new("context_guard");
    let mut b = MintBlock::new(
        10,
        vec![MintSpec::standard(51, recipient(51), 0, vec![])],
        false,
    );
    let err = apply(&h, &b, 0).await.unwrap_err();
    // The caller receives the operation context too, even when failure precedes evidence loading.
    for field in [
        "savepoint".into(),
        "autocommit=true".into(),
        format!(
            "db_path={}",
            h.data.join(crate::constants::MINER_PASS_DB_FILE).display()
        ),
        format!("inscription_id={}", b.mints[0].inscription_id),
        "block_height=10".into(),
        format!("mint_txid={}", b.mints[0].mint_txid),
        format!("mint_owner={}", b.mints[0].mint_owner),
        format!("satpoint={}", b.mints[0].satpoint),
    ] {
        assert!(err.contains(&field), "missing {field}: {err}");
    }

    let g = crate::storage::MinePassStorageSavePointGuard::new(&h.storage).unwrap();
    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(err.contains("expected=10, pending=None"), "{err}");
    h.energy.begin_block_sync(9).unwrap();
    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(err.contains("expected=10, pending=Some(9)"), "{err}");
    h.energy.abort_pending_block_sync(9).unwrap();
    h.energy.begin_block_sync(10).unwrap();

    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(
        err.contains("expected_height=10, collector_height=None"),
        "{err}"
    );
    h.manager.begin_block_mutation_collection(9).unwrap();
    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(
        err.contains("expected_height=10, collector_height=Some(9)"),
        "{err}"
    );
    h.manager.clear_block_mutation_collection();
    h.manager.begin_block_mutation_collection(10).unwrap();

    let inputs = b.evidence.block_prevouts().unwrap();
    // Same-height wrong-fork evidence needs both hashes; a different height needs both heights.
    for other_height in [10, 11] {
        let other = MintBlock::new(
            other_height,
            vec![MintSpec::standard(52, recipient(52), 0, vec![])],
            false,
        );
        let err = h
            .manager
            .on_mint_pass(
                &b.mints[0],
                &b.evidence,
                &other.balances,
                &h.rules
                    .indexer_context_at(b.mints[0].mint_block_height)
                    .unwrap(),
            )
            .await
            .unwrap_err();
        for field in [
            format!("balance_height={other_height}"),
            format!(
                "balance_block_hash={}",
                other
                    .evidence
                    .block_prevouts()
                    .unwrap()
                    .block()
                    .block_hash()
            ),
            "evidence_height=10".into(),
            format!("evidence_block_hash={}", inputs.block().block_hash()),
        ] {
            assert!(err.contains(&field), "missing {field}: {err}");
        }
        if other_height == 11 {
            let err = h
                .manager
                .on_mint_pass(
                    &b.mints[0],
                    &other.evidence,
                    &other.balances,
                    &h.rules
                        .indexer_context_at(b.mints[0].mint_block_height)
                        .unwrap(),
                )
                .await
                .unwrap_err();
            assert!(err.contains("mint_height=10, evidence_height=11"), "{err}");
        }
    }

    let actual_txid = b.mints[0].mint_txid;
    b.mints[0].mint_txid = id(90).txid;
    let err = apply(&h, &b, 0).await.unwrap_err();
    assert!(
        err.contains(&format!(
            "mint_txid={}, inscription_txid={actual_txid}",
            id(90).txid
        )),
        "{err}"
    );
    b.mints[0].mint_txid = actual_txid;

    let actual_owner = b.mints[0].mint_owner;
    let actual_satpoint = b.mints[0].satpoint;
    b.mints[0].mint_owner = recipient(91).to_btc_script_hash();
    b.mints[0].satpoint.offset += 1;
    let err = apply(&h, &b, 0).await.unwrap_err();
    for field in [
        format!("mint_owner={}", b.mints[0].mint_owner),
        format!("reveal_owner={actual_owner}"),
        format!("mint_satpoint={}", b.mints[0].satpoint),
        format!("reveal_satpoint={actual_satpoint}"),
    ] {
        assert!(err.contains(&field), "missing {field}: {err}");
    }
    // All diagnostic guards fail before either pass/history or mutation writes.
    assert!(
        h.manager
            .take_block_mutation_collector(10)
            .unwrap()
            .mutations()
            .is_empty()
    );
    assert!(
        h.storage
            .get_pass_by_inscription_id(&b.mints[0].inscription_id)
            .unwrap()
            .is_none()
    );
    assert!(!h.has_history(actual_owner, 10));
    h.abort(10, g);
    h.cleanup();
}

#[tokio::test]
async fn consumed_pass_transfer_does_not_create_an_acquisition_for_a_new_owner() {
    let h = Harness::new("consumed_transfer");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    let dest = recipient(53);
    let owner = dest.to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    h.seed(2, source, 4, vec![id(1)]).await;
    let g = h.begin(5);
    h.manager
        .on_pass_transfer(&id(1), &owner, &satpoint(53), 5)
        .await
        .unwrap();
    h.finish(5, g);
    assert!(!h.has_history(owner, 5));
    assert_eq!(
        h.storage
            .get_pass_by_inscription_id(&id(1))
            .unwrap()
            .unwrap()
            .owner,
        source
    );
    let b = MintBlock::new(10, vec![MintSpec::standard(53, dest, 0, vec![])], false);
    let g = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::FirstOpening)
    );
    h.finish(10, g);
    h.cleanup();
}

#[tokio::test]
async fn multi_prev_inheritance_saturates_after_each_individual_discount() {
    let h = Harness::new("inherit-saturation");
    let source = source_script(SpendKind::Witness).to_btc_script_hash();
    h.seed(1, source, 3, vec![]).await;
    h.seed(2, source, 4, vec![]).await;
    for (tag, state) in [(1, MinerPassState::Dormant), (2, MinerPassState::Active)] {
        h.energy
            .insert_pass_energy_record_for_test(&crate::storage::PassEnergyRecord {
                inscription_id: id(tag),
                block_height: 4,
                state,
                active_block_height: 3,
                owner_address: source,
                owner_balance: 0,
                owner_delta: 0,
                energy: u128::MAX,
            })
            .unwrap();
    }
    let b = MintBlock::new(
        10,
        vec![MintSpec::standard(91, recipient(91), 0, vec![id(1), id(2)])],
        false,
    );
    let guard = h.begin(10);
    assert_eq!(
        apply(&h, &b, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    h.finish(10, guard);
    assert_eq!(h.state(&id(1)), MinerPassState::Consumed);
    assert_eq!(h.state(&id(2)), MinerPassState::Consumed);
    assert_eq!(
        h.energy
            .get_pass_energy(&b.mints[0].inscription_id, 10)
            .await
            .unwrap()
            .unwrap()
            .energy,
        u128::MAX
    );
    h.cleanup();
}

#[tokio::test]
async fn collab_inheritance_uses_raw_energy_without_applying_contribution_weight() {
    let h = Harness::new("collab-raw-inheritance");
    let source = source_script(SpendKind::Witness);
    h.seed(1, recipient(90).to_btc_script_hash(), 3, vec![])
        .await;
    let mut first = MintSpec::standard(92, source.clone(), 0, vec![]);
    first.kind = MinerPassKind::Collab;
    first.leader = Some(id(1));
    let first = MintBlock::new(10, vec![first], false);
    let g = h.begin(10);
    apply(&h, &first, 0).await.unwrap();
    h.finish(10, g);
    let raw = 1_000_003;
    h.energy
        .insert_pass_energy_record_for_test(&crate::storage::PassEnergyRecord {
            inscription_id: first.mints[0].inscription_id,
            block_height: 10,
            state: MinerPassState::Active,
            active_block_height: 10,
            owner_address: source.to_btc_script_hash(),
            owner_balance: 0,
            owner_delta: 0,
            energy: raw,
        })
        .unwrap();
    let mut next = MintSpec::standard(93, recipient(93), 0, vec![first.mints[0].inscription_id]);
    next.kind = MinerPassKind::Collab;
    next.leader = Some(id(1));
    let next = MintBlock::new(11, vec![next], false);
    let g = h.begin(11);
    assert_eq!(
        apply(&h, &next, 0).await.unwrap(),
        Some(MintOperationPath::CrossOwner)
    );
    h.finish(11, g);
    assert_eq!(
        h.state(&first.mints[0].inscription_id),
        MinerPassState::Consumed
    );
    assert_eq!(
        h.energy
            .get_pass_energy(&next.mints[0].inscription_id, 11)
            .await
            .unwrap()
            .unwrap()
            .energy,
        calc_inheritable_energy(raw)
    );
    assert_ne!(
        calc_inheritable_energy(raw),
        calc_inheritable_energy(crate::index::energy_formula::calc_collab_contribution(raw))
    );
    h.cleanup();
}
