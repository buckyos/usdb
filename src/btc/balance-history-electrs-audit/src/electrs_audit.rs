use crate::model::{ElectrsSummary, SampleResult, SampleStatus};
use crate::snapshot::AuditSample;
use bitcoincore_rpc::bitcoin::{Script, Transaction, Txid};
use electrum_client::{Client, ConfigBuilder, ElectrumApi, GetHistoryRes};
use moka::sync::Cache;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;
use usdb_util::is_core_unspendable;

const BIP30_AMBIGUOUS_SCRIPT_HASHES: [&str; 2] = [
    "76d95f02197b7c685b972104f6d7688a78bdcbb6a757fd5a139a195e59505fab",
    "49df7a6bfea6c409a5f03fd734a1f1a13cb8fafee6a3e08dd94db352498f99a6",
];

#[derive(Clone, Debug)]
pub struct ElectrsAuditSettings {
    pub url: String,
    pub target_height: u32,
    pub target_block_hash: String,
    pub timeout_secs: u8,
    pub max_history_entries: usize,
    pub concurrency: usize,
    pub transaction_cache_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ServerLimitValidation {
    pub configured_limit: Option<usize>,
    pub config_validated: bool,
    pub runtime_operator_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AuditExecutionStats {
    pub history_requests: u64,
    pub transaction_requests: u64,
    pub transaction_cache_hits: u64,
    pub transaction_cache_entries: u64,
    pub transaction_cache_weighted_bytes: u64,
}

#[derive(Default)]
struct AuditMetrics {
    history_requests: AtomicU64,
    transaction_requests: AtomicU64,
    transaction_cache_hits: AtomicU64,
}

#[derive(Debug)]
enum SampleFailure {
    TooPopular(String),
    Bip30Ambiguous(String),
    Error(String),
}

#[derive(Debug, Deserialize)]
struct ElectrsRuntimeConfig {
    #[serde(default)]
    index_lookup_limit: Option<usize>,
}

pub fn validate_server_limit(
    config_path: Option<&Path>,
    max_history_entries: usize,
    runtime_operator_confirmed: bool,
    allow_unverified: bool,
) -> Result<ServerLimitValidation, String> {
    if max_history_entries == 0 {
        return Err("max_history_entries must be greater than zero".to_string());
    }
    let Some(config_path) = config_path else {
        if allow_unverified {
            return Ok(ServerLimitValidation {
                configured_limit: None,
                config_validated: false,
                runtime_operator_confirmed: false,
            });
        }
        return Err(
            "Electrs server limit is unverified; pass --electrs-config or explicitly use --allow-unverified-electrs-limit"
                .to_string(),
        );
    };
    let data = std::fs::read_to_string(config_path).map_err(|error| {
        format!(
            "Failed to read electrs config {}: {error}",
            config_path.display()
        )
    })?;
    let config: ElectrsRuntimeConfig = toml::from_str(&data).map_err(|error| {
        format!(
            "Failed to parse electrs config {}: {error}",
            config_path.display()
        )
    })?;
    let limit = config.index_lookup_limit.unwrap_or_default();
    if limit == 0 {
        return Err(format!(
            "Electrs config {} has no active index_lookup_limit; set a non-zero value before auditing",
            config_path.display()
        ));
    }
    if limit > max_history_entries {
        return Err(format!(
            "Electrs index_lookup_limit {} exceeds audit max_history_entries {}; lower the server limit or raise the audited bound explicitly",
            limit, max_history_entries
        ));
    }
    if !runtime_operator_confirmed && !allow_unverified {
        return Err(
            "Electrs config is safe but the running process is unverified; restart electrs with that config and pass --confirm-electrs-restarted-with-config"
                .to_string(),
        );
    }
    Ok(ServerLimitValidation {
        configured_limit: Some(limit),
        config_validated: true,
        runtime_operator_confirmed,
    })
}

pub fn preflight(
    settings: &ElectrsAuditSettings,
    server_limit: ServerLimitValidation,
) -> Result<ElectrsSummary, String> {
    let client = build_client(&settings.url, settings.timeout_secs)?;
    let features = client
        .server_features()
        .map_err(|error| format!("Failed to query electrs server.features: {error}"))?;
    let tip = client
        .block_headers_subscribe()
        .map_err(|error| format!("Failed to query electrs tip: {error}"))?;
    let tip_height = u32::try_from(tip.height)
        .map_err(|_| format!("Electrs tip height is outside u32 range: {}", tip.height))?;
    if tip_height < settings.target_height {
        return Err(format!(
            "Electrs tip {} is below audit target {}",
            tip_height, settings.target_height
        ));
    }
    let header = client
        .block_header(settings.target_height as usize)
        .map_err(|error| {
            format!(
                "Failed to query electrs header at {}: {error}",
                settings.target_height
            )
        })?;
    let target_header_hash = header.block_hash().to_string();
    if target_header_hash != settings.target_block_hash {
        return Err(format!(
            "Electrs target header mismatch at {}: snapshot={}, electrs={}",
            settings.target_height, settings.target_block_hash, target_header_hash
        ));
    }

    Ok(ElectrsSummary {
        url: settings.url.clone(),
        server_version: features.server_version,
        protocol_min: features.protocol_min,
        protocol_max: features.protocol_max,
        tip_height,
        target_header_hash,
        configured_index_lookup_limit: server_limit.configured_limit,
        server_limit_config_validated: server_limit.config_validated,
        runtime_limit_operator_confirmed: server_limit.runtime_operator_confirmed,
    })
}

pub fn audit_samples<F>(
    samples: Vec<AuditSample>,
    settings: &ElectrsAuditSettings,
    mut on_result: F,
) -> Result<AuditExecutionStats, String>
where
    F: FnMut(SampleResult) -> Result<(), String>,
{
    if settings.concurrency == 0 {
        return Err("concurrency must be greater than zero".to_string());
    }
    if settings.transaction_cache_bytes == 0 {
        return Err("transaction_cache_bytes must be greater than zero".to_string());
    }
    if samples.is_empty() {
        return Ok(AuditExecutionStats::default());
    }

    let worker_count = settings.concurrency.min(samples.len());
    let mut clients = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        clients.push(build_client(&settings.url, settings.timeout_secs)?);
    }

    let samples = Arc::new(samples);
    let next_index = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let cache = Cache::builder()
        .max_capacity(settings.transaction_cache_bytes)
        .weigher(|_txid, tx: &Arc<Transaction>| {
            u32::try_from(tx.total_size().saturating_add(128)).unwrap_or(u32::MAX)
        })
        .build();
    let metrics = Arc::new(AuditMetrics::default());
    let (sender, receiver) = mpsc::channel();
    let mut callback_error = None;

    std::thread::scope(|scope| {
        for client in clients {
            let samples = samples.clone();
            let next_index = next_index.clone();
            let stop = stop.clone();
            let cache = cache.clone();
            let metrics = metrics.clone();
            let sender = sender.clone();
            let settings = settings.clone();
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(sample) = samples.get(index) else {
                        break;
                    };
                    let result = audit_sample(&client, &cache, &metrics, sample, &settings);
                    if sender.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        for result in receiver {
            if callback_error.is_none()
                && let Err(error) = on_result(result)
            {
                stop.store(true, Ordering::Relaxed);
                callback_error = Some(error);
            }
        }
    });

    if let Some(error) = callback_error {
        return Err(error);
    }
    Ok(AuditExecutionStats {
        history_requests: metrics.history_requests.load(Ordering::Relaxed),
        transaction_requests: metrics.transaction_requests.load(Ordering::Relaxed),
        transaction_cache_hits: metrics.transaction_cache_hits.load(Ordering::Relaxed),
        transaction_cache_entries: cache.entry_count(),
        transaction_cache_weighted_bytes: cache.weighted_size(),
    })
}

fn build_client(url: &str, timeout_secs: u8) -> Result<Client, String> {
    if timeout_secs == 0 {
        return Err("timeout_secs must be greater than zero".to_string());
    }
    let config = ConfigBuilder::new()
        .timeout(Some(timeout_secs))
        .retry(0)
        .build();
    Client::from_config(url, config)
        .map_err(|error| format!("Failed to connect to electrs {url}: {error}"))
}

fn audit_sample(
    client: &Client,
    cache: &Cache<Txid, Arc<Transaction>>,
    metrics: &AuditMetrics,
    sample: &AuditSample,
    settings: &ElectrsAuditSettings,
) -> SampleResult {
    let started = Instant::now();
    let mut result = SampleResult {
        sample_id: sample.sample_id.clone(),
        kind: sample.kind,
        script_hash: format!("{:x}", sample.script_hash),
        script_pubkey_hex: hex::encode(sample.script_pubkey.as_bytes()),
        script_type: sample.script_type.clone(),
        expected_balance: sample.expected_balance,
        computed_balance: None,
        last_change_height: sample.last_change_height,
        history_entries: None,
        confirmed_entries_replayed: None,
        status: SampleStatus::Error,
        detail: None,
        elapsed_ms: 0,
    };

    match replay_sample(client, cache, metrics, sample, settings) {
        Ok(replayed) => {
            result.computed_balance = Some(replayed.balance);
            result.history_entries = Some(replayed.history_entries);
            result.confirmed_entries_replayed = Some(replayed.confirmed_entries);
            if replayed.balance == sample.expected_balance {
                result.status = SampleStatus::Matched;
            } else {
                result.status = SampleStatus::Mismatch;
                result.detail = Some(format!(
                    "Snapshot balance {} does not match electrs replay {}",
                    sample.expected_balance, replayed.balance
                ));
            }
        }
        Err(SampleFailure::TooPopular(detail)) => {
            result.status = SampleStatus::SkippedTooPopular;
            result.detail = Some(detail);
        }
        Err(SampleFailure::Bip30Ambiguous(detail)) => {
            result.status = SampleStatus::SkippedBip30Ambiguous;
            result.detail = Some(detail);
        }
        Err(SampleFailure::Error(detail)) => {
            result.status = SampleStatus::Error;
            result.detail = Some(detail);
        }
    }
    result.elapsed_ms = started.elapsed().as_millis();
    result
}

struct ReplayedSample {
    balance: u64,
    history_entries: usize,
    confirmed_entries: usize,
}

fn replay_sample(
    client: &Client,
    cache: &Cache<Txid, Arc<Transaction>>,
    metrics: &AuditMetrics,
    sample: &AuditSample,
    settings: &ElectrsAuditSettings,
) -> Result<ReplayedSample, SampleFailure> {
    let script_hash = format!("{:x}", sample.script_hash);
    if is_known_bip30_script_hash(&script_hash) {
        return Err(SampleFailure::Bip30Ambiguous(format!(
            "Script hash {script_hash} is affected by a historical BIP30 duplicate coinbase; electrs txid-only replay is not an independent oracle"
        )));
    }
    metrics.history_requests.fetch_add(1, Ordering::Relaxed);
    let history = client
        .script_get_history(sample.script_pubkey.as_script())
        .map_err(|error| {
            let detail = format!("Failed to query script history: {error}");
            if is_too_popular_error(&detail) {
                SampleFailure::TooPopular(detail)
            } else {
                SampleFailure::Error(detail)
            }
        })?;
    if history.len() > settings.max_history_entries {
        return Err(SampleFailure::TooPopular(format!(
            "Electrs returned {} history entries, exceeding client bound {}",
            history.len(),
            settings.max_history_entries
        )));
    }
    detect_duplicate_txids(&history, settings.target_height)?;

    let mut replay = BlockReplay::default();
    let mut confirmed_entries = 0usize;
    for entry in history
        .iter()
        .filter(|entry| entry.height > 0 && (entry.height as u32) <= settings.target_height)
    {
        let height = entry.height as u32;
        let tx = get_transaction(client, cache, metrics, &entry.tx_hash)?;
        let delta = transaction_delta(
            client,
            cache,
            metrics,
            &tx,
            sample.script_pubkey.as_script(),
        )?;
        replay.push(height, delta)?;
        confirmed_entries += 1;
    }
    let balance = replay.finish()?;
    Ok(ReplayedSample {
        balance,
        history_entries: history.len(),
        confirmed_entries,
    })
}

fn detect_duplicate_txids(
    history: &[GetHistoryRes],
    target_height: u32,
) -> Result<(), SampleFailure> {
    let mut seen = HashMap::new();
    for entry in history
        .iter()
        .filter(|entry| entry.height > 0 && (entry.height as u32) <= target_height)
    {
        if let Some(previous_height) = seen.insert(entry.tx_hash, entry.height)
            && previous_height != entry.height
        {
            return Err(SampleFailure::Bip30Ambiguous(format!(
                "Transaction {} appears at heights {} and {}; txid-only electrs replay cannot resolve BIP30 displacement",
                entry.tx_hash, previous_height, entry.height
            )));
        }
    }
    Ok(())
}

fn get_transaction(
    client: &Client,
    cache: &Cache<Txid, Arc<Transaction>>,
    metrics: &AuditMetrics,
    txid: &Txid,
) -> Result<Arc<Transaction>, SampleFailure> {
    if let Some(tx) = cache.get(txid) {
        metrics
            .transaction_cache_hits
            .fetch_add(1, Ordering::Relaxed);
        return Ok(tx);
    }
    metrics.transaction_requests.fetch_add(1, Ordering::Relaxed);
    let tx = Arc::new(client.transaction_get(txid).map_err(|error| {
        SampleFailure::Error(format!("Failed to fetch transaction {txid}: {error}"))
    })?);
    cache.insert(*txid, tx.clone());
    Ok(tx)
}

fn transaction_delta(
    client: &Client,
    cache: &Cache<Txid, Arc<Transaction>>,
    metrics: &AuditMetrics,
    tx: &Transaction,
    target_script: &Script,
) -> Result<i128, SampleFailure> {
    let mut delta = 0i128;
    for input in &tx.input {
        if input.previous_output.is_null() {
            continue;
        }
        let previous = get_transaction(client, cache, metrics, &input.previous_output.txid)?;
        let previous_output = previous
            .output
            .get(input.previous_output.vout as usize)
            .ok_or_else(|| {
                SampleFailure::Error(format!(
                    "Transaction {} references missing output {}:{}",
                    tx.compute_txid(),
                    input.previous_output.txid,
                    input.previous_output.vout
                ))
            })?;
        if previous_output.script_pubkey.as_script() == target_script {
            delta = delta
                .checked_sub(i128::from(previous_output.value.to_sat()))
                .ok_or_else(|| SampleFailure::Error("Input delta underflow".to_string()))?;
        }
    }
    for output in &tx.output {
        if !is_core_unspendable(&output.script_pubkey)
            && output.script_pubkey.as_script() == target_script
        {
            delta = delta
                .checked_add(i128::from(output.value.to_sat()))
                .ok_or_else(|| SampleFailure::Error("Output delta overflow".to_string()))?;
        }
    }
    Ok(delta)
}

#[derive(Default)]
struct BlockReplay {
    current_height: Option<u32>,
    current_delta: i128,
    balance: i128,
}

impl BlockReplay {
    fn push(&mut self, height: u32, delta: i128) -> Result<(), SampleFailure> {
        if let Some(current_height) = self.current_height {
            if height < current_height {
                return Err(SampleFailure::Error(format!(
                    "Electrs confirmed history is not ordered: height {height} follows {current_height}"
                )));
            }
            if height > current_height {
                self.flush()?;
                self.current_height = Some(height);
            }
        } else {
            self.current_height = Some(height);
        }
        self.current_delta = self
            .current_delta
            .checked_add(delta)
            .ok_or_else(|| SampleFailure::Error("Block delta overflow".to_string()))?;
        Ok(())
    }

    fn finish(mut self) -> Result<u64, SampleFailure> {
        self.flush()?;
        u64::try_from(self.balance).map_err(|_| {
            SampleFailure::Error(format!(
                "Final balance is outside u64 range: {}",
                self.balance
            ))
        })
    }

    fn flush(&mut self) -> Result<(), SampleFailure> {
        if self.current_height.take().is_none() {
            return Ok(());
        }
        self.balance = self
            .balance
            .checked_add(self.current_delta)
            .ok_or_else(|| SampleFailure::Error("Balance overflow".to_string()))?;
        if self.balance < 0 {
            return Err(SampleFailure::Error(format!(
                "Balance became negative at block terminal state: {}",
                self.balance
            )));
        }
        self.current_delta = 0;
        Ok(())
    }
}

fn is_too_popular_error(error: &str) -> bool {
    error.contains("index entries, query may take too long")
}

fn is_known_bip30_script_hash(script_hash: &str) -> bool {
    BIP30_AMBIGUOUS_SCRIPT_HASHES.contains(&script_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn server_limit_must_be_nonzero_and_within_client_bound() {
        let mut file = NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"index_lookup_limit = 20000\n").unwrap();
        let validated = validate_server_limit(Some(file.path()), 20_000, true, false).unwrap();
        assert_eq!(validated.configured_limit, Some(20_000));
        assert!(validated.config_validated);
        assert!(validated.runtime_operator_confirmed);

        std::fs::write(file.path(), "index_lookup_limit = 0\n").unwrap();
        assert!(validate_server_limit(Some(file.path()), 20_000, true, false).is_err());
        std::fs::write(file.path(), "index_lookup_limit = 20001\n").unwrap();
        assert!(validate_server_limit(Some(file.path()), 20_000, true, false).is_err());
    }

    #[test]
    fn safe_config_still_requires_runtime_restart_confirmation() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "index_lookup_limit = 20000\n").unwrap();
        assert!(validate_server_limit(Some(file.path()), 20_000, false, false).is_err());
        let unverified = validate_server_limit(Some(file.path()), 20_000, false, true).unwrap();
        assert!(unverified.config_validated);
        assert!(!unverified.runtime_operator_confirmed);
    }

    #[test]
    fn block_replay_aggregates_same_height_before_balance_check() {
        let mut replay = BlockReplay::default();
        replay.push(10, 100).unwrap();
        replay.push(10, -40).unwrap();
        replay.push(11, 25).unwrap();
        assert_eq!(replay.finish().unwrap(), 85);
    }

    #[test]
    fn block_replay_rejects_descending_height_and_negative_terminal() {
        let mut descending = BlockReplay::default();
        descending.push(11, 1).unwrap();
        assert!(descending.push(10, 1).is_err());

        let mut negative = BlockReplay::default();
        negative.push(10, -1).unwrap();
        assert!(negative.finish().is_err());
    }

    #[test]
    fn popular_error_is_classified_without_retry() {
        assert!(is_too_popular_error(
            ">20000 index entries, query may take too long"
        ));
    }

    #[test]
    fn known_bip30_scripts_are_classified_proactively() {
        assert!(is_known_bip30_script_hash(
            "76d95f02197b7c685b972104f6d7688a78bdcbb6a757fd5a139a195e59505fab"
        ));
        assert!(!is_known_bip30_script_hash(&"00".repeat(32)));
    }
}
