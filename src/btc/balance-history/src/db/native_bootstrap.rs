//! Durable native bootstrap metadata, verification and atomic origin sealing.

use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rust_rocksdb::{DB, IteratorMode, Options, ReadOptions, WriteBatch, WriteOptions};
use sha2::{Digest, Sha256};

use super::{
    BALANCE_HISTORY_COLUMN_FAMILIES, BLOCK_COMMITS_CF, BLOCK_UNDO_BALANCE_INDEX_CF,
    BLOCK_UNDO_CREATED_UTXOS_CF, BLOCK_UNDO_META_CF, BLOCK_UNDO_SPENT_UTXOS_CF, BalanceHistoryDB,
    BlockCommitEntry, META_CF, META_KEY_BALANCE_QUERY_FLOOR, META_KEY_BTC_BLOCK_HEIGHT,
    META_KEY_HISTORY_QUERY_FLOOR, META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT,
    META_KEY_UNDO_RETAINED_FROM_HEIGHT, UTXO_CF,
};
use crate::assumeutxo::{SnapshotCoin, SnapshotScan};
use crate::bootstrap::{
    BOOTSTRAP_COMMIT_PROTOCOL_VERSION, BootstrapOriginIdentity, NATIVE_BOOTSTRAP_SCHEMA,
    NativeBootstrapIdentity, NativeBootstrapPhase, NativeBootstrapState, OriginTableDigest,
    derive_origin_state_digest,
};

const NATIVE_STATE: &str = "native_bootstrap_v1";

impl BalanceHistoryDB {
    /// Read and validate native provenance without opening a source file or scanning logical tables.
    pub fn get_native_bootstrap_state(&self) -> Result<Option<NativeBootstrapState>, String> {
        let state: Option<NativeBootstrapState> = self.get_json_meta(NATIVE_STATE)?;
        if let Some(state) = &state {
            state.validate()?;
        }
        Ok(state)
    }

    /// Resolve the protocol from durable database identity, never from a mutable runtime flag alone.
    pub fn block_commit_protocol_version(&self) -> Result<&'static str, String> {
        Ok(if self.get_native_bootstrap_state()?.is_some() {
            BOOTSTRAP_COMMIT_PROTOCOL_VERSION
        } else {
            crate::COMMIT_PROTOCOL_VERSION
        })
    }

    /// Reject incomplete, incompatible or substituted native state before a normal service opens it.
    pub fn validate_native_bootstrap_service(&self) -> Result<(), String> {
        let state = self.get_native_bootstrap_state()?;
        match (&self.config.bootstrap, state) {
            (None, None) => Ok(()),
            (Some(config), Some(state)) => {
                let height = state.identity.origin_height;
                if state.identity != config.identity || state.phase != NativeBootstrapPhase::Sealed {
                    return Err("Native bootstrap is not sealed or its configuration identity differs".to_string());
                }
                let commit = self.get_block_commit(height)?.ok_or("Native origin block commit is missing")?;
                if self.get_btc_block_height()? < height
                    || self.get_query_retention_floors()? != (height, height + 1)
                    || commit.btc_block_hash != state.identity.origin_block_hash
                    || Some(crate::assumeutxo::format::hex(&commit.balance_delta_root)) != state.origin_balance_delta_root
                    || Some(crate::assumeutxo::format::hex(&commit.block_commit)) != state.origin_commit {
                    return Err("Native origin commit, height or query floors are inconsistent".to_string());
                }
                Ok(())
            }
            _ => Err("Native bootstrap configuration and database kind differ; use a separate matching state directory".to_string()),
        }
    }

    pub(crate) fn begin_native_bootstrap(
        &self,
        identity: &NativeBootstrapIdentity,
    ) -> Result<NativeBootstrapState, String> {
        if let Some(state) = self.get_native_bootstrap_state()? {
            if state.identity != *identity {
                return Err("Native bootstrap resume identity mismatch".to_string());
            }
            return Ok(state);
        }
        if self.get_btc_block_height()? != 0
            || self.get_assumeutxo_import_state()?.is_some()
            || self.get_snapshot_install_provenance()?.is_some()
        {
            return Err("Native bootstrap requires a new empty database".to_string());
        }
        for name in BALANCE_HISTORY_COLUMN_FAMILIES {
            if name == META_CF {
                continue;
            }
            let cf = self
                .db
                .cf_handle(name)
                .ok_or("Missing native bootstrap column family")?;
            if let Some(row) = self.db.iterator_cf(cf, IteratorMode::Start).next() {
                row.map_err(|e| e.to_string())?;
                return Err(format!(
                    "Native bootstrap requires empty state: column_family={name}"
                ));
            }
        }
        let state = NativeBootstrapState {
            schema_version: NATIVE_BOOTSTRAP_SCHEMA.to_string(),
            identity: identity.clone(),
            checkpoint: identity.checkpoint()?,
            phase: NativeBootstrapPhase::Importing,
            imported_coins: 0,
            source_verification: None,
            origin: None,
            origin_commit: None,
            origin_balance_delta_root: None,
            origin_state_digest: None,
            commit_protocol_version: BOOTSTRAP_COMMIT_PROTOCOL_VERSION.to_string(),
        };
        self.write_native_bootstrap_batch(WriteBatch::default(), &state)?;
        Ok(state)
    }

    // A resumed prefix must equal the current verified source, even after a failed earlier scan.
    pub(crate) fn verify_native_import_prefix(&self, coins: &[SnapshotCoin]) -> Result<(), String> {
        use usdb_util::ToBtcScriptHash;
        let cf = self
            .db
            .cf_handle(UTXO_CF)
            .ok_or("Missing native import UTXOs")?;
        let keys: Vec<_> = coins
            .iter()
            .map(|coin| Self::make_utxo_key(&coin.outpoint))
            .collect();
        let saved = self
            .db
            .multi_get_pinned_cf(keys.iter().map(|key| (cf, key.as_slice())));
        for (coin, saved) in coins.iter().zip(saved) {
            let expected =
                usdb_util::UTXOValue::encode(&coin.script.to_btc_script_hash(), coin.value);
            if saved
                .map_err(|e| e.to_string())?
                .is_none_or(|saved| saved.as_ref() != expected)
            {
                return Err("Native import prefix differs from source; keep this staging for diagnosis and bootstrap a fresh directory".to_string());
            }
        }
        Ok(())
    }

    // Source coin effects and checkpoint are one synchronous WAL batch, including zero-valued coins.
    pub(crate) fn import_native_bootstrap_coins(
        &self,
        coins: &[SnapshotCoin],
        processed: u64,
    ) -> Result<(), String> {
        let mut state = self
            .get_native_bootstrap_state()?
            .ok_or("Missing native import marker")?;
        if state.phase != NativeBootstrapPhase::Importing
            || state.imported_coins.checked_add(coins.len() as u64) != Some(processed)
        {
            return Err("Noncontiguous native import checkpoint".to_string());
        }
        let mut batch = WriteBatch::default();
        self.append_snapshot_coins(&mut batch, coins, state.identity.snapshot.base_height)?;
        state.imported_coins = processed;
        self.write_native_bootstrap_batch(batch, &state)
    }

    pub(crate) fn finish_native_bootstrap_import(&self, scan: &SnapshotScan) -> Result<(), String> {
        let mut state = self
            .get_native_bootstrap_state()?
            .ok_or("Missing native import marker")?;
        if state.phase != NativeBootstrapPhase::Importing
            || state.identity.snapshot != scan.identity
            || state.imported_coins != scan.coins
        {
            return Err("Native import verification identity/count mismatch".to_string());
        }
        // Core authenticates the UTXO state; our independently reviewed checkpoint authenticates
        // the history prefix. Never seed this chain with zero or the current-state digest.
        let anchor = state.checkpoint.block_entry()?;
        let mut batch = WriteBatch::default();
        self.append_native_origin_metadata(&mut batch, &anchor)?;
        state.phase = NativeBootstrapPhase::Replaying;
        state.source_verification = Some(scan.clone());
        self.write_native_bootstrap_batch(batch, &state)
    }

    // Seal the state audit and query/recovery floors without changing the replayed v1 record.
    pub(crate) fn seal_native_bootstrap(
        &self,
        origin: &BootstrapOriginIdentity,
    ) -> Result<(), String> {
        let mut state = self
            .get_native_bootstrap_state()?
            .ok_or("Missing native bootstrap marker")?;
        if state.phase != NativeBootstrapPhase::Replaying
            || self.get_btc_block_height()? != state.identity.origin_height
            || origin.origin_height != state.identity.origin_height
            || origin.origin_block_hash != state.identity.origin_block_hash
            || self.is_rollback_in_progress()?
        {
            return Err("Native sealing requires exact, verified origin state".to_string());
        }
        let state_digest = derive_origin_state_digest(origin)?;
        let anchor = self
            .get_block_commit(origin.origin_height)?
            .ok_or("Missing replayed genesis commit")?;
        if anchor.btc_block_hash != origin.origin_block_hash {
            return Err("Replayed genesis commit differs from pinned BTC identity".to_string());
        }
        let mut batch = WriteBatch::default();
        let cf = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or("Missing commit column family")?;
        batch.delete_range_cf(cf, 0u32.to_be_bytes(), origin.origin_height.to_be_bytes());
        for name in [
            BLOCK_UNDO_META_CF,
            BLOCK_UNDO_CREATED_UTXOS_CF,
            BLOCK_UNDO_SPENT_UTXOS_CF,
            BLOCK_UNDO_BALANCE_INDEX_CF,
        ] {
            let cf = self
                .db
                .cf_handle(name)
                .ok_or("Missing undo column family")?;
            batch.delete_range_cf(
                cf,
                0u32.to_be_bytes(),
                (origin.origin_height + 1).to_be_bytes(),
            );
        }
        self.append_native_origin_metadata(&mut batch, &anchor)?;
        state.phase = NativeBootstrapPhase::Sealed;
        state.origin = Some(origin.clone());
        state.origin_commit = Some(crate::assumeutxo::format::hex(&anchor.block_commit));
        state.origin_balance_delta_root =
            Some(crate::assumeutxo::format::hex(&anchor.balance_delta_root));
        state.origin_state_digest = Some(state_digest);
        self.write_native_bootstrap_batch(batch, &state)
    }

    fn append_native_origin_metadata(
        &self,
        batch: &mut WriteBatch,
        anchor: &BlockCommitEntry,
    ) -> Result<(), String> {
        let meta = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing native metadata column family")?;
        let commits = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or("Missing native commits column family")?;
        let height = anchor.block_height;
        for (key, value) in [
            (META_KEY_BTC_BLOCK_HEIGHT, height),
            (META_KEY_BALANCE_QUERY_FLOOR, height),
            (META_KEY_HISTORY_QUERY_FLOOR, height + 1),
            (META_KEY_ROLLBACK_SUPPORTED_FROM_HEIGHT, height + 1),
            (META_KEY_UNDO_RETAINED_FROM_HEIGHT, height + 1),
        ] {
            batch.put_cf(meta, key, value.to_be_bytes());
        }
        batch.put_cf(
            commits,
            height.to_be_bytes(),
            Self::serialize_block_commit_value(anchor),
        );
        Ok(())
    }

    fn write_native_bootstrap_batch(
        &self,
        mut batch: WriteBatch,
        state: &NativeBootstrapState,
    ) -> Result<(), String> {
        state.validate()?;
        let meta = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing native metadata column family")?;
        batch.put_cf(
            meta,
            NATIVE_STATE,
            serde_json::to_vec(state).map_err(|e| e.to_string())?,
        );
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(&batch, &options)
            .map_err(|e| format!("Commit native bootstrap {:?}: {e}", state.phase))
    }

    /// Independently aggregate persisted live UTXOs using bounded batches and a private scratch DB.
    /// Matching the complete ordered projection detects per-script errors even when totals agree.
    pub(crate) fn verify_native_bootstrap_balances(
        &self,
        origin: &BootstrapOriginIdentity,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let parent = self
            .file
            .parent()
            .ok_or("Missing native verification parent directory")?;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let scratch_path = parent.join(format!("native-verify-{}-{unique}", std::process::id()));
        std::fs::create_dir(&scratch_path)
            .map_err(|e| format!("Create native verification workspace: {e}"))?;
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                if let Err(error) = std::fs::remove_dir_all(&self.0) {
                    log::warn!(
                        "Native verification scratch cleanup failed: path={}, error={error}",
                        self.0.display()
                    );
                }
            }
        }
        let _scratch = Scratch(scratch_path.clone());
        let mut options = Options::default();
        options.create_if_missing(true);
        options.set_write_buffer_size(32 * 1024 * 1024);
        options.set_max_write_buffer_number(2);
        options.set_max_open_files(64);
        let scratch = DB::open(&options, &scratch_path).map_err(|e| e.to_string())?;
        let cf = self
            .db
            .cf_handle(UTXO_CF)
            .ok_or("Missing verification UTXO column family")?;
        let view = self.db.snapshot();
        let mut sums = HashMap::<[u8; 32], u64>::new();
        let mut scanned = 0u64;
        let started = Instant::now();
        let mut progress = Instant::now();
        eprintln!(
            "Native balance verification started: height={}",
            origin.origin_height
        );
        let mut read_options = ReadOptions::default();
        read_options.set_total_order_seek(true);
        read_options.fill_cache(false);
        for row in view.iterator_cf_opt(cf, read_options, IteratorMode::Start) {
            let (_, value) = row.map_err(|e| e.to_string())?;
            if value.len() != 40 {
                return Err("Invalid native verification UTXO value".to_string());
            }
            let hash: [u8; 32] = value[..32].try_into().unwrap();
            let amount = u64::from_be_bytes(value[32..].try_into().unwrap());
            if amount != 0 {
                let total = sums.entry(hash).or_default();
                *total = total
                    .checked_add(amount)
                    .ok_or("Native verification balance overflow")?;
            }
            scanned += 1;
            if scanned.is_multiple_of(20_000) {
                if cancelled() {
                    return Err(
                        "Native bootstrap cancelled during balance verification".to_string()
                    );
                }
                flush_sums(&scratch, &mut sums)?;
            }
            if progress.elapsed().as_secs() >= 10 {
                eprintln!(
                    "Native balance verification progress: utxos={scanned}, elapsed_seconds={:.1}",
                    started.elapsed().as_secs_f64()
                );
                progress = Instant::now();
            }
        }
        flush_sums(&scratch, &mut sums)?;
        let mut hash = Sha256::new();
        let mut rows = 0u64;
        let mut total_sats = 0u64;
        for row in scratch.iterator(IteratorMode::End) {
            let (key, value) = row.map_err(|e| e.to_string())?;
            if rows.is_multiple_of(4096) && cancelled() {
                return Err("Native bootstrap cancelled during balance comparison".to_string());
            }
            if key.len() != 32 || value.len() != 8 {
                return Err("Invalid native verification aggregate".to_string());
            }
            hash.update(&key);
            hash.update(&value);
            rows += 1;
            if progress.elapsed().as_secs() >= 10 {
                eprintln!(
                    "Native balance comparison progress: balances={rows}, elapsed_seconds={:.1}",
                    started.elapsed().as_secs_f64()
                );
                progress = Instant::now();
            }
            total_sats = total_sats
                .checked_add(u64::from_be_bytes(value.as_ref().try_into().unwrap()))
                .ok_or("Native verification total overflow")?;
        }
        let actual = OriginTableDigest {
            rows,
            total_sats,
            sha256: crate::assumeutxo::format::hex(&hash.finalize()),
        };
        if scanned != origin.utxos.rows || actual != origin.balances {
            return Err(format!(
                "Native per-script balance verification failed: expected={:?}, actual={actual:?}",
                origin.balances
            ));
        }
        eprintln!(
            "Native balance verification finished: utxos={scanned}, balances={rows}, elapsed_seconds={:.1}",
            started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}

// This aggregation reads persisted UTXOs, independently of import/replay balance updates.
fn flush_sums(db: &DB, sums: &mut HashMap<[u8; 32], u64>) -> Result<(), String> {
    let mut batch = WriteBatch::default();
    for (script, amount) in sums.drain() {
        let previous = match db.get(script).map_err(|e| e.to_string())? {
            Some(bytes) if bytes.len() == 8 => {
                u64::from_be_bytes(bytes.as_slice().try_into().unwrap())
            }
            Some(_) => return Err("Invalid verification scratch balance".to_string()),
            None => 0,
        };
        batch.put(
            script,
            previous
                .checked_add(amount)
                .ok_or("Native verification aggregate overflow")?
                .to_be_bytes(),
        );
    }
    let mut options = WriteOptions::default();
    options.disable_wal(true); // Scratch is never resumed or published as authoritative state.
    db.write_opt(&batch, &options).map_err(|e| e.to_string())
}
