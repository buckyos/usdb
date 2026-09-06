//! Direct immutable SQLite comparison using the existing frozen semantic rules.

use super::*;
use crate::snapshot_audit::{SplitSnapshotAudit, open_immutable_audit_db};

/// Full semantic report for a legacy snapshot versus concrete split artifacts.
#[derive(Debug, Serialize)]
pub struct SplitLegacyComparisonReport {
    /// Independent schema identifying the SQLite-to-SQLite source pair.
    pub schema: String,
    /// True only after all selected tables match the frozen semantic rules and row counts.
    pub ok: bool,
    /// Immutable legacy input path.
    pub snapshot_db: PathBuf,
    /// Exact comparison height.
    pub target_height: u32,
    /// Shared target BTC block hash.
    pub target_btc_block_hash: String,
    /// Integrity check applied to every compared SQLite file.
    pub integrity_check: LegacySnapshotIntegrityCheck,
    /// Metadata from the legacy producer.
    pub legacy_meta: LegacySnapshotMetaSummary,
    /// Current artifact identities and explicit file-hash verification status.
    pub current: SplitSnapshotAudit,
    /// Whether the registry was fully compared.
    pub script_registry_compared: bool,
    /// Per-table differences and counts.
    pub tables: Vec<LegacyStateTableComparison>,
    /// Frozen, explainable differences across all tables.
    pub expected_difference_rows: u64,
    /// Unexpected differences, including metadata count mismatches.
    pub unexpected_difference_rows: u64,
    /// Total scan wall time.
    pub duration_seconds: f64,
}

/// Compares a legacy v2 file with already identity-validated core/registry artifacts.
/// It never opens RocksDB or materializes full tables in memory.
pub fn compare_legacy_split_snapshot(
    options: &LegacyStateCompareOptions,
    current: SplitSnapshotAudit,
    progress: Option<LegacyStateCompareProgressRef>,
) -> Result<SplitLegacyComparisonReport, String> {
    let result = compare_inner(options, current, progress);
    if let Err(error) = &result {
        log::error!(
            "Split legacy comparison failed: legacy={}, height={}, error={error}",
            options.snapshot_db.display(),
            options.target_height
        );
    }
    result
}

fn compare_inner(
    options: &LegacyStateCompareOptions,
    current: SplitSnapshotAudit,
    progress: Option<LegacyStateCompareProgressRef>,
) -> Result<SplitLegacyComparisonReport, String> {
    validate_options(options)?;
    let started = Instant::now();
    let path = options
        .snapshot_db
        .canonicalize()
        .map_err(|error| format!("Failed to resolve legacy input: {error}"))?;
    let legacy = open_immutable_audit_db(&path)?;
    validate_legacy_schema(&legacy)?;
    let meta = load_legacy_meta(&legacy)?;
    if meta.version != 2
        || meta.block_height != options.target_height
        || current.core_meta.block_height != options.target_height
    {
        return Err("Legacy/core audit schema or exact height mismatch".to_string());
    }
    let target = load_legacy_commit(&legacy, options.target_height)?;
    let block_hash = &current.core.manifest.state_ref.stable_block_hash;
    if bitcoincore_rpc::bitcoin::BlockHash::from_byte_array(target.btc_block_hash).to_string()
        != *block_hash
    {
        return Err("Legacy/core audit target BTC block hash mismatch".to_string());
    }
    if options.include_script_registry && current.script_registry.is_none() {
        return Err("Full registry comparison requires --script-registry-db".to_string());
    }
    run_integrity_check(&legacy, options.integrity_check)?;
    let core_db = open_immutable_audit_db(&current.core.file)?;
    run_integrity_check(&core_db, options.integrity_check)?;
    if let Some(registry) = &current.script_registry {
        run_integrity_check(
            &open_immutable_audit_db(&registry.file)?,
            options.integrity_check,
        )?;
    }
    let mut tables = Vec::new();
    for (table, old_count, current_count) in [
        (
            "balance_history",
            meta.balance_history_count,
            current.core_meta.balance_history_count,
        ),
        ("utxos", meta.utxo_count, current.core_meta.utxo_count),
        (
            "block_commits",
            meta.block_commit_count,
            current.core_meta.block_commit_count,
        ),
        (
            "script_registry",
            meta.script_registry_count,
            current
                .script_registry
                .as_ref()
                .map_or(0, |value| value.manifest.entry_count),
        ),
    ] {
        if table == "script_registry" && !options.include_script_registry {
            continue;
        }
        log::info!(
            "Split comparison table started: table={table}, legacy_rows={old_count}, current_rows={current_count}"
        );
        let current_file = if table == "script_registry" {
            &current
                .script_registry
                .as_ref()
                .expect("registry validated")
                .file
        } else {
            &current.core.file
        };
        let report = if table == "block_commits" {
            let began = Instant::now();
            let mut report = LegacyStateTableComparison {
                table: table.to_string(),
                ..Default::default()
            };
            compare_sqlite_shard(options, current_file, table, None)?
                .merge_into(&mut report, options.max_examples);
            report.duration_seconds = began.elapsed().as_secs_f64();
            report
        } else {
            compare_sharded_table(table, options, progress.clone(), |shard| {
                compare_sqlite_shard(options, current_file, table, Some(shard))
            })?
        };
        let mut report = with_meta_count_check(report, old_count, options.max_examples);
        if report.current_rows != current_count {
            let mut mismatch = ShardComparison::default();
            mismatch.unexpected(
                options.max_examples,
                "current_meta_count_mismatch",
                table.to_string(),
                Some(format!("metadata_count={current_count}")),
                Some(format!("scanned_count={}", report.current_rows)),
            );
            mismatch.merge_into(&mut report, options.max_examples);
        }
        log::info!(
            "Split comparison table completed: table={table}, unexpected={}",
            report.unexpected_difference_rows
        );
        tables.push(report);
    }
    let expected_difference_rows = tables
        .iter()
        .map(|table| table.expected_difference_rows)
        .sum();
    let unexpected_difference_rows = tables
        .iter()
        .map(|table| table.unexpected_difference_rows)
        .sum();
    Ok(SplitLegacyComparisonReport {
        schema: "balance-history-legacy-split-comparison:v1".to_string(),
        ok: unexpected_difference_rows == 0,
        snapshot_db: path,
        target_height: options.target_height,
        target_btc_block_hash: block_hash.clone(),
        integrity_check: options.integrity_check,
        legacy_meta: meta,
        current,
        script_registry_compared: options.include_script_registry,
        tables,
        expected_difference_rows,
        unexpected_difference_rows,
        duration_seconds: started.elapsed().as_secs_f64(),
    })
}

fn compare_sqlite_shard(
    options: &LegacyStateCompareOptions,
    current: &Path,
    table: &str,
    shard: Option<u8>,
) -> Result<ShardComparison, String> {
    let legacy = open_immutable_audit_db(&options.snapshot_db)?;
    let current = open_immutable_audit_db(current)?;
    // Table/column names are internal constants; keys are bound SQLite parameters.
    let (columns, key, key_len) = match table {
        "balance_history" => ("script_hash, height, balance, delta", "script_hash", 32),
        "utxos" => ("outpoint, script_hash, value", "outpoint", UTXO_KEY_LEN),
        "script_registry" => ("script_hash, script_pubkey", "script_hash", 32),
        "block_commits" => (
            "block_height, btc_block_hash, balance_delta_root, block_commit",
            "block_height",
            4,
        ),
        _ => return Err(format!("Unknown comparison table: {table}")),
    };
    let mut keys = Vec::<Vec<u8>>::new();
    let sql = if let Some(shard) = shard {
        let mut lower = vec![0; key_len];
        lower[0] = shard;
        keys.push(lower);
        let upper = if let Some(next) = shard.checked_add(1) {
            let mut upper = vec![0; key_len];
            upper[0] = next;
            keys.push(upper);
            format!(" AND {key} < ?2")
        } else {
            String::new()
        };
        format!("SELECT {columns} FROM {table} WHERE {key} >= ?1{upper} ORDER BY {key} DESC")
    } else {
        format!("SELECT {columns} FROM {table} ORDER BY {key} ASC")
    };
    let mut old_statement = legacy
        .prepare(&sql)
        .map_err(sql_error("prepare legacy audit shard"))?;
    let mut new_statement = current
        .prepare(&sql)
        .map_err(sql_error("prepare current audit shard"))?;
    let old_rows = old_statement
        .query(rusqlite::params_from_iter(&keys))
        .map_err(sql_error("query legacy audit shard"))?;
    let new_rows = new_statement
        .query(rusqlite::params_from_iter(&keys))
        .map_err(sql_error("query current audit shard"))?;
    match table {
        "balance_history" => merge_balance_rows(
            parsed_rows(old_rows, parse_legacy_balance_row),
            parsed_rows(new_rows, parse_legacy_balance_row),
            &legacy,
            options.max_examples,
        ),
        "utxos" => merge_utxo_rows(
            parsed_rows(old_rows, parse_legacy_utxo_row),
            parsed_rows(new_rows, parse_legacy_utxo_row),
            &legacy,
            options.max_examples,
        ),
        "script_registry" => merge_registry_rows(
            parsed_rows(old_rows, parse_legacy_registry_row),
            parsed_rows(new_rows, parse_legacy_registry_row),
            options.max_examples,
        ),
        "block_commits" => merge_commit_rows(
            parsed_rows(old_rows, parse_legacy_commit_row),
            parsed_rows(new_rows, parse_legacy_commit_row),
            options.max_examples,
        ),
        _ => unreachable!(),
    }
}

fn parsed_rows<'a, T: 'a>(
    mut rows: rusqlite::Rows<'a>,
    parse: fn(&rusqlite::Row<'_>) -> Result<T, String>,
) -> impl Iterator<Item = Result<T, String>> + 'a {
    std::iter::from_fn(move || match rows.next() {
        Ok(Some(row)) => Some(parse(row)),
        Ok(None) => None,
        Err(error) => Some(Err(format!("Failed to read audit row: {error}"))),
    })
}
