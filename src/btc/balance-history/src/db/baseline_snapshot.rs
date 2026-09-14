//! Export both supported source kinds through one consistent, normalized read view.

use std::collections::BTreeMap;

use bitcoincore_rpc::bitcoin::Block;
use rust_rocksdb::{Direction, IteratorMode, ReadOptions};
use usdb_util::ToBtcScriptHash;

use super::{
    BALANCE_HISTORY_CF, BLOCK_COMMITS_CF, BalanceHistoryDB, BalanceHistoryDBIdentity, META_CF,
    META_KEY_BTC_BLOCK_HEIGHT, SCRIPT_REGISTRY_CF, UTXO_CF,
};
use crate::baseline_snapshot::{BaselineIdentity, BaselineSource, Result, storage::Writer};
use crate::bootstrap::NativeBootstrapPhase;

impl BalanceHistoryDB {
    pub(crate) fn baseline_source(&self, identity: &BaselineIdentity) -> Result<BaselineSource> {
        if self.get_btc_block_height()? != identity.height || self.is_rollback_in_progress()? {
            return Err("Baseline export requires exact genesis height and no rollback".into());
        }
        let source = if let Some(state) = self.get_native_bootstrap_state()? {
            if state.phase != NativeBootstrapPhase::Sealed
                || state.identity.origin_height != identity.height
                || self.get_db_identity()?
                    != Some(BalanceHistoryDBIdentity::for_native_network(
                        identity.network,
                    ))
                || state.identity.origin_block_hash != identity.block_hash
                || state.identity.snapshot.network != identity.network
                || self.get_query_retention_floors()? != (identity.height, identity.height + 1)
            {
                return Err("Baseline export requires a matching sealed native genesis".into());
            }
            BaselineSource::Assumeutxo {
                state: Box::new(state),
            }
        } else {
            let actual = self
                .get_db_identity()?
                .ok_or("Missing source database identity")?;
            if actual != BalanceHistoryDBIdentity::for_network(identity.network)
                || self.get_assumeutxo_import_state()?.is_some()
                || self.get_snapshot_install_provenance()?.is_some()
            {
                return Err("Baseline export requires full replay or sealed native state; use the explicit legacy converter for split artifacts".into());
            }
            BaselineSource::FullReplay {
                db_identity: actual,
            }
        };
        Ok(source)
    }

    /// Read a bounded descending page; the caller rejects any mutation of the offline file set.
    pub(crate) fn baseline_rows(
        &self,
        balances: bool,
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let view = self.db.snapshot();
        let cf = self
            .db
            .cf_handle(if balances {
                BALANCE_HISTORY_CF
            } else {
                UTXO_CF
            })
            .ok_or("Missing baseline source table")?;
        let mut options = ReadOptions::default();
        options.set_total_order_seek(true);
        options.fill_cache(false);
        let mode = after
            .map(|key| IteratorMode::From(key, Direction::Reverse))
            .unwrap_or(IteratorMode::End);
        let mut result = Vec::new();
        for row in view.iterator_cf_opt(cf, options, mode) {
            let (key, value) = row?;
            if after == Some(key.as_ref()) {
                continue;
            }
            result.push((key.to_vec(), value.to_vec()));
            if result.len() == limit {
                break;
            }
        }
        Ok(result)
    }

    pub(crate) fn export_baseline_view(
        &self,
        writer: &mut Writer,
        block: &Block,
    ) -> Result<BaselineSource> {
        let identity = &writer.identity;
        let source = self.baseline_source(identity)?;
        let view = self.db.snapshot();
        let meta = self
            .db
            .cf_handle(META_CF)
            .ok_or("Missing source metadata")?;
        let height = view
            .get_cf(meta, META_KEY_BTC_BLOCK_HEIGHT)?
            .ok_or("Missing source height")?;
        if height.as_slice() != identity.height.to_be_bytes() {
            return Err("Baseline height changed before the read view was acquired".into());
        }
        let commits = self
            .db
            .cf_handle(BLOCK_COMMITS_CF)
            .ok_or("Missing source commits")?;
        let raw = view
            .get_cf(commits, identity.height.to_be_bytes())?
            .ok_or("Missing original C(G)")?;
        let commit = Self::parse_block_commit_value(identity.height, &raw)?;
        if commit.btc_block_hash != identity.block_hash {
            return Err("Source C(G) has a different BTC block hash".into());
        }
        writer.conn.execute(
            "INSERT INTO block_commits VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                identity.height,
                commit.btc_block_hash.as_ref() as &[u8],
                &commit.balance_delta_root[..],
                &commit.block_commit[..]
            ],
        )?;
        let mut previous_script: Option<Vec<u8>> = None;
        for name in [UTXO_CF, BALANCE_HISTORY_CF] {
            eprintln!("Baseline export stage started: stage={name}");
            let cf = self
                .db
                .cf_handle(name)
                .ok_or("Missing source state table")?;
            let mut options = ReadOptions::default();
            options.set_total_order_seek(true);
            options.fill_cache(false);
            for row in view.iterator_cf_opt(cf, options, IteratorMode::End) {
                let (key, value) = row?;
                if name == UTXO_CF {
                    if key.len() != 36 || value.len() != 40 {
                        return Err("Invalid source UTXO encoding".into());
                    }
                    writer.put_utxo(
                        &key,
                        &value[..32],
                        u64::from_be_bytes(value[32..].try_into()?),
                    )?;
                } else {
                    if key.len() != 36 || value.len() != 16 {
                        return Err("Invalid source balance encoding".into());
                    }
                    if u32::from_be_bytes(key[32..].try_into()?) > writer.identity.height {
                        return Err("Source balance row is above genesis".into());
                    }
                    if previous_script.as_deref() != Some(&key[..32]) {
                        writer
                            .put_balance(&key[..32], u64::from_be_bytes(value[8..].try_into()?))?;
                        previous_script = Some(key[..32].to_vec());
                    } else {
                        writer.progress("balance_history_scan")?;
                    }
                }
            }
            eprintln!("Baseline export stage finished: stage={name}");
        }
        writer.required_scripts(block)?;
        let genesis_scripts: BTreeMap<Vec<u8>, Vec<u8>> = block
            .txdata
            .iter()
            .flat_map(|tx| &tx.output)
            .map(|output| {
                (
                    (output.script_pubkey.to_btc_script_hash().as_ref() as &[u8]).to_vec(),
                    output.script_pubkey.as_bytes().to_vec(),
                )
            })
            .collect();
        let registry = self
            .db
            .cf_handle(SCRIPT_REGISTRY_CF)
            .ok_or("Missing source registry")?;
        let mut after: Option<Vec<u8>> = None;
        loop {
            let keys = writer.script_page(after.as_deref())?;
            if keys.is_empty() {
                break;
            }
            for key in &keys {
                let script = match view.get_cf(registry, key)? {
                    Some(script) => script,
                    None => genesis_scripts
                        .get(key)
                        .cloned()
                        .ok_or("Source registry is missing a live UTXO script")?,
                };
                writer.put_script(key, &script)?;
            }
            after = keys.last().cloned();
        }
        Ok(source)
    }
}
