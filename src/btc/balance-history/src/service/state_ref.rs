use crate::config::BalanceHistoryConfig;
use crate::db::BalanceHistoryDB;
use crate::service::{
    BALANCE_HISTORY_API_VERSION, BALANCE_HISTORY_SEMANTICS_VERSION, BALANCE_HISTORY_STABLE_LAG,
    HistoricalSnapshotStateRef,
};
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, CONSENSUS_SOURCE_CHAIN_BTC,
    ConsensusSnapshotIdentity, USDB_INDEX_FORMULA_VERSION, USDB_INDEX_PROTOCOL_VERSION,
    build_consensus_snapshot_id,
};

/// Public version string of the first balance-history block commit protocol.
pub const COMMIT_PROTOCOL_VERSION: &str = "1.0.0";
/// Hash algorithm used by both balance delta roots and rolling block commits.
pub const COMMIT_HASH_ALGO: &str = "sha256";

/// encode_commit_hex converts internal commit bytes to the lowercase hex strings returned by RPC.
pub fn encode_commit_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

/// Builds the canonical consensus snapshot identity for one exact committed BTC height.
pub fn build_consensus_snapshot_identity(
    config: &BalanceHistoryConfig,
    stable_height: u32,
    stable_block_hash: &str,
) -> ConsensusSnapshotIdentity {
    ConsensusSnapshotIdentity {
        source_chain: CONSENSUS_SOURCE_CHAIN_BTC.to_string(),
        network: config.btc.network().to_string(),
        stable_height,
        stable_block_hash: stable_block_hash.to_string(),
        stable_lag: BALANCE_HISTORY_STABLE_LAG,
        balance_history_api_version: BALANCE_HISTORY_API_VERSION.to_string(),
        balance_history_semantics_version: BALANCE_HISTORY_SEMANTICS_VERSION.to_string(),
        usdb_index_formula_version: USDB_INDEX_FORMULA_VERSION.to_string(),
        usdb_index_protocol_version: USDB_INDEX_PROTOCOL_VERSION.to_string(),
    }
}

/// Reconstructs the exact historical state ref at one committed BTC height from local DB state.
pub fn build_historical_state_ref_at_height(
    config: &BalanceHistoryConfig,
    db: &BalanceHistoryDB,
    block_height: u32,
) -> Result<Option<HistoricalSnapshotStateRef>, String> {
    let commit = db.get_block_commit(block_height)?;
    let Some(commit) = commit else {
        return Ok(None);
    };

    let stable_block_hash = format!("{:x}", commit.btc_block_hash);
    let consensus_identity =
        build_consensus_snapshot_identity(config, block_height, &stable_block_hash);
    let snapshot_id = build_consensus_snapshot_id(&consensus_identity);

    Ok(Some(HistoricalSnapshotStateRef {
        block_height,
        stable_block_hash,
        latest_block_commit: encode_commit_hex(&commit.block_commit),
        consensus_identity,
        snapshot_id,
        snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
        commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
    }))
}
