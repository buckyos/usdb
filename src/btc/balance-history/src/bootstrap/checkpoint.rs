//! Reviewed v1 rolling checkpoints paired with exact Bitcoin Core UTXO snapshot identities.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::assumeutxo::SnapshotIdentity;
use crate::{
    BALANCE_HISTORY_DATA_MODEL_VERSION, BalanceHistoryConfig, BalanceHistoryDB, BlockCommitEntry,
    COMMIT_PROTOCOL_VERSION,
};

/// Return the reviewed checkpoint paired with this exact supported mainnet source.
/// Provenance is recorded in the P4 validation and P6 bootstrap design documents.
pub fn embedded_bootstrap_checkpoint(
    snapshot: &SnapshotIdentity,
) -> Result<BootstrapCommitCheckpoint, String> {
    let checkpoint: BootstrapCommitCheckpoint =
        serde_json::from_str(include_str!("checkpoints/mainnet-935000.json"))
            .map_err(|e| format!("Invalid embedded bootstrap checkpoint: {e}"))?;
    checkpoint.block_entry()?;
    if checkpoint.snapshot != *snapshot {
        return Err(
            "No reviewed embedded commit checkpoint for this snapshot identity".to_string(),
        );
    }
    Ok(checkpoint)
}

/// A checkpoint authenticates the USDB history prefix independently of Core's UTXO commitment.
/// The delta root preserves the baseline block's RPC record even when business genesis equals it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootstrapCommitCheckpoint {
    /// Snapshot identity whose post-block state accompanies this rolling commit.
    pub snapshot: SnapshotIdentity,
    /// Logical interpretation under which the original history was computed.
    pub data_model_version: String,
    /// Existing rolling commitment protocol, shared with full replay.
    pub commit_protocol_version: String,
    /// Hex v1 rolling commit after applying the snapshot block.
    pub block_commit: String,
    /// Hex canonical balance delta root of the snapshot block, taken from the same history.
    pub balance_delta_root: String,
}

impl BootstrapCommitCheckpoint {
    /// Validate versions and encoding and reconstruct the complete baseline commit record.
    pub fn block_entry(&self) -> Result<BlockCommitEntry, String> {
        if self.data_model_version != BALANCE_HISTORY_DATA_MODEL_VERSION
            || self.commit_protocol_version != COMMIT_PROTOCOL_VERSION
            || self.snapshot.base_height == 0
            || self.snapshot.base_height == u32::MAX
        {
            return Err("Unsupported bootstrap checkpoint model, protocol or height".to_string());
        }
        for value in [
            &self.block_commit,
            &self.balance_delta_root,
            &self.snapshot.file_sha256,
            &self.snapshot.hash_serialized_3,
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(
                    "Bootstrap checkpoint hashes must be canonical lowercase hexadecimal"
                        .to_string(),
                );
            }
        }
        Ok(BlockCommitEntry {
            block_height: self.snapshot.base_height,
            btc_block_hash: self
                .snapshot
                .base_hash
                .parse()
                .map_err(|e| format!("Invalid checkpoint BTC hash: {e}"))?,
            block_commit: crate::assumeutxo::format::hash_bytes(&self.block_commit)?,
            balance_delta_root: crate::assumeutxo::format::hash_bytes(&self.balance_delta_root)?,
        })
    }
}

/// Extract a candidate checkpoint from a stopped database, without scanning state or changing files.
/// Extraction is not approval: the source identity and historical chain still require release review.
pub fn inspect_bootstrap_checkpoint(
    state_dir: &Path,
    snapshot: SnapshotIdentity,
) -> Result<BootstrapCommitCheckpoint, String> {
    let result = (|| {
        if !state_dir.is_absolute() {
            return Err("Checkpoint inspection requires an absolute state directory".to_string());
        }
        let mut config = BalanceHistoryConfig {
            root_dir: state_dir.to_path_buf(),
            ..Default::default()
        };
        config.btc.network = snapshot.network;
        let db = BalanceHistoryDB::open_read_only(Arc::new(config))?;
        let entry = db
            .get_block_commit(snapshot.base_height)?
            .ok_or("Checkpoint block is not retained")?;
        if entry.btc_block_hash.to_string() != snapshot.base_hash {
            return Err("Checkpoint BTC hash differs from snapshot identity".to_string());
        }
        let checkpoint = BootstrapCommitCheckpoint {
            snapshot,
            data_model_version: BALANCE_HISTORY_DATA_MODEL_VERSION.to_string(),
            commit_protocol_version: db.block_commit_protocol_version()?.to_string(),
            block_commit: crate::assumeutxo::format::hex(&entry.block_commit),
            balance_delta_root: crate::assumeutxo::format::hex(&entry.balance_delta_root),
        };
        checkpoint.block_entry()?;
        Ok(checkpoint)
    })();
    result.map_err(|error| {
        let message = format!(
            "Bootstrap checkpoint inspection failed: state_dir={}, error={error}",
            state_dir.display()
        );
        log::error!("{message}");
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::{NativeBootstrapConfig, NativeBootstrapIdentity};
    use bitcoincore_rpc::bitcoin::{BlockHash, Network, hashes::Hash};

    #[test]
    fn embedded_checkpoint_pins_history_and_source_but_allows_later_business_genesis() {
        let checkpoint: BootstrapCommitCheckpoint =
            serde_json::from_str(include_str!("checkpoints/mainnet-935000.json")).unwrap();
        assert_eq!(
            checkpoint.block_commit,
            "6108f77e4abaafbc3a7a246024e942483c18fb37a3c710209ea6294c5617fe81"
        );
        let mut config = NativeBootstrapConfig {
            snapshot_file: "/tmp/mainnet-935000-utxos.dat".into(),
            identity: NativeBootstrapIdentity {
                snapshot: checkpoint.snapshot.clone(),
                origin_height: 935000,
                origin_block_hash: checkpoint.block_entry().unwrap().btc_block_hash,
                regtest_checkpoint: None,
            },
            import_batch_size: 31,
            replay_batch_size: 2,
        };
        config.validate(Network::Bitcoin).unwrap();
        config.identity.origin_height -= 1;
        assert!(config.validate(Network::Bitcoin).is_err());
        config.identity.origin_height = 1_000_000;
        config.identity.origin_block_hash = BlockHash::from_byte_array([1; 32]);
        // RPC preflight validates the actual canonical G hash; G is not tied to the lab's 963800.
        config.validate(Network::Bitcoin).unwrap();
        config.identity.regtest_checkpoint = Some(checkpoint.clone());
        assert!(
            config
                .validate(Network::Bitcoin)
                .unwrap_err()
                .contains("cannot override")
        );
        config.identity.regtest_checkpoint = None;
        config.identity.snapshot.file_sha256 = "11".repeat(32);
        assert!(
            config
                .validate(Network::Bitcoin)
                .unwrap_err()
                .contains("No reviewed embedded")
        );
        for field in [
            "data_model_version",
            "commit_protocol_version",
            "block_commit",
            "balance_delta_root",
        ] {
            let mut invalid = serde_json::to_value(&checkpoint).unwrap();
            invalid[field] = "invalid".into();
            let invalid: BootstrapCommitCheckpoint = serde_json::from_value(invalid).unwrap();
            assert!(invalid.block_entry().is_err());
        }
    }
}
