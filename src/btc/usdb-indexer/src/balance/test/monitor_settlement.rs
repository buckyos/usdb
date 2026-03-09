use super::common::{cleanup_data_dir, make_pass, script_hash, test_data_dir};
use crate::balance::{
    BalanceMonitor, ConcurrentBalanceLoader, MockBalanceBackend, MockResponse, SerialBalanceLoader,
};
use crate::storage::MinerPassStorage;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

fn add_active_pass_with_history(
    storage: &MinerPassStorage,
    tag: u8,
    index: u32,
    owner: USDBScriptHash,
    mint_block_height: u32,
) {
    let pass = make_pass(tag, index, owner, mint_block_height);
    storage
        .add_new_mint_pass_at_height(&pass, mint_block_height)
        .unwrap();
}

#[tokio::test]
async fn test_settle_active_balance_empty_active_addresses() {
    let dir = test_data_dir("empty");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    let backend = Arc::new(MockBalanceBackend::new(vec![]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let snapshot = monitor.settle_active_balance(100).await.unwrap();
    assert_eq!(snapshot.block_height, 100);
    assert_eq!(snapshot.active_address_count, 0);
    assert_eq!(snapshot.total_balance, 0);

    let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
    assert_eq!(stored, snapshot);
    assert_eq!(backend.call_count(), 0);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_sum_and_snapshot_written() {
    let dir = test_data_dir("sum");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 11, 0, script_hash(1), 90);
    add_active_pass_with_history(&storage, 12, 1, script_hash(2), 91);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 1_500,
                delta: 10,
            }],
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 2_500,
                delta: 20,
            }],
        ],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let snapshot = monitor.settle_active_balance(100).await.unwrap();
    assert_eq!(snapshot.active_address_count, 2);
    assert_eq!(snapshot.total_balance, 4_000);

    let call = backend.last_call().unwrap();
    assert_eq!(call.0, 2);
    assert_eq!(call.1, Some(100));

    let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
    assert_eq!(stored.total_balance, 4_000);
    assert_eq!(stored.active_address_count, 2);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_rpc_batch_size_mismatch() {
    let dir = test_data_dir("batch_mismatch");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 21, 0, script_hash(3), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend, 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Address balance batch size mismatch"));
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_balance_item_count_mismatch() {
    let dir = test_data_dir("item_mismatch");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 31, 0, script_hash(4), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![vec![]],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend, 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Expected exactly one balance item"));
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_balance_item_count_mismatch_multiple_items() {
    let dir = test_data_dir("item_mismatch_multiple_items");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 71, 0, script_hash(14), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![vec![
            balance_history::AddressBalance {
                block_height: 100,
                balance: 1_000,
                delta: 10,
            },
            balance_history::AddressBalance {
                block_height: 100,
                balance: 1_200,
                delta: 20,
            },
        ]],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend, 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Expected exactly one balance item"));
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_sum_across_multiple_batches() {
    let dir = test_data_dir("sum_multi_batch");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 81, 0, script_hash(21), 90);
    add_active_pass_with_history(&storage, 82, 1, script_hash(22), 91);
    add_active_pass_with_history(&storage, 83, 2, script_hash(23), 92);
    add_active_pass_with_history(&storage, 84, 3, script_hash(24), 93);
    add_active_pass_with_history(&storage, 85, 4, script_hash(25), 94);

    let backend = Arc::new(MockBalanceBackend::new(vec![
        MockResponse::Immediate(Ok(vec![
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 100,
                delta: 1,
            }],
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 200,
                delta: 2,
            }],
        ])),
        MockResponse::Immediate(Ok(vec![
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 300,
                delta: 3,
            }],
            vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 400,
                delta: 4,
            }],
        ])),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: 100,
            balance: 500,
            delta: 5,
        }]])),
    ]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 2).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 2);

    let snapshot = monitor.settle_active_balance(100).await.unwrap();
    assert_eq!(snapshot.active_address_count, 5);
    assert_eq!(snapshot.total_balance, 1_500);
    assert_eq!(backend.call_count(), 3);

    let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
    assert_eq!(stored.total_balance, 1_500);
    assert_eq!(stored.active_address_count, 5);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_fail_on_future_data_guard() {
    let dir = test_data_dir("future_data_guard");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 41, 0, script_hash(5), 120);

    let backend = Arc::new(MockBalanceBackend::new(vec![]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Future miner pass data exists"));
    assert_eq!(backend.call_count(), 0);
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_retry_on_rpc_error() {
    let dir = test_data_dir("retry_rpc_error");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 51, 0, script_hash(6), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![
        MockResponse::Immediate(Err("temporary rpc failure".to_string())),
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: 100,
            balance: 3_000,
            delta: 30,
        }]])),
    ]));
    let loader =
        Arc::new(ConcurrentBalanceLoader::new(backend.clone(), 1024, 1, 10_000, 1).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let snapshot = monitor.settle_active_balance(100).await.unwrap();
    assert_eq!(snapshot.total_balance, 3_000);
    assert_eq!(backend.call_count(), 2);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_retry_on_timeout() {
    let dir = test_data_dir("retry_timeout");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 61, 0, script_hash(7), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![
        MockResponse::Delayed {
            delay_ms: 50,
            result: Ok(vec![vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 1_000,
                delta: 10,
            }]]),
        },
        MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
            block_height: 100,
            balance: 2_000,
            delta: 20,
        }]])),
    ]));
    let loader = Arc::new(ConcurrentBalanceLoader::new(backend.clone(), 1024, 1, 10, 1).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let snapshot = monitor.settle_active_balance(100).await.unwrap();
    assert_eq!(snapshot.total_balance, 2_000);
    assert_eq!(backend.call_count(), 2);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_fail_on_duplicate_owner_in_history_view() {
    // Purpose: ensure duplicate-owner protection still works in history-based active loading.
    // Expected behavior: settlement fails fast, does not call balance RPC, and writes no snapshot.
    let dir = test_data_dir("duplicate_owner_history_guard");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());

    let owner = script_hash(11);
    let pass1 = make_pass(91, 0, owner, 90);
    let pass2 = make_pass(92, 1, script_hash(12), 91);
    storage
        .add_new_mint_pass_at_height(&pass1, pass1.mint_block_height)
        .unwrap();
    storage
        .add_new_mint_pass_at_height(&pass2, pass2.mint_block_height)
        .unwrap();

    // Inject abnormal history: pass2 also becomes active on owner=owner at height=100.
    storage
        .append_pass_history_event_for_test(
            &pass2.inscription_id,
            100,
            "test_corrupt_owner_overlap",
            Some(crate::index::MinerPassState::Active),
            crate::index::MinerPassState::Active,
            Some(pass2.owner),
            owner,
            Some(pass2.satpoint),
            pass2.satpoint,
        )
        .unwrap();

    let backend = Arc::new(MockBalanceBackend::new(vec![]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    // Duplicate active owner is a hard invariant violation and must stop processing.
    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Duplicate active owner detected"));
    assert_eq!(backend.call_count(), 0);
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_accepts_at_or_before_balance_height_and_zeroes_delta() {
    // Purpose: balance-history returns "at or before" entries for height lookup.
    // Expected behavior: monitor accepts older balance height and applies zero effective delta at query height.
    let dir = test_data_dir("at_or_before_height_with_zero_delta");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    let owner = script_hash(15);
    add_active_pass_with_history(&storage, 101, 0, owner, 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![vec![balance_history::AddressBalance {
            block_height: 99,
            balance: 3_210,
            delta: 77,
        }]],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let settlement = monitor
        .settle_active_balance_with_details(100)
        .await
        .unwrap();
    assert_eq!(settlement.snapshot.block_height, 100);
    assert_eq!(settlement.snapshot.active_address_count, 1);
    assert_eq!(settlement.snapshot.total_balance, 3_210);
    assert_eq!(settlement.active_pass_balances.len(), 1);
    let pass_balance = &settlement.active_pass_balances[0];
    assert_eq!(pass_balance.owner, owner);
    assert_eq!(pass_balance.block_height, 100);
    assert_eq!(pass_balance.balance, 3_210);
    assert_eq!(pass_balance.delta, 0);

    let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
    assert_eq!(stored.total_balance, 3_210);
    assert_eq!(stored.active_address_count, 1);
    assert_eq!(backend.call_count(), 1);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_long_window_at_or_before_keeps_zero_delta() {
    // Purpose: regression for repeated at-or-before responses across multiple blocks.
    // Expected behavior: each queried block uses carried balance and forces effective delta=0.
    let dir = test_data_dir("at_or_before_long_window");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    let owner = script_hash(17);
    add_active_pass_with_history(&storage, 103, 0, owner, 80);

    let mut responses = Vec::new();
    for _ in 100..=104 {
        responses.push(MockResponse::Immediate(Ok(vec![vec![
            balance_history::AddressBalance {
                block_height: 99,
                balance: 4_321,
                delta: 99,
            },
        ]])));
    }
    let backend = Arc::new(MockBalanceBackend::new(responses));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    for query_height in 100..=104 {
        let settlement = monitor
            .settle_active_balance_with_details(query_height)
            .await
            .unwrap();
        assert_eq!(settlement.snapshot.block_height, query_height);
        assert_eq!(settlement.snapshot.active_address_count, 1);
        assert_eq!(settlement.snapshot.total_balance, 4_321);
        assert_eq!(settlement.active_pass_balances.len(), 1);

        let pass_balance = &settlement.active_pass_balances[0];
        assert_eq!(pass_balance.owner, owner);
        assert_eq!(pass_balance.block_height, query_height);
        assert_eq!(pass_balance.balance, 4_321);
        assert_eq!(
            pass_balance.delta, 0,
            "effective delta must stay zero for at-or-before balances"
        );

        let stored = storage
            .get_active_balance_snapshot(query_height)
            .unwrap()
            .unwrap();
        assert_eq!(stored.block_height, query_height);
        assert_eq!(stored.total_balance, 4_321);
        assert_eq!(stored.active_address_count, 1);
    }

    assert_eq!(backend.call_count(), 5);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}

#[tokio::test]
async fn test_settle_active_balance_rejects_future_balance_height() {
    // Purpose: reject impossible RPC data where returned balance height is in the future.
    // Expected behavior: settlement fails and no snapshot is persisted.
    let dir = test_data_dir("future_balance_height_guard");
    let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
    add_active_pass_with_history(&storage, 102, 0, script_hash(16), 80);

    let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
        vec![vec![balance_history::AddressBalance {
            block_height: 101,
            balance: 1_234,
            delta: 12,
        }]],
    ))]));
    let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
    let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024, 1024);

    let err = monitor.settle_active_balance(100).await.unwrap_err();
    assert!(err.contains("Unexpected future owner balance height returned by RPC"));
    assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());
    assert_eq!(backend.call_count(), 1);

    drop(monitor);
    drop(storage);
    cleanup_data_dir(&dir);
}
