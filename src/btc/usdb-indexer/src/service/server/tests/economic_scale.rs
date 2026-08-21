use super::*;
use bitcoincore_rpc::bitcoin::{Address, Network};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const QUERY_HEIGHT: u32 = 120;
const MINT_HEIGHT: u32 = 100;
const MAINNET_LEADER_ADDRESS: &str = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
const REPORT_VERSION: &str = "usdb-economic-view-scale:v4";
const CAPACITY_REPORT_VERSION: &str = "usdb-economic-view-capacity-supplement:v1";
const HISTORICAL_CONTEXT_END_HEIGHT: u32 = 124;
const CHURN_DORMANT_HEIGHT: u32 = 130;
const CHURN_REMINT_HEIGHT: u32 = 131;

static SQLITE_STATEMENTS: AtomicU64 = AtomicU64::new(0);
static SQLITE_READ_STATEMENTS: AtomicU64 = AtomicU64::new(0);
static SQLITE_HISTORY_READ_STATEMENTS: AtomicU64 = AtomicU64::new(0);
static SQLITE_RESULT_ROWS: AtomicU64 = AtomicU64::new(0);
static SQLITE_VM_STEPS: AtomicU64 = AtomicU64::new(0);
static SQLITE_FULLSCAN_STEPS: AtomicU64 = AtomicU64::new(0);
static SQLITE_SORTS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
struct StorageReadMetrics {
    sqlite_statements: u64,
    sqlite_read_statements: u64,
    sqlite_history_read_statements: u64,
    sqlite_result_rows: u64,
    sqlite_vm_steps: u64,
    sqlite_fullscan_steps: u64,
    sqlite_sorts: u64,
    rocksdb_point_gets: u64,
    rocksdb_iterator_seeks: u64,
    rocksdb_records_decoded: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProcessIoSnapshot {
    read_chars: u64,
    write_chars: u64,
    read_syscalls: u64,
    write_syscalls: u64,
    physical_read_bytes: u64,
    physical_write_bytes: u64,
    cancelled_write_bytes: u64,
}

#[derive(Debug, Serialize)]
struct ProcessIoMetrics {
    read_chars: u64,
    write_chars: u64,
    read_syscalls: u64,
    write_syscalls: u64,
    physical_read_bytes: u64,
    physical_write_bytes: u64,
    cancelled_write_bytes: u64,
}

#[derive(Debug, Serialize)]
struct QueryRunMetrics {
    first_page_latency_us: u64,
    total_latency_us: u64,
    page_count: usize,
    item_count: usize,
    rss_before_kib: u64,
    rss_after_kib: u64,
    rss_delta_kib: i64,
    vm_hwm_kib: u64,
    reads: StorageReadMetrics,
    process_io: ProcessIoMetrics,
}

#[derive(Debug, Serialize)]
struct CandidateRunResult {
    metrics: QueryRunMetrics,
    ordered_digest: String,
    external_state: EconomicExternalState,
    leader_item: CandidateSetViewItem,
    aggregate_collab_contribution: String,
    contributing_leader_count: usize,
}

#[derive(Debug, Serialize)]
struct BreakdownRunResult {
    metrics: QueryRunMetrics,
    ordered_digest: String,
    canonical_digest: String,
    external_state: EconomicExternalState,
    total: u64,
    aggregate_collab_contribution: String,
    fixed_leader_count: usize,
    address_leader_count: usize,
}

#[derive(Debug, Serialize)]
struct ProfileRunResult {
    latency_us: u64,
    rss_delta_kib: i64,
    reads: StorageReadMetrics,
    profile: PassEconomicProfileView,
}

#[derive(Debug, Serialize)]
struct CacheEvictionMetrics {
    file_count: usize,
    file_bytes: u64,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
struct ColdCacheRunResult {
    candidate_eviction: CacheEvictionMetrics,
    candidate: CandidateRunResult,
    breakdown_eviction: CacheEvictionMetrics,
    breakdown: BreakdownRunResult,
}

#[derive(Debug, Serialize)]
struct ConcurrentQueryRunResult {
    client_count: usize,
    iterations_per_client: usize,
    hot_leader_client_count: usize,
    candidate_traversal_count: usize,
    breakdown_traversal_count: usize,
    candidate_latency_p50_us: u64,
    candidate_latency_p95_us: u64,
    candidate_latency_max_us: u64,
    breakdown_latency_p50_us: u64,
    breakdown_latency_p95_us: u64,
    breakdown_latency_max_us: u64,
    wall_latency_us: u64,
    combined_metrics: QueryRunMetrics,
}

struct ConcurrentQuerySpec<'a> {
    pass_count: usize,
    leader: &'a ScaleLeaderFixture,
    page_limit: usize,
    pinned_state: &'a EconomicExternalState,
    expected_candidate_digest: &'a str,
    expected_breakdown_digest: &'a str,
    client_count: usize,
    iterations_per_client: usize,
}

#[derive(Debug, Serialize)]
struct CacheProbeResult {
    block_height: u32,
    expected_cache_hit: bool,
    observed_cache_hit: bool,
    cache_entry_count: usize,
    ordered_digest: String,
    metrics: QueryRunMetrics,
}

#[derive(Debug, Serialize)]
struct HistoricalCacheEvictionResult {
    cache_max_entries: usize,
    query_sequence: Vec<u32>,
    candidate_probes: Vec<CacheProbeResult>,
    breakdown_probes: Vec<CacheProbeResult>,
}

#[derive(Debug, Serialize)]
struct ColdConcurrentRunResult {
    client_count: usize,
    traversal_count: usize,
    latency_p50_us: u64,
    latency_p95_us: u64,
    latency_max_us: u64,
    wall_latency_us: u64,
    ordered_digest: String,
    combined_metrics: QueryRunMetrics,
}

#[derive(Debug, Serialize)]
struct ColdFirstDerivationResult {
    candidate_single: CandidateRunResult,
    candidate_concurrent: ColdConcurrentRunResult,
    breakdown_single: BreakdownRunResult,
    breakdown_concurrent: ColdConcurrentRunResult,
}

#[derive(Debug, Serialize)]
struct ChurnBranchResult {
    standard_remint_count: usize,
    collab_remint_count: usize,
    apply_latency_ms: u64,
    candidate: CandidateRunResult,
    breakdown: BreakdownRunResult,
}

#[derive(Debug, Serialize)]
struct ChurnReorgReplayResult {
    dormant_height: u32,
    remint_height: u32,
    rollback_latency_ms: u64,
    orphan_context_rejected: bool,
    orphan_error_code: i64,
    orphan: ChurnBranchResult,
    replacement: ChurnBranchResult,
    restart_base_candidate: CandidateRunResult,
    restart_base_breakdown: BreakdownRunResult,
    restart_replacement_candidate: CandidateRunResult,
    restart_replacement_breakdown: BreakdownRunResult,
}

#[derive(Debug, Serialize)]
struct EconomicCapacitySupplementReport {
    report_version: &'static str,
    build_profile: &'static str,
    standard_pass_count: usize,
    collab_pass_count: usize,
    leader_count: usize,
    churn_count_per_kind: usize,
    page_limit: usize,
    cold_start_client_count: usize,
    fixture_build_latency_ms: u64,
    fixture_rss_delta_kib: i64,
    database_size_bytes_before_churn: u64,
    database_size_bytes_after_replay: u64,
    historical_cache_eviction: HistoricalCacheEvictionResult,
    cold_first_derivation: ColdFirstDerivationResult,
    churn_reorg_replay: ChurnReorgReplayResult,
}

#[derive(Debug, Serialize)]
struct EconomicScaleReport {
    report_version: &'static str,
    build_profile: &'static str,
    standard_pass_count: usize,
    collab_pass_count: usize,
    fixed_collab_count: usize,
    address_collab_count: usize,
    total_pass_count: usize,
    leader_count: usize,
    min_collab_per_leader: usize,
    max_collab_per_leader: usize,
    page_limit: usize,
    fixture_build_latency_ms: u64,
    fixture_rss_delta_kib: i64,
    database_size_bytes: u64,
    expected_collab_contribution: String,
    expected_total_collab_contribution: String,
    candidate_initial: CandidateRunResult,
    candidate_replay: CandidateRunResult,
    breakdown_by_pass_id: BreakdownRunResult,
    breakdown_by_pass_id_replay: BreakdownRunResult,
    breakdown_by_contribution: BreakdownRunResult,
    leader_profile: ProfileRunResult,
    restart_candidate_replay: CandidateRunResult,
    restart_breakdown_replay: BreakdownRunResult,
    cold_cache: Option<ColdCacheRunResult>,
    concurrent_queries: Option<ConcurrentQueryRunResult>,
}

struct MetricStart {
    started_at: Instant,
    rss_before_kib: u64,
    process_io_before: ProcessIoSnapshot,
}

fn trace_sql(event: rusqlite::trace::TraceEvent<'_>) {
    match event {
        rusqlite::trace::TraceEvent::Stmt(_, statement) => {
            SQLITE_STATEMENTS.fetch_add(1, Ordering::Relaxed);
            let normalized = statement.trim_start().to_ascii_lowercase();
            if normalized.starts_with("select") || normalized.starts_with("with") {
                SQLITE_READ_STATEMENTS.fetch_add(1, Ordering::Relaxed);
                if normalized.contains("miner_pass_state_history") {
                    SQLITE_HISTORY_READ_STATEMENTS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        rusqlite::trace::TraceEvent::Row(_) => {
            SQLITE_RESULT_ROWS.fetch_add(1, Ordering::Relaxed);
        }
        rusqlite::trace::TraceEvent::Profile(statement, _) => {
            add_statement_status(
                &SQLITE_VM_STEPS,
                statement.get_status(rusqlite::StatementStatus::VmStep),
            );
            add_statement_status(
                &SQLITE_FULLSCAN_STEPS,
                statement.get_status(rusqlite::StatementStatus::FullscanStep),
            );
            add_statement_status(
                &SQLITE_SORTS,
                statement.get_status(rusqlite::StatementStatus::Sort),
            );
        }
        _ => {}
    }
}

fn add_statement_status(counter: &AtomicU64, value: i32) {
    counter.fetch_add(u64::try_from(value).unwrap_or(0), Ordering::Relaxed);
}

fn reset_read_metrics(server: &UsdbIndexerRpcServer) -> MetricStart {
    SQLITE_STATEMENTS.store(0, Ordering::Relaxed);
    SQLITE_READ_STATEMENTS.store(0, Ordering::Relaxed);
    SQLITE_HISTORY_READ_STATEMENTS.store(0, Ordering::Relaxed);
    SQLITE_RESULT_ROWS.store(0, Ordering::Relaxed);
    SQLITE_VM_STEPS.store(0, Ordering::Relaxed);
    SQLITE_FULLSCAN_STEPS.store(0, Ordering::Relaxed);
    SQLITE_SORTS.store(0, Ordering::Relaxed);
    server
        .indexer
        .pass_energy_manager()
        .reset_storage_read_metrics_for_test();
    MetricStart {
        started_at: Instant::now(),
        rss_before_kib: process_memory_kib().0,
        process_io_before: process_io_snapshot(),
    }
}

fn finish_read_metrics(
    server: &UsdbIndexerRpcServer,
    start: MetricStart,
    first_page_latency: Duration,
    page_count: usize,
    item_count: usize,
) -> QueryRunMetrics {
    let (rss_after_kib, vm_hwm_kib) = process_memory_kib();
    let process_io_after = process_io_snapshot();
    let rocks = server
        .indexer
        .pass_energy_manager()
        .storage_read_metrics_for_test();
    QueryRunMetrics {
        first_page_latency_us: duration_us(first_page_latency),
        total_latency_us: duration_us(start.started_at.elapsed()),
        page_count,
        item_count,
        rss_before_kib: start.rss_before_kib,
        rss_after_kib,
        rss_delta_kib: signed_delta(rss_after_kib, start.rss_before_kib),
        vm_hwm_kib,
        reads: StorageReadMetrics {
            sqlite_statements: SQLITE_STATEMENTS.load(Ordering::Relaxed),
            sqlite_read_statements: SQLITE_READ_STATEMENTS.load(Ordering::Relaxed),
            sqlite_history_read_statements: SQLITE_HISTORY_READ_STATEMENTS.load(Ordering::Relaxed),
            sqlite_result_rows: SQLITE_RESULT_ROWS.load(Ordering::Relaxed),
            sqlite_vm_steps: SQLITE_VM_STEPS.load(Ordering::Relaxed),
            sqlite_fullscan_steps: SQLITE_FULLSCAN_STEPS.load(Ordering::Relaxed),
            sqlite_sorts: SQLITE_SORTS.load(Ordering::Relaxed),
            rocksdb_point_gets: rocks.point_gets,
            rocksdb_iterator_seeks: rocks.iterator_seeks,
            rocksdb_records_decoded: rocks.records_decoded,
        },
        process_io: ProcessIoMetrics {
            read_chars: process_io_after
                .read_chars
                .saturating_sub(start.process_io_before.read_chars),
            write_chars: process_io_after
                .write_chars
                .saturating_sub(start.process_io_before.write_chars),
            read_syscalls: process_io_after
                .read_syscalls
                .saturating_sub(start.process_io_before.read_syscalls),
            write_syscalls: process_io_after
                .write_syscalls
                .saturating_sub(start.process_io_before.write_syscalls),
            physical_read_bytes: process_io_after
                .physical_read_bytes
                .saturating_sub(start.process_io_before.physical_read_bytes),
            physical_write_bytes: process_io_after
                .physical_write_bytes
                .saturating_sub(start.process_io_before.physical_write_bytes),
            cancelled_write_bytes: process_io_after
                .cancelled_write_bytes
                .saturating_sub(start.process_io_before.cancelled_write_bytes),
        },
    }
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn signed_delta(after: u64, before: u64) -> i64 {
    i128::from(after)
        .saturating_sub(i128::from(before))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn update_ordered_digest<T: Serialize>(hasher: &mut Sha256, item: &T) {
    let encoded = serde_json::to_vec(item).unwrap();
    hasher.update(u64::try_from(encoded.len()).unwrap().to_be_bytes());
    hasher.update(encoded);
}

fn process_memory_kib() -> (u64, u64) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return (0, 0);
    };
    let parse = |field: &str| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(field).and_then(|value| {
                    value
                        .split_whitespace()
                        .next()
                        .and_then(|number| number.parse::<u64>().ok())
                })
            })
            .unwrap_or(0)
    };
    (parse("VmRSS:"), parse("VmHWM:"))
}

fn process_io_snapshot() -> ProcessIoSnapshot {
    let Ok(io) = fs::read_to_string("/proc/self/io") else {
        return ProcessIoSnapshot::default();
    };
    let parse = |field: &str| {
        io.lines()
            .find_map(|line| {
                line.strip_prefix(field)
                    .and_then(|value| value.trim().parse::<u64>().ok())
            })
            .unwrap_or(0)
    };
    ProcessIoSnapshot {
        read_chars: parse("rchar:"),
        write_chars: parse("wchar:"),
        read_syscalls: parse("syscr:"),
        write_syscalls: parse("syscw:"),
        physical_read_bytes: parse("read_bytes:"),
        physical_write_bytes: parse("write_bytes:"),
        cancelled_write_bytes: parse("cancelled_write_bytes:"),
    }
}

fn directory_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| directory_size(&entry.path()))
        .fold(0u64, u64::saturating_add)
}

fn collect_regular_files(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        collect_regular_files(&entry.path(), files);
    }
}

fn evict_directory_page_cache(path: &Path) -> CacheEvictionMetrics {
    let started_at = Instant::now();
    let mut files = Vec::new();
    collect_regular_files(path, &mut files);
    let mut file_bytes = 0u64;
    for file in &files {
        let metadata = fs::metadata(file).unwrap();
        if metadata.len() == 0 {
            continue;
        }
        file_bytes = file_bytes.saturating_add(metadata.len());
        fs::File::open(file).unwrap().sync_all().unwrap();
        let status = Command::new("dd")
            .arg(format!("if={}", file.display()))
            .arg("of=/dev/null")
            .arg("iflag=nocache")
            .arg("status=none")
            .status()
            .unwrap();
        assert!(
            status.success(),
            "failed to evict file page cache: {}",
            file.display()
        );
    }
    CacheEvictionMetrics {
        file_count: files.len(),
        file_bytes,
        elapsed_ms: duration_ms(started_at.elapsed()),
    }
}

fn scale_inscription_id(namespace: u8, index: u32) -> InscriptionId {
    InscriptionId {
        txid: Txid::from_slice(&[namespace; 32]).unwrap(),
        index,
    }
}

fn scale_owner(namespace: u8, index: u32) -> BtcScriptHash {
    let mut script = vec![namespace; 36];
    script[32..].copy_from_slice(&index.to_be_bytes());
    ScriptBuf::from(script).to_btc_script_hash()
}

fn scale_satpoint(namespace: u8, index: u32) -> SatPoint {
    SatPoint {
        outpoint: OutPoint {
            txid: Txid::from_slice(&[namespace; 32]).unwrap(),
            vout: index,
        },
        offset: 0,
    }
}

struct ScaleLeaderFixture {
    pass: MinerPassInfo,
    address: String,
    owner: BtcScriptHash,
    expected_contribution: Energy,
    collab_count: usize,
    fixed_collab_count: usize,
    address_collab_count: usize,
}

struct ScaleFixture {
    leaders: Vec<ScaleLeaderFixture>,
    total_expected_contribution: Energy,
    fixed_collab_count: usize,
    address_collab_count: usize,
    min_collab_per_leader: usize,
    max_collab_per_leader: usize,
}

enum ScaleLeaderRef<'a> {
    None,
    PassId(InscriptionId),
    Address {
        address: &'a str,
        owner: BtcScriptHash,
    },
}

fn scale_pass(
    namespace: u8,
    index: u32,
    inscription_number: i32,
    owner: BtcScriptHash,
    pass_kind: MinerPassKind,
    leader_ref: ScaleLeaderRef<'_>,
) -> MinerPassInfo {
    let inscription_id = scale_inscription_id(namespace, index);
    let (leader_pass_id, leader_btc_addr, leader_btc_owner) = match leader_ref {
        ScaleLeaderRef::None => (None, None, None),
        ScaleLeaderRef::PassId(pass_id) => (Some(pass_id), None, None),
        ScaleLeaderRef::Address { address, owner } => {
            (None, Some(address.to_string()), Some(owner))
        }
    };
    MinerPassInfo {
        inscription_id,
        inscription_number,
        mint_txid: inscription_id.txid,
        mint_block_height: MINT_HEIGHT,
        mint_owner: owner,
        satpoint: scale_satpoint(namespace, index),
        mint_version: 1,
        pass_kind,
        usdb_main: if pass_kind == MinerPassKind::Standard {
            "0x1111111111111111111111111111111111111111".to_string()
        } else {
            String::new()
        },
        leader_pass_id,
        leader_btc_addr,
        leader_btc_owner,
        prev: Vec::new(),
        invalid_code: None,
        invalid_reason: None,
        owner,
        state: MinerPassState::Active,
    }
}

fn scale_leader_identity(index: usize) -> (String, BtcScriptHash) {
    if index == 0 {
        let owner =
            address_string_to_script_hash(MAINNET_LEADER_ADDRESS, &Network::Bitcoin).unwrap();
        return (MAINNET_LEADER_ADDRESS.to_string(), owner);
    }
    let mut script = vec![0x51, 0x04];
    script.extend_from_slice(&u32::try_from(index).unwrap().to_be_bytes());
    let address = Address::p2wsh(&ScriptBuf::from_bytes(script), Network::Bitcoin);
    let owner = address.script_pubkey().to_btc_script_hash();
    (address.to_string(), owner)
}

fn seed_scale_fixture(
    server: &UsdbIndexerRpcServer,
    pass_count: usize,
    leader_count: usize,
) -> ScaleFixture {
    let storage = server.indexer.miner_pass_storage();
    let mut records = Vec::with_capacity(pass_count.saturating_mul(2));
    let fixed_count = pass_count / 2;
    let address_count = pass_count.saturating_sub(fixed_count);
    let leader_identities = (0..leader_count)
        .map(scale_leader_identity)
        .collect::<Vec<_>>();
    let mut leaders = Vec::with_capacity(leader_count);

    storage.savepoint_begin().unwrap();
    for index in 0..pass_count {
        let index_u32 = u32::try_from(index).unwrap();
        let leader_identity = leader_identities.get(index);
        let owner = leader_identity
            .map(|(_, owner)| *owner)
            .unwrap_or_else(|| scale_owner(11, index_u32));
        let pass = scale_pass(
            1,
            index_u32,
            i32::try_from(index).unwrap(),
            owner,
            MinerPassKind::Standard,
            ScaleLeaderRef::None,
        );
        storage
            .add_new_mint_pass_at_height(&pass, MINT_HEIGHT)
            .unwrap();
        let raw_energy = 1_000_000u128.saturating_add((pass_count - index) as u128);
        records.push(PassEnergyRecord {
            inscription_id: pass.inscription_id,
            block_height: QUERY_HEIGHT,
            state: MinerPassState::Active,
            active_block_height: MINT_HEIGHT,
            owner_address: pass.owner,
            owner_balance: 100_000,
            owner_delta: 0,
            energy: raw_energy,
        });
        if let Some((address, _)) = leader_identity {
            leaders.push(ScaleLeaderFixture {
                pass,
                address: address.clone(),
                owner,
                expected_contribution: 0,
                collab_count: 0,
                fixed_collab_count: 0,
                address_collab_count: 0,
            });
        }
    }

    for index in 0..pass_count {
        let index_u32 = u32::try_from(index).unwrap();
        let fixed = index < fixed_count;
        let leader_index = index % leader_count;
        let leader_pass_id = leaders[leader_index].pass.inscription_id;
        let leader_address = leaders[leader_index].address.clone();
        let leader_owner = leaders[leader_index].owner;
        let leader_ref = if fixed {
            ScaleLeaderRef::PassId(leader_pass_id)
        } else {
            ScaleLeaderRef::Address {
                address: &leader_address,
                owner: leader_owner,
            }
        };
        let pass = scale_pass(
            2,
            index_u32,
            i32::try_from(pass_count + index).unwrap(),
            scale_owner(12, index_u32),
            MinerPassKind::Collab,
            leader_ref,
        );
        storage
            .add_new_mint_pass_at_height(&pass, MINT_HEIGHT)
            .unwrap();
        let raw_energy = ((index as u128).saturating_add(1)).saturating_mul(2);
        let contribution = calc_collab_contribution(raw_energy);
        let leader = &mut leaders[leader_index];
        leader.expected_contribution = leader.expected_contribution.saturating_add(contribution);
        leader.collab_count = leader.collab_count.saturating_add(1);
        if fixed {
            leader.fixed_collab_count = leader.fixed_collab_count.saturating_add(1);
        } else {
            leader.address_collab_count = leader.address_collab_count.saturating_add(1);
        }
        records.push(PassEnergyRecord {
            inscription_id: pass.inscription_id,
            block_height: QUERY_HEIGHT,
            state: MinerPassState::Active,
            active_block_height: MINT_HEIGHT,
            owner_address: pass.owner,
            owner_balance: 100_000,
            owner_delta: 0,
            energy: raw_energy,
        });
    }
    storage.savepoint_commit().unwrap();
    server
        .indexer
        .pass_energy_manager()
        .insert_pass_energy_records_for_test(&records)
        .unwrap();
    seed_state_ref_context(server, QUERY_HEIGHT);

    let min_collab_per_leader = leaders
        .iter()
        .map(|leader| leader.collab_count)
        .min()
        .unwrap();
    let max_collab_per_leader = leaders
        .iter()
        .map(|leader| leader.collab_count)
        .max()
        .unwrap();
    let total_expected_contribution = leaders.iter().fold(0u128, |total, leader| {
        total.saturating_add(leader.expected_contribution)
    });
    ScaleFixture {
        leaders,
        total_expected_contribution,
        fixed_collab_count: fixed_count,
        address_collab_count: address_count,
        min_collab_per_leader,
        max_collab_per_leader,
    }
}

fn expected_collab_contribution_at_height(
    pass_count: usize,
    leader_count: usize,
    leader_index: Option<usize>,
    block_height: u32,
) -> Energy {
    let growth = calc_growth_delta(100_000, block_height.saturating_sub(QUERY_HEIGHT));
    (0..pass_count).fold(0u128, |total, index| {
        if leader_index.is_some_and(|leader| index % leader_count != leader) {
            return total;
        }
        let raw_energy = ((index as u128).saturating_add(1))
            .saturating_mul(2)
            .saturating_add(growth);
        total.saturating_add(calc_collab_contribution(raw_energy))
    })
}

fn seed_state_ref_context_with_identity(
    server: &UsdbIndexerRpcServer,
    block_height: u32,
    hash_byte: u8,
    commit_byte: u8,
) -> balance_history::SnapshotInfo {
    let mut snapshot = ready_balance_history_snapshot(block_height);
    snapshot.stable_block_hash = Some(format!("{hash_byte:02x}").repeat(32));
    snapshot.latest_block_commit = Some(format!("{commit_byte:02x}").repeat(32));
    let mut readiness = ready_balance_history_readiness(block_height);
    readiness.stable_block_hash = snapshot.stable_block_hash.clone();
    readiness.latest_block_commit = snapshot.latest_block_commit.clone();
    server
        .status
        .set_balance_history_snapshot(Some(snapshot.clone()));
    server.status.set_balance_history_readiness(Some(readiness));
    server
        .indexer
        .miner_pass_storage()
        .upsert_balance_history_snapshot_anchor(&snapshot)
        .unwrap();
    server
        .indexer
        .miner_pass_storage()
        .upsert_active_balance_snapshot(block_height, 5_000, 2)
        .unwrap();
    snapshot
}

fn resolve_external_state(
    server: &UsdbIndexerRpcServer,
    block_height: u32,
) -> EconomicExternalState {
    let (_, state_ref) = server
        .resolve_economic_query_context(USDB_ECONOMIC_STATE_VIEW_VERSION, Some(block_height), None)
        .unwrap();
    EconomicExternalState::from(&state_ref)
}

fn collect_candidate_pages(
    server: &UsdbIndexerRpcServer,
    expected_total: usize,
    leader_id: &str,
    expected_total_contribution: Energy,
    expected_contributing_leaders: usize,
    limit: usize,
    pinned_state: Option<&EconomicExternalState>,
) -> CandidateRunResult {
    collect_candidate_pages_at_height(
        server,
        CandidateCollectionSpec {
            query_height: QUERY_HEIGHT,
            expected_total,
            leader_id,
            expected_total_contribution,
            expected_contributing_leaders,
            limit,
            pinned_state,
        },
    )
}

struct CandidateCollectionSpec<'a> {
    query_height: u32,
    expected_total: usize,
    leader_id: &'a str,
    expected_total_contribution: Energy,
    expected_contributing_leaders: usize,
    limit: usize,
    pinned_state: Option<&'a EconomicExternalState>,
}

fn collect_candidate_pages_at_height(
    server: &UsdbIndexerRpcServer,
    spec: CandidateCollectionSpec<'_>,
) -> CandidateRunResult {
    let CandidateCollectionSpec {
        query_height,
        expected_total,
        leader_id,
        expected_total_contribution,
        expected_contributing_leaders,
        limit,
        pinned_state,
    } = spec;
    let metric_start = reset_read_metrics(server);
    let mut cursor = None;
    let mut page_count = 0usize;
    let mut item_count = 0usize;
    let mut first_page_latency = Duration::ZERO;
    let mut hasher = Sha256::new();
    let mut external_state = None;
    let mut leader_item = None;
    let mut previous_item: Option<(Energy, String)> = None;
    let mut aggregate_collab_contribution: Energy = 0;
    let mut contributing_leader_count = 0usize;

    loop {
        let page_started = Instant::now();
        let page = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: cursor.is_none().then_some(query_height),
                context: if cursor.is_none() {
                    pinned_state.map(ConsensusQueryContext::from)
                } else {
                    None
                },
                selection_rule: None,
                cursor,
                limit,
            })
            .unwrap();
        let page_latency = page_started.elapsed();
        if page_count == 0 {
            first_page_latency = page_latency;
            external_state = Some(page.external_state.clone());
        }
        if let Some(pinned_state) = pinned_state {
            assert_eq!(&page.external_state, pinned_state);
        }
        assert_eq!(page.total, expected_total as u64);
        assert!(
            page.items
                .iter()
                .all(|item| { item.state == "active" && item.pass_kind == "standard" })
        );
        for item in &page.items {
            let effective_energy = item.effective_energy.parse::<Energy>().unwrap();
            let collab_contribution = item.collab_contribution.parse::<Energy>().unwrap();
            if let Some((previous_energy, previous_pass_id)) = previous_item.as_ref() {
                assert!(
                    *previous_energy > effective_energy
                        || (*previous_energy == effective_energy
                            && previous_pass_id <= &item.pass_id)
                );
            }
            previous_item = Some((effective_energy, item.pass_id.clone()));
            if item.pass_id == leader_id {
                leader_item = Some(item.clone());
            }
            aggregate_collab_contribution =
                aggregate_collab_contribution.saturating_add(collab_contribution);
            if collab_contribution > 0 {
                contributing_leader_count = contributing_leader_count.saturating_add(1);
            }
            update_ordered_digest(&mut hasher, item);
        }
        item_count = item_count.saturating_add(page.items.len());
        page_count = page_count.saturating_add(1);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(item_count, expected_total);
    assert_eq!(aggregate_collab_contribution, expected_total_contribution);
    assert_eq!(contributing_leader_count, expected_contributing_leaders);
    CandidateRunResult {
        metrics: finish_read_metrics(
            server,
            metric_start,
            first_page_latency,
            page_count,
            item_count,
        ),
        ordered_digest: encode_hex(&hasher.finalize()),
        external_state: external_state.unwrap(),
        leader_item: leader_item.expect("Leader must be present in candidate set"),
        aggregate_collab_contribution: aggregate_collab_contribution.to_string(),
        contributing_leader_count,
    }
}

fn collect_breakdown_pages(
    server: &UsdbIndexerRpcServer,
    leader_id: &str,
    expected_total: usize,
    limit: usize,
    sort: &str,
    pinned_state: Option<&EconomicExternalState>,
) -> BreakdownRunResult {
    collect_breakdown_pages_at_height(
        server,
        QUERY_HEIGHT,
        leader_id,
        expected_total,
        limit,
        sort,
        pinned_state,
    )
}

fn collect_breakdown_pages_at_height(
    server: &UsdbIndexerRpcServer,
    query_height: u32,
    leader_id: &str,
    expected_total: usize,
    limit: usize,
    sort: &str,
    pinned_state: Option<&EconomicExternalState>,
) -> BreakdownRunResult {
    let metric_start = reset_read_metrics(server);
    let mut cursor = None;
    let mut page_count = 0usize;
    let mut item_count = 0usize;
    let mut first_page_latency = Duration::ZERO;
    let mut ordered_hasher = Sha256::new();
    let mut canonical_items = BTreeMap::new();
    let mut external_state = None;
    let mut aggregate = None;
    let mut contribution_sum: Energy = 0;
    let mut fixed_leader_count = 0usize;
    let mut address_leader_count = 0usize;
    let mut previous_contribution = None;
    let mut previous_pass_id = String::new();

    loop {
        let page_started = Instant::now();
        let page = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader_id.to_string(),
                block_height: cursor.is_none().then_some(query_height),
                context: if cursor.is_none() {
                    pinned_state.map(ConsensusQueryContext::from)
                } else {
                    None
                },
                sort: Some(sort.to_string()),
                cursor,
                limit,
            })
            .unwrap();
        let page_latency = page_started.elapsed();
        if page_count == 0 {
            first_page_latency = page_latency;
            external_state = Some(page.external_state.clone());
            aggregate = Some(page.aggregate_collab_contribution.clone());
        }
        if let Some(pinned_state) = pinned_state {
            assert_eq!(&page.external_state, pinned_state);
        }
        assert_eq!(page.total, expected_total as u64);
        assert_eq!(
            aggregate.as_ref(),
            Some(&page.aggregate_collab_contribution)
        );

        for item in &page.items {
            let contribution = item.collab_contribution.parse::<Energy>().unwrap();
            if sort == "contribution_desc_pass_id_asc" {
                if let Some(previous) = previous_contribution {
                    assert!(
                        previous > contribution
                            || (previous == contribution
                                && previous_pass_id <= item.collab_pass_id)
                    );
                }
                previous_contribution = Some(contribution);
                previous_pass_id.clone_from(&item.collab_pass_id);
            }
            contribution_sum = contribution_sum.saturating_add(contribution);
            match item.leader_ref_kind.as_str() {
                "leader_pass_id" => fixed_leader_count = fixed_leader_count.saturating_add(1),
                "leader_btc_addr" => address_leader_count = address_leader_count.saturating_add(1),
                other => panic!("unexpected Leader reference kind: {other}"),
            }
            assert!(
                canonical_items
                    .insert(
                        item.collab_pass_id.clone(),
                        item.collab_contribution.clone(),
                    )
                    .is_none()
            );
        }
        for item in &page.items {
            update_ordered_digest(&mut ordered_hasher, item);
        }
        item_count = item_count.saturating_add(page.items.len());
        page_count = page_count.saturating_add(1);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(item_count, expected_total);
    assert_eq!(contribution_sum.to_string(), aggregate.clone().unwrap());
    let canonical_digest = encode_hex(&Sha256::digest(
        serde_json::to_vec(&canonical_items).unwrap(),
    ));
    BreakdownRunResult {
        metrics: finish_read_metrics(
            server,
            metric_start,
            first_page_latency,
            page_count,
            item_count,
        ),
        ordered_digest: encode_hex(&ordered_hasher.finalize()),
        canonical_digest,
        external_state: external_state.unwrap(),
        total: item_count as u64,
        aggregate_collab_contribution: aggregate.unwrap(),
        fixed_leader_count,
        address_leader_count,
    }
}

struct TraversalMetrics {
    elapsed: Duration,
    page_count: usize,
    item_count: usize,
}

fn replay_candidate_pages(
    server: &UsdbIndexerRpcServer,
    expected_total: usize,
    expected_digest: &str,
    limit: usize,
    pinned_state: &EconomicExternalState,
) -> TraversalMetrics {
    replay_candidate_pages_at_height(
        server,
        QUERY_HEIGHT,
        expected_total,
        expected_digest,
        limit,
        pinned_state,
    )
}

fn replay_candidate_pages_at_height(
    server: &UsdbIndexerRpcServer,
    query_height: u32,
    expected_total: usize,
    expected_digest: &str,
    limit: usize,
    pinned_state: &EconomicExternalState,
) -> TraversalMetrics {
    let started_at = Instant::now();
    let mut cursor = None;
    let mut page_count = 0usize;
    let mut item_count = 0usize;
    let mut hasher = Sha256::new();
    loop {
        let page = server
            .get_candidate_set_view(GetCandidateSetViewParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                block_height: cursor.is_none().then_some(query_height),
                context: cursor
                    .is_none()
                    .then(|| ConsensusQueryContext::from(pinned_state)),
                selection_rule: None,
                cursor,
                limit,
            })
            .unwrap();
        assert_eq!(&page.external_state, pinned_state);
        assert_eq!(page.total, expected_total as u64);
        for item in &page.items {
            update_ordered_digest(&mut hasher, item);
        }
        item_count = item_count.saturating_add(page.items.len());
        page_count = page_count.saturating_add(1);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(item_count, expected_total);
    assert_eq!(encode_hex(&hasher.finalize()), expected_digest);
    TraversalMetrics {
        elapsed: started_at.elapsed(),
        page_count,
        item_count,
    }
}

fn replay_breakdown_pages(
    server: &UsdbIndexerRpcServer,
    leader_id: &str,
    expected_total: usize,
    expected_contribution: Energy,
    expected_digest: &str,
    limit: usize,
    pinned_state: &EconomicExternalState,
) -> TraversalMetrics {
    replay_breakdown_pages_at_height(
        server,
        BreakdownReplaySpec {
            query_height: QUERY_HEIGHT,
            leader_id,
            expected_total,
            expected_contribution,
            expected_digest,
            limit,
            pinned_state,
        },
    )
}

struct BreakdownReplaySpec<'a> {
    query_height: u32,
    leader_id: &'a str,
    expected_total: usize,
    expected_contribution: Energy,
    expected_digest: &'a str,
    limit: usize,
    pinned_state: &'a EconomicExternalState,
}

fn replay_breakdown_pages_at_height(
    server: &UsdbIndexerRpcServer,
    spec: BreakdownReplaySpec<'_>,
) -> TraversalMetrics {
    let BreakdownReplaySpec {
        query_height,
        leader_id,
        expected_total,
        expected_contribution,
        expected_digest,
        limit,
        pinned_state,
    } = spec;
    let started_at = Instant::now();
    let mut cursor = None;
    let mut page_count = 0usize;
    let mut item_count = 0usize;
    let mut hasher = Sha256::new();
    loop {
        let page = server
            .get_collab_breakdown(GetCollabBreakdownParams {
                view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
                leader_pass_id: leader_id.to_string(),
                block_height: cursor.is_none().then_some(query_height),
                context: cursor
                    .is_none()
                    .then(|| ConsensusQueryContext::from(pinned_state)),
                sort: Some("collab_pass_id_asc".to_string()),
                cursor,
                limit,
            })
            .unwrap();
        assert_eq!(&page.external_state, pinned_state);
        assert_eq!(page.total, expected_total as u64);
        assert_eq!(
            page.aggregate_collab_contribution,
            expected_contribution.to_string()
        );
        for item in &page.items {
            update_ordered_digest(&mut hasher, item);
        }
        item_count = item_count.saturating_add(page.items.len());
        page_count = page_count.saturating_add(1);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(item_count, expected_total);
    assert_eq!(encode_hex(&hasher.finalize()), expected_digest);
    TraversalMetrics {
        elapsed: started_at.elapsed(),
        page_count,
        item_count,
    }
}

fn duration_percentile(values: &mut [u64], percentile: usize) -> u64 {
    assert!(!values.is_empty());
    values.sort_unstable();
    let rank = percentile
        .saturating_mul(values.len())
        .div_ceil(100)
        .saturating_sub(1)
        .min(values.len() - 1);
    values[rank]
}

fn run_concurrent_queries(
    server: &UsdbIndexerRpcServer,
    spec: ConcurrentQuerySpec<'_>,
) -> ConcurrentQueryRunResult {
    let ConcurrentQuerySpec {
        pass_count,
        leader,
        page_limit,
        pinned_state,
        expected_candidate_digest,
        expected_breakdown_digest,
        client_count,
        iterations_per_client,
    } = spec;
    let metric_start = reset_read_metrics(server);
    let wall_started_at = Instant::now();
    let barrier = Arc::new(Barrier::new(client_count));
    let mut workers = Vec::with_capacity(client_count);
    for _ in 0..client_count {
        let server = server.clone();
        let barrier = barrier.clone();
        let pinned_state = pinned_state.clone();
        let leader_id = leader.pass.inscription_id.to_string();
        let expected_candidate_digest = expected_candidate_digest.to_string();
        let expected_breakdown_digest = expected_breakdown_digest.to_string();
        let expected_breakdown_total = leader.collab_count;
        let expected_contribution = leader.expected_contribution;
        workers.push(thread::spawn(move || {
            barrier.wait();
            let mut candidate_runs = Vec::with_capacity(iterations_per_client);
            let mut breakdown_runs = Vec::with_capacity(iterations_per_client);
            for _ in 0..iterations_per_client {
                candidate_runs.push(replay_candidate_pages(
                    &server,
                    pass_count,
                    &expected_candidate_digest,
                    page_limit,
                    &pinned_state,
                ));
                breakdown_runs.push(replay_breakdown_pages(
                    &server,
                    &leader_id,
                    expected_breakdown_total,
                    expected_contribution,
                    &expected_breakdown_digest,
                    page_limit,
                    &pinned_state,
                ));
            }
            (candidate_runs, breakdown_runs)
        }));
    }

    let mut candidate_latencies = Vec::new();
    let mut breakdown_latencies = Vec::new();
    let mut total_pages = 0usize;
    let mut total_items = 0usize;
    for worker in workers {
        let (candidate_runs, breakdown_runs) = worker.join().unwrap();
        for run in candidate_runs {
            candidate_latencies.push(duration_us(run.elapsed));
            total_pages = total_pages.saturating_add(run.page_count);
            total_items = total_items.saturating_add(run.item_count);
        }
        for run in breakdown_runs {
            breakdown_latencies.push(duration_us(run.elapsed));
            total_pages = total_pages.saturating_add(run.page_count);
            total_items = total_items.saturating_add(run.item_count);
        }
    }
    let wall_latency = wall_started_at.elapsed();
    let candidate_latency_max_us = *candidate_latencies.iter().max().unwrap();
    let breakdown_latency_max_us = *breakdown_latencies.iter().max().unwrap();
    let candidate_latency_p50_us = duration_percentile(&mut candidate_latencies, 50);
    let candidate_latency_p95_us = duration_percentile(&mut candidate_latencies, 95);
    let breakdown_latency_p50_us = duration_percentile(&mut breakdown_latencies, 50);
    let breakdown_latency_p95_us = duration_percentile(&mut breakdown_latencies, 95);
    let combined_metrics = finish_read_metrics(
        server,
        metric_start,
        Duration::ZERO,
        total_pages,
        total_items,
    );
    ConcurrentQueryRunResult {
        client_count,
        iterations_per_client,
        hot_leader_client_count: client_count,
        candidate_traversal_count: client_count.saturating_mul(iterations_per_client),
        breakdown_traversal_count: client_count.saturating_mul(iterations_per_client),
        candidate_latency_p50_us,
        candidate_latency_p95_us,
        candidate_latency_max_us,
        breakdown_latency_p50_us,
        breakdown_latency_p95_us,
        breakdown_latency_max_us,
        wall_latency_us: duration_us(wall_latency),
        combined_metrics,
    }
}

fn candidate_cache_contains(
    server: &UsdbIndexerRpcServer,
    external_state: &EconomicExternalState,
) -> bool {
    server
        .candidate_set_view_cache
        .lock()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.external_state == *external_state)
}

fn breakdown_cache_contains(
    server: &UsdbIndexerRpcServer,
    external_state: &EconomicExternalState,
    leader_id: &str,
) -> bool {
    server
        .collab_breakdown_cache
        .lock()
        .unwrap()
        .entries
        .iter()
        .any(|entry| {
            entry.external_state == *external_state
                && entry.leader_pass_id == leader_id
                && entry.sort == "collab_pass_id_asc"
        })
}

fn run_historical_cache_eviction(
    server: &UsdbIndexerRpcServer,
    pass_count: usize,
    leader_count: usize,
    primary_leader: &ScaleLeaderFixture,
    page_limit: usize,
    states: &BTreeMap<u32, EconomicExternalState>,
) -> HistoricalCacheEvictionResult {
    let query_sequence = vec![120, 121, 120, 122, 120, 121];
    let expected_hits = [false, false, true, false, true, false];
    let leader_id = primary_leader.pass.inscription_id.to_string();
    let mut candidate_digests = BTreeMap::new();
    let mut candidate_probes = Vec::with_capacity(query_sequence.len());
    for (&block_height, &expected_cache_hit) in query_sequence.iter().zip(&expected_hits) {
        let state = states.get(&block_height).unwrap();
        let observed_cache_hit = candidate_cache_contains(server, state);
        assert_eq!(observed_cache_hit, expected_cache_hit);
        let run = collect_candidate_pages_at_height(
            server,
            CandidateCollectionSpec {
                query_height: block_height,
                expected_total: pass_count,
                leader_id: &leader_id,
                expected_total_contribution: expected_collab_contribution_at_height(
                    pass_count,
                    leader_count,
                    None,
                    block_height,
                ),
                expected_contributing_leaders: leader_count,
                limit: page_limit,
                pinned_state: Some(state),
            },
        );
        if let Some(previous) = candidate_digests.insert(block_height, run.ordered_digest.clone()) {
            assert_eq!(run.ordered_digest, previous);
        }
        if expected_cache_hit {
            assert_eq!(run.metrics.reads.rocksdb_records_decoded, 0);
            assert_eq!(run.metrics.reads.rocksdb_iterator_seeks, 0);
        } else {
            assert!(run.metrics.reads.rocksdb_records_decoded > 0);
        }
        let cache_entry_count = server
            .candidate_set_view_cache
            .lock()
            .unwrap()
            .entries
            .len();
        assert!(cache_entry_count <= ECONOMIC_VIEW_CACHE_MAX_ENTRIES);
        candidate_probes.push(CacheProbeResult {
            block_height,
            expected_cache_hit,
            observed_cache_hit,
            cache_entry_count,
            ordered_digest: run.ordered_digest,
            metrics: run.metrics,
        });
    }

    let mut breakdown_digests = BTreeMap::new();
    let mut breakdown_probes = Vec::with_capacity(query_sequence.len());
    for (&block_height, &expected_cache_hit) in query_sequence.iter().zip(&expected_hits) {
        let state = states.get(&block_height).unwrap();
        let observed_cache_hit = breakdown_cache_contains(server, state, &leader_id);
        assert_eq!(observed_cache_hit, expected_cache_hit);
        let run = collect_breakdown_pages_at_height(
            server,
            block_height,
            &leader_id,
            primary_leader.collab_count,
            page_limit,
            "collab_pass_id_asc",
            Some(state),
        );
        assert_eq!(
            run.aggregate_collab_contribution,
            expected_collab_contribution_at_height(
                pass_count,
                leader_count,
                Some(0),
                block_height,
            )
            .to_string()
        );
        if let Some(previous) = breakdown_digests.insert(block_height, run.ordered_digest.clone()) {
            assert_eq!(run.ordered_digest, previous);
        }
        if expected_cache_hit {
            assert_eq!(run.metrics.reads.rocksdb_records_decoded, 0);
            assert_eq!(run.metrics.reads.rocksdb_iterator_seeks, 0);
        } else {
            assert!(run.metrics.reads.rocksdb_records_decoded > 0);
        }
        let cache_entry_count = server.collab_breakdown_cache.lock().unwrap().entries.len();
        assert!(cache_entry_count <= ECONOMIC_VIEW_CACHE_MAX_ENTRIES);
        breakdown_probes.push(CacheProbeResult {
            block_height,
            expected_cache_hit,
            observed_cache_hit,
            cache_entry_count,
            ordered_digest: run.ordered_digest,
            metrics: run.metrics,
        });
    }

    HistoricalCacheEvictionResult {
        cache_max_entries: ECONOMIC_VIEW_CACHE_MAX_ENTRIES,
        query_sequence,
        candidate_probes,
        breakdown_probes,
    }
}

fn run_cold_candidate_concurrent(
    server: &UsdbIndexerRpcServer,
    block_height: u32,
    expected_total: usize,
    expected_digest: &str,
    page_limit: usize,
    pinned_state: &EconomicExternalState,
    client_count: usize,
) -> ColdConcurrentRunResult {
    let metric_start = reset_read_metrics(server);
    let wall_started_at = Instant::now();
    let barrier = Arc::new(Barrier::new(client_count));
    let mut workers = Vec::with_capacity(client_count);
    for _ in 0..client_count {
        let server = server.clone();
        let barrier = barrier.clone();
        let pinned_state = pinned_state.clone();
        let expected_digest = expected_digest.to_string();
        workers.push(thread::spawn(move || {
            barrier.wait();
            replay_candidate_pages_at_height(
                &server,
                block_height,
                expected_total,
                &expected_digest,
                page_limit,
                &pinned_state,
            )
        }));
    }

    let mut latencies = Vec::with_capacity(client_count);
    let mut total_pages = 0usize;
    let mut total_items = 0usize;
    for worker in workers {
        let run = worker.join().unwrap();
        latencies.push(duration_us(run.elapsed));
        total_pages = total_pages.saturating_add(run.page_count);
        total_items = total_items.saturating_add(run.item_count);
    }
    let wall_latency = wall_started_at.elapsed();
    let latency_max_us = *latencies.iter().max().unwrap();
    let latency_p50_us = duration_percentile(&mut latencies, 50);
    let latency_p95_us = duration_percentile(&mut latencies, 95);
    ColdConcurrentRunResult {
        client_count,
        traversal_count: client_count,
        latency_p50_us,
        latency_p95_us,
        latency_max_us,
        wall_latency_us: duration_us(wall_latency),
        ordered_digest: expected_digest.to_string(),
        combined_metrics: finish_read_metrics(
            server,
            metric_start,
            Duration::ZERO,
            total_pages,
            total_items,
        ),
    }
}

struct ColdBreakdownSpec<'a> {
    block_height: u32,
    leader_id: &'a str,
    expected_total: usize,
    expected_contribution: Energy,
    expected_digest: &'a str,
    page_limit: usize,
    pinned_state: &'a EconomicExternalState,
    client_count: usize,
}

fn run_cold_breakdown_concurrent(
    server: &UsdbIndexerRpcServer,
    spec: ColdBreakdownSpec<'_>,
) -> ColdConcurrentRunResult {
    let metric_start = reset_read_metrics(server);
    let wall_started_at = Instant::now();
    let barrier = Arc::new(Barrier::new(spec.client_count));
    let mut workers = Vec::with_capacity(spec.client_count);
    for _ in 0..spec.client_count {
        let server = server.clone();
        let barrier = barrier.clone();
        let pinned_state = spec.pinned_state.clone();
        let leader_id = spec.leader_id.to_string();
        let expected_digest = spec.expected_digest.to_string();
        let block_height = spec.block_height;
        let expected_total = spec.expected_total;
        let expected_contribution = spec.expected_contribution;
        let page_limit = spec.page_limit;
        workers.push(thread::spawn(move || {
            barrier.wait();
            replay_breakdown_pages_at_height(
                &server,
                BreakdownReplaySpec {
                    query_height: block_height,
                    leader_id: &leader_id,
                    expected_total,
                    expected_contribution,
                    expected_digest: &expected_digest,
                    limit: page_limit,
                    pinned_state: &pinned_state,
                },
            )
        }));
    }

    let mut latencies = Vec::with_capacity(spec.client_count);
    let mut total_pages = 0usize;
    let mut total_items = 0usize;
    for worker in workers {
        let run = worker.join().unwrap();
        latencies.push(duration_us(run.elapsed));
        total_pages = total_pages.saturating_add(run.page_count);
        total_items = total_items.saturating_add(run.item_count);
    }
    let wall_latency = wall_started_at.elapsed();
    let latency_max_us = *latencies.iter().max().unwrap();
    let latency_p50_us = duration_percentile(&mut latencies, 50);
    let latency_p95_us = duration_percentile(&mut latencies, 95);
    ColdConcurrentRunResult {
        client_count: spec.client_count,
        traversal_count: spec.client_count,
        latency_p50_us,
        latency_p95_us,
        latency_max_us,
        wall_latency_us: duration_us(wall_latency),
        ordered_digest: spec.expected_digest.to_string(),
        combined_metrics: finish_read_metrics(
            server,
            metric_start,
            Duration::ZERO,
            total_pages,
            total_items,
        ),
    }
}

struct AppliedChurnBranch {
    standard_remint_count: usize,
    collab_remint_count: usize,
    total_collab_contribution: Energy,
    primary_leader_contribution: Energy,
    apply_latency_ms: u64,
}

fn apply_churn_branch(
    server: &UsdbIndexerRpcServer,
    pass_count: usize,
    leader_count: usize,
    churn_count: usize,
    standard_namespace: u8,
    collab_namespace: u8,
) -> AppliedChurnBranch {
    use crate::index::energy_formula::calc_inheritable_energy;

    let started_at = Instant::now();
    let storage = server.indexer.miner_pass_storage();
    let mut energy_records = Vec::with_capacity(churn_count.saturating_mul(6));
    storage.savepoint_begin().unwrap();

    for offset in 0..churn_count {
        let index = leader_count.saturating_add(offset);
        let index_u32 = u32::try_from(index).unwrap();
        let old_id = scale_inscription_id(1, index_u32);
        let old = storage
            .get_pass_by_inscription_id(&old_id)
            .unwrap()
            .expect("standard prev pass must exist");
        let dormant_energy = 1_000_000u128
            .saturating_add((pass_count - index) as u128)
            .saturating_add(calc_growth_delta(
                100_000,
                CHURN_DORMANT_HEIGHT - QUERY_HEIGHT,
            ));
        storage
            .update_state_at_height(
                &old_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                CHURN_DORMANT_HEIGHT,
            )
            .unwrap();
        storage
            .update_state_at_height(
                &old_id,
                MinerPassState::Consumed,
                MinerPassState::Dormant,
                CHURN_REMINT_HEIGHT,
            )
            .unwrap();
        let mut remint = scale_pass(
            standard_namespace,
            index_u32,
            i32::try_from(pass_count.saturating_mul(2).saturating_add(index)).unwrap(),
            old.owner,
            MinerPassKind::Standard,
            ScaleLeaderRef::None,
        );
        remint.mint_block_height = CHURN_REMINT_HEIGHT;
        remint.prev = vec![old_id];
        storage
            .add_new_mint_pass_at_height(&remint, CHURN_REMINT_HEIGHT)
            .unwrap();
        energy_records.extend([
            PassEnergyRecord {
                inscription_id: old_id,
                block_height: CHURN_DORMANT_HEIGHT,
                state: MinerPassState::Dormant,
                active_block_height: MINT_HEIGHT,
                owner_address: old.owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: dormant_energy,
            },
            PassEnergyRecord {
                inscription_id: old_id,
                block_height: CHURN_REMINT_HEIGHT,
                state: MinerPassState::Consumed,
                active_block_height: CHURN_REMINT_HEIGHT,
                owner_address: old.owner,
                owner_balance: 0,
                owner_delta: 0,
                energy: 0,
            },
            PassEnergyRecord {
                inscription_id: remint.inscription_id,
                block_height: CHURN_REMINT_HEIGHT,
                state: MinerPassState::Active,
                active_block_height: CHURN_REMINT_HEIGHT,
                owner_address: remint.owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: calc_inheritable_energy(dormant_energy),
            },
        ]);
    }

    for index in 0..churn_count {
        let index_u32 = u32::try_from(index).unwrap();
        let old_id = scale_inscription_id(2, index_u32);
        let old = storage
            .get_pass_by_inscription_id(&old_id)
            .unwrap()
            .expect("collab prev pass must exist");
        let dormant_energy = ((index as u128).saturating_add(1))
            .saturating_mul(2)
            .saturating_add(calc_growth_delta(
                100_000,
                CHURN_DORMANT_HEIGHT - QUERY_HEIGHT,
            ));
        storage
            .update_state_at_height(
                &old_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                CHURN_DORMANT_HEIGHT,
            )
            .unwrap();
        storage
            .update_state_at_height(
                &old_id,
                MinerPassState::Consumed,
                MinerPassState::Dormant,
                CHURN_REMINT_HEIGHT,
            )
            .unwrap();
        let leader_ref = if let Some(leader_pass_id) = old.leader_pass_id {
            ScaleLeaderRef::PassId(leader_pass_id)
        } else {
            ScaleLeaderRef::Address {
                address: old
                    .leader_btc_addr
                    .as_deref()
                    .expect("address collab must retain Leader address"),
                owner: old
                    .leader_btc_owner
                    .expect("address collab must retain Leader owner"),
            }
        };
        let mut remint = scale_pass(
            collab_namespace,
            index_u32,
            i32::try_from(pass_count.saturating_mul(3).saturating_add(index)).unwrap(),
            old.owner,
            MinerPassKind::Collab,
            leader_ref,
        );
        remint.mint_block_height = CHURN_REMINT_HEIGHT;
        remint.prev = vec![old_id];
        storage
            .add_new_mint_pass_at_height(&remint, CHURN_REMINT_HEIGHT)
            .unwrap();
        energy_records.extend([
            PassEnergyRecord {
                inscription_id: old_id,
                block_height: CHURN_DORMANT_HEIGHT,
                state: MinerPassState::Dormant,
                active_block_height: MINT_HEIGHT,
                owner_address: old.owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: dormant_energy,
            },
            PassEnergyRecord {
                inscription_id: old_id,
                block_height: CHURN_REMINT_HEIGHT,
                state: MinerPassState::Consumed,
                active_block_height: CHURN_REMINT_HEIGHT,
                owner_address: old.owner,
                owner_balance: 0,
                owner_delta: 0,
                energy: 0,
            },
            PassEnergyRecord {
                inscription_id: remint.inscription_id,
                block_height: CHURN_REMINT_HEIGHT,
                state: MinerPassState::Active,
                active_block_height: CHURN_REMINT_HEIGHT,
                owner_address: remint.owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: calc_inheritable_energy(dormant_energy),
            },
        ]);
    }
    storage.savepoint_commit().unwrap();
    server
        .indexer
        .pass_energy_manager()
        .insert_pass_energy_records_for_test(&energy_records)
        .unwrap();

    let projected_growth = calc_growth_delta(100_000, CHURN_REMINT_HEIGHT - QUERY_HEIGHT);
    let dormant_growth = calc_growth_delta(100_000, CHURN_DORMANT_HEIGHT - QUERY_HEIGHT);
    let mut total_collab_contribution = 0u128;
    let mut primary_leader_contribution = 0u128;
    for index in 0..pass_count {
        let base_energy = ((index as u128).saturating_add(1)).saturating_mul(2);
        let raw_energy = if index < churn_count {
            calc_inheritable_energy(base_energy.saturating_add(dormant_growth))
        } else {
            base_energy.saturating_add(projected_growth)
        };
        let contribution = calc_collab_contribution(raw_energy);
        total_collab_contribution = total_collab_contribution.saturating_add(contribution);
        if index % leader_count == 0 {
            primary_leader_contribution = primary_leader_contribution.saturating_add(contribution);
        }
    }

    AppliedChurnBranch {
        standard_remint_count: churn_count,
        collab_remint_count: churn_count,
        total_collab_contribution,
        primary_leader_contribution,
        apply_latency_ms: duration_ms(started_at.elapsed()),
    }
}

fn get_leader_profile(
    server: &UsdbIndexerRpcServer,
    leader_id: &str,
    pinned_state: &EconomicExternalState,
) -> ProfileRunResult {
    let metric_start = reset_read_metrics(server);
    let profile = server
        .get_pass_economic_profile(GetPassEconomicProfileParams {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            pass_id: leader_id.to_string(),
            block_height: Some(QUERY_HEIGHT),
            context: Some(ConsensusQueryContext::from(pinned_state)),
        })
        .unwrap();
    let elapsed = metric_start.started_at.elapsed();
    let metrics = finish_read_metrics(server, metric_start, elapsed, 1, 1);
    ProfileRunResult {
        latency_us: metrics.total_latency_us,
        rss_delta_kib: metrics.rss_delta_kib,
        reads: metrics.reads,
        profile,
    }
}

fn reopen_server(root_dir: &Path) -> UsdbIndexerRpcServer {
    let config = Arc::new(ConfigManager::load(Some(root_dir.to_path_buf())).unwrap());
    let output = Arc::new(IndexOutput::new());
    let status = Arc::new(StatusManager::new(config.clone(), output).unwrap());
    let indexer = Arc::new(InscriptionIndexer::new(config.clone(), status.clone()).unwrap());
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(());
    UsdbIndexerRpcServer::new(
        config,
        status,
        indexer,
        "127.0.0.1:0".parse().unwrap(),
        shutdown_tx,
    )
}

fn write_report(report: &EconomicScaleReport) {
    let json = serde_json::to_string_pretty(report).unwrap();
    let Ok(path) = std::env::var("USDB_ECONOMIC_SCALE_REPORT_FILE") else {
        println!("USDB_ECONOMIC_SCALE_REPORT={json}");
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, json).unwrap();
    println!("USDB_ECONOMIC_SCALE_REPORT_FILE={}", path.display());
}

fn write_capacity_report(report: &EconomicCapacitySupplementReport) {
    let json = serde_json::to_string_pretty(report).unwrap();
    let Ok(path) = std::env::var("USDB_ECONOMIC_CAPACITY_REPORT_FILE") else {
        println!("USDB_ECONOMIC_CAPACITY_REPORT={json}");
        return;
    };
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, json).unwrap();
    println!("USDB_ECONOMIC_CAPACITY_REPORT_FILE={}", path.display());
}

#[test]
#[ignore = "release-mode economic scale evaluation; run through run_economic_scale_eval.sh"]
fn test_economic_view_scale_profile() {
    let pass_count = std::env::var("USDB_ECONOMIC_SCALE_SIZE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let page_limit = std::env::var("USDB_ECONOMIC_SCALE_PAGE_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let leader_count = std::env::var("USDB_ECONOMIC_SCALE_LEADER_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let concurrent_clients = std::env::var("USDB_ECONOMIC_SCALE_CONCURRENT_CLIENTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let concurrent_iterations = std::env::var("USDB_ECONOMIC_SCALE_CONCURRENT_ITERATIONS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let cold_cache_enabled = std::env::var("USDB_ECONOMIC_SCALE_COLD_CACHE")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"));
    assert!(pass_count > 0);
    assert!((1..=pass_count).contains(&leader_count));
    assert!((1..=ECONOMIC_PAGE_MAX_LIMIT).contains(&page_limit));
    assert!(pass_count.saturating_mul(2) <= i32::MAX as usize);
    assert!(concurrent_clients == 0 || concurrent_iterations > 0);

    let (server, root_dir) = build_server(
        &format!("economic_scale_{pass_count}_{leader_count}"),
        QUERY_HEIGHT,
    );
    server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(Some(trace_sql));
    let fixture_rss_before = process_memory_kib().0;
    let fixture_started = Instant::now();
    let fixture = seed_scale_fixture(&server, pass_count, leader_count);
    let fixture_build_latency_ms = duration_ms(fixture_started.elapsed());
    let fixture_rss_after = process_memory_kib().0;
    let database_size_bytes = directory_size(&root_dir);
    let primary_leader = &fixture.leaders[0];
    let leader_id = primary_leader.pass.inscription_id.to_string();

    let candidate_initial = collect_candidate_pages(
        &server,
        pass_count,
        &leader_id,
        fixture.total_expected_contribution,
        leader_count,
        page_limit,
        None,
    );
    assert_eq!(
        candidate_initial.leader_item.collab_contribution,
        primary_leader.expected_contribution.to_string()
    );
    let pinned_state = candidate_initial.external_state.clone();
    let candidate_replay = collect_candidate_pages(
        &server,
        pass_count,
        &leader_id,
        fixture.total_expected_contribution,
        leader_count,
        page_limit,
        Some(&pinned_state),
    );
    assert_eq!(
        candidate_replay.ordered_digest,
        candidate_initial.ordered_digest
    );

    let breakdown_by_pass_id = collect_breakdown_pages(
        &server,
        &leader_id,
        primary_leader.collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&pinned_state),
    );
    let breakdown_by_pass_id_replay = collect_breakdown_pages(
        &server,
        &leader_id,
        primary_leader.collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&pinned_state),
    );
    let breakdown_by_contribution = collect_breakdown_pages(
        &server,
        &leader_id,
        primary_leader.collab_count,
        page_limit,
        "contribution_desc_pass_id_asc",
        Some(&pinned_state),
    );
    assert_eq!(
        breakdown_by_pass_id.ordered_digest,
        breakdown_by_pass_id_replay.ordered_digest
    );
    assert_eq!(
        breakdown_by_pass_id.canonical_digest,
        breakdown_by_contribution.canonical_digest
    );
    assert_eq!(
        breakdown_by_pass_id.aggregate_collab_contribution,
        primary_leader.expected_contribution.to_string()
    );
    assert_eq!(
        breakdown_by_pass_id.fixed_leader_count,
        primary_leader.fixed_collab_count
    );
    assert_eq!(
        breakdown_by_pass_id.address_leader_count,
        primary_leader.address_collab_count
    );

    let leader_profile = get_leader_profile(&server, &leader_id, &pinned_state);
    assert_eq!(
        leader_profile.profile.pass.collab_contribution,
        primary_leader.expected_contribution.to_string()
    );
    assert_eq!(
        leader_profile.profile.pass.effective_energy,
        candidate_initial.leader_item.effective_energy
    );
    assert_eq!(
        leader_profile.profile.pass.collab_breakdown_count,
        primary_leader.collab_count as u64
    );
    let concurrent_queries = (concurrent_clients > 0).then(|| {
        run_concurrent_queries(
            &server,
            ConcurrentQuerySpec {
                pass_count,
                leader: primary_leader,
                page_limit,
                pinned_state: &pinned_state,
                expected_candidate_digest: &candidate_initial.ordered_digest,
                expected_breakdown_digest: &breakdown_by_pass_id.ordered_digest,
                client_count: concurrent_clients,
                iterations_per_client: concurrent_iterations,
            },
        )
    });

    server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(server);

    let restarted = reopen_server(&root_dir);
    seed_state_ref_context(&restarted, QUERY_HEIGHT);
    restarted
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(Some(trace_sql));
    let restart_candidate_replay = collect_candidate_pages(
        &restarted,
        pass_count,
        &leader_id,
        fixture.total_expected_contribution,
        leader_count,
        page_limit,
        Some(&pinned_state),
    );
    let restart_breakdown_replay = collect_breakdown_pages(
        &restarted,
        &leader_id,
        primary_leader.collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&pinned_state),
    );
    assert_eq!(
        restart_candidate_replay.ordered_digest,
        candidate_initial.ordered_digest
    );
    assert_eq!(
        restart_breakdown_replay.ordered_digest,
        breakdown_by_pass_id.ordered_digest
    );
    restarted
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(restarted);

    let cold_cache = if cold_cache_enabled {
        let candidate_eviction = evict_directory_page_cache(&root_dir);
        let cold_candidate_server = reopen_server(&root_dir);
        seed_state_ref_context(&cold_candidate_server, QUERY_HEIGHT);
        cold_candidate_server
            .indexer
            .miner_pass_storage()
            .set_sql_trace_for_test(Some(trace_sql));
        let candidate = collect_candidate_pages(
            &cold_candidate_server,
            pass_count,
            &leader_id,
            fixture.total_expected_contribution,
            leader_count,
            page_limit,
            Some(&pinned_state),
        );
        assert_eq!(candidate.ordered_digest, candidate_initial.ordered_digest);
        cold_candidate_server
            .indexer
            .miner_pass_storage()
            .set_sql_trace_for_test(None);
        drop(cold_candidate_server);

        let breakdown_eviction = evict_directory_page_cache(&root_dir);
        let cold_breakdown_server = reopen_server(&root_dir);
        seed_state_ref_context(&cold_breakdown_server, QUERY_HEIGHT);
        cold_breakdown_server
            .indexer
            .miner_pass_storage()
            .set_sql_trace_for_test(Some(trace_sql));
        let breakdown = collect_breakdown_pages(
            &cold_breakdown_server,
            &leader_id,
            primary_leader.collab_count,
            page_limit,
            "collab_pass_id_asc",
            Some(&pinned_state),
        );
        assert_eq!(
            breakdown.ordered_digest,
            breakdown_by_pass_id.ordered_digest
        );
        cold_breakdown_server
            .indexer
            .miner_pass_storage()
            .set_sql_trace_for_test(None);
        drop(cold_breakdown_server);
        Some(ColdCacheRunResult {
            candidate_eviction,
            candidate,
            breakdown_eviction,
            breakdown,
        })
    } else {
        None
    };

    let report = EconomicScaleReport {
        report_version: REPORT_VERSION,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        standard_pass_count: pass_count,
        collab_pass_count: pass_count,
        fixed_collab_count: fixture.fixed_collab_count,
        address_collab_count: fixture.address_collab_count,
        total_pass_count: pass_count.saturating_mul(2),
        leader_count,
        min_collab_per_leader: fixture.min_collab_per_leader,
        max_collab_per_leader: fixture.max_collab_per_leader,
        page_limit,
        fixture_build_latency_ms,
        fixture_rss_delta_kib: signed_delta(fixture_rss_after, fixture_rss_before),
        database_size_bytes,
        expected_collab_contribution: primary_leader.expected_contribution.to_string(),
        expected_total_collab_contribution: fixture.total_expected_contribution.to_string(),
        candidate_initial,
        candidate_replay,
        breakdown_by_pass_id,
        breakdown_by_pass_id_replay,
        breakdown_by_contribution,
        leader_profile,
        restart_candidate_replay,
        restart_breakdown_replay,
        cold_cache,
        concurrent_queries,
    };
    write_report(&report);

    if std::env::var_os("USDB_ECONOMIC_SCALE_KEEP_DB").is_none() {
        fs::remove_dir_all(root_dir).unwrap();
    }
}

#[test]
#[ignore = "release-mode cache/concurrency/churn capacity evaluation"]
fn test_economic_view_capacity_supplement() {
    let pass_count = std::env::var("USDB_ECONOMIC_CAPACITY_SIZE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1_000);
    let page_limit = std::env::var("USDB_ECONOMIC_CAPACITY_PAGE_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let leader_count = std::env::var("USDB_ECONOMIC_CAPACITY_LEADER_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8);
    let cold_start_clients = std::env::var("USDB_ECONOMIC_CAPACITY_COLD_START_CLIENTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8);
    let default_churn_count = (pass_count / 2).min(pass_count.saturating_sub(leader_count));
    let churn_count = std::env::var("USDB_ECONOMIC_CAPACITY_CHURN_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default_churn_count);
    assert!(pass_count > 1);
    assert!((1..pass_count).contains(&leader_count));
    assert!((1..=ECONOMIC_PAGE_MAX_LIMIT).contains(&page_limit));
    assert!(cold_start_clients >= 2);
    assert!((1..=pass_count.saturating_sub(leader_count)).contains(&churn_count));
    assert!(pass_count.saturating_mul(4) <= i32::MAX as usize);

    let (server, root_dir) = build_server(
        &format!("economic_capacity_{pass_count}_{leader_count}_{churn_count}"),
        HISTORICAL_CONTEXT_END_HEIGHT,
    );
    server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(Some(trace_sql));
    let fixture_rss_before = process_memory_kib().0;
    let fixture_started_at = Instant::now();
    let fixture = seed_scale_fixture(&server, pass_count, leader_count);
    let fixture_build_latency_ms = duration_ms(fixture_started_at.elapsed());
    let fixture_rss_after = process_memory_kib().0;
    for block_height in (QUERY_HEIGHT + 1)..=HISTORICAL_CONTEXT_END_HEIGHT {
        seed_state_ref_context_with_identity(
            &server,
            block_height,
            u8::try_from(block_height).unwrap(),
            u8::try_from(block_height + 64).unwrap(),
        );
    }
    let states = (QUERY_HEIGHT..=HISTORICAL_CONTEXT_END_HEIGHT)
        .map(|height| (height, resolve_external_state(&server, height)))
        .collect::<BTreeMap<_, _>>();
    let primary_leader = &fixture.leaders[0];
    let leader_id = primary_leader.pass.inscription_id.to_string();
    let primary_collab_count = primary_leader.collab_count;
    let historical_cache_eviction = run_historical_cache_eviction(
        &server,
        pass_count,
        leader_count,
        primary_leader,
        page_limit,
        &states,
    );
    let base_candidate_digest = historical_cache_eviction
        .candidate_probes
        .iter()
        .find(|probe| probe.block_height == QUERY_HEIGHT)
        .unwrap()
        .ordered_digest
        .clone();
    let base_breakdown_digest = historical_cache_eviction
        .breakdown_probes
        .iter()
        .find(|probe| probe.block_height == QUERY_HEIGHT)
        .unwrap()
        .ordered_digest
        .clone();
    let base_state = states.get(&QUERY_HEIGHT).unwrap().clone();
    let database_size_bytes_before_churn = directory_size(&root_dir);
    server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(server);

    let prepare_reopened = |server: &UsdbIndexerRpcServer| {
        seed_state_ref_context_with_identity(
            server,
            HISTORICAL_CONTEXT_END_HEIGHT,
            u8::try_from(HISTORICAL_CONTEXT_END_HEIGHT).unwrap(),
            u8::try_from(HISTORICAL_CONTEXT_END_HEIGHT + 64).unwrap(),
        );
        server
            .indexer
            .miner_pass_storage()
            .set_sql_trace_for_test(Some(trace_sql));
    };

    let candidate_single_server = reopen_server(&root_dir);
    prepare_reopened(&candidate_single_server);
    let candidate_single = collect_candidate_pages_at_height(
        &candidate_single_server,
        CandidateCollectionSpec {
            query_height: QUERY_HEIGHT,
            expected_total: pass_count,
            leader_id: &leader_id,
            expected_total_contribution: expected_collab_contribution_at_height(
                pass_count,
                leader_count,
                None,
                QUERY_HEIGHT,
            ),
            expected_contributing_leaders: leader_count,
            limit: page_limit,
            pinned_state: Some(&base_state),
        },
    );
    assert_eq!(candidate_single.ordered_digest, base_candidate_digest);
    candidate_single_server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(candidate_single_server);

    let candidate_concurrent_server = reopen_server(&root_dir);
    prepare_reopened(&candidate_concurrent_server);
    let candidate_concurrent = run_cold_candidate_concurrent(
        &candidate_concurrent_server,
        QUERY_HEIGHT,
        pass_count,
        &base_candidate_digest,
        page_limit,
        &base_state,
        cold_start_clients,
    );
    assert_eq!(
        candidate_concurrent
            .combined_metrics
            .reads
            .rocksdb_records_decoded,
        candidate_single.metrics.reads.rocksdb_records_decoded
    );
    assert_eq!(
        candidate_concurrent
            .combined_metrics
            .reads
            .rocksdb_iterator_seeks,
        candidate_single.metrics.reads.rocksdb_iterator_seeks
    );
    candidate_concurrent_server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(candidate_concurrent_server);

    let breakdown_single_server = reopen_server(&root_dir);
    prepare_reopened(&breakdown_single_server);
    let breakdown_single = collect_breakdown_pages_at_height(
        &breakdown_single_server,
        QUERY_HEIGHT,
        &leader_id,
        primary_collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&base_state),
    );
    assert_eq!(breakdown_single.ordered_digest, base_breakdown_digest);
    breakdown_single_server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(breakdown_single_server);

    let breakdown_concurrent_server = reopen_server(&root_dir);
    prepare_reopened(&breakdown_concurrent_server);
    let breakdown_concurrent = run_cold_breakdown_concurrent(
        &breakdown_concurrent_server,
        ColdBreakdownSpec {
            block_height: QUERY_HEIGHT,
            leader_id: &leader_id,
            expected_total: primary_collab_count,
            expected_contribution: expected_collab_contribution_at_height(
                pass_count,
                leader_count,
                Some(0),
                QUERY_HEIGHT,
            ),
            expected_digest: &base_breakdown_digest,
            page_limit,
            pinned_state: &base_state,
            client_count: cold_start_clients,
        },
    );
    assert_eq!(
        breakdown_concurrent
            .combined_metrics
            .reads
            .rocksdb_records_decoded,
        breakdown_single.metrics.reads.rocksdb_records_decoded
    );
    assert_eq!(
        breakdown_concurrent
            .combined_metrics
            .reads
            .rocksdb_iterator_seeks,
        breakdown_single.metrics.reads.rocksdb_iterator_seeks
    );
    breakdown_concurrent_server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(breakdown_concurrent_server);

    let cold_first_derivation = ColdFirstDerivationResult {
        candidate_single,
        candidate_concurrent,
        breakdown_single,
        breakdown_concurrent,
    };

    let churn_server = reopen_server(&root_dir);
    prepare_reopened(&churn_server);
    let orphan_applied =
        apply_churn_branch(&churn_server, pass_count, leader_count, churn_count, 3, 4);
    seed_state_ref_context_with_identity(&churn_server, CHURN_DORMANT_HEIGHT, 0x31, 0x32);
    seed_state_ref_context_with_identity(&churn_server, CHURN_REMINT_HEIGHT, 0x33, 0x34);
    let orphan_state = resolve_external_state(&churn_server, CHURN_REMINT_HEIGHT);
    let orphan_candidate = collect_candidate_pages_at_height(
        &churn_server,
        CandidateCollectionSpec {
            query_height: CHURN_REMINT_HEIGHT,
            expected_total: pass_count,
            leader_id: &leader_id,
            expected_total_contribution: orphan_applied.total_collab_contribution,
            expected_contributing_leaders: leader_count,
            limit: page_limit,
            pinned_state: Some(&orphan_state),
        },
    );
    let orphan_breakdown = collect_breakdown_pages_at_height(
        &churn_server,
        CHURN_REMINT_HEIGHT,
        &leader_id,
        primary_collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&orphan_state),
    );
    assert_eq!(
        orphan_breakdown.aggregate_collab_contribution,
        orphan_applied.primary_leader_contribution.to_string()
    );
    let orphan_candidate_digest = orphan_candidate.ordered_digest.clone();
    let orphan_breakdown_digest = orphan_breakdown.ordered_digest.clone();
    let orphan = ChurnBranchResult {
        standard_remint_count: orphan_applied.standard_remint_count,
        collab_remint_count: orphan_applied.collab_remint_count,
        apply_latency_ms: orphan_applied.apply_latency_ms,
        candidate: orphan_candidate,
        breakdown: orphan_breakdown,
    };

    let rollback_started_at = Instant::now();
    let base_anchor = ready_balance_history_snapshot(QUERY_HEIGHT);
    churn_server
        .indexer
        .miner_pass_storage()
        .rollback_to_block_height(QUERY_HEIGHT, Some(&base_anchor))
        .unwrap();
    churn_server
        .indexer
        .pass_energy_manager()
        .rollback_to_pass_synced_height(QUERY_HEIGHT)
        .unwrap();
    seed_state_ref_context_with_identity(&churn_server, QUERY_HEIGHT, 0xaa, 0xbb);
    let rollback_latency_ms = duration_ms(rollback_started_at.elapsed());
    assert_eq!(
        resolve_external_state(&churn_server, QUERY_HEIGHT),
        base_state
    );

    let replacement_applied =
        apply_churn_branch(&churn_server, pass_count, leader_count, churn_count, 5, 6);
    seed_state_ref_context_with_identity(&churn_server, CHURN_DORMANT_HEIGHT, 0x41, 0x42);
    seed_state_ref_context_with_identity(&churn_server, CHURN_REMINT_HEIGHT, 0x43, 0x44);
    let replacement_state = resolve_external_state(&churn_server, CHURN_REMINT_HEIGHT);
    assert_ne!(replacement_state, orphan_state);
    let replacement_candidate = collect_candidate_pages_at_height(
        &churn_server,
        CandidateCollectionSpec {
            query_height: CHURN_REMINT_HEIGHT,
            expected_total: pass_count,
            leader_id: &leader_id,
            expected_total_contribution: replacement_applied.total_collab_contribution,
            expected_contributing_leaders: leader_count,
            limit: page_limit,
            pinned_state: Some(&replacement_state),
        },
    );
    let replacement_breakdown = collect_breakdown_pages_at_height(
        &churn_server,
        CHURN_REMINT_HEIGHT,
        &leader_id,
        primary_collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&replacement_state),
    );
    assert_eq!(
        replacement_breakdown.aggregate_collab_contribution,
        replacement_applied.primary_leader_contribution.to_string()
    );
    assert_eq!(
        replacement_applied.total_collab_contribution,
        orphan_applied.total_collab_contribution
    );
    assert_eq!(
        replacement_applied.primary_leader_contribution,
        orphan_applied.primary_leader_contribution
    );
    assert_ne!(
        replacement_candidate.ordered_digest,
        orphan_candidate_digest
    );
    assert_ne!(
        replacement_breakdown.ordered_digest,
        orphan_breakdown_digest
    );
    let replacement_candidate_digest = replacement_candidate.ordered_digest.clone();
    let replacement_breakdown_digest = replacement_breakdown.ordered_digest.clone();
    let replacement = ChurnBranchResult {
        standard_remint_count: replacement_applied.standard_remint_count,
        collab_remint_count: replacement_applied.collab_remint_count,
        apply_latency_ms: replacement_applied.apply_latency_ms,
        candidate: replacement_candidate,
        breakdown: replacement_breakdown,
    };

    let orphan_error = churn_server
        .get_candidate_set_view(GetCandidateSetViewParams {
            view_version: USDB_ECONOMIC_STATE_VIEW_VERSION.to_string(),
            block_height: Some(CHURN_REMINT_HEIGHT),
            context: Some(ConsensusQueryContext::from(&orphan_state)),
            selection_rule: None,
            cursor: None,
            limit: page_limit,
        })
        .unwrap_err();
    let orphan_error_code = match orphan_error.code {
        ErrorCode::ServerError(code) => code,
        other => panic!("unexpected orphan replay error: {other:?}"),
    };
    assert_eq!(
        orphan_error_code,
        ConsensusRpcErrorCode::SnapshotIdMismatch.code()
    );
    churn_server
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(churn_server);

    let restarted = reopen_server(&root_dir);
    seed_state_ref_context_with_identity(&restarted, CHURN_REMINT_HEIGHT, 0x43, 0x44);
    restarted
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(Some(trace_sql));
    let restart_base_candidate = collect_candidate_pages_at_height(
        &restarted,
        CandidateCollectionSpec {
            query_height: QUERY_HEIGHT,
            expected_total: pass_count,
            leader_id: &leader_id,
            expected_total_contribution: expected_collab_contribution_at_height(
                pass_count,
                leader_count,
                None,
                QUERY_HEIGHT,
            ),
            expected_contributing_leaders: leader_count,
            limit: page_limit,
            pinned_state: Some(&base_state),
        },
    );
    let restart_base_breakdown = collect_breakdown_pages_at_height(
        &restarted,
        QUERY_HEIGHT,
        &leader_id,
        primary_collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&base_state),
    );
    let restart_replacement_candidate = collect_candidate_pages_at_height(
        &restarted,
        CandidateCollectionSpec {
            query_height: CHURN_REMINT_HEIGHT,
            expected_total: pass_count,
            leader_id: &leader_id,
            expected_total_contribution: replacement_applied.total_collab_contribution,
            expected_contributing_leaders: leader_count,
            limit: page_limit,
            pinned_state: Some(&replacement_state),
        },
    );
    let restart_replacement_breakdown = collect_breakdown_pages_at_height(
        &restarted,
        CHURN_REMINT_HEIGHT,
        &leader_id,
        primary_collab_count,
        page_limit,
        "collab_pass_id_asc",
        Some(&replacement_state),
    );
    assert_eq!(restart_base_candidate.ordered_digest, base_candidate_digest);
    assert_eq!(restart_base_breakdown.ordered_digest, base_breakdown_digest);
    assert_eq!(
        restart_replacement_candidate.ordered_digest,
        replacement_candidate_digest
    );
    assert_eq!(
        restart_replacement_breakdown.ordered_digest,
        replacement_breakdown_digest
    );
    restarted
        .indexer
        .miner_pass_storage()
        .set_sql_trace_for_test(None);
    drop(restarted);

    let database_size_bytes_after_replay = directory_size(&root_dir);
    let churn_reorg_replay = ChurnReorgReplayResult {
        dormant_height: CHURN_DORMANT_HEIGHT,
        remint_height: CHURN_REMINT_HEIGHT,
        rollback_latency_ms,
        orphan_context_rejected: true,
        orphan_error_code,
        orphan,
        replacement,
        restart_base_candidate,
        restart_base_breakdown,
        restart_replacement_candidate,
        restart_replacement_breakdown,
    };
    let report = EconomicCapacitySupplementReport {
        report_version: CAPACITY_REPORT_VERSION,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        standard_pass_count: pass_count,
        collab_pass_count: pass_count,
        leader_count,
        churn_count_per_kind: churn_count,
        page_limit,
        cold_start_client_count: cold_start_clients,
        fixture_build_latency_ms,
        fixture_rss_delta_kib: signed_delta(fixture_rss_after, fixture_rss_before),
        database_size_bytes_before_churn,
        database_size_bytes_after_replay,
        historical_cache_eviction,
        cold_first_derivation,
        churn_reorg_replay,
    };
    write_capacity_report(&report);

    if std::env::var_os("USDB_ECONOMIC_CAPACITY_KEEP_DB").is_none() {
        fs::remove_dir_all(root_dir).unwrap();
    }
}
