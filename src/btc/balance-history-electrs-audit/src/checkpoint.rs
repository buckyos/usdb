use crate::model::{AUDIT_CHECKPOINT_VERSION, AuditCheckpoint, RunIdentity, SampleResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn build_run_id(identity: &RunIdentity) -> Result<String, String> {
    let canonical = serde_json::to_vec(identity)
        .map_err(|error| format!("Failed to serialize audit run identity: {error}"))?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

pub fn load_or_create_checkpoint(path: &Path, run_id: &str) -> Result<AuditCheckpoint, String> {
    if !path.exists() {
        return Ok(AuditCheckpoint {
            checkpoint_version: AUDIT_CHECKPOINT_VERSION.to_string(),
            run_id: run_id.to_string(),
            started_at_unix: now_unix(),
            results: Vec::new(),
        });
    }
    let data = std::fs::read_to_string(path)
        .map_err(|error| format!("Failed to read checkpoint {}: {error}", path.display()))?;
    let checkpoint: AuditCheckpoint = serde_json::from_str(&data)
        .map_err(|error| format!("Failed to parse checkpoint {}: {error}", path.display()))?;
    if checkpoint.checkpoint_version != AUDIT_CHECKPOINT_VERSION {
        return Err(format!(
            "Unsupported checkpoint version {} in {}",
            checkpoint.checkpoint_version,
            path.display()
        ));
    }
    if checkpoint.run_id != run_id {
        return Err(format!(
            "Checkpoint run_id mismatch in {}: expected {}, got {}",
            path.display(),
            run_id,
            checkpoint.run_id
        ));
    }
    Ok(checkpoint)
}

pub fn save_checkpoint(path: &Path, checkpoint: &AuditCheckpoint) -> Result<(), String> {
    atomic_write_json(path, checkpoint)
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create report directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let data = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Failed to serialize JSON output: {error}"))?;
    let temp = temporary_path(path);
    std::fs::write(&temp, data)
        .map_err(|error| format!("Failed to write temporary file {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        format!(
            "Failed to atomically publish {} from {}: {error}",
            path.display(),
            temp.display()
        )
    })
}

pub fn sort_results(results: &mut [SampleResult]) {
    results.sort_by(|left, right| left.sample_id.cmp(&right.sample_id));
}

fn temporary_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("audit.json");
    path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RunIdentity;
    use tempfile::TempDir;

    fn identity(seed: &str) -> RunIdentity {
        RunIdentity {
            snapshot_file: "/snapshot.db".to_string(),
            declared_snapshot_sha256: "11".repeat(32),
            snapshot_height: 10,
            snapshot_block_hash: "22".repeat(32),
            electrs_url: "tcp://127.0.0.1:50001".to_string(),
            seed: seed.to_string(),
            sample_count: 8,
            zero_sample_percent: 25,
            max_history_entries: 20_000,
            blacklist_id: "33".repeat(32),
        }
    }

    #[test]
    fn run_id_changes_with_sampling_identity() {
        assert_ne!(
            build_run_id(&identity("a")).unwrap(),
            build_run_id(&identity("b")).unwrap()
        );
    }

    #[test]
    fn checkpoint_rejects_different_run() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("checkpoint.json");
        let first_id = build_run_id(&identity("a")).unwrap();
        let checkpoint = load_or_create_checkpoint(&path, &first_id).unwrap();
        save_checkpoint(&path, &checkpoint).unwrap();
        let second_id = build_run_id(&identity("b")).unwrap();
        assert!(load_or_create_checkpoint(&path, &second_id).is_err());
    }
}
