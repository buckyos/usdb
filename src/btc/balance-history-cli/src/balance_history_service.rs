use super::cmd::{Cli, Commands, UserId};
use crate::client::RpcClient;
use bitcoincore_rpc::bitcoin::Network;
use std::{sync::Arc};
use std::str::FromStr;
use balance_history::{AddressBalance, IndexOutput, SyncPhase, SyncStatusManager};

pub struct BalanceHistoryService {
    network: Network,
    client: RpcClient,
    // Implementation details would go here
}

impl BalanceHistoryService {
    pub async fn new(url: &str) -> Result<Self, String> {
        println!("Connecting to Balance History Service at {}", url);
        let client = RpcClient::new(url)?;

        // Try get network type to verify connection
        let network_type = client.get_network_type().await?;
        let network = Network::from_str(&network_type).map_err(|e| {
            let msg = format!("Invalid network type received from server: {}", e);
            log::error!("{}", msg);
            msg
        })?;

        println!("Connected to network type: {}", network);

        Ok(Self { network, client })
    }

    pub async fn process_command(&self, cli: Cli) -> Result<(), String> {
        match cli.command {
            Commands::NetworkType => {
                let network_type = self.client.get_network_type().await?;
                println!("Network Type: {}", network_type);
            }
            Commands::Height => {
                let height = self.client.get_block_height().await?;
                println!("Current Height: {}", height);
            }
            Commands::Status => {
                // Handle Status command
                self.process_sync_status().await?;
            }
            Commands::Balance {
                user,
                height,
                range,
            } => {
                // Handle Balance command
                let user_id = match UserId::from_str(&user) {
                    Ok(id) => id,
                    Err(e) => {
                        let msg = format!("Invalid user ID '{}': {}", user, e);
                        println!("{}", msg);
                        return Err(msg);
                    }
                };

                let script_hash = user_id.to_script_hash(self.network)?;
                let balances = self
                    .client
                    .get_address_balance(script_hash, height, range)
                    .await?;

                BalanceFormatter::print_balances(&balances);
            }
            Commands::Balances {
                users,
                height,
                range,
            } => {
                let mut script_hashes = Vec::new();
                for user in &users {
                    let user_id = match UserId::from_str(&user) {
                        Ok(id) => id,
                        Err(e) => {
                            let msg = format!("Invalid user ID '{}': {}", user, e);
                            println!("{}", msg);
                            return Err(msg);
                        }
                    };

                    let script_hash = user_id.to_script_hash(self.network)?;
                    script_hashes.push(script_hash);
                }

                let all_balances = self
                    .client
                    .get_addresses_balances(script_hashes, height, range)
                    .await?;
                for (user, balances) in users.iter().zip(all_balances) {
                    println!("\nBalance history for user: {}", user);
                    BalanceFormatter::print_balances(&balances);
                }
            }
        }

        Ok(())
    }

    async fn process_sync_status(&self) -> Result<(), String> {
        let status_manager = SyncStatusManager::new();
        let output = IndexOutput::new(Arc::new(status_manager));
        let mut phase = SyncPhase::Initializing;

        loop {
            let status = match self.client.get_sync_status().await {
                Ok(s) => s,
                Err(e) => {
                    let msg = format!("Failed to get sync status: {}", e);
                    output.println(&msg);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

            if phase != status.phase {
                if phase == SyncPhase::Initializing && status.phase == SyncPhase::Loading {
                    phase = SyncPhase::Loading;
                    output.println("Starting loading phase");
                    output.start_load(status.total);
                } else if phase == SyncPhase::Loading && status.phase == SyncPhase::Indexing {
                    phase = SyncPhase::Indexing;
                    output.finish_load();
                    output.println("Starting indexing phase");
                    output.start_index(status.total);
                } else if phase == SyncPhase::Indexing && status.phase == SyncPhase::Synced {
                    output.finish_index();
                    output.println("Syncing complete");
                    phase = SyncPhase::Synced;
                    break;
                } else {
                    let msg = format!("Invalid phase transition from {:?} to {:?}", phase, status.phase);
                    output.println(&msg);
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }
            }

            if phase == SyncPhase::Loading {
                output.update_load_current_count(status.current);
                output.update_load_total_count(status.total);
                if let Some(ref message) = status.message {
                    output.set_load_message(message);
                }
            } else if phase == SyncPhase::Indexing {
                output.update_current_height(status.current);
                output.update_total_block_height(status.total);
                if let Some(ref message) = status.message {
                    output.set_index_message(message);
                }
            } else if phase == SyncPhase::Synced {
                output.println("Service is fully synced.");
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }
}

struct BalanceFormatter;

impl BalanceFormatter {
    fn print_balances(balances: &[AddressBalance]) {
        if balances.is_empty() {
            println!("No balance data found.");
            return;
        }

        println!("\n┌───────────────┬────────────────────┬────────────────────┐");
        println!("│ Block Height  │ Balance (sat)      │ Delta (sat)        │");
        println!("├───────────────┼────────────────────┼────────────────────┤");

        for b in balances {
            println!(
                "│ {:>13} │ {:>18} │ {:>18} │",
                b.block_height,
                Self::format_number(b.balance),
                Self::format_delta(b.delta)
            );
        }

        println!("└───────────────┴────────────────────┴────────────────────┘");
    }

    // Add thousand separators for better readability
    fn format_number(n: u64) -> String {
        let s = n.to_string();
        let mut result = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    }

    fn format_delta(d: i64) -> String {
        if d >= 0 {
            format!("+{}", Self::format_number(d as u64))
        } else {
            format!("-{}", Self::format_number((-d) as u64))
        }
    }
}
