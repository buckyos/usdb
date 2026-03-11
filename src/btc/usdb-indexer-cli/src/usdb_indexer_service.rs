use crate::cmd::{Cli, Commands};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::{Duration, sleep};

pub struct UsdbIndexerService {
    client: RpcClient,
}

impl UsdbIndexerService {
    pub async fn new(url: &str) -> Result<Self, String> {
        println!("Connecting to USDB indexer at {}", url);
        let client = RpcClient::new(url)?;

        // Probe endpoint during initialization to fail fast on connectivity issues.
        let _ = client.call("get_rpc_info", json!([])).await?;

        Ok(Self { client })
    }

    pub async fn process_command(&self, cli: Cli) -> Result<(), String> {
        match cli.command {
            Commands::RpcInfo => {
                let result = self.client.call("get_rpc_info", json!([])).await?;
                print_pretty_json(&result)?;
            }
            Commands::NetworkType => {
                let result = self.client.call("get_network_type", json!([])).await?;
                print_pretty_json(&result)?;
            }
            Commands::SyncedHeight => {
                let result = self
                    .client
                    .call("get_synced_block_height", json!([]))
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassBlockCommit { block_height } => {
                let result = self
                    .client
                    .call(
                        "get_pass_block_commit",
                        json!([{
                            "block_height": block_height,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::SyncStatus { watch, interval_ms } => {
                if watch {
                    self.watch_sync_status(interval_ms).await?;
                } else {
                    let result = self.client.call("get_sync_status", json!([])).await?;
                    print_pretty_json(&result)?;
                }
            }
            Commands::Stop => {
                println!("Sending stop signal to usdb-indexer...");
                let result = self.client.call("stop", json!([])).await?;
                print_pretty_json(&result)?;
            }
            Commands::PassSnapshot {
                inscription_id,
                at_height,
            } => {
                let result = self
                    .client
                    .call(
                        "get_pass_snapshot",
                        json!([{
                            "inscription_id": inscription_id,
                            "at_height": at_height,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::ActivePasses {
                at_height,
                page,
                page_size,
            } => {
                let result = self
                    .client
                    .call(
                        "get_active_passes_at_height",
                        json!([{
                            "at_height": at_height,
                            "page": page,
                            "page_size": page_size,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassStats { at_height } => {
                let result = self
                    .client
                    .call(
                        "get_pass_stats_at_height",
                        json!([{
                            "at_height": at_height,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::OwnerActivePass { owner, at_height } => {
                let result = self
                    .client
                    .call(
                        "get_owner_active_pass_at_height",
                        json!([{
                            "owner": owner,
                            "at_height": at_height,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassHistory {
                inscription_id,
                from_height,
                to_height,
                order,
                page,
                page_size,
            } => {
                let result = self
                    .client
                    .call(
                        "get_pass_history",
                        json!([{
                            "inscription_id": inscription_id,
                            "from_height": from_height,
                            "to_height": to_height,
                            "order": order,
                            "page": page,
                            "page_size": page_size,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassEnergy {
                inscription_id,
                block_height,
                mode,
            } => {
                let result = self
                    .client
                    .call(
                        "get_pass_energy",
                        json!([{
                            "inscription_id": inscription_id,
                            "block_height": block_height,
                            "mode": mode,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassEnergyRange {
                inscription_id,
                from_height,
                to_height,
                order,
                page,
                page_size,
            } => {
                let result = self
                    .client
                    .call(
                        "get_pass_energy_range",
                        json!([{
                            "inscription_id": inscription_id,
                            "from_height": from_height,
                            "to_height": to_height,
                            "order": order,
                            "page": page,
                            "page_size": page_size,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::PassEnergyLeaderboard {
                at_height,
                scope,
                page,
                page_size,
            } => {
                let result = self
                    .client
                    .call(
                        "get_pass_energy_leaderboard",
                        json!([{
                            "at_height": at_height,
                            "scope": scope,
                            "page": page,
                            "page_size": page_size,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::ActiveBalanceSnapshot { block_height } => {
                let result = self
                    .client
                    .call(
                        "get_active_balance_snapshot",
                        json!([{
                            "block_height": block_height,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::LatestActiveBalanceSnapshot => {
                let result = self
                    .client
                    .call("get_latest_active_balance_snapshot", json!([]))
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::InvalidPasses {
                error_code,
                from_height,
                to_height,
                page,
                page_size,
            } => {
                let result = self
                    .client
                    .call(
                        "get_invalid_passes",
                        json!([{
                            "error_code": error_code,
                            "from_height": from_height,
                            "to_height": to_height,
                            "page": page,
                            "page_size": page_size,
                        }]),
                    )
                    .await?;
                print_pretty_json(&result)?;
            }
            Commands::Raw { method, params } => {
                let parsed_params: Value = serde_json::from_str(&params).map_err(|e| {
                    format!("Invalid JSON in --params: error={}, params={}", e, params)
                })?;

                let result = self.client.call(&method, parsed_params).await?;
                print_pretty_json(&result)?;
            }
        }

        Ok(())
    }

    async fn watch_sync_status(&self, interval_ms: u64) -> Result<(), String> {
        let interval = Duration::from_millis(interval_ms.max(500));
        let progress = ProgressBar::new(1);
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos:>7}/{len:<7} {msg}",
        )
        .map_err(|e| format!("Failed to build progress style: {}", e))?
        .progress_chars("=>-");
        progress.set_style(style);
        progress.enable_steady_tick(Duration::from_millis(120));

        loop {
            let status = self.client.call("get_sync_status", json!([])).await?;
            let status: IndexerSyncStatus = serde_json::from_value(status)
                .map_err(|e| format!("Failed to decode get_sync_status response: {}", e))?;

            let total = u64::from(status.total.max(1));
            if progress.length() != Some(total) {
                progress.set_length(total);
            }
            let current = u64::from(status.current.min(status.total));
            progress.set_position(current.min(total));

            let synced_height = status
                .synced_block_height
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string());
            let stable_height = status
                .balance_history_stable_height
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string());
            let message = status.message.unwrap_or_else(|| "syncing".to_string());

            progress.set_message(format!(
                "synced={} stable={} genesis={} {}",
                synced_height,
                stable_height,
                status.genesis_block_height,
                message
            ));

            if status
                .synced_block_height
                .zip(status.balance_history_stable_height)
                .is_some_and(|(synced_height, stable_height)| synced_height >= stable_height)
                && status.current >= status.total
            {
                progress.finish_with_message(format!(
                    "synced={} stable={} completed",
                    synced_height, stable_height
                ));
                break;
            }

            sleep(interval).await;
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IndexerSyncStatus {
    genesis_block_height: u32,
    synced_block_height: Option<u32>,
    #[serde(alias = "latest_depend_synced_block_height")]
    balance_history_stable_height: Option<u32>,
    current: u32,
    total: u32,
    message: Option<String>,
}

struct RpcClient {
    url: String,
    http: Client,
}

impl RpcClient {
    fn new(url: &str) -> Result<Self, String> {
        let http = Client::builder()
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        Ok(Self {
            url: url.to_string(),
            http,
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let req_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });

        let response = self
            .http
            .post(&self.url)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "Failed to send usdb-indexer RPC request: method={}, url={}, error={}",
                    method, self.url, e
                )
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            format!(
                "Failed to read usdb-indexer RPC response body: method={}, url={}, error={}",
                method, self.url, e
            )
        })?;

        if !status.is_success() {
            return Err(format!(
                "USDB indexer RPC HTTP error: method={}, url={}, status={}, body={}",
                method, self.url, status, body
            ));
        }

        let parsed: JsonRpcResponse = serde_json::from_str(&body).map_err(|e| {
            format!(
                "Failed to parse usdb-indexer RPC response JSON: method={}, url={}, error={}, body={}",
                method, self.url, e, body
            )
        })?;

        if let Some(err) = parsed.error {
            return Err(format!(
                "USDB indexer RPC returned error: method={}, url={}, error={}",
                method, self.url, err
            ));
        }

        parsed.result.ok_or_else(|| {
            format!(
                "USDB indexer RPC missing result field: method={}, url={}, body={}",
                method, self.url, body
            )
        })
    }
}

fn print_pretty_json(v: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(v)
        .map_err(|e| format!("Failed to pretty print JSON result: {}", e))?;
    println!("{}", text);
    Ok(())
}
