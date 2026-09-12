//! Bounded offline RPC acceptance over read-only databases; no service readiness is inferred.

use super::*;
use crate::service::{
    BalanceHistoryRpc, BalanceHistoryRpcServer, GetAddressBalanceSummaryParams, GetBalanceParams,
    GetStateRefAtHeightParams, ResolveScriptHashesParams, ScriptHashResolutionStatus,
    ScriptRegistryCoverageMode,
};
use crate::status::{SyncPhase, SyncStatusManager};
use bitcoincore_rpc::bitcoin::hashes::Hash;
use usdb_util::{BtcScriptHash, ConsensusRpcErrorCode, ToBtcScriptHash};

fn require(condition: bool, description: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(format!("P5 semantic audit failed: {description}"))
    }
}

/// Audit exact-height commits and deterministic script samples without writing to RocksDB.
/// The caller must hold the workspace lock. Only the JSON report is written under `root`.
/// This calls production RPC handlers in process with an offline status harness; it does not
/// test Bitcoin connectivity, live stable-lag advancement, HTTP, or downstream Ord indexing.
pub fn audit_snapshot(root: &Path, samples_per_prefix: u32) -> Result<serde_json::Value, String> {
    require(
        (1..=64).contains(&samples_per_prefix),
        "samples-per-prefix must be in 1..=64",
    )?;
    let started = Instant::now();
    let options = read_import_options(root)?;
    let reference = reference(&options)?;
    let meta = reference.read_meta()?;
    let base = options.identity.base_height;
    let target = meta.block_height;
    require(
        base > 0 && base < target && target < u32::MAX,
        "unsupported audit height interval",
    )?;
    let cfg = Arc::new(config(&root.join("state"), options.identity.network));
    let db = Arc::new(BalanceHistoryDB::open_read_only(cfg.clone())?);
    let import = db
        .get_assumeutxo_import_state()?
        .ok_or("Missing AssumeUTXO import marker")?;
    require(
        import.complete
            && import.identity == options.identity
            && import.reference_sha256 == options.reference_sha256,
        "import provenance mismatch",
    )?;
    require(
        db.get_btc_block_height()? == target,
        "candidate is not at reference height",
    )?;
    check_commits(&db, &reference, base, target)?;
    let status = Arc::new(SyncStatusManager::new());
    // Enable the handlers only inside this offline harness; never persisted or exposed on a port.
    status.set_rpc_alive(true);
    status.update_phase(SyncPhase::Synced, None);
    status.update_status(target as u64, target as u64, None);
    let (shutdown, _) = tokio::sync::watch::channel(());
    let rpc = BalanceHistoryRpcServer::new(
        cfg,
        "127.0.0.1:0".parse().unwrap(),
        status,
        db.clone(),
        shutdown,
    );
    let snapshot = rpc.get_snapshot_info().map_err(|e| e.to_string())?;
    require(
        snapshot.balance_query_floor == base && snapshot.history_query_floor == base + 1,
        "query retention floors mismatch",
    )?;
    let state_ref = rpc
        .get_state_ref_at_height(GetStateRefAtHeightParams {
            block_height: target,
            context: None,
        })
        .map_err(|e| e.to_string())?;
    require(
        state_ref.snapshot_id == meta.core_snapshot_id,
        "snapshot ID differs from reference metadata",
    )?;
    require(
        state_ref.latest_block_commit == format::hex(&commit_at(&reference, target)?.block_commit),
        "state-ref logical commit differs from reference",
    )?;

    let mut samples = std::collections::BTreeMap::new();
    for prefix in 0..=255u8 {
        let mut bytes = [0; 32];
        bytes[0] = prefix;
        for row in reference.get_balance_history_entries(
            samples_per_prefix,
            Some(&BtcScriptHash::from_byte_array(bytes)),
        )? {
            samples.insert(row.script_hash, row);
        }
    }
    require(!samples.is_empty(), "reference has no balance samples")?;
    let mut baseline_only = 0u64;
    let mut changed_after_base = 0u64;
    let mut evidence = Vec::new();
    let mut progress = Instant::now();
    for expected in samples.values() {
        let hash = expected.script_hash;
        let current = rpc
            .get_address_balance(GetBalanceParams {
                script_hash: hash,
                block_height: Some(target),
                block_range: None,
            })
            .map_err(|e| e.to_string())?;
        require(
            current.len() == 1 && current[0].balance == expected.balance,
            "sample balance mismatch",
        )?;
        if expected.block_height <= base {
            baseline_only += 1;
            require(
                current[0].block_height == base && current[0].delta == 0,
                "baseline metadata is not a zero-delta anchor",
            )?;
        } else {
            changed_after_base += 1;
            require(
                current[0].block_height == expected.block_height
                    && current[0].delta == expected.delta,
                "post-baseline balance metadata mismatch",
            )?;
        }
        // Aggregate endpoints must agree with point and range queries over the retained interval.
        let initial = rpc
            .get_address_balance(GetBalanceParams {
                script_hash: hash,
                block_height: Some(base),
                block_range: None,
            })
            .map_err(|e| e.to_string())?;
        let history = rpc
            .get_address_balance(GetBalanceParams {
                script_hash: hash,
                block_height: None,
                block_range: Some(base + 1..target + 1),
            })
            .map_err(|e| e.to_string())?;
        let initial_balance = initial.first().map_or(0, |row| row.balance);
        require(
            initial_balance as i128 + history.iter().map(|row| row.delta as i128).sum::<i128>()
                == expected.balance as i128,
            "range deltas do not reconcile baseline and target balance",
        )?;
        let summary = rpc
            .get_address_balance_summary(GetAddressBalanceSummaryParams {
                script_hash: hash,
                block_range: base + 1..target + 1,
            })
            .map_err(|e| e.to_string())?;
        require(
            summary.start_balance == initial_balance
                && summary.end_balance == expected.balance
                && summary.change_count == history.len() as u64
                && summary.total_inflow as i128 - summary.total_outflow as i128
                    == summary.net_delta as i128
                && summary.net_delta as i128 == expected.balance as i128 - initial_balance as i128,
            "summary differs from point/range queries",
        )?;
        evidence.push(serde_json::json!({"script_hash":hash,
            "reference":{"block_height":expected.block_height,"balance":expected.balance,"delta":expected.delta},"actual":current,
            "baseline_balance":initial_balance,"history_rows":history.len(),"summary":summary}));
        if progress.elapsed().as_secs() >= 10 {
            eprintln!(
                "P5 audit progress: balance_samples={}, total={}, elapsed_seconds={:.1}",
                evidence.len(),
                samples.len(),
                started.elapsed().as_secs_f64()
            );
            progress = Instant::now();
        }
    }
    let hashes: Vec<_> = samples.keys().copied().collect();
    let mut registry_checked = 0usize;
    let mut nonstandard = 0usize;
    for chunk in hashes.chunks(128) {
        let response = rpc
            .resolve_script_hashes(ResolveScriptHashesParams {
                script_hashes: chunk.to_vec(),
                include_script_pubkey: Some(true),
            })
            .map_err(|e| e.to_string())?;
        response.registry.validate()?;
        require(
            response.registry.coverage_mode == ScriptRegistryCoverageMode::PostSnapshotOnly
                && !response
                    .registry
                    .capabilities
                    .script_registry_complete_coverage,
            "registry claims complete history",
        )?;
        require(
            response.items.len() == chunk.len(),
            "registry response length mismatch",
        )?;
        for (item, hash) in response.items.iter().zip(chunk) {
            require(
                item.status == ScriptHashResolutionStatus::FoundOverlay,
                "sample live script is missing or invalid",
            )?;
            let stored = db
                .get_script_registry_entry(hash)?
                .ok_or("Sample live script missing in registry")?;
            require(
                stored.to_btc_script_hash() == *hash,
                "stored script hash mismatch",
            )?;
            require(
                item.script_pubkey.as_deref() == Some(format::hex(stored.as_bytes()).as_str()),
                "RPC script differs from stored script",
            )?;
            registry_checked += 1;
            nonstandard += usize::from(!item.standard);
        }
    }
    let hash = hashes[0];
    let unknown = (0u64..)
        .find_map(|nonce| {
            let hash = BtcScriptHash::from_byte_array(
                Sha256::digest(format!("assumeutxo-p5-unknown:{nonce}")).into(),
            );
            match db.get_script_registry_entry(&hash) {
                Ok(None) => Some(Ok(hash)),
                Ok(Some(_)) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .ok_or("Cannot select absent script hash")??;
    let missing = rpc
        .resolve_script_hashes(ResolveScriptHashesParams {
            script_hashes: vec![unknown],
            include_script_pubkey: Some(true),
        })
        .map_err(|e| e.to_string())?;
    require(
        missing.items.len() == 1
            && missing.items[0].status == ScriptHashResolutionStatus::Unresolved,
        "registry miss was reported as definitive absence",
    )?;
    let pre_base_point = rpc.get_address_balance(GetBalanceParams {
        script_hash: hash,
        block_height: Some(base - 1),
        block_range: None,
    });
    let base_delta = rpc.get_address_balance_delta(GetBalanceParams {
        script_hash: hash,
        block_height: Some(base),
        block_range: None,
    });
    let pre_base_ref = rpc.get_state_ref_at_height(GetStateRefAtHeightParams {
        block_height: base - 1,
        context: None,
    });
    require(
        pre_base_point
            .is_err_and(|e| e.message == ConsensusRpcErrorCode::StateNotRetained.as_str())
            && base_delta
                .is_err_and(|e| e.message == ConsensusRpcErrorCode::StateNotRetained.as_str())
            && pre_base_ref
                .is_err_and(|e| e.message == ConsensusRpcErrorCode::StateNotRetained.as_str()),
        "pre-baseline request did not fail closed",
    )?;
    let future = rpc.get_address_balance(GetBalanceParams {
        script_hash: hash,
        block_height: Some(target + 1),
        block_range: None,
    });
    require(
        future.is_err_and(|e| e.message == ConsensusRpcErrorCode::HeightNotSynced.as_str()),
        "future request did not fail closed",
    )?;
    require(
        db.get_btc_block_height()? == target,
        "audit changed candidate height",
    )?;
    let report = serde_json::json!({
        "schema":"assumeutxo-p5-offline-audit:v1", "passed":true,
        "scope":"read-only database and in-process RPC handlers; deterministic balance/registry samples",
        "live_service_readiness_checked":false, "full_state_comparison_rerun":false,
        "reference_core":options.reference_core, "reference_sha256":options.reference_sha256,
        "source_identity":options.identity, "target_height":target, "state_ref":state_ref,
        "snapshot":snapshot, "commits_checked":target-base+1,
        "sampling":"first N balance keys strictly after each SHA256 first-byte boundary, deduplicated",
        "samples_per_prefix":samples_per_prefix, "balance_samples":samples.len(),
        "baseline_only_samples":baseline_only, "changed_after_base_samples":changed_after_base,
        "registry_samples":registry_checked, "nonstandard_registry_samples":nonstandard,
        "absent_script_hash":unknown, "absent_script_status":"unresolved",
        "samples":evidence, "elapsed_seconds":started.elapsed().as_secs_f64(),
    });
    write_report(&root.join("p5-semantics-result.json"), &report)?;
    Ok(report)
}
