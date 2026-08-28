use super::rocksdb::{Direction, IteratorMode};
use super::{
    BALANCE_HISTORY_CF, BALANCE_HISTORY_KEY_LEN, BLOCK_COMMIT_VALUE_LEN, BLOCK_COMMITS_CF,
    BalanceHistoryDB, BalanceHistoryDBIdentity, SCRIPT_REGISTRY_CF, SCRIPT_REGISTRY_KEY_LEN,
    UTXO_CF, UTXO_KEY_LEN,
};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::time::Instant;
use usdb_util::{BtcScriptHash, is_core_unspendable};

const REPORT_SCHEMA: &str = "balance-history-legacy-state-comparison:v1";
const SHARD_COUNT: usize = 256;
const BIP30_FIRST_HEIGHT: u32 = 91_842;

const BIP30_BALANCE_CASES: [(&str, u32); 2] = [
    (
        "76d95f02197b7c685b972104f6d7688a78bdcbb6a757fd5a139a195e59505fab",
        91_842,
    ),
    (
        "49df7a6bfea6c409a5f03fd734a1f1a13cb8fafee6a3e08dd94db352498f99a6",
        91_880,
    ),
];

const BIP30_DELTA_ROOT_CASES: [(u32, &str, &str); 2] = [
    (
        91_842,
        "f5f79ab0192a4814c44d93eb9c77134cd343ef0b7ec085ecb081795c349ffd19",
        "6ceb72f9bf934c00b79267000a51024d5fed0f460fa2289715a1189b9ea313cf",
    ),
    (
        91_880,
        "6d7c5522e74f928e1fcebe5be0b817b2440797a6e386142ddc4fa7e6561b075f",
        "0ff84f58b11cc80dc0684f2bc3b281c35caff5c0c0cf7743e8ddc0acdcf58f71",
    ),
];

/// Integrity check applied to the immutable legacy SQLite snapshot before comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySnapshotIntegrityCheck {
    /// Do not run a SQLite integrity pragma. Table scans still validate row encodings.
    Off,
    /// Run SQLite's bounded `quick_check` before comparing table contents.
    Quick,
    /// Run SQLite's complete `integrity_check` before comparing table contents.
    Full,
}

/// Options for comparing a legacy SQLite snapshot with a rebuilt RocksDB state.
#[derive(Debug, Clone)]
pub struct LegacyStateCompareOptions {
    /// Immutable legacy SQLite snapshot to use as the reference artifact.
    pub snapshot_db: PathBuf,
    /// Exact height at which both stores must be frozen.
    pub target_height: u32,
    /// Also compare the non-consensus script registry, which dominates runtime at mainnet scale.
    pub include_script_registry: bool,
    /// Maximum number of worker threads reading independent key shards.
    pub parallelism: usize,
    /// Maximum expected and unexpected examples retained per table in the report.
    pub max_examples: usize,
    /// SQLite integrity check performed before semantic comparison.
    pub integrity_check: LegacySnapshotIntegrityCheck,
}

/// One long-running comparison progress event.
#[derive(Debug, Clone, Serialize)]
pub struct LegacyStateCompareProgress {
    /// Table currently being compared.
    pub table: String,
    /// Number of completed key shards.
    pub completed_shards: usize,
    /// Total key shards in this phase.
    pub total_shards: usize,
    /// Legacy rows consumed by completed shards.
    pub legacy_rows: u64,
    /// Current RocksDB rows consumed by completed shards.
    pub current_rows: u64,
}

/// Thread-safe progress sink used by the standalone comparator CLI.
pub type LegacyStateCompareProgressRef = Arc<dyn Fn(LegacyStateCompareProgress) + Send + Sync>;

/// Metadata read from the legacy v2 SQLite schema.
#[derive(Debug, Clone, Serialize)]
pub struct LegacySnapshotMetaSummary {
    /// Frozen BTC height represented by the snapshot.
    pub block_height: u32,
    /// Legacy snapshot schema version.
    pub version: u32,
    /// Balance row count recorded by the legacy producer.
    pub balance_history_count: u64,
    /// UTXO row count recorded by the legacy producer.
    pub utxo_count: u64,
    /// Block-commit row count recorded by the legacy producer.
    pub block_commit_count: u64,
    /// Script-registry row count recorded by the legacy producer.
    pub script_registry_count: u64,
}

/// One bounded human-readable difference example.
#[derive(Debug, Clone, Serialize)]
pub struct LegacyStateDifferenceExample {
    /// Stable machine-readable classification.
    pub kind: String,
    /// Canonical table key in hexadecimal or decimal height form.
    pub key: String,
    /// Legacy snapshot value summary.
    pub legacy: Option<String>,
    /// Current RocksDB value summary.
    pub current: Option<String>,
}

/// Comparison totals and bounded examples for one logical table.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LegacyStateTableComparison {
    /// Logical table name.
    pub table: String,
    /// Rows observed in the legacy snapshot.
    pub legacy_rows: u64,
    /// Rows observed in the current RocksDB view.
    pub current_rows: u64,
    /// Rows whose canonical key and value matched exactly.
    pub exact_match_rows: u64,
    /// Rows that differed only for a frozen, known legacy semantic reason.
    pub expected_difference_rows: u64,
    /// Rows that could not be explained by a frozen legacy semantic rule.
    pub unexpected_difference_rows: u64,
    /// Expected differences grouped by classification.
    pub expected_by_kind: BTreeMap<String, u64>,
    /// Unexpected differences grouped by classification.
    pub unexpected_by_kind: BTreeMap<String, u64>,
    /// Bounded examples of expected differences.
    pub expected_examples: Vec<LegacyStateDifferenceExample>,
    /// Bounded examples of unexpected differences.
    pub unexpected_examples: Vec<LegacyStateDifferenceExample>,
    /// Wall-clock duration for this table phase.
    pub duration_seconds: f64,
}

/// Complete cross-version semantic comparison report.
#[derive(Debug, Clone, Serialize)]
pub struct LegacyStateComparisonReport {
    /// Report schema identifier.
    pub schema: String,
    /// True when every observed difference was classified as expected.
    pub ok: bool,
    /// Legacy snapshot path.
    pub snapshot_db: PathBuf,
    /// Current balance-history service root.
    pub balance_history_root: PathBuf,
    /// Exact comparison height.
    pub target_height: u32,
    /// BTC block hash shared by both stores at the comparison height.
    pub target_btc_block_hash: String,
    /// Integrity check applied to the legacy snapshot.
    pub integrity_check: LegacySnapshotIntegrityCheck,
    /// Legacy snapshot metadata.
    pub legacy_meta: LegacySnapshotMetaSummary,
    /// Immutable identity of the current RocksDB.
    pub current_db_identity: BalanceHistoryDBIdentity,
    /// Whether the auxiliary script registry was included.
    pub script_registry_compared: bool,
    /// Per-table reports.
    pub tables: Vec<LegacyStateTableComparison>,
    /// Total expected difference rows across all tables.
    pub expected_difference_rows: u64,
    /// Total unexpected difference rows across all tables.
    pub unexpected_difference_rows: u64,
    /// Total comparison wall-clock duration.
    pub duration_seconds: f64,
}

#[derive(Debug, Clone)]
struct BalanceRow {
    key: [u8; BtcScriptHash::LEN],
    height: u32,
    balance: u64,
    delta: i64,
}

#[derive(Debug, Clone)]
struct UtxoRow {
    key: [u8; UTXO_KEY_LEN],
    script_hash: [u8; BtcScriptHash::LEN],
    value: u64,
}

#[derive(Debug, Clone)]
struct RegistryRow {
    key: [u8; SCRIPT_REGISTRY_KEY_LEN],
    script_pubkey: Vec<u8>,
}

#[derive(Debug, Clone)]
struct CommitRow {
    key: [u8; 4],
    height: u32,
    btc_block_hash: [u8; 32],
    balance_delta_root: [u8; 32],
    block_commit: [u8; 32],
}

#[derive(Debug, Default)]
struct ShardComparison {
    legacy_rows: u64,
    current_rows: u64,
    exact_match_rows: u64,
    expected_difference_rows: u64,
    unexpected_difference_rows: u64,
    expected_by_kind: BTreeMap<String, u64>,
    unexpected_by_kind: BTreeMap<String, u64>,
    expected_examples: Vec<LegacyStateDifferenceExample>,
    unexpected_examples: Vec<LegacyStateDifferenceExample>,
}

#[derive(Debug, Clone, Copy)]
enum MergeOrder {
    Ascending,
    Descending,
}

impl ShardComparison {
    fn exact(&mut self) {
        self.exact_match_rows += 1;
    }

    fn expected(
        &mut self,
        max_examples: usize,
        kind: &str,
        key: String,
        legacy: Option<String>,
        current: Option<String>,
    ) {
        self.expected_difference_rows += 1;
        *self.expected_by_kind.entry(kind.to_string()).or_default() += 1;
        if self.expected_examples.len() < max_examples {
            self.expected_examples.push(LegacyStateDifferenceExample {
                kind: kind.to_string(),
                key,
                legacy,
                current,
            });
        }
    }

    fn unexpected(
        &mut self,
        max_examples: usize,
        kind: &str,
        key: String,
        legacy: Option<String>,
        current: Option<String>,
    ) {
        self.unexpected_difference_rows += 1;
        *self.unexpected_by_kind.entry(kind.to_string()).or_default() += 1;
        if self.unexpected_examples.len() < max_examples {
            self.unexpected_examples.push(LegacyStateDifferenceExample {
                kind: kind.to_string(),
                key,
                legacy,
                current,
            });
        }
    }

    fn merge_into(self, target: &mut LegacyStateTableComparison, max_examples: usize) {
        target.legacy_rows += self.legacy_rows;
        target.current_rows += self.current_rows;
        target.exact_match_rows += self.exact_match_rows;
        target.expected_difference_rows += self.expected_difference_rows;
        target.unexpected_difference_rows += self.unexpected_difference_rows;
        merge_counts(&mut target.expected_by_kind, self.expected_by_kind);
        merge_counts(&mut target.unexpected_by_kind, self.unexpected_by_kind);
        append_bounded(
            &mut target.expected_examples,
            self.expected_examples,
            max_examples,
        );
        append_bounded(
            &mut target.unexpected_examples,
            self.unexpected_examples,
            max_examples,
        );
    }
}

impl BalanceHistoryDB {
    /// Compares one legacy v2 SQLite snapshot with this read-only RocksDB.
    ///
    /// Both stores must represent `options.target_height`. The comparison is
    /// memory-bounded and does not create a replacement snapshot. Known BIP30
    /// and Core-unspendable corrections are reported as expected differences;
    /// every other mismatch makes the report fail.
    pub fn compare_legacy_snapshot(
        &self,
        options: &LegacyStateCompareOptions,
        progress: Option<LegacyStateCompareProgressRef>,
    ) -> Result<LegacyStateComparisonReport, String> {
        validate_options(options)?;
        let started = Instant::now();
        let legacy_path = options.snapshot_db.canonicalize().map_err(|e| {
            format!(
                "Failed to canonicalize legacy snapshot {}: {}",
                options.snapshot_db.display(),
                e
            )
        })?;
        let legacy = open_legacy_snapshot(&legacy_path)?;
        validate_legacy_schema(&legacy)?;
        let legacy_meta = load_legacy_meta(&legacy)?;
        if legacy_meta.block_height != options.target_height {
            return Err(format!(
                "Legacy snapshot height mismatch: expected {}, got {}",
                options.target_height, legacy_meta.block_height
            ));
        }
        if legacy_meta.version != 2 {
            return Err(format!(
                "compare-legacy only accepts snapshot schema v2, got v{}",
                legacy_meta.version
            ));
        }
        let current_height = self.get_btc_block_height()?;
        if current_height != options.target_height {
            return Err(format!(
                "Current RocksDB must be frozen at height {} for full UTXO comparison, got {}",
                options.target_height, current_height
            ));
        }
        let legacy_target_commit = load_legacy_commit(&legacy, options.target_height)?;
        let current_target_commit =
            self.get_block_commit(options.target_height)?
                .ok_or_else(|| {
                    format!(
                        "Current RocksDB has no block commit at target height {}",
                        options.target_height
                    )
                })?;
        if legacy_target_commit.btc_block_hash
            != current_target_commit.btc_block_hash.to_byte_array()
        {
            return Err(format!(
                "Target BTC block hash mismatch at height {}: legacy={}, current={}",
                options.target_height,
                hex_bytes(&legacy_target_commit.btc_block_hash),
                current_target_commit.btc_block_hash
            ));
        }
        let target_btc_block_hash = current_target_commit.btc_block_hash.to_string();
        run_integrity_check(&legacy, options.integrity_check)?;
        drop(legacy);
        let current_db_identity = self
            .get_db_identity()?
            .ok_or_else(|| "Current RocksDB has no DB identity".to_string())?;

        let mut tables = Vec::new();
        let balance = self.compare_balance_table(options, progress.clone())?;
        tables.push(with_meta_count_check(
            balance,
            legacy_meta.balance_history_count,
            options.max_examples,
        ));
        let utxos = self.compare_utxo_table(options, progress.clone())?;
        tables.push(with_meta_count_check(
            utxos,
            legacy_meta.utxo_count,
            options.max_examples,
        ));
        let commits = self.compare_commit_table(options)?;
        tables.push(with_meta_count_check(
            commits,
            legacy_meta.block_commit_count,
            options.max_examples,
        ));
        if options.include_script_registry {
            let registry = self.compare_registry_table(options, progress)?;
            tables.push(with_meta_count_check(
                registry,
                legacy_meta.script_registry_count,
                options.max_examples,
            ));
        }

        let expected_difference_rows = tables
            .iter()
            .map(|table| table.expected_difference_rows)
            .sum();
        let unexpected_difference_rows = tables
            .iter()
            .map(|table| table.unexpected_difference_rows)
            .sum();
        Ok(LegacyStateComparisonReport {
            schema: REPORT_SCHEMA.to_string(),
            ok: unexpected_difference_rows == 0,
            snapshot_db: legacy_path,
            balance_history_root: self.config.root_dir.clone(),
            target_height: options.target_height,
            target_btc_block_hash,
            integrity_check: options.integrity_check,
            legacy_meta,
            current_db_identity,
            script_registry_compared: options.include_script_registry,
            tables,
            expected_difference_rows,
            unexpected_difference_rows,
            duration_seconds: started.elapsed().as_secs_f64(),
        })
    }

    fn compare_balance_table(
        &self,
        options: &LegacyStateCompareOptions,
        progress: Option<LegacyStateCompareProgressRef>,
    ) -> Result<LegacyStateTableComparison, String> {
        self.compare_sharded_table("balance_history", options, progress, |shard| {
            self.compare_balance_shard(options, shard)
        })
    }

    fn compare_utxo_table(
        &self,
        options: &LegacyStateCompareOptions,
        progress: Option<LegacyStateCompareProgressRef>,
    ) -> Result<LegacyStateTableComparison, String> {
        self.compare_sharded_table("utxos", options, progress, |shard| {
            self.compare_utxo_shard(options, shard)
        })
    }

    fn compare_registry_table(
        &self,
        options: &LegacyStateCompareOptions,
        progress: Option<LegacyStateCompareProgressRef>,
    ) -> Result<LegacyStateTableComparison, String> {
        self.compare_sharded_table("script_registry", options, progress, |shard| {
            self.compare_registry_shard(options, shard)
        })
    }

    fn compare_sharded_table<F>(
        &self,
        table: &str,
        options: &LegacyStateCompareOptions,
        progress: Option<LegacyStateCompareProgressRef>,
        compare_shard: F,
    ) -> Result<LegacyStateTableComparison, String>
    where
        F: Fn(u8) -> Result<ShardComparison, String> + Send + Sync,
    {
        let started = Instant::now();
        let completed = AtomicUsize::new(0);
        let legacy_rows = AtomicU64::new(0);
        let current_rows = AtomicU64::new(0);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.parallelism)
            .thread_name(move |index| format!("legacy-compare-{}", index))
            .build()
            .map_err(|e| format!("Failed to build comparator thread pool: {}", e))?;
        let reports = pool.install(|| {
            (0u16..SHARD_COUNT as u16)
                .into_par_iter()
                .map(|shard| {
                    let report = compare_shard(shard as u8)?;
                    legacy_rows.fetch_add(report.legacy_rows, AtomicOrdering::Relaxed);
                    current_rows.fetch_add(report.current_rows, AtomicOrdering::Relaxed);
                    let done = completed.fetch_add(1, AtomicOrdering::Relaxed) + 1;
                    if let Some(progress) = &progress
                        && (done == SHARD_COUNT || done.is_multiple_of(8))
                    {
                        progress(LegacyStateCompareProgress {
                            table: table.to_string(),
                            completed_shards: done,
                            total_shards: SHARD_COUNT,
                            legacy_rows: legacy_rows.load(AtomicOrdering::Relaxed),
                            current_rows: current_rows.load(AtomicOrdering::Relaxed),
                        });
                    }
                    Ok(report)
                })
                .collect::<Result<Vec<_>, String>>()
        })?;

        let mut table_report = LegacyStateTableComparison {
            table: table.to_string(),
            ..LegacyStateTableComparison::default()
        };
        for report in reports {
            report.merge_into(&mut table_report, options.max_examples);
        }
        table_report.duration_seconds = started.elapsed().as_secs_f64();
        Ok(table_report)
    }

    fn compare_balance_shard(
        &self,
        options: &LegacyStateCompareOptions,
        shard: u8,
    ) -> Result<ShardComparison, String> {
        let connection = open_legacy_snapshot(&options.snapshot_db)?;
        let (lower, upper) = shard_bounds::<{ BtcScriptHash::LEN }>(shard);
        let (sql, upper_param): (&str, Option<Vec<u8>>) = match upper {
            Some(upper) => (
                "SELECT script_hash, height, balance, delta FROM balance_history WHERE script_hash >= ?1 AND script_hash < ?2 ORDER BY script_hash DESC",
                Some(upper.to_vec()),
            ),
            None => (
                "SELECT script_hash, height, balance, delta FROM balance_history WHERE script_hash >= ?1 ORDER BY script_hash DESC",
                None,
            ),
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(sql_error("prepare balance shard"))?;
        let mut rows = match upper_param.as_ref() {
            Some(upper) => statement.query(params![lower.as_slice(), upper.as_slice()]),
            None => statement.query(params![lower.as_slice()]),
        }
        .map_err(sql_error("query balance shard"))?;
        let legacy_iter = std::iter::from_fn(move || match rows.next() {
            Ok(Some(row)) => Some(parse_legacy_balance_row(row)),
            Ok(None) => None,
            Err(e) => Some(Err(format!("Failed to iterate legacy balance rows: {}", e))),
        });

        let cf = self
            .db
            .cf_handle(BALANCE_HISTORY_CF)
            .ok_or_else(|| format!("Column family {} not found", BALANCE_HISTORY_CF))?;
        let mut seek = vec![shard];
        seek.resize(BtcScriptHash::LEN, 0xFF);
        let seek_hash = BtcScriptHash::from_slice(&seek)
            .map_err(|e| format!("Failed to build balance shard seek hash: {}", e))?;
        let seek_key = Self::make_balance_history_key(&seek_hash, u32::MAX);
        let mut rocks = self
            .db
            .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        let mut ended = false;
        let mut last_script_hash: Option<[u8; BtcScriptHash::LEN]> = None;
        let current_iter = std::iter::from_fn(move || {
            if ended {
                return None;
            }
            loop {
                let item = rocks.next()?;
                let (key, value) = match item {
                    Ok(item) => item,
                    Err(e) => return Some(Err(format!("RocksDB balance iterator failed: {}", e))),
                };
                if key.len() != BALANCE_HISTORY_KEY_LEN {
                    return Some(Err(format!(
                        "Invalid RocksDB balance key length {}, expected {}",
                        key.len(),
                        BALANCE_HISTORY_KEY_LEN
                    )));
                }
                if key[0] != shard {
                    ended = true;
                    return None;
                }
                let mut script_hash = [0u8; BtcScriptHash::LEN];
                script_hash.copy_from_slice(&key[..BtcScriptHash::LEN]);
                if last_script_hash == Some(script_hash) {
                    continue;
                }
                last_script_hash = Some(script_hash);
                let height = u32::from_be_bytes(
                    key[BtcScriptHash::LEN..]
                        .try_into()
                        .expect("validated key length"),
                );
                let (delta, balance) = Self::parse_balance_from_value(&value);
                if balance == 0 {
                    continue;
                }
                return Some(Ok(BalanceRow {
                    key: script_hash,
                    height,
                    balance,
                    delta,
                }));
            }
        });

        merge_balance_rows(legacy_iter, current_iter, &connection, options.max_examples)
    }

    fn compare_utxo_shard(
        &self,
        options: &LegacyStateCompareOptions,
        shard: u8,
    ) -> Result<ShardComparison, String> {
        let connection = open_legacy_snapshot(&options.snapshot_db)?;
        let (lower, upper) = shard_bounds::<UTXO_KEY_LEN>(shard);
        let (sql, upper_param): (&str, Option<Vec<u8>>) = match upper {
            Some(upper) => (
                "SELECT outpoint, script_hash, value FROM utxos WHERE outpoint >= ?1 AND outpoint < ?2 ORDER BY outpoint DESC",
                Some(upper.to_vec()),
            ),
            None => (
                "SELECT outpoint, script_hash, value FROM utxos WHERE outpoint >= ?1 ORDER BY outpoint DESC",
                None,
            ),
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(sql_error("prepare UTXO shard"))?;
        let mut rows = match upper_param.as_ref() {
            Some(upper) => statement.query(params![lower.as_slice(), upper.as_slice()]),
            None => statement.query(params![lower.as_slice()]),
        }
        .map_err(sql_error("query UTXO shard"))?;
        let legacy_iter = std::iter::from_fn(move || match rows.next() {
            Ok(Some(row)) => Some(parse_legacy_utxo_row(row)),
            Ok(None) => None,
            Err(e) => Some(Err(format!("Failed to iterate legacy UTXO rows: {}", e))),
        });

        let cf = self
            .db
            .cf_handle(UTXO_CF)
            .ok_or_else(|| format!("Column family {} not found", UTXO_CF))?;
        let mut seek_key = [0xFFu8; UTXO_KEY_LEN];
        seek_key[0] = shard;
        let mut rocks = self
            .db
            .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        let mut ended = false;
        let current_iter = std::iter::from_fn(move || {
            if ended {
                return None;
            }
            let item = rocks.next()?;
            let (key, value) = match item {
                Ok(item) => item,
                Err(e) => return Some(Err(format!("RocksDB UTXO iterator failed: {}", e))),
            };
            if key.len() != UTXO_KEY_LEN {
                return Some(Err(format!(
                    "Invalid RocksDB UTXO key length {}, expected {}",
                    key.len(),
                    UTXO_KEY_LEN
                )));
            }
            if key[0] != shard {
                ended = true;
                return None;
            }
            let mut outpoint = [0u8; UTXO_KEY_LEN];
            outpoint.copy_from_slice(&key);
            let value = Self::parse_utxo_from_value(&value);
            Some(Ok(UtxoRow {
                key: outpoint,
                script_hash: value.script_hash.to_byte_array(),
                value: value.value,
            }))
        });

        merge_utxo_rows(legacy_iter, current_iter, &connection, options.max_examples)
    }

    fn compare_registry_shard(
        &self,
        options: &LegacyStateCompareOptions,
        shard: u8,
    ) -> Result<ShardComparison, String> {
        let connection = open_legacy_snapshot(&options.snapshot_db)?;
        let (lower, upper) = shard_bounds::<SCRIPT_REGISTRY_KEY_LEN>(shard);
        let (sql, upper_param): (&str, Option<Vec<u8>>) = match upper {
            Some(upper) => (
                "SELECT script_hash, script_pubkey FROM script_registry WHERE script_hash >= ?1 AND script_hash < ?2 ORDER BY script_hash DESC",
                Some(upper.to_vec()),
            ),
            None => (
                "SELECT script_hash, script_pubkey FROM script_registry WHERE script_hash >= ?1 ORDER BY script_hash DESC",
                None,
            ),
        };
        let mut statement = connection
            .prepare(sql)
            .map_err(sql_error("prepare script-registry shard"))?;
        let mut rows = match upper_param.as_ref() {
            Some(upper) => statement.query(params![lower.as_slice(), upper.as_slice()]),
            None => statement.query(params![lower.as_slice()]),
        }
        .map_err(sql_error("query script-registry shard"))?;
        let legacy_iter = std::iter::from_fn(move || match rows.next() {
            Ok(Some(row)) => Some(parse_legacy_registry_row(row)),
            Ok(None) => None,
            Err(e) => Some(Err(format!(
                "Failed to iterate legacy script-registry rows: {}",
                e
            ))),
        });

        let cf = self
            .db
            .cf_handle(SCRIPT_REGISTRY_CF)
            .ok_or_else(|| format!("Column family {} not found", SCRIPT_REGISTRY_CF))?;
        let mut seek_key = [0xFFu8; SCRIPT_REGISTRY_KEY_LEN];
        seek_key[0] = shard;
        let mut rocks = self
            .db
            .full_iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Reverse));
        let mut ended = false;
        let current_iter = std::iter::from_fn(move || {
            if ended {
                return None;
            }
            let item = rocks.next()?;
            let (key, value) = match item {
                Ok(item) => item,
                Err(e) => {
                    return Some(Err(format!(
                        "RocksDB script-registry iterator failed: {}",
                        e
                    )));
                }
            };
            if key.len() != SCRIPT_REGISTRY_KEY_LEN {
                return Some(Err(format!(
                    "Invalid RocksDB script-registry key length {}, expected {}",
                    key.len(),
                    SCRIPT_REGISTRY_KEY_LEN
                )));
            }
            if key[0] != shard {
                ended = true;
                return None;
            }
            let mut script_hash = [0u8; SCRIPT_REGISTRY_KEY_LEN];
            script_hash.copy_from_slice(&key);
            Some(Ok(RegistryRow {
                key: script_hash,
                script_pubkey: value.to_vec(),
            }))
        });

        merge_registry_rows(legacy_iter, current_iter, options.max_examples)
    }

    fn compare_commit_table(
        &self,
        options: &LegacyStateCompareOptions,
    ) -> Result<LegacyStateTableComparison, String> {
        let started = Instant::now();
        let connection = open_legacy_snapshot(&options.snapshot_db)?;
        let mut statement = connection
            .prepare(
                "SELECT block_height, btc_block_hash, balance_delta_root, block_commit FROM block_commits ORDER BY block_height ASC",
            )
            .map_err(sql_error("prepare block commits"))?;
        let mut rows = statement
            .query([])
            .map_err(sql_error("query block commits"))?;
        let legacy_iter = std::iter::from_fn(move || match rows.next() {
            Ok(Some(row)) => Some(parse_legacy_commit_row(row)),
            Ok(None) => None,
            Err(e) => Some(Err(format!(
                "Failed to iterate legacy block commits: {}",
                e
            ))),
        });

        let cf = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or_else(|| format!("Column family {} not found", BLOCK_COMMITS_CF))?;
        let rocks = self.db.iterator_cf(&cf, IteratorMode::Start);
        let target_height = options.target_height;
        let current_iter = rocks.filter_map(move |item| match item {
            Err(e) => Some(Err(format!("RocksDB block-commit iterator failed: {}", e))),
            Ok((key, value)) => {
                if key.len() != 4 {
                    return Some(Err(format!(
                        "Invalid RocksDB block-commit key length {}, expected 4",
                        key.len()
                    )));
                }
                if value.len() != BLOCK_COMMIT_VALUE_LEN {
                    return Some(Err(format!(
                        "Invalid RocksDB block-commit value length {}, expected {}",
                        value.len(),
                        BLOCK_COMMIT_VALUE_LEN
                    )));
                }
                let height = u32::from_be_bytes(key.as_ref().try_into().expect("validated key"));
                if height > target_height {
                    return None;
                }
                Some(
                    Self::parse_block_commit_value(height, &value).map(|entry| CommitRow {
                        key: height.to_be_bytes(),
                        height,
                        btc_block_hash: entry.btc_block_hash.to_byte_array(),
                        balance_delta_root: entry.balance_delta_root,
                        block_commit: entry.block_commit,
                    }),
                )
            }
        });

        let shard = merge_commit_rows(legacy_iter, current_iter, options.max_examples)?;
        let mut report = LegacyStateTableComparison {
            table: "block_commits".to_string(),
            duration_seconds: started.elapsed().as_secs_f64(),
            ..LegacyStateTableComparison::default()
        };
        shard.merge_into(&mut report, options.max_examples);
        Ok(report)
    }
}

fn validate_options(options: &LegacyStateCompareOptions) -> Result<(), String> {
    if !options.snapshot_db.is_file() {
        return Err(format!(
            "Legacy snapshot DB does not exist: {}",
            options.snapshot_db.display()
        ));
    }
    if options.parallelism == 0 {
        return Err("Comparator parallelism must be greater than zero".to_string());
    }
    if options.max_examples == 0 {
        return Err("Comparator max_examples must be greater than zero".to_string());
    }
    Ok(())
}

fn open_legacy_snapshot(path: &Path) -> Result<Connection, String> {
    let canonical = path.canonicalize().map_err(|e| {
        format!(
            "Failed to canonicalize legacy snapshot {}: {}",
            path.display(),
            e
        )
    })?;
    let mut uri = url::Url::from_file_path(&canonical).map_err(|_| {
        format!(
            "Failed to encode legacy snapshot as file URI: {}",
            canonical.display()
        )
    })?;
    uri.query_pairs_mut().append_pair("immutable", "1");
    let connection = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| {
        format!(
            "Failed to open legacy snapshot {} read-only: {}",
            canonical.display(),
            e
        )
    })?;
    connection
        .pragma_update(None, "query_only", true)
        .and_then(|_| connection.pragma_update(None, "trusted_schema", false))
        .and_then(|_| connection.pragma_update(None, "cache_size", -262_144i64))
        .map_err(|e| format!("Failed to configure legacy snapshot connection: {}", e))?;
    Ok(connection)
}

fn validate_legacy_schema(connection: &Connection) -> Result<(), String> {
    for table in [
        "meta",
        "balance_history",
        "utxos",
        "block_commits",
        "script_registry",
    ] {
        let exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| format!("Failed to inspect legacy table {}: {}", table, e))?
            .is_some();
        if !exists {
            return Err(format!(
                "Legacy snapshot is missing required table {}",
                table
            ));
        }
    }
    Ok(())
}

fn load_legacy_meta(connection: &Connection) -> Result<LegacySnapshotMetaSummary, String> {
    connection
        .query_row(
            "SELECT block_height, balance_history_count, utxo_count, block_commit_count, script_registry_count, version FROM meta ORDER BY generated_at DESC LIMIT 1",
            [],
            |row| {
                Ok(LegacySnapshotMetaSummary {
                    block_height: checked_u32(row.get(0)?, "meta.block_height")?,
                    balance_history_count: checked_u64(row.get(1)?, "meta.balance_history_count")?,
                    utxo_count: checked_u64(row.get(2)?, "meta.utxo_count")?,
                    block_commit_count: checked_u64(row.get(3)?, "meta.block_commit_count")?,
                    script_registry_count: checked_u64(row.get(4)?, "meta.script_registry_count")?,
                    version: checked_u32(row.get(5)?, "meta.version")?,
                })
            },
        )
        .map_err(|e| format!("Failed to read legacy snapshot metadata: {}", e))
}

fn load_legacy_commit(connection: &Connection, height: u32) -> Result<CommitRow, String> {
    let mut statement = connection
        .prepare(
            "SELECT block_height, btc_block_hash, balance_delta_root, block_commit FROM block_commits WHERE block_height=?1",
        )
        .map_err(sql_error("prepare legacy target block commit"))?;
    let mut rows = statement
        .query([height])
        .map_err(sql_error("query legacy target block commit"))?;
    let row = rows
        .next()
        .map_err(sql_error("read legacy target block commit"))?
        .ok_or_else(|| {
            format!(
                "Legacy snapshot has no block commit at target height {}",
                height
            )
        })?;
    parse_legacy_commit_row(row)
}

fn run_integrity_check(
    connection: &Connection,
    check: LegacySnapshotIntegrityCheck,
) -> Result<(), String> {
    let pragma = match check {
        LegacySnapshotIntegrityCheck::Off => return Ok(()),
        LegacySnapshotIntegrityCheck::Quick => "PRAGMA quick_check",
        LegacySnapshotIntegrityCheck::Full => "PRAGMA integrity_check",
    };
    let result: String = connection
        .query_row(pragma, [], |row| row.get(0))
        .map_err(|e| format!("Legacy snapshot integrity check failed to run: {}", e))?;
    if result != "ok" {
        return Err(format!(
            "Legacy snapshot integrity check returned: {}",
            result
        ));
    }
    Ok(())
}

fn parse_legacy_balance_row(row: &rusqlite::Row<'_>) -> Result<BalanceRow, String> {
    let key = fixed_blob::<{ BtcScriptHash::LEN }>(row, 0, "balance_history.script_hash")?;
    Ok(BalanceRow {
        key,
        height: checked_u32(
            row.get(1).map_err(|e| e.to_string())?,
            "balance_history.height",
        )
        .map_err(|e| e.to_string())?,
        balance: checked_u64(
            row.get(2).map_err(|e| e.to_string())?,
            "balance_history.balance",
        )
        .map_err(|e| e.to_string())?,
        delta: row.get(3).map_err(|e| e.to_string())?,
    })
}

fn parse_legacy_utxo_row(row: &rusqlite::Row<'_>) -> Result<UtxoRow, String> {
    Ok(UtxoRow {
        key: fixed_blob::<UTXO_KEY_LEN>(row, 0, "utxos.outpoint")?,
        script_hash: fixed_blob::<{ BtcScriptHash::LEN }>(row, 1, "utxos.script_hash")?,
        value: checked_u64(row.get(2).map_err(|e| e.to_string())?, "utxos.value")
            .map_err(|e| e.to_string())?,
    })
}

fn parse_legacy_registry_row(row: &rusqlite::Row<'_>) -> Result<RegistryRow, String> {
    Ok(RegistryRow {
        key: fixed_blob::<SCRIPT_REGISTRY_KEY_LEN>(row, 0, "script_registry.script_hash")?,
        script_pubkey: row.get(1).map_err(|e| e.to_string())?,
    })
}

fn parse_legacy_commit_row(row: &rusqlite::Row<'_>) -> Result<CommitRow, String> {
    Ok(CommitRow {
        height: checked_u32(
            row.get(0).map_err(|e| e.to_string())?,
            "block_commits.block_height",
        )
        .map_err(|e| e.to_string())?,
        key: checked_u32(
            row.get(0).map_err(|e| e.to_string())?,
            "block_commits.block_height",
        )
        .map_err(|e| e.to_string())?
        .to_be_bytes(),
        btc_block_hash: fixed_blob::<32>(row, 1, "block_commits.btc_block_hash")?,
        balance_delta_root: fixed_blob::<32>(row, 2, "block_commits.balance_delta_root")?,
        block_commit: fixed_blob::<32>(row, 3, "block_commits.block_commit")?,
    })
}

fn merge_balance_rows<L, C>(
    legacy: L,
    current: C,
    connection: &Connection,
    max_examples: usize,
) -> Result<ShardComparison, String>
where
    L: Iterator<Item = Result<BalanceRow, String>>,
    C: Iterator<Item = Result<BalanceRow, String>>,
{
    merge_ordered(
        legacy,
        current,
        MergeOrder::Descending,
        |row| &row.key,
        |report, legacy, current| {
            if legacy.height == current.height
                && legacy.balance == current.balance
                && legacy.delta == current.delta
            {
                report.exact();
            } else if is_expected_bip30_balance(legacy, current) {
                report.expected(
                    max_examples,
                    "bip30_displaced_generation_balance",
                    hex_bytes(&legacy.key),
                    Some(describe_balance(legacy)),
                    Some(describe_balance(current)),
                );
            } else {
                report.unexpected(
                    max_examples,
                    "balance_value_mismatch",
                    hex_bytes(&legacy.key),
                    Some(describe_balance(legacy)),
                    Some(describe_balance(current)),
                );
            }
            Ok(())
        },
        |report, legacy| {
            let key = hex_bytes(&legacy.key);
            if legacy_script_is_core_unspendable(connection, &legacy.key)? {
                report.expected(
                    max_examples,
                    "legacy_core_unspendable_balance",
                    key,
                    Some(describe_balance(legacy)),
                    None,
                );
            } else {
                report.unexpected(
                    max_examples,
                    "legacy_only_balance",
                    key,
                    Some(describe_balance(legacy)),
                    None,
                );
            }
            Ok(())
        },
        |report, current| {
            report.unexpected(
                max_examples,
                "current_only_balance",
                hex_bytes(&current.key),
                None,
                Some(describe_balance(current)),
            );
            Ok(())
        },
    )
}

fn merge_utxo_rows<L, C>(
    legacy: L,
    current: C,
    connection: &Connection,
    max_examples: usize,
) -> Result<ShardComparison, String>
where
    L: Iterator<Item = Result<UtxoRow, String>>,
    C: Iterator<Item = Result<UtxoRow, String>>,
{
    merge_ordered(
        legacy,
        current,
        MergeOrder::Descending,
        |row| &row.key,
        |report, legacy, current| {
            if legacy.script_hash == current.script_hash && legacy.value == current.value {
                report.exact();
            } else {
                report.unexpected(
                    max_examples,
                    "utxo_value_mismatch",
                    hex_bytes(&legacy.key),
                    Some(describe_utxo(legacy)),
                    Some(describe_utxo(current)),
                );
            }
            Ok(())
        },
        |report, legacy| {
            let key = hex_bytes(&legacy.key);
            if legacy_script_is_core_unspendable(connection, &legacy.script_hash)? {
                report.expected(
                    max_examples,
                    "legacy_core_unspendable_utxo",
                    key,
                    Some(describe_utxo(legacy)),
                    None,
                );
            } else {
                report.unexpected(
                    max_examples,
                    "legacy_only_utxo",
                    key,
                    Some(describe_utxo(legacy)),
                    None,
                );
            }
            Ok(())
        },
        |report, current| {
            report.unexpected(
                max_examples,
                "current_only_utxo",
                hex_bytes(&current.key),
                None,
                Some(describe_utxo(current)),
            );
            Ok(())
        },
    )
}

fn merge_registry_rows<L, C>(
    legacy: L,
    current: C,
    max_examples: usize,
) -> Result<ShardComparison, String>
where
    L: Iterator<Item = Result<RegistryRow, String>>,
    C: Iterator<Item = Result<RegistryRow, String>>,
{
    merge_ordered(
        legacy,
        current,
        MergeOrder::Descending,
        |row| &row.key,
        |report, legacy, current| {
            if legacy.script_pubkey == current.script_pubkey {
                report.exact();
            } else {
                report.unexpected(
                    max_examples,
                    "script_registry_value_mismatch",
                    hex_bytes(&legacy.key),
                    Some(describe_registry(legacy)),
                    Some(describe_registry(current)),
                );
            }
            Ok(())
        },
        |report, legacy| {
            if is_core_unspendable(bitcoincore_rpc::bitcoin::Script::from_bytes(
                &legacy.script_pubkey,
            )) {
                report.expected(
                    max_examples,
                    "legacy_core_unspendable_script_registry",
                    hex_bytes(&legacy.key),
                    Some(describe_registry(legacy)),
                    None,
                );
            } else {
                report.unexpected(
                    max_examples,
                    "legacy_only_script_registry",
                    hex_bytes(&legacy.key),
                    Some(describe_registry(legacy)),
                    None,
                );
            }
            Ok(())
        },
        |report, current| {
            report.unexpected(
                max_examples,
                "current_only_script_registry",
                hex_bytes(&current.key),
                None,
                Some(describe_registry(current)),
            );
            Ok(())
        },
    )
}

fn merge_commit_rows<L, C>(
    legacy: L,
    current: C,
    max_examples: usize,
) -> Result<ShardComparison, String>
where
    L: Iterator<Item = Result<CommitRow, String>>,
    C: Iterator<Item = Result<CommitRow, String>>,
{
    let mut previous_legacy: Option<CommitRow> = None;
    let mut previous_current: Option<CommitRow> = None;
    merge_ordered(
        legacy,
        current,
        MergeOrder::Ascending,
        |row| &row.key,
        |report, legacy, current| {
            let legacy_chain_valid = previous_legacy
                .as_ref()
                .is_none_or(|previous| commit_follows(previous, legacy));
            let current_chain_valid = previous_current
                .as_ref()
                .is_none_or(|previous| commit_follows(previous, current));
            previous_legacy = Some(legacy.clone());
            previous_current = Some(current.clone());
            if !legacy_chain_valid || !current_chain_valid {
                let kind = match (legacy_chain_valid, current_chain_valid) {
                    (false, false) => "both_block_commit_chains_invalid",
                    (false, true) => "legacy_block_commit_chain_invalid",
                    (true, false) => "current_block_commit_chain_invalid",
                    (true, true) => unreachable!(),
                };
                report.unexpected(
                    max_examples,
                    kind,
                    legacy.height.to_string(),
                    Some(describe_commit(legacy)),
                    Some(describe_commit(current)),
                );
                return Ok(());
            }
            if legacy.btc_block_hash != current.btc_block_hash {
                report.unexpected(
                    max_examples,
                    "block_commit_btc_hash_mismatch",
                    legacy.height.to_string(),
                    Some(describe_commit(legacy)),
                    Some(describe_commit(current)),
                );
                return Ok(());
            }
            let root_matches = legacy.balance_delta_root == current.balance_delta_root;
            let commit_matches = legacy.block_commit == current.block_commit;
            if root_matches && commit_matches {
                report.exact();
                return Ok(());
            }
            let expected_root = root_matches || is_expected_bip30_delta_root(legacy, current);
            let expected_commit = commit_matches || legacy.height >= BIP30_FIRST_HEIGHT;
            if expected_root && expected_commit {
                let kind = if !root_matches {
                    "bip30_delta_root_and_rolling_commit"
                } else {
                    "bip30_rolling_commit"
                };
                report.expected(
                    max_examples,
                    kind,
                    legacy.height.to_string(),
                    Some(describe_commit(legacy)),
                    Some(describe_commit(current)),
                );
            } else {
                report.unexpected(
                    max_examples,
                    "block_commit_value_mismatch",
                    legacy.height.to_string(),
                    Some(describe_commit(legacy)),
                    Some(describe_commit(current)),
                );
            }
            Ok(())
        },
        |report, legacy| {
            report.unexpected(
                max_examples,
                "legacy_only_block_commit",
                legacy.height.to_string(),
                Some(describe_commit(legacy)),
                None,
            );
            Ok(())
        },
        |report, current| {
            report.unexpected(
                max_examples,
                "current_only_block_commit",
                current.height.to_string(),
                None,
                Some(describe_commit(current)),
            );
            Ok(())
        },
    )
}

fn commit_follows(previous: &CommitRow, current: &CommitRow) -> bool {
    current.height == previous.height.saturating_add(1)
        && current.block_commit
            == compute_block_commit(
                current.height,
                &current.btc_block_hash,
                &current.balance_delta_root,
                &previous.block_commit,
            )
}

fn compute_block_commit(
    block_height: u32,
    btc_block_hash: &[u8; 32],
    balance_delta_root: &[u8; 32],
    previous_block_commit: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"balance-history:block-commit:v1");
    hasher.update(block_height.to_be_bytes());
    hasher.update(btc_block_hash);
    hasher.update(balance_delta_root);
    hasher.update(previous_block_commit);
    hasher.finalize().into()
}

fn merge_ordered<T, L, C, K, E, LO, CO>(
    mut legacy: L,
    mut current: C,
    order: MergeOrder,
    key: K,
    mut equal: E,
    mut legacy_only: LO,
    mut current_only: CO,
) -> Result<ShardComparison, String>
where
    L: Iterator<Item = Result<T, String>>,
    C: Iterator<Item = Result<T, String>>,
    K: Fn(&T) -> &[u8],
    E: FnMut(&mut ShardComparison, &T, &T) -> Result<(), String>,
    LO: FnMut(&mut ShardComparison, &T) -> Result<(), String>,
    CO: FnMut(&mut ShardComparison, &T) -> Result<(), String>,
{
    let mut report = ShardComparison::default();
    let mut legacy_row = next_row(&mut legacy)?;
    let mut current_row = next_row(&mut current)?;
    while legacy_row.is_some() || current_row.is_some() {
        match (&legacy_row, &current_row) {
            (Some(legacy_value), Some(current_value)) => {
                let ordering = key(legacy_value).cmp(key(current_value));
                match ordering {
                    Ordering::Equal => {
                        report.legacy_rows += 1;
                        report.current_rows += 1;
                        equal(&mut report, legacy_value, current_value)?;
                        legacy_row = next_row(&mut legacy)?;
                        current_row = next_row(&mut current)?;
                    }
                    Ordering::Greater if matches!(order, MergeOrder::Descending) => {
                        report.legacy_rows += 1;
                        legacy_only(&mut report, legacy_value)?;
                        legacy_row = next_row(&mut legacy)?;
                    }
                    Ordering::Less if matches!(order, MergeOrder::Ascending) => {
                        report.legacy_rows += 1;
                        legacy_only(&mut report, legacy_value)?;
                        legacy_row = next_row(&mut legacy)?;
                    }
                    Ordering::Greater | Ordering::Less => {
                        report.current_rows += 1;
                        current_only(&mut report, current_value)?;
                        current_row = next_row(&mut current)?;
                    }
                }
            }
            (Some(legacy_value), None) => {
                report.legacy_rows += 1;
                legacy_only(&mut report, legacy_value)?;
                legacy_row = next_row(&mut legacy)?;
            }
            (None, Some(current_value)) => {
                report.current_rows += 1;
                current_only(&mut report, current_value)?;
                current_row = next_row(&mut current)?;
            }
            (None, None) => break,
        }
    }
    Ok(report)
}

fn next_row<T, I>(iter: &mut I) -> Result<Option<T>, String>
where
    I: Iterator<Item = Result<T, String>>,
{
    iter.next().transpose()
}

fn legacy_script_is_core_unspendable(
    connection: &Connection,
    script_hash: &[u8; BtcScriptHash::LEN],
) -> Result<bool, String> {
    let script: Option<Vec<u8>> = connection
        .query_row(
            "SELECT script_pubkey FROM script_registry WHERE script_hash=?1",
            [script_hash.as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| {
            format!(
                "Failed to resolve legacy script registry entry {}: {}",
                hex_bytes(script_hash),
                e
            )
        })?;
    Ok(script.is_some_and(|script| {
        is_core_unspendable(bitcoincore_rpc::bitcoin::Script::from_bytes(&script))
    }))
}

fn is_expected_bip30_balance(legacy: &BalanceRow, current: &BalanceRow) -> bool {
    let key = hex_bytes(&legacy.key);
    BIP30_BALANCE_CASES.iter().any(|(expected_key, height)| {
        key == *expected_key
            && legacy.height == *height
            && current.height == *height
            && legacy.balance == 10_000_000_000
            && legacy.delta == 5_000_000_000
            && current.balance == 5_000_000_000
            && current.delta == 0
    })
}

fn is_expected_bip30_delta_root(legacy: &CommitRow, current: &CommitRow) -> bool {
    BIP30_DELTA_ROOT_CASES
        .iter()
        .any(|(height, legacy_root, current_root)| {
            legacy.height == *height
                && current.height == *height
                && hex_bytes(&legacy.balance_delta_root) == *legacy_root
                && hex_bytes(&current.balance_delta_root) == *current_root
        })
}

fn with_meta_count_check(
    mut report: LegacyStateTableComparison,
    metadata_count: u64,
    max_examples: usize,
) -> LegacyStateTableComparison {
    if metadata_count != report.legacy_rows {
        report.unexpected_difference_rows += 1;
        *report
            .unexpected_by_kind
            .entry("legacy_meta_count_mismatch".to_string())
            .or_default() += 1;
        if report.unexpected_examples.len() < max_examples {
            report
                .unexpected_examples
                .push(LegacyStateDifferenceExample {
                    kind: "legacy_meta_count_mismatch".to_string(),
                    key: report.table.clone(),
                    legacy: Some(format!("metadata_count={}", metadata_count)),
                    current: Some(format!("scanned_count={}", report.legacy_rows)),
                });
        }
    }
    report
}

fn shard_bounds<const N: usize>(shard: u8) -> ([u8; N], Option<[u8; N]>) {
    let mut lower = [0u8; N];
    lower[0] = shard;
    let upper = shard.checked_add(1).map(|next| {
        let mut value = [0u8; N];
        value[0] = next;
        value
    });
    (lower, upper)
}

fn fixed_blob<const N: usize>(
    row: &rusqlite::Row<'_>,
    index: usize,
    field: &str,
) -> Result<[u8; N], String> {
    let value: Vec<u8> = row.get(index).map_err(|e| e.to_string())?;
    value.try_into().map_err(|value: Vec<u8>| {
        format!(
            "Legacy {} has length {}, expected {}",
            field,
            value.len(),
            N
        )
    })
}

fn checked_u32(value: i64, _field: &str) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn checked_u64(value: i64, field: &str) -> rusqlite::Result<u64> {
    let _ = field;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, value))
}

fn describe_balance(row: &BalanceRow) -> String {
    format!(
        "height={} balance={} delta={}",
        row.height, row.balance, row.delta
    )
}

fn describe_utxo(row: &UtxoRow) -> String {
    format!(
        "script_hash={} value={}",
        hex_bytes(&row.script_hash),
        row.value
    )
}

fn describe_registry(row: &RegistryRow) -> String {
    format!("script_bytes={}", row.script_pubkey.len())
}

fn describe_commit(row: &CommitRow) -> String {
    format!(
        "btc_block_hash={} balance_delta_root={} block_commit={}",
        hex_bytes(&row.btc_block_hash),
        hex_bytes(&row.balance_delta_root),
        hex_bytes(&row.block_commit)
    )
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{:02x}", byte).expect("writing to String cannot fail");
    }
    encoded
}

fn merge_counts(target: &mut BTreeMap<String, u64>, source: BTreeMap<String, u64>) {
    for (kind, count) in source {
        *target.entry(kind).or_default() += count;
    }
}

fn append_bounded<T>(target: &mut Vec<T>, source: Vec<T>, limit: usize) {
    let remaining = limit.saturating_sub(target.len());
    target.extend(source.into_iter().take(remaining));
}

fn sql_error(context: &'static str) -> impl FnOnce(rusqlite::Error) -> String {
    move |error| format!("Failed to {}: {}", context, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use crate::db::{
        BalanceHistoryDBMode, BalanceHistoryEntry, BlockCommitEntry, ScriptRegistryEntry,
    };
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{BlockHash, Network, OutPoint, ScriptBuf, Txid};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{ToBtcScriptHash, UTXOEntry};

    const TARGET_HEIGHT: u32 = 91_880;

    struct Fixture {
        root: PathBuf,
        snapshot: PathBuf,
        db: Option<BalanceHistoryDB>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.db.take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "balance-history-legacy-compare-{}-{}-{}",
            tag,
            std::process::id(),
            nonce
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixture(normal_balance_override: Option<u64>) -> Fixture {
        let root = temp_root("fixture");
        let mut config = BalanceHistoryConfig {
            root_dir: root.clone(),
            ..BalanceHistoryConfig::default()
        };
        config.btc.network = Network::Regtest;
        let db = BalanceHistoryDB::open(Arc::new(config), BalanceHistoryDBMode::Normal).unwrap();

        let bip30_hash = BtcScriptHash::from_str(BIP30_BALANCE_CASES[1].0).unwrap();
        let normal_script = ScriptBuf::from_bytes(vec![0x51]);
        let normal_hash = normal_script.to_btc_script_hash();
        let oversized_script = ScriptBuf::from_bytes(vec![0x51; 10_001]);
        let oversized_hash = oversized_script.to_btc_script_hash();

        db.put_address_history_async(&vec![
            BalanceHistoryEntry {
                script_hash: bip30_hash,
                block_height: TARGET_HEIGHT,
                delta: 0,
                balance: 5_000_000_000,
            },
            BalanceHistoryEntry {
                script_hash: normal_hash,
                block_height: TARGET_HEIGHT,
                delta: 25,
                balance: 25,
            },
        ])
        .unwrap();

        let normal_outpoint = OutPoint {
            txid: Txid::from_byte_array([0x11; 32]),
            vout: 1,
        };
        db.put_utxos(&[UTXOEntry {
            outpoint: normal_outpoint,
            script_hash: normal_hash,
            value: 25,
        }])
        .unwrap();
        db.put_script_registry_entries(&[ScriptRegistryEntry {
            script_hash: normal_hash,
            script_pubkey: normal_script.clone(),
        }])
        .unwrap();

        let block_hash = BlockHash::from_byte_array([0x22; 32]);
        let current_root = decode_32(BIP30_DELTA_ROOT_CASES[1].2);
        db.put_block_commits_async(&[BlockCommitEntry {
            block_height: TARGET_HEIGHT,
            btc_block_hash: block_hash,
            balance_delta_root: current_root,
            block_commit: [0x33; 32],
        }])
        .unwrap();
        db.put_btc_block_height(TARGET_HEIGHT).unwrap();
        db.flush_all().unwrap();

        let snapshot = root.join("legacy-v2.db");
        let connection = Connection::open(&snapshot).unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE meta (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    block_height INTEGER NOT NULL,
                    balance_history_count INTEGER NOT NULL,
                    utxo_count INTEGER NOT NULL,
                    block_commit_count INTEGER NOT NULL,
                    script_registry_count INTEGER NOT NULL,
                    generated_at INTEGER NOT NULL,
                    version INTEGER NOT NULL
                );
                CREATE TABLE balance_history (
                    script_hash BLOB NOT NULL PRIMARY KEY,
                    height INTEGER NOT NULL,
                    balance INTEGER NOT NULL,
                    delta INTEGER NOT NULL
                );
                CREATE TABLE utxos (
                    outpoint BLOB NOT NULL PRIMARY KEY,
                    script_hash BLOB NOT NULL,
                    value INTEGER NOT NULL
                );
                CREATE TABLE block_commits (
                    block_height INTEGER NOT NULL PRIMARY KEY,
                    btc_block_hash BLOB NOT NULL,
                    balance_delta_root BLOB NOT NULL,
                    block_commit BLOB NOT NULL
                );
                CREATE TABLE script_registry (
                    script_hash BLOB NOT NULL PRIMARY KEY,
                    script_pubkey BLOB NOT NULL
                );
                "#,
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO balance_history VALUES (?1, ?2, ?3, ?4)",
                params![
                    bip30_hash.as_ref() as &[u8],
                    TARGET_HEIGHT,
                    10_000_000_000i64,
                    5_000_000_000i64
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO balance_history VALUES (?1, ?2, ?3, ?4)",
                params![
                    normal_hash.as_ref() as &[u8],
                    TARGET_HEIGHT,
                    normal_balance_override.unwrap_or(25) as i64,
                    25i64
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO balance_history VALUES (?1, ?2, ?3, ?4)",
                params![
                    oversized_hash.as_ref() as &[u8],
                    TARGET_HEIGHT,
                    50i64,
                    50i64
                ],
            )
            .unwrap();

        let oversized_outpoint = OutPoint {
            txid: Txid::from_byte_array([0x44; 32]),
            vout: 2,
        };
        for entry in [
            UTXOEntry {
                outpoint: normal_outpoint,
                script_hash: normal_hash,
                value: 25,
            },
            UTXOEntry {
                outpoint: oversized_outpoint,
                script_hash: oversized_hash,
                value: 50,
            },
        ] {
            connection
                .execute(
                    "INSERT INTO utxos VALUES (?1, ?2, ?3)",
                    params![
                        entry.outpoint_vec(),
                        entry.script_hash.as_ref() as &[u8],
                        entry.value as i64
                    ],
                )
                .unwrap();
        }
        for (hash, script) in [
            (normal_hash, normal_script),
            (oversized_hash, oversized_script),
        ] {
            connection
                .execute(
                    "INSERT INTO script_registry VALUES (?1, ?2)",
                    params![hash.as_ref() as &[u8], script.as_bytes()],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO block_commits VALUES (?1, ?2, ?3, ?4)",
                params![
                    TARGET_HEIGHT,
                    block_hash.as_ref() as &[u8],
                    decode_32(BIP30_DELTA_ROOT_CASES[1].1).as_slice(),
                    [0x55u8; 32].as_slice()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta (block_height, balance_history_count, utxo_count, block_commit_count, script_registry_count, generated_at, version) VALUES (?1, 3, 2, 1, 2, 1, 2)",
                [TARGET_HEIGHT],
            )
            .unwrap();
        drop(connection);

        Fixture {
            root,
            snapshot,
            db: Some(db),
        }
    }

    fn options(snapshot: PathBuf) -> LegacyStateCompareOptions {
        LegacyStateCompareOptions {
            snapshot_db: snapshot,
            target_height: TARGET_HEIGHT,
            include_script_registry: true,
            parallelism: 2,
            max_examples: 8,
            integrity_check: LegacySnapshotIntegrityCheck::Quick,
        }
    }

    fn decode_32(value: &str) -> [u8; 32] {
        assert_eq!(value.len(), 64);
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }

    #[test]
    fn legacy_comparator_accepts_only_frozen_expected_semantic_differences() {
        let fixture = fixture(None);
        let report = fixture
            .db
            .as_ref()
            .unwrap()
            .compare_legacy_snapshot(&options(fixture.snapshot.clone()), None)
            .unwrap();

        assert!(report.ok, "{:#?}", report.tables);
        assert_eq!(report.unexpected_difference_rows, 0);
        assert_eq!(report.expected_difference_rows, 5);
        assert!(report.tables.iter().any(|table| {
            table
                .expected_by_kind
                .contains_key("bip30_displaced_generation_balance")
        }));
        assert!(report.tables.iter().any(|table| {
            table
                .expected_by_kind
                .contains_key("legacy_core_unspendable_script_registry")
        }));
    }

    #[test]
    fn legacy_comparator_rejects_unclassified_balance_difference() {
        let fixture = fixture(Some(26));
        let report = fixture
            .db
            .as_ref()
            .unwrap()
            .compare_legacy_snapshot(&options(fixture.snapshot.clone()), None)
            .unwrap();

        assert!(!report.ok);
        assert_eq!(report.unexpected_difference_rows, 1);
        let balance = report
            .tables
            .iter()
            .find(|table| table.table == "balance_history")
            .unwrap();
        assert_eq!(balance.unexpected_by_kind["balance_value_mismatch"], 1);
    }

    #[test]
    fn legacy_comparator_rejects_target_block_hash_mismatch_before_scanning() {
        let fixture = fixture(None);
        let connection = Connection::open(&fixture.snapshot).unwrap();
        connection
            .execute(
                "UPDATE block_commits SET btc_block_hash=?1 WHERE block_height=?2",
                params![[0x99u8; 32].as_slice(), TARGET_HEIGHT],
            )
            .unwrap();
        drop(connection);

        let error = fixture
            .db
            .as_ref()
            .unwrap()
            .compare_legacy_snapshot(&options(fixture.snapshot.clone()), None)
            .unwrap_err();

        assert!(error.contains("Target BTC block hash mismatch"));
    }

    #[test]
    fn block_commit_comparison_rejects_invalid_rolling_chain() {
        let base = CommitRow {
            key: 91_841u32.to_be_bytes(),
            height: 91_841,
            btc_block_hash: [1; 32],
            balance_delta_root: [2; 32],
            block_commit: [3; 32],
        };
        let legacy_bip30_root = decode_32(BIP30_DELTA_ROOT_CASES[0].1);
        let current_bip30_root = decode_32(BIP30_DELTA_ROOT_CASES[0].2);
        let legacy_bip30 = CommitRow {
            key: 91_842u32.to_be_bytes(),
            height: 91_842,
            btc_block_hash: [4; 32],
            balance_delta_root: legacy_bip30_root,
            block_commit: compute_block_commit(
                91_842,
                &[4; 32],
                &legacy_bip30_root,
                &base.block_commit,
            ),
        };
        let current_bip30 = CommitRow {
            key: 91_842u32.to_be_bytes(),
            height: 91_842,
            btc_block_hash: [4; 32],
            balance_delta_root: current_bip30_root,
            block_commit: compute_block_commit(
                91_842,
                &[4; 32],
                &current_bip30_root,
                &base.block_commit,
            ),
        };
        let next_root = [5; 32];
        let legacy_next = CommitRow {
            key: 91_843u32.to_be_bytes(),
            height: 91_843,
            btc_block_hash: [6; 32],
            balance_delta_root: next_root,
            block_commit: compute_block_commit(
                91_843,
                &[6; 32],
                &next_root,
                &legacy_bip30.block_commit,
            ),
        };
        let current_next = CommitRow {
            key: 91_843u32.to_be_bytes(),
            height: 91_843,
            btc_block_hash: [6; 32],
            balance_delta_root: next_root,
            block_commit: [7; 32],
        };

        let report = merge_commit_rows(
            vec![base.clone(), legacy_bip30, legacy_next]
                .into_iter()
                .map(Ok::<_, String>),
            vec![base, current_bip30, current_next]
                .into_iter()
                .map(Ok::<_, String>),
            8,
        )
        .unwrap();

        assert_eq!(report.unexpected_difference_rows, 1);
        assert_eq!(
            report.unexpected_by_kind["current_block_commit_chain_invalid"],
            1
        );
    }

    #[test]
    fn block_commit_comparison_reports_missing_row_on_correct_side() {
        let first = CommitRow {
            key: 1u32.to_be_bytes(),
            height: 1,
            btc_block_hash: [1; 32],
            balance_delta_root: [2; 32],
            block_commit: [3; 32],
        };
        let second = CommitRow {
            key: 2u32.to_be_bytes(),
            height: 2,
            btc_block_hash: [4; 32],
            balance_delta_root: [5; 32],
            block_commit: compute_block_commit(2, &[4; 32], &[5; 32], &first.block_commit),
        };

        let report = merge_commit_rows(
            vec![first, second.clone()].into_iter().map(Ok::<_, String>),
            vec![second].into_iter().map(Ok::<_, String>),
            8,
        )
        .unwrap();

        assert_eq!(report.legacy_rows, 2);
        assert_eq!(report.current_rows, 1);
        assert_eq!(report.unexpected_difference_rows, 1);
        assert_eq!(report.unexpected_by_kind["legacy_only_block_commit"], 1);
    }
}
