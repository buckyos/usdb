//! SQLite publication of rebuildable mint diagnostics.

use ord::InscriptionId;
use rusqlite::OptionalExtension;
pub use usdb_util::{MinerPassMintAudit, MintSourceAudit};

use super::{BTC_SYNCED_BLOCK_HEIGHT_KEY, MinerPassStorage};

impl MinerPassStorage {
    pub(super) fn init_mint_audit(&self) -> Result<(), String> {
        self.conn.lock().unwrap().execute_batch(
            "CREATE TABLE IF NOT EXISTS miner_pass_mint_audit (
                inscription_id TEXT PRIMARY KEY, block_height INTEGER NOT NULL, audit_json TEXT NOT NULL
             ); CREATE INDEX IF NOT EXISTS idx_mint_audit_height ON miner_pass_mint_audit(block_height);"
        ).map_err(|err| Self::mint_audit_error("initialize", err))
    }

    /// Write inside the enclosing block savepoint; audit fields do not alter mutation encoding.
    pub fn put_mint_audit(&self, audit: &MinerPassMintAudit) -> Result<(), String> {
        self.require_block_savepoint()?;
        let json =
            serde_json::to_string(audit).map_err(|err| Self::mint_audit_error("encode", err))?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO miner_pass_mint_audit(inscription_id,block_height,audit_json) VALUES (?1,?2,?3)",
            rusqlite::params![audit.inscription_id, audit.block_height, json],
        ).map_err(|err| Self::mint_audit_error(&format!("write inscription_id={}", audit.inscription_id), err))?;
        Ok(())
    }

    /// Return only a durably published record, never a pending block's provisional diagnostic.
    #[cfg(test)]
    pub fn get_mint_audit(&self, id: &InscriptionId) -> Result<Option<MinerPassMintAudit>, String> {
        let json: Option<String> = self
            .committed_conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT audit_json FROM miner_pass_mint_audit WHERE inscription_id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| Self::mint_audit_error(&format!("read inscription_id={id}"), err))?;
        json.map(|json| {
            serde_json::from_str(&json).map_err(|err| Self::mint_audit_error("decode", err))
        })
        .transpose()
    }

    /// Read audit and mint existence in one committed snapshot matching the selected block commit.
    /// Returns the mint height separately to distinguish legacy/unknown mints from missing v2 audit.
    pub fn get_mint_audit_at_height(
        &self,
        id: &InscriptionId,
        height: u32,
        expected_block_commit: &str,
    ) -> Result<(Option<MinerPassMintAudit>, Option<u32>), String> {
        let result = (|| {
            let mut conn = self.committed_conn.lock().unwrap();
            let tx = conn.transaction().map_err(|err| err.to_string())?;
            let durable_height = Self::history_number(&tx, BTC_SYNCED_BLOCK_HEIGHT_KEY)?;
            let commit: Option<String> = tx
                .query_row(
                    "SELECT block_commit FROM pass_block_commits WHERE block_height=?1",
                    [height],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|err| err.to_string())?;
            if durable_height.is_none_or(|tip| tip < i64::from(height))
                || commit.as_deref() != Some(expected_block_commit)
            {
                return Err(format!(
                    "Selected state is no longer durably available: durable_height={durable_height:?}, expected_block_commit={expected_block_commit}, actual_block_commit={commit:?}"
                ));
            }
            let mint_height: Option<u32> = tx.query_row(
                "SELECT mint_block_height FROM miner_passes WHERE inscription_id=?1 AND mint_block_height<=?2",
                rusqlite::params![id.to_string(), height], |row| row.get(0),
            ).optional().map_err(|err| err.to_string())?;
            let json: Option<String> = tx.query_row(
                "SELECT audit_json FROM miner_pass_mint_audit WHERE inscription_id=?1 AND block_height<=?2",
                rusqlite::params![id.to_string(), height], |row| row.get(0),
            ).optional().map_err(|err| err.to_string())?;
            let audit: Option<MinerPassMintAudit> = json
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|err| err.to_string())?;
            if let Some(audit) = &audit {
                let hash: Option<String> = tx.query_row(
                    "SELECT stable_block_hash FROM balance_history_snapshot_history WHERE block_height=?1", [audit.block_height], |row| row.get(0),
                ).optional().map_err(|err| err.to_string())?;
                if mint_height != Some(audit.block_height)
                    || audit.inscription_id != id.to_string()
                    || hash.as_deref() != Some(&audit.block_hash)
                    || audit.schema_version != "miner-pass-mint-audit:v1"
                {
                    return Err(format!(
                        "Audit metadata disagrees with committed mint: mint_height={mint_height:?}, audit_height={}, audit_id={}, schema_version={}, expected_block_hash={hash:?}, audit_block_hash={}",
                        audit.block_height,
                        audit.inscription_id,
                        audit.schema_version,
                        audit.block_hash
                    ));
                }
            }
            Ok((audit, mint_height))
        })();
        result.map_err(|err: String| {
            Self::mint_audit_error(
                &format!("read committed snapshot inscription_id={id}, query_height={height}"),
                err,
            )
        })
    }

    fn mint_audit_error(action: &str, error: impl std::fmt::Display) -> String {
        let msg = format!("MinerPass mint audit failed: action={action}, error={error}");
        error!("{msg}");
        msg
    }
}
