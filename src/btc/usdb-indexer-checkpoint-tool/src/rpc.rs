use crate::{IndexerCheckpointManifest, IndexerCheckpointStateIdentity};
use balance_history::HistoricalSnapshotStateRef;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Lightweight JSON-RPC client used by checkpoint export and post-restart verification.
pub struct CheckpointRpcClient {
    url: String,
    client: Client,
}

impl CheckpointRpcClient {
    /// Creates a client for one local service endpoint.
    pub fn new(url: impl Into<String>) -> Result<Self, String> {
        let client = Client::builder()
            .build()
            .map_err(|error| format!("Failed to build checkpoint RPC client: {error}"))?;
        Ok(Self {
            url: url.into(),
            client,
        })
    }

    /// Reads and validates the current usdb-indexer readiness object.
    pub async fn indexer_readiness(&self) -> Result<Value, String> {
        self.call("get_readiness", json!([])).await
    }

    /// Recomputes the exact usdb-indexer historical state-ref at one height.
    pub async fn indexer_state_ref(&self, height: u32) -> Result<Value, String> {
        self.call(
            "get_state_ref_at_height",
            json!([{"block_height": height, "context": null}]),
        )
        .await
    }

    /// Requests a graceful usdb-indexer shutdown.
    pub async fn stop_indexer(&self) -> Result<(), String> {
        self.call::<Value>("stop", json!([])).await.map(|_| ())
    }

    /// Reads and validates the current balance-history readiness object.
    pub async fn balance_history_readiness(&self) -> Result<Value, String> {
        self.call("get_readiness", json!([])).await
    }

    /// Recomputes the exact balance-history historical state-ref at one height.
    pub async fn balance_history_state_ref(
        &self,
        height: u32,
    ) -> Result<HistoricalSnapshotStateRef, String> {
        self.call(
            "get_state_ref_at_height",
            json!([{"block_height": height, "context": null}]),
        )
        .await
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1,
        });
        let response = self
            .client
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .map_err(|error| {
                format!(
                    "Checkpoint RPC request failed: url={}, method={}, error={error}",
                    self.url, method
                )
            })?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|error| {
            format!(
                "Checkpoint RPC response decode failed: url={}, method={}, status={}, error={error}",
                self.url, method, status
            )
        })?;
        if let Some(error) = body.get("error") {
            return Err(format!(
                "Checkpoint RPC returned an error: url={}, method={}, error={}",
                self.url, method, error
            ));
        }
        let result = body.get("result").ok_or_else(|| {
            format!(
                "Checkpoint RPC response has no result: url={}, method={}, body={}",
                self.url, method, body
            )
        })?;
        serde_json::from_value(result.clone()).map_err(|error| {
            format!(
                "Checkpoint RPC result decode failed: url={}, method={}, error={error}",
                self.url, method
            )
        })
    }
}

/// Extracts the normalized identity committed by an indexer historical state-ref.
pub fn extract_indexer_state_identity(
    state_ref: &Value,
) -> Result<IndexerCheckpointStateIdentity, String> {
    fn string_at(value: &Value, pointer: &str) -> Result<String, String> {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("Indexer state-ref is missing string field {pointer}"))
    }

    let block_height = state_ref
        .get("block_height")
        .and_then(Value::as_u64)
        .and_then(|height| u32::try_from(height).ok())
        .ok_or_else(|| "Indexer state-ref has an invalid block_height".to_string())?;
    Ok(IndexerCheckpointStateIdentity {
        block_height,
        stable_block_hash: string_at(state_ref, "/snapshot_info/stable_block_hash")?,
        latest_block_commit: string_at(state_ref, "/snapshot_info/latest_block_commit")?,
        snapshot_id: string_at(state_ref, "/snapshot_info/snapshot_id")?,
        activation_registry_id: string_at(
            state_ref,
            "/local_state_commit_info/activation_registry_id",
        )?,
        active_version_set_id: string_at(
            state_ref,
            "/local_state_commit_info/active_version_set_id",
        )?,
        local_state_commit: string_at(state_ref, "/local_state_commit_info/local_state_commit")?,
        system_state_id: string_at(state_ref, "/system_state_info/system_state_id")?,
    })
}

/// Validates that the two state-ref objects describe exactly the same upstream BTC snapshot.
pub fn validate_paired_state_refs(
    indexer: &IndexerCheckpointStateIdentity,
    balance_history: &HistoricalSnapshotStateRef,
) -> Result<(), String> {
    let expected = (
        balance_history.block_height,
        balance_history.stable_block_hash.as_str(),
        balance_history.latest_block_commit.as_str(),
        balance_history.snapshot_id.as_str(),
    );
    let actual = (
        indexer.block_height,
        indexer.stable_block_hash.as_str(),
        indexer.latest_block_commit.as_str(),
        indexer.snapshot_id.as_str(),
    );
    if actual != expected {
        return Err(format!(
            "Paired state-ref mismatch: indexer={actual:?}, balance_history={expected:?}"
        ));
    }
    Ok(())
}

pub(crate) fn require_consensus_ready(
    readiness: &Value,
    expected_height: u32,
    service: &str,
) -> Result<(), String> {
    if readiness.get("consensus_ready").and_then(Value::as_bool) != Some(true) {
        return Err(format!("{service} is not consensus-ready: {}", readiness));
    }
    let height = readiness
        .get(if service == "usdb-indexer" {
            "synced_block_height"
        } else {
            "stable_height"
        })
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    if height != Some(expected_height) {
        return Err(format!(
            "{service} readiness height mismatch: expected {expected_height}, got {height:?}"
        ));
    }
    Ok(())
}

fn require_consensus_ready_at_or_after(
    readiness: &Value,
    minimum_height: u32,
    service: &str,
) -> Result<(), String> {
    if readiness.get("consensus_ready").and_then(Value::as_bool) != Some(true) {
        return Err(format!("{service} is not consensus-ready: {}", readiness));
    }
    let field = if service == "usdb-indexer" {
        "synced_block_height"
    } else {
        "stable_height"
    };
    let height = readiness
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("{service} readiness has no valid {field}"))?;
    if height < minimum_height {
        return Err(format!(
            "{service} has not recovered checkpoint height {minimum_height}: current={height}"
        ));
    }
    Ok(())
}

/// Recomputes both service state refs after restart and compares them with a signed manifest.
pub async fn verify_restarted_state_refs(
    manifest: &IndexerCheckpointManifest,
    indexer_rpc_url: &str,
    balance_history_rpc_url: &str,
) -> Result<IndexerCheckpointStateIdentity, String> {
    let indexer = CheckpointRpcClient::new(indexer_rpc_url)?;
    let balance_history = CheckpointRpcClient::new(balance_history_rpc_url)?;
    let height = manifest.checkpoint_height;

    let indexer_readiness = indexer.indexer_readiness().await?;
    require_consensus_ready_at_or_after(&indexer_readiness, height, "usdb-indexer")?;
    let balance_history_readiness = balance_history.balance_history_readiness().await?;
    require_consensus_ready_at_or_after(&balance_history_readiness, height, "balance-history")?;

    let rebuilt_indexer_ref = indexer.indexer_state_ref(height).await?;
    let rebuilt_identity = extract_indexer_state_identity(&rebuilt_indexer_ref)?;
    if rebuilt_identity != manifest.state_identity {
        return Err(format!(
            "Recomputed indexer state identity does not match checkpoint: expected={:?}, actual={:?}",
            manifest.state_identity, rebuilt_identity
        ));
    }
    if rebuilt_indexer_ref != manifest.indexer_state_ref {
        return Err("Recomputed full indexer state-ref does not match checkpoint manifest".into());
    }

    let rebuilt_balance_history_ref = balance_history.balance_history_state_ref(height).await?;
    if rebuilt_balance_history_ref != manifest.balance_history.state_ref {
        return Err(format!(
            "Recomputed balance-history state-ref does not match checkpoint: expected={:?}, actual={:?}",
            manifest.balance_history.state_ref, rebuilt_balance_history_ref
        ));
    }
    validate_paired_state_refs(&rebuilt_identity, &rebuilt_balance_history_ref)?;
    Ok(rebuilt_identity)
}

/// Waits until both restarted services are consensus-ready at or beyond the checkpoint height.
///
/// Readiness and connection failures are transient during startup. Once both services are ready,
/// any historical state-ref mismatch is final and is returned without retrying.
pub async fn wait_for_restarted_state_refs(
    manifest: &IndexerCheckpointManifest,
    indexer_rpc_url: &str,
    balance_history_rpc_url: &str,
    timeout: Duration,
) -> Result<IndexerCheckpointStateIdentity, String> {
    let indexer = CheckpointRpcClient::new(indexer_rpc_url)?;
    let balance_history = CheckpointRpcClient::new(balance_history_rpc_url)?;
    let started = Instant::now();
    let mut last_readiness_error = String::new();

    loop {
        let readiness = async {
            let indexer_readiness = indexer.indexer_readiness().await?;
            require_consensus_ready_at_or_after(
                &indexer_readiness,
                manifest.checkpoint_height,
                "usdb-indexer",
            )?;
            let balance_history_readiness = balance_history.balance_history_readiness().await?;
            require_consensus_ready_at_or_after(
                &balance_history_readiness,
                manifest.checkpoint_height,
                "balance-history",
            )
        }
        .await;
        match readiness {
            Ok(()) => break,
            Err(error) if started.elapsed() < timeout => {
                last_readiness_error = error;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => {
                return Err(format!(
                    "Timed out waiting for paired checkpoint recovery readiness after {:.1}s: {error}",
                    timeout.as_secs_f64()
                ));
            }
        }
    }

    verify_restarted_state_refs(manifest, indexer_rpc_url, balance_history_rpc_url)
        .await
        .map_err(|error| {
            if last_readiness_error.is_empty() {
                error
            } else {
                format!("{error}; last startup readiness error: {last_readiness_error}")
            }
        })
}
