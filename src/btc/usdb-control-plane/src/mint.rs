//! Read-only MinerPass V2 planning and post-reveal verification. No balance migration is broadcast.

use super::{
    AppState, BtcMintIdentity, PreparedBtcMintContext, build_btc_mint_inscription_payload,
    build_capabilities_summary, build_services_summary, normalize_btc_mint_identity,
    normalize_prev_list, normalize_required_text, parse_balance_history_network,
    resolve_runtime_btc_network_name,
};
use crate::models::{
    ApiError, BtcMintPrepareRequest, BtcMintPrepareResponse, BtcMintPrepareRuntimeSummary,
    BtcMintVerifyRequest, BtcMintVerifyResponse,
};
use axum::{Json, extract::State, http::StatusCode};
use bitcoincore_rpc::bitcoin::{Address, Amount, Network};
use serde_json::{Value, json};
use usdb_util::address_string_to_script_hash;

pub(super) static EXECUTION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub(super) fn unavailable(error: String) -> (StatusCode, Json<ApiError>) {
    warn!("MinerPass tool evidence unavailable: {error}");
    (StatusCode::BAD_GATEWAY, Json(ApiError { error }))
}
fn invalid(error: String) -> (StatusCode, Json<ApiError>) {
    warn!("MinerPass tool request rejected: {error}");
    (StatusCode::BAD_REQUEST, Json(ApiError { error }))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("Missing string evidence field: {key}"))
}
fn integer(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("Missing unsigned evidence field: {key}"))
}
fn boolean(value: &Value, key: &str) -> Result<bool, String> {
    value[key]
        .as_bool()
        .ok_or_else(|| format!("Missing boolean evidence field: {key}"))
}
async fn indexer(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    state
        .rpc_client
        .usdb_indexer_proxy(&state.config.rpc.usdb_indexer_url, method, params)
        .await
        .map_err(|err| format!("{method}: {err}"))
}
async fn balance(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    state
        .rpc_client
        .balance_history_proxy(&state.config.rpc.balance_history_url, method, params)
        .await
        .map_err(|err| format!("{method}: {err}"))
}

struct Draft {
    network: Network,
    source: String,
    recipient: String,
    source_hash: String,
    recipient_hash: String,
    source_supported: bool,
    recipient_taproot: bool,
    identity: BtcMintIdentity,
    prev: Vec<String>,
}
fn normalize(request: &BtcMintPrepareRequest, network: Network) -> Result<Draft, String> {
    let address = |label, raw: &str| -> Result<Address, String> {
        normalize_required_text(label, raw)?
            .parse::<Address<_>>()
            .map_err(|err| format!("Invalid {label}: {err}"))?
            .require_network(network)
            .map_err(|err| format!("Wrong {label} network: {err}"))
    };
    let source = address("source_address", &request.source_address)?;
    let recipient = address("recipient_address", &request.recipient_address)?;
    let source_script = source.script_pubkey();
    let prev = normalize_prev_list(&request.prev)?;
    if prev.len() > 64 {
        return Err("The control plane supports at most 64 prev entries per draft".into());
    }
    Ok(Draft {
        network,
        source_hash: address_string_to_script_hash(&source.to_string(), &network)?.to_string(),
        recipient_hash: address_string_to_script_hash(&recipient.to_string(), &network)?
            .to_string(),
        source_supported: source_script.is_p2pkh()
            || source_script.is_p2wpkh()
            || source_script.is_p2tr(),
        recipient_taproot: recipient.script_pubkey().is_p2tr(),
        source: source.to_string(),
        recipient: recipient.to_string(),
        prev,
        identity: normalize_btc_mint_identity(
            request.usdb_main.as_deref(),
            request.leader_pass_id.as_deref(),
            request.leader_btc_addr.as_deref(),
            &network,
        )?,
    })
}

struct Observation {
    height: u32,
    state: Value,
    source_balance: u64,
    recipient_balance: u64,
    recipient_history: bool,
    passes: Vec<Value>,
}
async fn owner_page(
    state: &AppState,
    owner: &str,
    height: u32,
    page: usize,
    size: usize,
) -> Result<Value, String> {
    let result = indexer(
        state,
        "get_owner_passes_at_height",
        json!([{
        "owner":owner,"at_height":height,"page":page,"page_size":size,"order":"desc"}]),
    )
    .await?;
    if integer(&result, "resolved_height")? != u64::from(height)
        || string(&result, "owner")? != owner
    {
        return Err(format!(
            "Owner observation mismatch: owner={owner}, height={height}"
        ));
    }
    Ok(result)
}
async fn owner_passes(state: &AppState, owner: &str, height: u32) -> Result<Vec<Value>, String> {
    let mut items = Vec::new();
    // Bounded tooling query: fail visibly instead of claiming an incomplete list is complete.
    for page in 0..20 {
        let value = owner_page(state, owner, height, page, 100).await?;
        let rows = value["items"]
            .as_array()
            .ok_or("Missing owner pass items")?;
        if rows.is_empty() && (items.len() as u64) < integer(&value, "total")? {
            return Err("Incomplete owner pass page".into());
        }
        items.extend(rows.iter().cloned());
        if items.len() as u64 == integer(&value, "total")? {
            return Ok(items);
        }
    }
    Err("Owner has too many passes for this control-plane operation".into())
}
async fn state_at(state: &AppState, height: u32) -> Result<Value, String> {
    let value = indexer(
        state,
        "get_state_ref_at_height",
        json!([{"block_height":height}]),
    )
    .await?;
    if integer(&value, "block_height")? != u64::from(height) {
        return Err(format!(
            "State height mismatch: requested={height}, actual={}",
            value["block_height"]
        ));
    }
    let versions = &value["local_state_commit_info"]["active_version_set"];
    if versions["inscription_schema_version"] != "uip-0001-miner-pass-inscription:v1"
        || versions["pass_state_machine_version"] != "uip-0002-pass-state-machine:v2"
    {
        return Err(format!(
            "Indexer state does not execute MinerPass V2: height={height}, schema={}, state_machine={}",
            versions["inscription_schema_version"], versions["pass_state_machine_version"]
        ));
    }
    Ok(value)
}
async fn ensure_current_observation(
    state: &AppState,
    observation: &Observation,
) -> Result<(), String> {
    if state_at(state, observation.height).await? != observation.state {
        return Err(format!(
            "Indexer state changed during observation: height={}; retry",
            observation.height
        ));
    }
    let snapshot = &observation.state["snapshot_info"];
    let bh = balance(
        state,
        "get_state_ref_at_height",
        json!([{"block_height":observation.height}]),
    )
    .await?;
    if string(&bh, "snapshot_id")? != string(snapshot, "snapshot_id")? {
        return Err(format!(
            "Balance History and indexer snapshots disagree: height={}, balance_snapshot={}, indexer_snapshot={}",
            observation.height, bh["snapshot_id"], snapshot["snapshot_id"]
        ));
    }
    let hash = state
        .rpc_client
        .bitcoin_call(&state.config, "getblockhash", json!([observation.height]))
        .await?;
    if hash.as_str() != Some(string(snapshot, "stable_block_hash")?) {
        return Err(format!(
            "Bitcoin canonical block differs from index: height={}, bitcoin_hash={hash}, indexed_hash={}",
            observation.height, snapshot["stable_block_hash"]
        ));
    }
    let readiness = state
        .rpc_client
        .usdb_indexer_readiness(&state.config.rpc.usdb_indexer_url)
        .await?;
    if !readiness.consensus_ready {
        return Err("Indexer became unavailable during observation".into());
    }
    Ok(())
}
async fn observe(state: &AppState, draft: &Draft) -> Result<Observation, String> {
    let ready = state
        .rpc_client
        .usdb_indexer_readiness(&state.config.rpc.usdb_indexer_url)
        .await?;
    if !ready.consensus_ready {
        return Err("Indexer consensus state is not ready".into());
    }
    let height = ready
        .synced_block_height
        .ok_or("Indexer height unavailable")?;
    let selected = state_at(state, height).await?;
    let observed_network = parse_balance_history_network(string(
        &selected["snapshot_info"]["consensus_identity"],
        "network",
    )?)?;
    if observed_network != draft.network {
        return Err(format!(
            "Observation network mismatch: expected={}, observed={observed_network}",
            draft.network
        ));
    }
    let response = balance(
        state,
        "get_addresses_balances",
        json!([{
        "script_hashes":[draft.source_hash,draft.recipient_hash],"block_height":height}]),
    )
    .await?;
    let rows = response
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or("Incomplete address balances")?;
    let amount = |row: &Value| -> Result<u64, String> {
        let records = row.as_array().ok_or("Missing balance records")?;
        match records.last() {
            None => Ok(0), // An explicit successful empty history is a zero balance, not an RPC error.
            Some(record) if integer(record, "block_height")? <= u64::from(height) => {
                integer(record, "balance")
            }
            _ => Err("Balance record is newer than the observation".into()),
        }
    };
    let recipient = owner_page(state, &draft.recipient_hash, height, 0, 1).await?;
    let result = Observation {
        height,
        state: selected,
        source_balance: amount(&rows[0])?,
        recipient_balance: amount(&rows[1])?,
        recipient_history: boolean(&recipient, "ever_valid_owner")?,
        passes: owner_passes(state, &draft.source_hash, height).await?,
    };
    ensure_current_observation(state, &result).await?;
    Ok(result)
}

fn operation_path(draft: &Draft, observation: &Observation) -> Result<&'static str, String> {
    let fresh = observation.recipient_balance == 0 && !observation.recipient_history;
    for prev in &draft.prev {
        if !observation.passes.iter().any(|p| {
            p["inscription_id"] == *prev
                && p["owner"] == draft.source_hash
                && matches!(p["state"].as_str(), Some("active" | "dormant"))
        }) {
            return Err(format!(
                "prev is not an Active/Dormant pass of the source: {prev}"
            ));
        }
    }
    if fresh && draft.prev.is_empty() {
        return Ok("first_opening");
    }
    if !draft.source_supported {
        return Err("Source script is outside P2PKH/P2WPKH/P2TR key-path support".into());
    }
    if draft.source_hash == draft.recipient_hash {
        return Ok("same_owner");
    }
    if fresh && !draft.prev.is_empty() {
        return Ok("cross_owner");
    }
    Err("Recipient cannot open: existing balance or valid-pass history; if a fresh address was dusted or occupied, choose another fresh recipient; otherwise use same-owner mint or a fresh recipient with source-owned prev".into())
}

pub(super) async fn prepare(
    state: &AppState,
    request: &BtcMintPrepareRequest,
) -> ApiResult<PreparedBtcMintContext> {
    let services = build_services_summary(state).await;
    let capabilities = build_capabilities_summary(&services, state.config.development_mint.enabled);
    let network_name = resolve_runtime_btc_network_name(&services)
        .ok_or_else(|| unavailable("BTC network unavailable".into()))?;
    let network = parse_balance_history_network(&network_name).map_err(unavailable)?;
    let draft = normalize(request, network).map_err(invalid)?;
    let mut blockers = Vec::new();
    let mut observation = Value::Null;
    let mut passes = Vec::new();
    let mut path = None;
    let mut leader_unresolved = false;
    match observe(state, &draft).await {
        Ok(value) => {
            match operation_path(&draft, &value) {
                Ok(selected) => path = Some(selected.to_string()),
                Err(error) => blockers.push(error),
            }
            // Leader eligibility is observed at the same height as prev and balance checks.
            let leader = if let Some(id) = draft.identity.leader_pass_id() {
                Some(
                    indexer(
                        state,
                        "get_pass_snapshot",
                        json!([{"inscription_id":id,"at_height":value.height}]),
                    )
                    .await,
                )
            } else if let Some(addr) = draft.identity.leader_btc_addr() {
                Some(
                    indexer(
                        state,
                        "get_owner_active_pass_at_height",
                        json!([{"owner":addr,"at_height":value.height}]),
                    )
                    .await,
                )
            } else {
                None
            };
            if let Some(result) = leader {
                match result {
                    Ok(pass) if pass["state"] == "active" && pass["pass_kind"] == "standard" => {},
                    Ok(_) if draft.identity.leader_pass_id().is_some() => blockers.push("Fixed Leader must resolve to an Active standard pass at the observed height".into()),
                    Ok(_) => leader_unresolved = true,
                    Err(error) => blockers.push(error),
                }
            }
            if let Err(error) = ensure_current_observation(state, &value).await {
                blockers.push(error);
            }
            observation = json!({"height":value.height,"state":value.state,
                "source_balance_sats":value.source_balance.to_string(),"recipient_balance_sats":value.recipient_balance.to_string(),
                "recipient_ever_valid_owner":value.recipient_history});
            passes = value.passes;
        }
        Err(error) => {
            warn!(
                "MinerPass prepare unavailable: source={}, recipient={}, error={error}",
                draft.source, draft.recipient
            );
            blockers.push(error);
        }
    }
    let mut warnings = vec![
        "This is a confirmed-state observation, not a guarantee of reveal-time eligibility. Dust, another mint, pending transactions or a reorg can change the result.".into(),
        "Source verification supports P2PKH/P2WPKH SIGHASH_ALL and P2TR key-path DEFAULT/ALL; other scripts cannot receive tool funding approval even if first opening is protocol-valid.".into(),
        "Use pass addresses only for explicit pass operations. Ordinary payments to a malicious commit address can authorize an unwanted beneficiary change; source participation is not content consent.".into(),
        "Verify the exact inscription, source, recipient, configuration and prev consumption before depositing or migrating BTC. The tool never sweeps funds automatically.".into(),
    ];
    if leader_unresolved {
        warnings.push("Address-bound Leader has no Active standard pass at the observed height. The collab mint can be valid, but currently contributes no effective energy.".into());
    }
    if draft.recipient_taproot {
        warnings.push("Recipient is Taproot: its output key is already public. A never-exposed hash address is needed for the cold-address strategy.".into());
    }
    if path.as_deref() == Some("cross_owner") {
        warnings.push("After verification, move the old balance promptly and check change and remaining pass UTXOs. Rotation still has a public-key exposure window; collaborators bound to the old leader/address do not follow automatically.".into());
    }
    let active_pass = passes
        .iter()
        .find(|p| p["state"] == "active")
        .map(|p| {
            let mut p = p.clone();
            p["prev"] = json!([]);
            serde_json::from_value(p)
        })
        .transpose()
        .map_err(|err| unavailable(format!("Incomplete source active pass: {err}")))?;
    let suggested_prev = passes
        .iter()
        .filter(|p| p["state"] == "active")
        .filter_map(|p| p["inscription_id"].as_str().map(String::from))
        .collect();
    let retained = passes
        .iter()
        .filter(|p| matches!(p["state"].as_str(), Some("active" | "dormant")))
        .filter_map(|p| p["inscription_id"].as_str())
        .filter(|id| !draft.prev.iter().any(|p| p == id))
        .map(String::from)
        .collect();
    let payload = build_btc_mint_inscription_payload(&draft.identity, &draft.prev);
    let payload_json =
        serde_json::to_string_pretty(&payload).map_err(|err| unavailable(err.to_string()))?;
    let execution_available = state.config.development_mint.enabled
        && capabilities.btc_runtime_profile == "development"
        && services
            .ord
            .data
            .as_ref()
            .is_some_and(|v| v.query_ready == Some(true));
    Ok(PreparedBtcMintContext {
        runtime_profile: capabilities.btc_runtime_profile.clone(),
        response: BtcMintPrepareResponse {
            eligible: blockers.is_empty(),
            execution_available,
            prepare_mode: "draft_only".into(),
            blockers,
            warnings,
            runtime: BtcMintPrepareRuntimeSummary {
                btc_network: network_name.clone(),
                btc_runtime_profile: capabilities.btc_runtime_profile,
                btc_console_mode: capabilities.btc_console_mode,
                ord_available: capabilities.ord_available,
                ord_query_ready: services
                    .ord
                    .data
                    .as_ref()
                    .is_some_and(|v| v.query_ready == Some(true)),
                balance_history_ready: services
                    .balance_history
                    .data
                    .as_ref()
                    .is_some_and(|v| v.query_ready == Some(true)),
                usdb_indexer_ready: services
                    .usdb_indexer
                    .data
                    .as_ref()
                    .is_some_and(|v| v.query_ready == Some(true)),
                ord_synced_block_height: services
                    .ord
                    .data
                    .as_ref()
                    .and_then(|v| v.synced_block_height),
                btc_tip_height: services.btc_node.data.as_ref().and_then(|v| v.blocks),
                ord_sync_gap: services.ord.data.as_ref().and_then(|v| v.sync_gap),
            },
            source_address: draft.source.clone(),
            source_script_hash: draft.source_hash.clone(),
            recipient_address: draft.recipient.clone(),
            owner_address: draft.recipient.clone(),
            owner_script_hash: draft.recipient_hash.clone(),
            operation_path: path,
            observation,
            source_passes: passes,
            retained_pass_ids: retained,
            pass_kind: draft.identity.pass_kind().into(),
            usdb_main: draft.identity.usdb_main().map(String::from),
            leader_pass_id: draft.identity.leader_pass_id().map(String::from),
            leader_btc_addr: draft.identity.leader_btc_addr().map(String::from),
            prev: draft.prev,
            suggested_prev,
            active_pass,
            inscription_payload: payload.clone(),
            inscription_payload_json: payload_json,
            prepare_request: json!({"protocol":"usdb-btc-mint-v2","prepare_mode":"draft_only","runtime_btc_network":network_name,
            "source_address":draft.source,"recipient_address":draft.recipient,"inscription":{"content_type":"application/json","payload":payload},"psbt":null}),
        },
    })
}

pub(super) async fn verify(
    State(state): State<AppState>,
    Json(request): Json<BtcMintVerifyRequest>,
) -> ApiResult<Json<BtcMintVerifyResponse>> {
    let services = build_services_summary(&state).await;
    let network_name = resolve_runtime_btc_network_name(&services)
        .ok_or_else(|| unavailable("BTC network unavailable".into()))?;
    let draft = normalize(
        &request.mint,
        parse_balance_history_network(&network_name).map_err(unavailable)?,
    )
    .map_err(invalid)?;
    if !super::is_valid_inscription_id(&request.inscription_id) {
        return Err(invalid("Invalid inscription_id".into()));
    }
    let observed = observe(&state, &draft).await.map_err(unavailable)?;
    let height = observed.height;
    let snapshot = indexer(
        &state,
        "get_pass_snapshot",
        json!([{"inscription_id":request.inscription_id,"at_height":height}]),
    )
    .await
    .map_err(unavailable)?;
    let mut blockers = Vec::new();
    if snapshot.is_null() {
        blockers
            .push("Inscription is not yet indexed at the confirmed height; wait and retry".into());
    } else if snapshot["inscription_id"] != request.inscription_id
        || !snapshot_matches(&draft, &snapshot)
    {
        blockers.push("Pass state, recipient or complete inscription configuration differs from the expected draft".into());
    }
    let required = integer(
        &observed.state["snapshot_info"]["consensus_identity"],
        "stable_lag",
    )
    .map_err(unavailable)?
    .saturating_add(1);
    let mut confirmations = 0;
    let mut source = Value::Null;
    if !snapshot.is_null() {
        match indexer(
            &state,
            "get_pass_mint_source",
            json!([{"inscription_id":request.inscription_id,"at_height":height}]),
        )
        .await
        {
            Ok(evidence) => {
                let audit = &evidence["mint"]["audit"];
                source = evidence["source"].clone();
                if source["source_owner"] != draft.source_hash
                    || !matches!(
                        source["authorization"].as_str(),
                        Some("p2pkh_all" | "p2wpkh_all" | "taproot_key_path")
                    )
                {
                    blockers.push("Actual inscription source or verified signature form does not match the expected source".into());
                }
                if request
                    .expected_source_outpoint
                    .as_ref()
                    .is_some_and(|p| source["source_outpoint"] != *p)
                {
                    blockers.push(
                        "Actual inscription sat did not come from the selected source UTXO".into(),
                    );
                }
                if audit["inscription_id"] != request.inscription_id
                    || audit["block_height"] != snapshot["mint_block_height"]
                    || evidence["mint"]["observed_at_height"] != height
                    || audit["recipient"] != draft.recipient_hash
                    || !audit["error_code"].is_null()
                {
                    blockers.push("Mint audit did not accept the expected recipient".into());
                }
                let block_hash = string(audit, "block_hash").map_err(unavailable)?;
                let header = state
                    .rpc_client
                    .bitcoin_call(&state.config, "getblockheader", json!([block_hash]))
                    .await
                    .map_err(unavailable)?;
                confirmations = header["confirmations"].as_u64().unwrap_or(0);
                if confirmations < required {
                    blockers.push(format!(
                        "Await canonical confirmations: have={confirmations}, required={required}"
                    ));
                }
            }
            Err(error) => {
                blockers.push(format!("Source evidence unavailable; do not fund: {error}"))
            }
        }
    }
    for prev in &draft.prev {
        let pass = indexer(
            &state,
            "get_pass_snapshot",
            json!([{"inscription_id":prev,"at_height":height}]),
        )
        .await
        .map_err(unavailable)?;
        if pass["state"] != "consumed" {
            blockers.push(format!("Expected prev has not been consumed: {prev}"));
        }
    }
    ensure_current_observation(&state, &observed)
        .await
        .map_err(unavailable)?;
    let remaining = observed
        .passes
        .iter()
        .filter(|p| matches!(p["state"].as_str(), Some("active" | "dormant")))
        .filter_map(|p| p["inscription_id"].as_str().map(String::from))
        .collect();
    Ok(Json(BtcMintVerifyResponse {
        verified: blockers.is_empty(),
        blockers,
        observed_height: height,
        confirmations,
        required_confirmations: required,
        source_balance_sats: observed.source_balance.to_string(),
        remaining_source_pass_ids: remaining,
        source,
        snapshot,
        state: observed.state,
    }))
}
fn snapshot_matches(draft: &Draft, snapshot: &Value) -> bool {
    let prev: Option<Vec<String>> = serde_json::from_value(snapshot["prev"].clone()).ok();
    snapshot["state"] == "active"
        && snapshot["mint_version"] == usdb_util::MINER_PASS_MINT_SCHEMA_VERSION
        && snapshot["owner"] == draft.recipient_hash
        && snapshot["mint_owner"] == draft.recipient_hash
        && snapshot["pass_kind"] == draft.identity.pass_kind()
        && snapshot["usdb_main"].as_str().unwrap_or("") == draft.identity.usdb_main().unwrap_or("")
        && snapshot["leader_pass_id"].as_str() == draft.identity.leader_pass_id()
        && snapshot["leader_btc_addr"].as_str() == draft.identity.leader_btc_addr()
        && prev.as_ref() == Some(&draft.prev)
}

/// Select a real confirmed cardinal coin owned by D; wallet naming is never source proof.
pub(super) async fn select_source_coin(
    state: &AppState,
    wallet: &str,
    address: &str,
    passes: &[Value],
) -> Result<String, String> {
    let tip = state
        .rpc_client
        .bitcoin_blockchain_info(&state.config)
        .await?;
    if tip.chain != "regtest" {
        return Err(format!(
            "Source selection requires regtest: chain={}",
            tip.chain
        ));
    }
    let ord_url = state.config.rpc.ord_url.trim_end_matches('/');
    let count = state
        .rpc_client
        .http_text(&format!("{ord_url}/blockcount"))
        .await?;
    let hash = state
        .rpc_client
        .http_text(&format!("{ord_url}/blockhash/{}", tip.blocks))
        .await?;
    if count.trim().parse::<u64>().ok() != tip.blocks.checked_add(1)
        || hash.trim().trim_matches('"') != tip.best_block_hash
    {
        return Err(format!(
            "Ord inventory is not at the canonical Bitcoin tip: height={}, hash={}",
            tip.blocks, tip.best_block_hash
        ));
    }
    let coins = state
        .rpc_client
        .bitcoin_wallet_call(
            &state.config,
            wallet,
            "listunspent",
            json!([1, 9999999, [address]]),
        )
        .await?;
    let coins = coins
        .as_array()
        .ok_or("Wallet returned no unspent coin array")?;
    for coin in coins {
        if coin["address"] != address || coin["spendable"] != true || coin["safe"] != true {
            continue;
        }
        let amount = coin["amount"]
            .as_f64()
            .and_then(|n| Amount::from_btc(n).ok())
            .ok_or("Invalid source coin amount")?;
        if amount.to_sat() < 100_000 {
            continue;
        }
        let outpoint = format!("{}:{}", string(coin, "txid")?, integer(coin, "vout")?);
        if passes.iter().any(|p| {
            p["satpoint"]
                .as_str()
                .is_some_and(|s| s.starts_with(&format!("{outpoint}:")))
        }) {
            continue;
        }
        let output = state
            .rpc_client
            .ord_output(&state.config.rpc.ord_url, &outpoint)
            .await?;
        if !output["inscriptions"]
            .as_array()
            .ok_or("Ord output lacks inscription inventory")?
            .is_empty()
        {
            continue;
        }
        let current = state
            .rpc_client
            .bitcoin_call(
                &state.config,
                "gettxout",
                json!([coin["txid"], coin["vout"], true]),
            )
            .await?;
        if current.is_null() || integer(&current, "confirmations")? == 0 {
            continue;
        }
        let expected = address
            .parse::<Address<_>>()
            .map_err(|err| format!("Invalid source address: {err}"))?
            .require_network(Network::Regtest)
            .map_err(|err| err.to_string())?
            .script_pubkey()
            .to_hex_string();
        if current["scriptPubKey"]["hex"] != expected {
            return Err(format!("Source coin script changed: outpoint={outpoint}"));
        }
        let final_tip = state
            .rpc_client
            .bitcoin_blockchain_info(&state.config)
            .await?;
        if final_tip.best_block_hash != tip.best_block_hash {
            return Err("Bitcoin tip changed during coin selection; retry".into());
        }
        return Ok(outpoint);
    }
    Err(format!(
        "No confirmed cardinal source coin >= 100000 sat: source={address}; fund D without spending prev inscriptions"
    ))
}

#[cfg(test)]
#[path = "../../../../tests/control_plane_mint.rs"]
mod tests;
