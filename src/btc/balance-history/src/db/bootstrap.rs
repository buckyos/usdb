//! Canonical business-origin projections from a single consistent RocksDB read view.

use std::time::Instant;

use bitcoincore_rpc::bitcoin::{BlockHash, Network};
use rust_rocksdb::{IteratorMode, ReadOptions};
use sha2::{Digest, Sha256};

use super::{
    BALANCE_HISTORY_CF, BALANCE_HISTORY_DATA_MODEL_VERSION, BLOCK_COMMIT_VALUE_LEN,
    BLOCK_COMMITS_CF, BalanceHistoryDB, BalanceHistoryDBIdentity, META_CF,
    META_KEY_BTC_BLOCK_HEIGHT, UTXO_CF,
};
use crate::bootstrap::{
    BOOTSTRAP_COMMIT_PROTOCOL_VERSION, BOOTSTRAP_ORIGIN_SCHEMA, BootstrapOriginIdentity,
    OriginTableDigest,
};

impl BalanceHistoryDB {
    /// Scan exact-height logical state without including old commits, registry or historical deltas.
    /// Used by the offline inspector and the native bootstrap verification before publication.
    pub(crate) fn bootstrap_origin_identity(
        &self,
        network: Network,
        height: u32,
        block_hash: BlockHash,
    ) -> Result<BootstrapOriginIdentity, String> {
        self.bootstrap_origin_identity_cancellable(network, height, block_hash, &|| false)
    }

    pub(crate) fn bootstrap_origin_identity_cancellable(
        &self,
        network: Network,
        height: u32,
        block_hash: BlockHash,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<BootstrapOriginIdentity, String> {
        let identity = self.get_db_identity()?;
        if (identity != Some(BalanceHistoryDBIdentity::for_network(network))
            && identity != Some(BalanceHistoryDBIdentity::for_native_network(network)))
            || self.get_btc_block_height()? != height
            || self.is_rollback_in_progress()?
        {
            return Err("Origin inspection requires matching database identity, exact height and no rollback".to_string());
        }
        if self
            .get_assumeutxo_import_state()?
            .is_some_and(|s| !s.complete)
        {
            return Err("Cannot inspect an incomplete AssumeUTXO import".to_string());
        }
        let view = self.db.snapshot();
        let meta_cf = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing origin metadata column family")?;
        let saved_height = view
            .get_cf(meta_cf, META_KEY_BTC_BLOCK_HEIGHT)
            .map_err(|e| e.to_string())?
            .ok_or("Missing origin height metadata")?;
        if saved_height.as_slice() != height.to_be_bytes() {
            return Err("Origin height changed before read view was acquired".to_string());
        }
        let commit_cf = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or("Missing origin block identity column family")?;
        let saved_commit = view
            .get_cf(commit_cf, height.to_be_bytes())
            .map_err(|e| e.to_string())?
            .ok_or("Missing BTC block identity at origin")?;
        if saved_commit.len() != BLOCK_COMMIT_VALUE_LEN
            || &saved_commit[..32] != block_hash.as_ref() as &[u8]
        {
            return Err("Origin BTC block hash differs from pinned hash".to_string());
        }
        let mut tables = Vec::with_capacity(2);
        for (name, cf_name) in [("utxos", UTXO_CF), ("balances", BALANCE_HISTORY_CF)] {
            let cf = self
                .db
                .cf_handle(cf_name)
                .ok_or("Missing origin state column family")?;
            let mut options = ReadOptions::default();
            options.set_total_order_seek(true);
            options.fill_cache(false);
            let mut hash = Sha256::new();
            let mut rows = 0u64;
            let mut scanned = 0u64;
            let mut total_sats = 0u64;
            let mut previous_script: Option<[u8; 32]> = None;
            let begin = Instant::now();
            let mut progress = Instant::now();
            eprintln!("Bootstrap origin scan started: table={name}, height={height}");
            for item in view.iterator_cf_opt(cf, options, IteratorMode::End) {
                let (key, value) = item.map_err(|e| format!("Read origin {name}: {e}"))?;
                scanned += 1;
                if scanned % 4096 == 1 && cancelled() {
                    return Err("Native bootstrap cancelled during origin scan".to_string());
                }
                if progress.elapsed().as_secs() >= 10 {
                    eprintln!(
                        "Bootstrap origin scan progress: table={name}, scanned={scanned}, rows={rows}, elapsed_seconds={:.1}",
                        begin.elapsed().as_secs_f64()
                    );
                    progress = Instant::now();
                }
                let amount = if name == "utxos" {
                    if key.len() != 36 || value.len() != 40 {
                        return Err("Invalid origin UTXO encoding".to_string());
                    }
                    hash.update(&key);
                    hash.update(&value);
                    u64::from_be_bytes(value[32..40].try_into().unwrap())
                } else {
                    if key.len() != 36 || value.len() != 16 {
                        return Err("Invalid origin balance encoding".to_string());
                    }
                    if Self::parse_block_height_from_key(&key) > height {
                        return Err(
                            "Balance history contains a row above origin height".to_string()
                        );
                    }
                    let script: [u8; 32] = key[..32].try_into().unwrap();
                    if previous_script == Some(script) {
                        continue;
                    }
                    previous_script = Some(script);
                    let balance = Self::parse_balance_from_value(&value).1;
                    if balance == 0 {
                        continue;
                    }
                    hash.update(script);
                    hash.update(balance.to_be_bytes());
                    balance
                };
                total_sats = total_sats
                    .checked_add(amount)
                    .ok_or("Origin amount sum overflow")?;
                rows += 1;
            }
            eprintln!(
                "Bootstrap origin scan finished: table={name}, scanned={scanned}, rows={rows}, total_sats={total_sats}, elapsed_seconds={:.1}",
                begin.elapsed().as_secs_f64()
            );
            tables.push(OriginTableDigest {
                rows,
                total_sats,
                sha256: crate::assumeutxo::format::hex(&hash.finalize()),
            });
        }
        let balances = tables.pop().unwrap();
        let utxos = tables.pop().unwrap();
        if utxos.total_sats != balances.total_sats {
            return Err(format!(
                "Origin UTXO/balance totals differ: utxos={}, balances={}",
                utxos.total_sats, balances.total_sats
            ));
        }
        Ok(BootstrapOriginIdentity {
            schema_version: BOOTSTRAP_ORIGIN_SCHEMA.to_string(),
            commit_protocol_version: BOOTSTRAP_COMMIT_PROTOCOL_VERSION.to_string(),
            network,
            origin_height: height,
            origin_block_hash: block_hash,
            data_model_version: BALANCE_HISTORY_DATA_MODEL_VERSION.to_string(),
            utxos,
            balances,
        })
    }
}
