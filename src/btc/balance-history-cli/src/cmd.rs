use bitcoincore_rpc::bitcoin::Network;
use bitcoincore_rpc::bitcoin::address::NetworkUnchecked;
use bitcoincore_rpc::bitcoin::{Address};
use usdb_util::{USDBScriptHash, ToUSDBScriptHash};
use clap::{Parser, Subcommand, Args};
use std::ops::Range;
use std::str::FromStr;
use usdb_util::BALANCE_HISTORY_SERVICE_HTTP_PORT;

#[derive(Parser)]
#[command(name = "btc-rpc-cli")]
#[command(about = "Bitcoin balance history JSON-RPC client")]
pub struct Cli {
    #[arg(short, long, default_value_t = format!("http://127.0.0.1:{}", BALANCE_HISTORY_SERVICE_HTTP_PORT))]
    pub url: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Get current network type
    NetworkType,

    /// Get current synced block height
    Height,

    /// Get sync status and keep displaying updates
    Status,

    /// Stop the balance history service
    Stop,

    /// Get balance history for one script_hash
    Balance {
        #[arg(value_name = "USER_ID")]
        user: String,

        #[clap(flatten)]
        position: BalancesPosition,
    },

    /// Get balance history for multiple script_hashes
    Balances {
        #[arg(value_name = "USER_ID", num_args = 1..)]
        users: Vec<String>,

        #[clap(flatten)]
        position: BalancesPosition,
    },
}

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = false)]
pub struct BalancesPosition {
    /// Block height to get balances at
    #[arg(long, value_name = "HEIGHT")]
    pub height: Option<u32>,

    /// Block range to get balances in
    #[arg(long, value_parser = parse_range, value_name = "START..END")]
    pub range: Option<Range<u32>>,

    /// Get balances for all blocks
    #[arg(short, long, default_value_t = false, value_name = "ALL")]
    pub all: bool,

    /// Get latest balance only
    #[arg(long, default_value_t = false, value_name = "LATEST")]
    pub latest: bool,
}

pub enum UserId {
    Address(Address<NetworkUnchecked>),
    ScriptHash(USDBScriptHash),
}

impl FromStr for UserId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First check if it's a valid Address by trying to parse it
        // If that fails, try to parse it as a USDBScriptHash
        if let Ok(addr) = Address::<NetworkUnchecked>::from_str(s) {
            return Ok(UserId::Address(addr));
        } else if let Ok(script_hash) = s.parse::<USDBScriptHash>() {
            Ok(UserId::ScriptHash(script_hash))
        } else {
            let msg = format!("Invalid user ID: {}", s);
            println!("{}", msg);

            Err(msg)
        }
    }
}

impl UserId {
    pub fn to_script_hash(&self, network: Network) -> Result<USDBScriptHash, String> {
        match self {
            UserId::Address(addr) => {
                // First convert to a NetworkChecked address
                let checked_addr = addr.clone().require_network(network).map_err(|e| {
                    let msg = format!("Address network mismatch: {}", e);
                    println!("{}", msg);
                    msg
                })?;

                Ok(checked_addr.script_pubkey().to_usdb_script_hash())
            }
            UserId::ScriptHash(sh) => Ok(*sh),
        }
    }
}

fn parse_range(s: &str) -> Result<Range<u32>, String> {
    let parts: Vec<&str> = s.split("..").collect();
    if parts.len() != 2 {
        let msg = format!("Invalid range format: {}. Expected format is start..end", s);
        println!("{}", msg);
        return Err(msg);
    }

    let start = parts[0].parse::<u32>().map_err(|e| {
        let msg = format!("Invalid start of range: {}", e);
        println!("{}", msg);
        msg
    })?;

    let end = parts[1].parse::<u32>().map_err(|e| {
        let msg = format!("Invalid end of range: {}", e);
        println!("{}", msg);
        msg
    })?;

    Ok(start..end)
}
