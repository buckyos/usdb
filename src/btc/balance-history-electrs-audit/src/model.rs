use serde::{Deserialize, Serialize};

pub const AUDIT_REPORT_VERSION: &str = "balance-history-electrs-audit-report:v1";
pub const AUDIT_CHECKPOINT_VERSION: &str = "balance-history-electrs-audit-checkpoint:v1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunIdentity {
    pub snapshot_file: String,
    pub declared_snapshot_sha256: String,
    pub snapshot_height: u32,
    pub snapshot_block_hash: String,
    pub electrs_url: String,
    pub seed: String,
    pub sample_count: usize,
    pub zero_sample_percent: u8,
    pub max_history_entries: usize,
    pub blacklist_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub file: String,
    pub manifest_file: String,
    pub manifest_version: String,
    pub declared_file_sha256: String,
    pub file_sha256_verified: bool,
    pub snapshot_id: Option<String>,
    pub height: u32,
    pub block_hash: String,
    pub db_schema_version: u32,
    pub balance_history_count: u64,
    pub utxo_count: u64,
    pub block_commit_count: u64,
    pub script_registry_count: u64,
    pub btc_network: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ElectrsSummary {
    pub url: String,
    pub server_version: String,
    pub protocol_min: String,
    pub protocol_max: String,
    pub tip_height: u32,
    pub target_header_hash: String,
    pub configured_index_lookup_limit: Option<usize>,
    pub server_limit_config_validated: bool,
    pub runtime_limit_operator_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SampleKind {
    PositiveBalance,
    ZeroBalance,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SampleStatus {
    Matched,
    Mismatch,
    SkippedTooPopular,
    SkippedBip30Ambiguous,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SampleResult {
    pub sample_id: String,
    pub kind: SampleKind,
    pub script_hash: String,
    pub script_pubkey_hex: String,
    pub script_type: String,
    pub expected_balance: u64,
    pub computed_balance: Option<u64>,
    pub last_change_height: Option<u32>,
    pub history_entries: Option<usize>,
    pub confirmed_entries_replayed: Option<usize>,
    pub status: SampleStatus,
    pub detail: Option<String>,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditSummary {
    pub planned_samples: usize,
    pub completed_samples: usize,
    pub matched: usize,
    pub mismatched: usize,
    pub skipped_too_popular: usize,
    pub skipped_bip30_ambiguous: usize,
    pub errors: usize,
    pub blacklisted_candidates_replaced: usize,
    pub duplicate_candidates_replaced: usize,
    pub history_requests_this_process: u64,
    pub transaction_requests_this_process: u64,
    pub transaction_cache_hits_this_process: u64,
    pub transaction_cache_entries_at_completion: u64,
    pub transaction_cache_weighted_bytes_at_completion: u64,
    pub complete: bool,
    pub ok: bool,
}

impl AuditSummary {
    pub fn from_results(
        planned_samples: usize,
        results: &[SampleResult],
        allow_skipped: bool,
    ) -> Self {
        let mut summary = Self {
            planned_samples,
            completed_samples: results.len(),
            ..Self::default()
        };
        for result in results {
            match result.status {
                SampleStatus::Matched => summary.matched += 1,
                SampleStatus::Mismatch => summary.mismatched += 1,
                SampleStatus::SkippedTooPopular => summary.skipped_too_popular += 1,
                SampleStatus::SkippedBip30Ambiguous => summary.skipped_bip30_ambiguous += 1,
                SampleStatus::Error => summary.errors += 1,
            }
        }
        let skipped = summary.skipped_too_popular + summary.skipped_bip30_ambiguous;
        summary.complete = summary.completed_samples == planned_samples && skipped == 0;
        summary.ok = summary.completed_samples == planned_samples
            && summary.mismatched == 0
            && summary.errors == 0
            && (allow_skipped || skipped == 0);
        summary
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditReport {
    pub report_version: String,
    pub run_id: String,
    pub started_at_unix: u64,
    pub completed_at_unix: u64,
    pub run_identity: RunIdentity,
    pub snapshot: SnapshotSummary,
    pub electrs: ElectrsSummary,
    pub summary: AuditSummary,
    pub results: Vec<SampleResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditCheckpoint {
    pub checkpoint_version: String,
    pub run_id: String,
    pub started_at_unix: u64,
    pub results: Vec<SampleResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: SampleStatus) -> SampleResult {
        SampleResult {
            sample_id: "positive:000000".to_string(),
            kind: SampleKind::PositiveBalance,
            script_hash: "00".repeat(32),
            script_pubkey_hex: "51".to_string(),
            script_type: "nonstandard".to_string(),
            expected_balance: 1,
            computed_balance: Some(1),
            last_change_height: Some(1),
            history_entries: Some(1),
            confirmed_entries_replayed: Some(1),
            status,
            detail: None,
            elapsed_ms: 1,
        }
    }

    #[test]
    fn skipped_samples_fail_closed_by_default() {
        let results = vec![result(SampleStatus::SkippedTooPopular)];
        assert!(!AuditSummary::from_results(1, &results, false).ok);
        assert!(AuditSummary::from_results(1, &results, true).ok);
    }
}
