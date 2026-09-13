//! Atomic import checkpoints and read-only logical comparison for the AssumeUTXO prototype.

use super::*;
use crate::assumeutxo::{SnapshotCoin, SnapshotIdentity};
use sha2::{Digest, Sha256};

const IMPORT_STATE: &str = "assumeutxo_import_v1";

/// Persisted identity and atomic progress for one staged AssumeUTXO import.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssumeUtxoImportState {
    /// Exact source snapshot identity.
    pub identity: SnapshotIdentity,
    /// Verified source core SQLite file hash for the USDB commit anchor.
    pub reference_sha256: String,
    /// Canonical source commit at the baseline, encoded as raw SHA-256 bytes.
    pub base_commit: String,
    /// Baseline block delta root from the same reference.
    pub base_delta_root: String,
    /// Coins durably applied in the same write batch as this checkpoint.
    pub imported_coins: u64,
    /// True only after full input and logical commitment checks succeeded.
    pub complete: bool,
}

/// Count and ordered SHA-256 commitment to a canonical logical table projection.
#[derive(Debug, Serialize)]
pub struct AssumeUtxoTableDigest {
    /// Number of projected rows; balances omit zero rows on both sides.
    pub rows: u64,
    /// SHA-256 of fixed-width canonical row encodings.
    pub sha256: String,
}

/// Full-stream comparison against the existing core snapshot at its exact height.
#[derive(Debug, Serialize)]
pub struct AssumeUtxoComparison {
    /// Exact reference and candidate height.
    pub height: u32,
    /// True if all three canonical table projections match.
    pub equal: bool,
    /// Candidate and reference digests, keyed by table name.
    pub tables: std::collections::BTreeMap<String, [AssumeUtxoTableDigest; 2]>,
}

impl BalanceHistoryDB {
    /// Make replay WAL durable before publishing an external progress report.
    pub(crate) fn sync_assumeutxo_checkpoint(&self) -> Result<(), String> {
        self.db
            .flush_wal(true)
            .map_err(|e| format!("Sync AssumeUTXO replay WAL: {e}"))
    }

    /// Returns the durable bootstrap identity and progress, if this is an imported DB.
    pub fn get_assumeutxo_import_state(&self) -> Result<Option<AssumeUtxoImportState>, String> {
        self.get_json_meta(IMPORT_STATE)
    }

    /// Coverage begins with baseline live scripts, followed by scripts observed during replay.
    pub fn get_utxo_bootstrap_coverage(&self) -> Result<Option<(bool, SnapshotIdentity)>, String> {
        if let Some(native) = self.get_native_bootstrap_state()? {
            return Ok(Some((
                native.phase == crate::bootstrap::NativeBootstrapPhase::Sealed,
                native.identity.snapshot,
            )));
        }
        Ok(self
            .get_assumeutxo_import_state()?
            .map(|s| (s.complete, s.identity)))
    }

    /// Marks complete-UTXO semantics, which prohibit historical RPC fallback on missing inputs.
    pub fn get_assumeutxo_base_height(&self) -> Result<Option<u32>, String> {
        if let Some(native) = self.get_native_bootstrap_state()? {
            return Ok(Some(native.identity.snapshot.base_height));
        }
        Ok(self
            .get_assumeutxo_import_state()?
            .map(|s| s.identity.base_height))
    }

    /// Initialize only an empty staging database, or verify a resumable import's immutable inputs.
    pub(crate) fn begin_assumeutxo_import(
        &self,
        expected: &AssumeUtxoImportState,
    ) -> Result<u64, String> {
        if let Some(mut existing) = self.get_assumeutxo_import_state()? {
            let imported = existing.imported_coins;
            existing.imported_coins = 0;
            existing.complete = false;
            if existing != *expected {
                return Err("AssumeUTXO resume input identity mismatch".to_string());
            }
            return Ok(imported);
        }
        for name in BALANCE_HISTORY_COLUMN_FAMILIES {
            if name == META_CF {
                continue;
            }
            let cf = self.db.cf_handle(name).ok_or("Missing column family")?;
            if let Some(row) = self.db.iterator_cf(cf, IteratorMode::Start).next() {
                row.map_err(|e| e.to_string())?;
                return Err(format!(
                    "AssumeUTXO import requires an empty staging DB: column_family={name}"
                ));
            }
        }
        if self.get_btc_block_height()? != 0 {
            return Err("AssumeUTXO import requires height zero".to_string());
        }
        let cf = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing meta column family")?;
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .put_cf_opt(
                cf,
                IMPORT_STATE,
                serde_json::to_vec(expected).map_err(|e| e.to_string())?,
                &options,
            )
            .map_err(|e| e.to_string())?;
        Ok(0)
    }

    /// Atomically import output keys, aggregate baseline balances, register scripts, and advance progress.
    pub(crate) fn import_assumeutxo_coins(
        &self,
        coins: &[SnapshotCoin],
        processed: u64,
    ) -> Result<(), String> {
        let mut state = self
            .get_assumeutxo_import_state()?
            .ok_or("Missing AssumeUTXO import marker")?;
        if state.complete || state.imported_coins.checked_add(coins.len() as u64) != Some(processed)
        {
            return Err("Noncontiguous AssumeUTXO import checkpoint".to_string());
        }
        let meta_cf = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing meta column family")?;
        let mut batch = WriteBatch::default();
        self.append_snapshot_coins(&mut batch, coins, state.identity.base_height)?;
        state.imported_coins = processed;
        batch.put_cf(
            meta_cf,
            IMPORT_STATE,
            serde_json::to_vec(&state).map_err(|e| e.to_string())?,
        );
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(&batch, &options)
            .map_err(|e| format!("Commit AssumeUTXO import batch at coin {processed}: {e}"))
    }

    // Share the validated Coin projection between legacy verification and native bootstrap.
    pub(super) fn append_snapshot_coins(
        &self,
        batch: &mut WriteBatch,
        coins: &[SnapshotCoin],
        base_height: u32,
    ) -> Result<(), String> {
        let utxo_cf = self
            .db
            .cf_handle(UTXO_CF)
            .ok_or("Missing UTXO column family")?;
        let balance_cf = self
            .db
            .cf_handle(BALANCE_HISTORY_CF)
            .ok_or("Missing balance column family")?;
        let mut deltas: HashMap<BtcScriptHash, u64> = HashMap::new();
        let mut scripts: HashMap<BtcScriptHash, ScriptBuf> = HashMap::new();
        use usdb_util::ToBtcScriptHash;
        for coin in coins {
            let script_hash = coin.script.to_btc_script_hash();
            let delta = deltas.entry(script_hash).or_default();
            *delta = delta
                .checked_add(coin.value)
                .ok_or("Baseline balance overflow")?;
            scripts
                .entry(script_hash)
                .or_insert_with(|| coin.script.clone());
            batch.put_cf(
                utxo_cf,
                Self::make_utxo_key(&coin.outpoint),
                UTXOValue::encode(&script_hash, coin.value),
            );
        }
        let changes: Vec<_> = deltas.into_iter().collect();
        let keys: Vec<_> = changes
            .iter()
            .map(|(hash, _)| Self::make_balance_history_key(hash, base_height))
            .collect();
        let old = self
            .db
            .multi_get_pinned_cf(keys.iter().map(|key| (balance_cf, key.as_slice())));
        for ((key, (_, added)), previous) in keys.iter().zip(changes.iter()).zip(old) {
            let previous = previous.map_err(|e| format!("Read import baseline: {e}"))?;
            let balance = match previous {
                Some(bytes) if bytes.len() == 16 => Self::parse_balance_from_value(&bytes).1,
                Some(_) => return Err("Invalid staged balance value".to_string()),
                None => 0,
            }
            .checked_add(*added)
            .ok_or("Baseline balance overflow")?;
            let mut value = [0u8; 16];
            // This is a balance baseline, not an invented historical income event.
            value[8..].copy_from_slice(&balance.to_be_bytes());
            batch.put_cf(balance_cf, key, value);
        }
        let scripts: Vec<_> = scripts
            .into_iter()
            .map(|(script_hash, script_pubkey)| ScriptRegistryEntry {
                script_hash,
                script_pubkey,
            })
            .collect();
        self.append_script_registry_entries_to_batch(batch, &scripts)?;
        Ok(())
    }

    /// Publish the baseline metadata atomically after the streaming verifier has succeeded.
    pub(crate) fn finish_assumeutxo_import(
        &self,
        count: u64,
        anchor: &BlockCommitEntry,
    ) -> Result<(), String> {
        let mut state = self
            .get_assumeutxo_import_state()?
            .ok_or("Missing AssumeUTXO import marker")?;
        if state.imported_coins != count || anchor.block_height != state.identity.base_height {
            return Err("AssumeUTXO completion count or height mismatch".to_string());
        }
        let meta_cf = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing meta column family")?;
        let commit_cf = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or("Missing commit column family")?;
        let mut batch = WriteBatch::default();
        let height = anchor.block_height;
        let history_floor = height.checked_add(1).ok_or("Baseline height overflow")?;
        for (key, value) in [
            (META_KEY_BTC_BLOCK_HEIGHT, height),
            (META_KEY_BALANCE_QUERY_FLOOR, height),
            (META_KEY_HISTORY_QUERY_FLOOR, history_floor),
            (META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT, history_floor),
            (META_KEY_UNDO_RETAINED_FROM_HEIGHT, history_floor),
        ] {
            batch.put_cf(meta_cf, key, value.to_be_bytes());
        }
        batch.put_cf(
            commit_cf,
            Self::make_block_commit_key(height),
            Self::serialize_block_commit_value(anchor),
        );
        state.complete = true;
        batch.put_cf(
            meta_cf,
            IMPORT_STATE,
            serde_json::to_vec(&state).map_err(|e| e.to_string())?,
        );
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(&batch, &options)
            .map_err(|e| format!("Complete AssumeUTXO import: {e}"))?;
        self.flush_all()
    }

    /// Compare every live UTXO, nonzero script balance and baseline-to-target commit with a frozen core SQLite.
    /// Pre-baseline last-change metadata and zero-only historical scripts cannot be reconstructed from UTXOs.
    pub fn compare_assumeutxo_core(
        &self,
        reference: &std::path::Path,
        target: u32,
    ) -> Result<AssumeUtxoComparison, String> {
        let state = self
            .get_assumeutxo_import_state()?
            .ok_or("Not an AssumeUTXO database")?;
        if !state.complete || self.get_btc_block_height()? != target {
            return Err(
                "Comparison requires a complete import at the exact target height".to_string(),
            );
        }
        let canonical = reference.canonicalize().map_err(|e| e.to_string())?;
        let mut uri = url::Url::from_file_path(canonical).map_err(|_| "Invalid reference path")?;
        uri.query_pairs_mut().append_pair("immutable", "1");
        let connection = rusqlite::Connection::open_with_flags(
            uri.as_str(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| e.to_string())?;
        connection
            .execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-65536;")
            .map_err(|e| e.to_string())?;
        let reference_height: u32 = connection
            .query_row("SELECT block_height FROM meta WHERE id=1", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        if reference_height != target {
            return Err("Reference height mismatch".to_string());
        }
        let mut tables = std::collections::BTreeMap::new();
        for name in ["utxos", "balances", "commits"] {
            let begin = Instant::now();
            eprintln!("AssumeUTXO comparison started: table={name}, height={target}");
            let cf_name = match name {
                "utxos" => UTXO_CF,
                "balances" => BALANCE_HISTORY_CF,
                _ => BLOCK_COMMITS_CF,
            };
            let cf = self
                .db
                .cf_handle(cf_name)
                .ok_or("Missing comparison column family")?;
            let mut candidate_hash = Sha256::new();
            let mut candidate_rows = 0u64;
            let mut previous_script: Option<[u8; 32]> = None;
            let mut progress = Instant::now();
            // Descending order selects the latest balance row for each script first.
            for item in self.db.full_iterator_cf(cf, IteratorMode::End) {
                let (key, value) = item.map_err(|e| format!("Read candidate {name}: {e}"))?;
                match name {
                    "utxos" => {
                        if key.len() != 36 || value.len() != 40 {
                            return Err("Invalid candidate UTXO encoding".to_string());
                        }
                        candidate_hash.update(&key);
                        candidate_hash.update(&value);
                    }
                    "balances" => {
                        if key.len() != 36 || value.len() != 16 {
                            return Err("Invalid candidate balance encoding".to_string());
                        }
                        let script: [u8; 32] =
                            key[..32].try_into().map_err(|_| "Invalid script key")?;
                        if previous_script == Some(script) {
                            continue;
                        }
                        previous_script = Some(script);
                        let balance = Self::parse_balance_from_value(&value).1;
                        if balance == 0 {
                            continue;
                        }
                        candidate_hash.update(script);
                        candidate_hash.update(balance.to_be_bytes());
                    }
                    _ => {
                        if key.len() != 4 || value.len() != 96 {
                            return Err("Invalid candidate commit encoding".to_string());
                        }
                        let height = u32::from_be_bytes(
                            key.as_ref()
                                .try_into()
                                .map_err(|_| "Invalid commit height")?,
                        );
                        if height < state.identity.base_height || height > target {
                            return Err("Unexpected commit outside imported range".to_string());
                        }
                        candidate_hash.update(&key);
                        candidate_hash.update(&value);
                    }
                }
                candidate_rows += 1;
                if progress.elapsed().as_secs() >= 10 {
                    eprintln!(
                        "AssumeUTXO comparison progress: side=candidate, table={name}, rows={candidate_rows}, elapsed_seconds={:.1}",
                        begin.elapsed().as_secs_f64()
                    );
                    progress = Instant::now();
                }
            }
            let query = match name {
                "utxos" => "SELECT outpoint,script_hash,value FROM utxos ORDER BY outpoint DESC".to_string(),
                "balances" => "SELECT script_hash,balance FROM balance_history WHERE balance != 0 ORDER BY script_hash DESC".to_string(),
                _ => format!("SELECT block_height,btc_block_hash,balance_delta_root,block_commit FROM block_commits WHERE block_height BETWEEN {} AND {target} ORDER BY block_height DESC", state.identity.base_height),
            };
            let mut statement = connection.prepare(&query).map_err(|e| e.to_string())?;
            let mut rows = statement.query([]).map_err(|e| e.to_string())?;
            let mut reference_hash = Sha256::new();
            let mut reference_rows = 0u64;
            while let Some(row) = rows
                .next()
                .map_err(|e| format!("Read reference {name}: {e}"))?
            {
                let blob = |index: usize, len: usize| -> Result<Vec<u8>, String> {
                    let bytes: Vec<u8> = row.get(index).map_err(|e| e.to_string())?;
                    if bytes.len() != len {
                        return Err(format!("Invalid reference {name} blob length"));
                    }
                    Ok(bytes)
                };
                match name {
                    "utxos" => {
                        reference_hash.update(blob(0, 36)?);
                        reference_hash.update(blob(1, 32)?);
                        reference_hash.update(
                            u64::try_from(row.get::<_, i64>(2).map_err(|e| e.to_string())?)
                                .map_err(|_| "Negative reference UTXO value")?
                                .to_be_bytes(),
                        );
                    }
                    "balances" => {
                        reference_hash.update(blob(0, 32)?);
                        reference_hash.update(
                            u64::try_from(row.get::<_, i64>(1).map_err(|e| e.to_string())?)
                                .map_err(|_| "Negative reference balance")?
                                .to_be_bytes(),
                        );
                    }
                    _ => {
                        reference_hash.update(
                            row.get::<_, u32>(0)
                                .map_err(|e| e.to_string())?
                                .to_be_bytes(),
                        );
                        for index in 1..=3 {
                            reference_hash.update(blob(index, 32)?);
                        }
                    }
                }
                reference_rows += 1;
                if progress.elapsed().as_secs() >= 10 {
                    eprintln!(
                        "AssumeUTXO comparison progress: side=reference, table={name}, rows={reference_rows}, elapsed_seconds={:.1}",
                        begin.elapsed().as_secs_f64()
                    );
                    progress = Instant::now();
                }
            }
            let candidate = AssumeUtxoTableDigest {
                rows: candidate_rows,
                sha256: crate::assumeutxo::format::hex(&candidate_hash.finalize()),
            };
            let reference = AssumeUtxoTableDigest {
                rows: reference_rows,
                sha256: crate::assumeutxo::format::hex(&reference_hash.finalize()),
            };
            eprintln!(
                "AssumeUTXO comparison finished: table={name}, candidate_rows={candidate_rows}, reference_rows={reference_rows}, equal={}",
                candidate.sha256 == reference.sha256 && candidate_rows == reference_rows
            );
            tables.insert(name.to_string(), [candidate, reference]);
        }
        let equal = tables
            .values()
            .all(|v| v[0].rows == v[1].rows && v[0].sha256 == v[1].sha256);
        Ok(AssumeUtxoComparison {
            height: target,
            equal,
            tables,
        })
    }
}
