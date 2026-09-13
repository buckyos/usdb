//! P6.3 sparse local files, canonical replay and bootstrap overlapping an advancing Core tip.

use super::test_common::{Fixture, Workspace};
use super::*;
use crate::bootstrap::{NativeBootstrapPhase, prepare_native_bootstrap};
use crate::btc::{BTCClient, CanonicalBlockLoader};
use std::cell::RefCell;
use std::sync::atomic::Ordering;

#[path = "common/block_files.rs"]
mod files;
use files::{GrowingChain, frame, write_file};

fn chain(branch: &str) -> Fixture {
    Fixture::load_at(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/assumeutxo-p5"),
        branch,
    )
}

fn loader_config(work: &Workspace, threshold: usize) -> Arc<BalanceHistoryConfig> {
    let mut cfg = config(&work.0.join("service"), Network::Regtest);
    cfg.btc.data_dir = Some(work.0.join("node"));
    cfg.sync.local_loader_threshold = threshold;
    Arc::new(cfg)
}

#[tokio::test]
async fn canonical_loader_sparse_dual_append_xor_restart_and_reorg() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let rpc = GrowingChain::new(chain.clone(), 113);
    let cfg = loader_config(&work, 0);
    let dir = cfg.btc.data_dir().join("blocks");
    fs::create_dir_all(&dir).unwrap();
    let xor = [2, 7, 11, 17, 23, 31, 37, 43];
    fs::write(dir.join("xor.dat"), xor).unwrap();
    let a = frame(cfg.btc.block_magic(), &chain.blocks[102]);
    let b = frame(cfg.btc.block_magic(), &chain.blocks[103]);
    // Two growing files, no blk00000 and no pre-baseline blocks. The latest record is unfinished.
    write_file(&dir.join("blk00007.dat"), &b, xor, 4096);
    write_file(&dir.join("blk00042.dat"), &a[..100], xor, 4096);
    let loader = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().local_blocks, 1);
    assert_eq!(loader.stats().rpc_blocks, 1);
    // Both files change without changing their preallocated length. Duplicate hashes are harmless.
    write_file(
        &dir.join("blk00042.dat"),
        &[a.clone(), b.clone()].concat(),
        xor,
        4096,
    );
    write_file(
        &dir.join("blk00007.dat"),
        &[b.clone(), a.clone()].concat(),
        xor,
        4096,
    );
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().local_blocks, 3);
    assert_eq!(loader.stats().rpc_blocks, 1);
    drop(loader);
    let loader = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().local_blocks, 2);
    assert_eq!(loader.stats().indexed_records, 0);
    drop(loader);

    let fork = self::chain("fork_blocks");
    let fork_rpc = GrowingChain::new(fork.clone(), 114);
    let c = frame(cfg.btc.block_magic(), &fork.blocks[102]);
    let d = frame(cfg.btc.block_magic(), &fork.blocks[103]);
    let e = frame(cfg.btc.block_magic(), &fork.blocks[104]);
    write_file(
        &dir.join("blk00007.dat"),
        &[b.clone(), a.clone(), c].concat(),
        xor,
        4096,
    );
    write_file(&dir.join("blk00042.dat"), &[a, b, d, e].concat(), xor, 4096);
    let loader = CanonicalBlockLoader::new(fork_rpc.client(), &cfg).unwrap();
    assert_eq!(
        loader.get_blocks(102, 104).await.unwrap(),
        fork.blocks[102..=104]
    );
    // Old fork locations remain cached but do not override canonical RPC selection.
    assert_eq!(loader.stats().local_blocks, 3);
    assert_eq!(loader.stats().rpc_blocks, 0);
    loader.stop().unwrap();
    assert!(loader.get_blocks(102, 103).await.is_err());
}

#[tokio::test]
async fn canonical_loader_near_tip_missing_files_and_corrupt_payload_fall_back() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let rpc = GrowingChain::new(chain.clone(), 113);
    let cfg = loader_config(&work, 500);
    let loader = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert!(!cfg.root_dir.join("local-block-index").exists());
    rpc.tip.store(2000, Ordering::Relaxed);
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().rpc_blocks, 4);
    let dir = cfg.btc.data_dir().join("blocks");
    fs::create_dir_all(&dir).unwrap();
    let mut bad = chain.blocks[102].clone();
    bad.txdata[0].output[0].value = bitcoincore_rpc::bitcoin::Amount::from_sat(1);
    assert_eq!(bad.block_hash(), chain.blocks[102].block_hash());
    assert!(!bad.check_merkle_root());
    write_file(
        &dir.join("blk00003.dat"),
        &frame(cfg.btc.block_magic(), &bad),
        [0; 8],
        4096,
    );
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().local_blocks, 0);
    assert_eq!(loader.stats().rpc_blocks, 6);
    let mut bad_witness = chain.blocks[102].clone();
    bad_witness.txdata[1].input[0].witness =
        bitcoincore_rpc::bitcoin::Witness::from_slice(&[vec![42u8; 32]]);
    assert!(bad_witness.check_merkle_root());
    assert!(!bad_witness.check_witness_commitment());
    write_file(
        &dir.join("blk00003.dat"),
        &frame(cfg.btc.block_magic(), &bad_witness),
        [0; 8],
        4096,
    );
    assert_eq!(loader.get_block_by_height(102).unwrap(), chain.blocks[102]);
    assert_eq!(loader.stats().local_blocks, 0);
    let mut duplicate = chain.blocks[103].clone();
    duplicate
        .txdata
        .push(duplicate.txdata.last().unwrap().clone());
    assert!(duplicate.check_merkle_root());
    write_file(
        &dir.join("blk00004.dat"),
        &frame(cfg.btc.block_magic(), &duplicate),
        [0; 8],
        4096,
    );
    assert_eq!(loader.get_block_by_height(103).unwrap(), chain.blocks[103]);
    assert_eq!(loader.stats().local_blocks, 0);
    // Returning near the tip disables scanning even after the local path has been opened.
    let scanned = loader.stats().indexed_records;
    rpc.tip.store(113, Ordering::Relaxed);
    loader.get_blocks(102, 103).await.unwrap();
    assert_eq!(loader.stats().indexed_records, scanned);
}

#[tokio::test]
async fn canonical_loader_bounded_scan_resumes_from_a_durable_file_cursor() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let rpc = GrowingChain::new(chain.clone(), 113);
    let cfg = loader_config(&work, 0);
    let dir = cfg.btc.data_dir().join("blocks");
    fs::create_dir_all(&dir).unwrap();
    let mut bytes = frame(cfg.btc.block_magic(), &chain.blocks[102]).repeat(4097);
    bytes.extend(frame(cfg.btc.block_magic(), &chain.blocks[103]));
    write_file(&dir.join("blk00011.dat"), &bytes, [0; 8], 0);
    let loader = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    assert_eq!(
        loader.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert_eq!(loader.stats().indexed_records, 4096);
    assert_eq!(loader.stats().rpc_blocks, 1);
    drop(loader);
    let resumed = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    assert_eq!(
        resumed.get_blocks(102, 103).await.unwrap(),
        chain.blocks[102..=103]
    );
    assert!(resumed.stats().indexed_records < 10);
    assert_eq!(resumed.stats().local_blocks, 2);
    assert_eq!(resumed.stats().rpc_blocks, 0);
}

#[tokio::test]
async fn canonical_loader_partial_headers_lengths_and_payloads_are_retried() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let rpc = GrowingChain::new(chain.clone(), 113);
    let cfg = loader_config(&work, 0);
    let dir = cfg.btc.data_dir().join("blocks");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blk00091.dat");
    let bytes = frame(cfg.btc.block_magic(), &chain.blocks[102]);
    let loader = CanonicalBlockLoader::new(rpc.client(), &cfg).unwrap();
    for cut in [1, 4, 7, 40, bytes.len() - 1] {
        write_file(&path, &bytes[..cut], [0; 8], 0);
        assert_eq!(loader.get_block_by_height(102).unwrap(), chain.blocks[102]);
        assert_eq!(loader.stats().local_blocks, 0);
    }
    write_file(&path, &bytes, [0; 8], 4096);
    assert_eq!(loader.get_block_by_height(102).unwrap(), chain.blocks[102]);
    assert_eq!(loader.stats().local_blocks, 1);
}

#[test]
fn native_bootstrap_imports_and_replays_before_core_reaches_business_genesis() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let cfg = chain.native_config(&work.0, 103);
    let rpc = GrowingChain::new(chain.clone(), 101);
    let progress = cfg.root_dir.join("bootstrap-progress.json");
    let observed = RefCell::new(Vec::new());
    let state = prepare_native_bootstrap(cfg.clone(), rpc.client(), &|| {
        if let Ok(bytes) = fs::read(&progress) {
            let p: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if p["phase"] == "waiting_for_blocks" {
                let height = p["height"].as_u64().unwrap() as u32;
                if !observed.borrow().contains(&height) {
                    assert!(!cfg.db_dir().join("balance_history").exists());
                    observed.borrow_mut().push(height);
                    rpc.tip
                        .store(if height == 101 { 112 } else { 113 }, Ordering::Relaxed);
                }
            }
        }
        false
    })
    .unwrap()
    .unwrap();
    assert_eq!(*observed.borrow(), vec![101, 102]);
    assert_eq!(state.phase, NativeBootstrapPhase::Sealed);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/assumeutxo-p5/downstream-inputs.json"
    ))
    .unwrap();
    assert_eq!(
        state.origin_commit.unwrap(),
        expected["blocks"][2]["full_replay"]["block_commit"]["block_commit"]
    );
}

#[test]
fn native_bootstrap_wait_is_cancellable_and_resumes_without_rescanning_source() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let cfg = chain.native_config(&work.0, 103);
    let rpc = GrowingChain::new(chain, 101);
    let progress = cfg.root_dir.join("bootstrap-progress.json");
    let result = prepare_native_bootstrap(cfg.clone(), rpc.client(), &|| {
        fs::read(&progress)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|p| p["phase"] == "waiting_for_blocks")
    });
    assert!(result.unwrap_err().contains("cancelled while waiting"));
    fs::remove_file(&cfg.bootstrap.as_ref().unwrap().snapshot_file).unwrap();
    rpc.tip.store(113, Ordering::Relaxed);
    assert_eq!(
        prepare_native_bootstrap(cfg, rpc.client(), &|| false)
            .unwrap()
            .unwrap()
            .phase,
        NativeBootstrapPhase::Sealed
    );
}

#[test]
fn native_bootstrap_sparse_local_replay_preserves_full_replay_commit_without_rpc_payloads() {
    let work = Workspace::new();
    let chain = chain("blocks");
    let mut cfg = (*chain.native_config(&work.0, 103)).clone();
    cfg.sync.local_loader_threshold = 0;
    cfg.btc.data_dir = Some(work.0.join("sparse-node"));
    let dir = cfg.btc.data_dir().join("blocks");
    fs::create_dir_all(&dir).unwrap();
    write_file(
        &dir.join("blk00123.dat"),
        &[
            frame(cfg.btc.block_magic(), &chain.blocks[103]),
            frame(cfg.btc.block_magic(), &chain.blocks[102]),
        ]
        .concat(),
        [0; 8],
        4096,
    );
    let cfg = Arc::new(cfg);
    let rpc = GrowingChain::new(chain.clone(), 113);
    let state = prepare_native_bootstrap(cfg.clone(), rpc.client(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(rpc.payload_calls.load(Ordering::Relaxed), 0);
    let reference = work.0.join("full-replay");
    chain.reference(&reference, &work.0.join("oracle.db"));
    let full =
        BalanceHistoryDB::open_read_only(Arc::new(config(&reference, Network::Regtest))).unwrap();
    let actual = BalanceHistoryDB::open_read_only(cfg).unwrap();
    assert_eq!(
        actual.get_block_commit(103).unwrap(),
        full.get_block_commit(103).unwrap()
    );
    assert_eq!(
        state.origin.unwrap(),
        full.bootstrap_origin_identity(Network::Regtest, 103, chain.blocks[103].block_hash())
            .unwrap()
    );
}
