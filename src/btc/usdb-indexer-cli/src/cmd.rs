use clap::{Parser, Subcommand};
use usdb_util::{
    USDB_CANDIDATE_SET_SELECTION_RULE, USDB_ECONOMIC_PAGE_MAX_LIMIT,
    USDB_ECONOMIC_STATE_VIEW_VERSION, USDB_INDEXER_SERVICE_HTTP_PORT,
};

#[derive(Parser, Debug)]
#[command(name = "usdb-indexer-cli")]
#[command(about = "USDB indexer JSON-RPC client")]
pub struct Cli {
    #[arg(short, long, default_value_t = format!("http://127.0.0.1:{}", USDB_INDEXER_SERVICE_HTTP_PORT))]
    pub url: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Get RPC metadata and feature list.
    RpcInfo,

    /// Get current Bitcoin network type.
    NetworkType,

    /// Get current synced block height.
    SyncedHeight,

    /// Get local pass block commit metadata at target height.
    PassBlockCommit {
        #[arg(long)]
        block_height: Option<u32>,
    },

    /// Get the exact historical snapshot/local/system state reference.
    StateRef {
        #[arg(long)]
        block_height: u32,

        /// Optional `ConsensusQueryContext` JSON object.
        #[arg(long, value_name = "JSON")]
        context: Option<String>,
    },

    /// Get indexer sync status.
    SyncStatus {
        /// Keep polling sync status.
        #[arg(long, default_value_t = false)]
        watch: bool,

        /// Polling interval in milliseconds when --watch is enabled.
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
    },

    /// Gracefully stop usdb-indexer service.
    Stop,

    /// Get pass snapshot at target height.
    PassSnapshot {
        #[arg(long)]
        inscription_id: String,

        #[arg(long)]
        at_height: Option<u32>,
    },

    /// Get active passes at target height with pagination.
    ActivePasses {
        #[arg(long)]
        at_height: Option<u32>,

        #[arg(long, default_value_t = 0)]
        page: usize,

        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },

    /// Get pass-state aggregate stats at target height.
    PassStats {
        #[arg(long)]
        at_height: Option<u32>,
    },

    /// Get active pass for one owner at target height.
    OwnerActivePass {
        #[arg(long)]
        owner: String,

        #[arg(long)]
        at_height: Option<u32>,
    },

    /// Get pass history in a closed height range.
    PassHistory {
        #[arg(long)]
        inscription_id: String,

        #[arg(long)]
        from_height: u32,

        #[arg(long)]
        to_height: u32,

        #[arg(long, default_value = "asc")]
        order: String,

        #[arg(long, default_value_t = 0)]
        page: usize,

        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },

    /// Get pass energy snapshot.
    PassEnergy {
        #[arg(long)]
        inscription_id: String,

        #[arg(long)]
        block_height: Option<u32>,

        #[arg(long)]
        mode: Option<String>,
    },

    /// Get pass energy timeline in a closed height range.
    PassEnergyRange {
        #[arg(long)]
        inscription_id: String,

        #[arg(long)]
        from_height: u32,

        #[arg(long)]
        to_height: u32,

        #[arg(long, default_value = "asc")]
        order: String,

        #[arg(long, default_value_t = 0)]
        page: usize,

        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },

    /// Get pass energy leaderboard at target height.
    PassEnergyLeaderboard {
        #[arg(long)]
        at_height: Option<u32>,

        /// Leaderboard scope: active | active_dormant | all.
        #[arg(long)]
        scope: Option<String>,

        #[arg(long, default_value_t = 0)]
        page: usize,

        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },

    /// Get one versioned UIP-0006 pass economic profile.
    PassEconomicProfile {
        #[arg(long)]
        pass_id: String,

        #[arg(long)]
        block_height: Option<u32>,

        /// Optional `ConsensusQueryContext` JSON object.
        #[arg(long, value_name = "JSON")]
        context: Option<String>,

        #[arg(long, default_value = USDB_ECONOMIC_STATE_VIEW_VERSION)]
        view_version: String,
    },

    /// Get one cursor page of the canonical UIP-0006 candidate set.
    CandidateSetView {
        #[arg(long)]
        block_height: Option<u32>,

        /// Optional `ConsensusQueryContext` JSON object.
        #[arg(long, value_name = "JSON")]
        context: Option<String>,

        #[arg(long)]
        cursor: Option<String>,

        #[arg(long, default_value_t = 100, value_parser = parse_economic_limit)]
        limit: usize,

        #[arg(long, default_value = USDB_ECONOMIC_STATE_VIEW_VERSION)]
        view_version: String,

        #[arg(long, default_value = USDB_CANDIDATE_SET_SELECTION_RULE)]
        selection_rule: String,
    },

    /// Get one cursor page of a Leader's UIP-0006 collab breakdown.
    CollabBreakdown {
        #[arg(long)]
        leader_pass_id: String,

        #[arg(long)]
        block_height: Option<u32>,

        /// Optional `ConsensusQueryContext` JSON object.
        #[arg(long, value_name = "JSON")]
        context: Option<String>,

        #[arg(long)]
        cursor: Option<String>,

        #[arg(long, default_value_t = 100, value_parser = parse_economic_limit)]
        limit: usize,

        #[arg(long, default_value = "collab_pass_id_asc")]
        sort: String,

        #[arg(long, default_value = USDB_ECONOMIC_STATE_VIEW_VERSION)]
        view_version: String,
    },

    /// Get the UIP-0006 miner BTC aggregate at one historical context.
    MinerEconomicAggregate {
        #[arg(long)]
        block_height: Option<u32>,

        /// Optional `ConsensusQueryContext` JSON object.
        #[arg(long, value_name = "JSON")]
        context: Option<String>,

        #[arg(long, default_value = USDB_ECONOMIC_STATE_VIEW_VERSION)]
        view_version: String,
    },

    /// Get invalid pass list in a closed height range.
    InvalidPasses {
        #[arg(long)]
        error_code: Option<String>,

        #[arg(long)]
        from_height: u32,

        #[arg(long)]
        to_height: u32,

        #[arg(long, default_value_t = 0)]
        page: usize,

        #[arg(long, default_value_t = 100)]
        page_size: usize,
    },

    /// Perform arbitrary JSON-RPC call for ad-hoc debugging.
    Raw {
        #[arg(long)]
        method: String,

        /// JSON array string, for example: '[{"at_height":900000,"page":0,"page_size":10}]'
        #[arg(long, default_value = "[]")]
        params: String,
    },
}

fn parse_economic_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|error| format!("Invalid economic page limit {}: {}", value, error))?;
    if !(1..=USDB_ECONOMIC_PAGE_MAX_LIMIT).contains(&limit) {
        return Err(format!(
            "Economic page limit must be between 1 and {}",
            USDB_ECONOMIC_PAGE_MAX_LIMIT
        ));
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_set_view_uses_protocol_defaults() {
        let cli = Cli::try_parse_from(["usdb-indexer-cli", "candidate-set-view"]).unwrap();

        let Commands::CandidateSetView {
            view_version,
            selection_rule,
            limit,
            cursor,
            ..
        } = cli.command
        else {
            panic!("expected candidate-set-view command");
        };

        assert_eq!(view_version, USDB_ECONOMIC_STATE_VIEW_VERSION);
        assert_eq!(selection_rule, USDB_CANDIDATE_SET_SELECTION_RULE);
        assert_eq!(limit, 100);
        assert!(cursor.is_none());
    }

    #[test]
    fn collab_breakdown_rejects_out_of_range_limit() {
        let error = Cli::try_parse_from([
            "usdb-indexer-cli",
            "collab-breakdown",
            "--leader-pass-id",
            "leaderi0",
            "--limit",
            "0",
        ])
        .unwrap_err();

        assert!(error.to_string().contains("Economic page limit"));
    }
}
