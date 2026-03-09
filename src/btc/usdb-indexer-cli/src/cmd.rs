use clap::{Parser, Subcommand};
use usdb_util::USDB_INDEXER_SERVICE_HTTP_PORT;

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

    /// Get active balance snapshot at exact block height.
    ActiveBalanceSnapshot {
        #[arg(long)]
        block_height: u32,
    },

    /// Get latest active balance snapshot.
    LatestActiveBalanceSnapshot,

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
