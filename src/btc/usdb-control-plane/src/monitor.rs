//! Bounded, read-only consumption of the host observer's sanitized snapshot.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use tokio::io::AsyncReadExt;

const MAX_BYTES: u64 = 256 * 1024;
const STALE_AFTER_MS: u64 = 120_000;

/// Freshness is independent of node readiness; old observations never imply health.
#[derive(Debug, Clone, Serialize)]
pub struct MonitorSnapshot {
    pub status: &'static str,
    pub age_ms: Option<u64>,
    pub report: Option<Value>,
}

/// Ord readiness expires independently of the host observer's own heartbeat.
pub fn minting_backend_ready(snapshot: &MonitorSnapshot, now_ms: u64) -> bool {
    let Some(report) = snapshot.report.as_ref() else {
        return false;
    };
    let minting = &report["minting"];
    snapshot.status == "available"
        && minting["enabled"] == true
        && minting["state"] == "READY"
        && minting["canonical"] == true
        && minting["history_validated"] == true
        && minting["txindex_synced"] == true
        && minting["backend_ready"] == true
        && minting["observed_at_ms"]
            .as_u64()
            .is_some_and(|observed| observed <= now_ms && now_ms - observed <= 60_000)
}

fn classify(value: Value, now_ms: u64) -> MonitorSnapshot {
    let observed = value["observed_at_ms"].as_u64();
    if value["schema_version"] != "usdb-console-monitor:v1"
        || !value["observation_available"].is_boolean()
        || !value["overall_state"].is_string()
        || !value["components"].as_array().is_some_and(|items| {
            items.len() <= 32
                && items
                    .iter()
                    .all(|item| item["id"].is_string() && item["state"].is_string())
        })
        || observed.is_none_or(|v| v > now_ms.saturating_add(5000))
    {
        return MonitorSnapshot {
            status: "invalid",
            age_ms: None,
            report: None,
        };
    }
    let age = now_ms.saturating_sub(observed.unwrap());
    let status = if age > STALE_AFTER_MS {
        "stale"
    } else if value["observation_available"] != true {
        "unavailable"
    } else {
        "available"
    };
    MonitorSnapshot {
        status,
        age_ms: Some(age),
        report: Some(value),
    }
}

/// A missing observer is visible and does not prevent the private console starting.
pub async fn read_snapshot(root: &Path, now_ms: u64) -> MonitorSnapshot {
    let fallback = |status| MonitorSnapshot {
        status,
        age_ms: None,
        report: None,
    };
    let path = root.join("node-progress.json");
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return fallback("missing"),
        Err(_) => return fallback("unavailable"),
    };
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return fallback("invalid");
    }
    let Ok(file) = tokio::fs::File::open(path).await else {
        return fallback("unavailable");
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() as u64 > MAX_BYTES
    {
        return fallback("invalid");
    }
    match serde_json::from_slice(&bytes) {
        Ok(value) => classify(value, now_ms),
        Err(_) => fallback("invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn minting_readiness_requires_both_freshness_and_all_dependencies() {
        let report = json!({"schema_version":"usdb-console-monitor:v1", "observed_at_ms":1000,
            "observation_available":true, "overall_state":"READY", "components":[],
            "minting":{"enabled":true, "state":"READY", "canonical":true,
                "history_validated":true, "txindex_synced":true, "backend_ready":true,
                "observed_at_ms":1000}});
        assert!(minting_backend_ready(&classify(report.clone(), 1001), 1001));
        assert!(!minting_backend_ready(
            &classify(report.clone(), 62000),
            62000
        ));
        for field in [
            "enabled",
            "canonical",
            "history_validated",
            "txindex_synced",
            "backend_ready",
        ] {
            let mut changed = report.clone();
            changed["minting"][field] = json!(false);
            assert!(!minting_backend_ready(&classify(changed, 1001), 1001));
        }
    }

    #[test]
    fn stale_ready_report_is_not_a_fresh_observation() {
        let report = json!({"schema_version": "usdb-console-monitor:v1", "observed_at_ms": 1000,
            "observation_available": true, "overall_state": "READY", "components": []});
        assert_eq!(classify(report.clone(), 1001).status, "available");
        assert_eq!(classify(report.clone(), 121001).status, "stale");
        assert_eq!(classify(report, 0).status, "available");
        assert_eq!(
            classify(json!({"schema_version":"unknown"}), 1).status,
            "invalid"
        );
        assert_eq!(
            classify(
                json!({"schema_version":"usdb-console-monitor:v1", "observed_at_ms": 1,
            "observation_available": true, "overall_state":"READY", "components":"bad"}),
                1
            )
            .status,
            "invalid"
        );
    }
}
