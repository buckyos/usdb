"""Pure text presentation of collected node progress.

The caller owns observations, timing, terminal capabilities and control flow.
Rendering never probes services, reads configuration, or changes the report.
The public progress JSON remains the observation contract; rows are disposable
presentation objects, not inputs to readiness or resource decisions.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
import math
import textwrap
from typing import Any


def duration_text(seconds: int | float) -> str:
    hours, remainder = divmod(max(0, int(seconds)), 3600)
    minutes, seconds = divmod(remainder, 60)
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}"


def human_size(value: int) -> str:
    size = float(max(0, value))
    for unit in ("B", "KiB", "MiB", "GiB"):
        if size < 1024:
            return f"{size:.1f}{unit}"
        size /= 1024
    return f"{size:.1f}TiB"


def network_status_lines(report: dict[str, Any]) -> list[str]:
    network = report.get("network")
    if not network:
        return []
    return [f"Network: {network['name']} | Chain ID: {network.get('chain_id')} | Role: {report.get('node_role', 'unknown')}",
            f"Genesis: {network.get('genesis_hash') or 'unknown'}",
            f"P2P network ID: {network.get('network_id')} | Bitcoin source: {network.get('bitcoin_network') or 'unknown'}"]


_COMPLETE = {"READY", "ACTIVE", "VALIDATED", "IDLE", "SKIPPED", "DISABLED"}
_ACTIVE = {"SYNCING", "INDEXING", "IMPORTING", "VERIFYING", "INSTALLING", "DOWNLOADING", "STARTING", "STOPPING", "SWITCHING", "RUNNING"}


@dataclass
class _Row:
    label: str
    state: str
    summary: str = ""
    info: list[str] = field(default_factory=list)
    percent: float | None = None
    attention: bool = False
    preparation: bool = False

    @property
    def group(self) -> str:
        if self.attention or self.state.startswith(("FAILED", "BLOCKED", "UNAVAILABLE", "STALE", "UNKNOWN")):
            return "Attention"
        if self.state not in _COMPLETE:
            return "Work in progress"
        return "Preparation" if self.preparation else "Node services"


def _height(value: Any) -> bool:
    return type(value) is int and value >= 0


def _identifier(value: str, details: bool) -> str:
    """Keep full identifiers in details/JSON while fitting the routine dashboard."""
    return value if details or len(value) <= 26 else value[:12] + "..." + value[-10:]


def _block_age(timestamp: Any, observed_at: str) -> str:
    """Use the report clock; rendering never samples clocks or performs RPC calls."""
    if not _height(timestamp):
        return "age unavailable"
    try:
        elapsed = int(datetime.fromisoformat(observed_at).timestamp()) - timestamp
    except (ValueError, TypeError, OverflowError, OSError):
        return "age unavailable"
    return duration_text(elapsed) + " ago" if elapsed >= 0 else "timestamp ahead of observation"


def _usdb_amount(atoms: str) -> str:
    """Format decimal atom strings exactly, including one-atom rewards."""
    whole, fraction = divmod(int(atoms), 10**18)
    suffix = f"{fraction:018d}".rstrip("0")
    return f"{whole:,}" + (f".{suffix}" if suffix else "")


def _chain_mining_lines(report: dict[str, Any], component: dict[str, Any], *, details: bool) -> list[str]:
    """Separate the network head, a proven local seal, and verified per-block income."""
    if component.get("last_observed_at") or component.get("observation_unavailable"):
        return []
    lines = []
    head = component.get("head") or {}
    if head.get("hash"):
        lines.append(f"Head: {_identifier(head['hash'], details)} | "
                     f"{_block_age(head.get('timestamp'), report.get('observed_at', ''))}")
    activity = (report.get("mining") or {}).get("activity")
    if not activity:
        return lines
    seal, state = activity.get("local_seal"), activity.get("state")
    if not seal:
        lines.append("Local block: " + activity.get("detail", "unavailable"))
        return lines
    lines.append(f"Local block: #{seal['height']:,} | {_identifier(seal['hash'], details)} | {state} | "
                 f"{_block_age(seal.get('timestamp'), report.get('observed_at', ''))}")
    if state != "canonical":
        lines.append(activity.get("detail", "Local block canonicality is not confirmed"))
        return lines
    reward = activity.get("reward") or {}
    if reward.get("state") == "verified":
        lines.append(f"Block income: {_usdb_amount(reward['total_atoms'])} USDB "
                     f"(issuance {_usdb_amount(reward['emission_atoms'])} + fees {_usdb_amount(reward['fee_atoms'])})")
        candidate = (report.get("mining") or {}).get("eligibility", {}).get("candidate", {}).get("pass", {})
        if details or candidate.get("pass_id") != reward.get("pass_id"):
            lines.append(f"Block Pass: {_identifier(reward['pass_id'], details)} | BTC height {reward['btc_height']:,}")
    else:
        lines.append("Block income: " + reward.get("detail", "unavailable"))
    if details:
        lines.append(f"Local seal checked: {activity.get('observed_at', 'unknown')} | "
                     f"confirmations at sample: {activity.get('confirmations', '?')}")
    return lines


def _coverage(current: Any, total: Any) -> float | None:
    if _height(current) and _height(total) and total > 0:
        return min(100.0, 100 * current / total)
    return None


def _component_row(component: dict[str, Any], *, details: bool) -> _Row:
    state = component.get("display_state", component["state"])
    stale = bool(component.get("last_observed_at") or str(component.get("detail", "")).startswith("STALE"))
    if component["state"] in {"FAILED", "BLOCKED"}:
        state = component["state"]
    elif stale:
        state = "STALE"
    elif component.get("observation_unavailable"):
        state = "UNAVAILABLE"
    background = component.get("background_validation")
    native_bitcoin = component["id"] == "bitcoin" and isinstance(background, dict)
    label = "Bitcoin foreground" if native_bitcoin and component.get("progress_phase") not in {"pre_snapshot_ibd", "initializing"} else component["label"]
    row = _Row(label, state, preparation=component["id"] in {"snapshot", "script_registry"})
    healthy = state in _COMPLETE
    current, total = component.get("current"), component.get("total")
    if _height(current):
        if component.get("unit") == "bytes":
            row.summary = human_size(current) + (f" / {human_size(total)}" if _height(total) else "")
        elif healthy:
            row.summary = f"{'block' if component['id'] == 'usdb_chain' else 'height'} {current:,}"
        else:
            row.summary = f"{current:,}" + (f" / {total:,}" if _height(total) else "")
            if component.get("unit") == "utxos":
                row.summary += " UTXOs"
    percent = component.get("progress_percent")
    if state in _ACTIVE and type(percent) in {int, float} and math.isfinite(percent):
        row.percent = min(100.0, max(0.0, percent))
    if details or not healthy or component["id"] == "usdb_chain":
        if component.get("detail"):
            row.info.append(component["detail"])
    if (native_bitcoin and state == "READY" and background.get("available")
            and not background.get("stale") and background.get("validated") is False):
        row.info.append("Foreground tip ready; Bitcoin Core background validation is not complete")
    file = component.get("file_preparation")
    if isinstance(file, dict):
        verification = ("verified" if file["state"] == "VERIFIED" else
                        "verification in progress" if component["state"] == "VERIFYING" else "not confirmed")
        if healthy and not details:
            row.summary = f"File: download complete ({human_size(file['size_bytes'])}) | SHA-256: {verification}"
        else:
            row.info += [f"File: download complete ({human_size(file['size_bytes'])})", f"File SHA-256: {verification}"]
    if type(component.get("baseline_header_height")) is int and component["state"] == "WAITING":
        row.info.append(f"Next: Core import after baseline block header {component['baseline_header_height']} is available")
    progress_phase = component.get("progress_phase")
    row.info.extend(component.get("preparation_details", []))
    if file and str(progress_phase).startswith("core_"):
        row.info.append("Progress above: Core import stage; file download is complete")
    if component.get("last_observed_at"):
        row.info += [f"Last observed: {component['last_observed_at']} ({component['stale_age_secs']}s ago, state={component['last_observed_state']})",
                     f"Latest probe: {component['latest_probe_detail']}"]
    milestone = component.get("genesis_milestone")
    if isinstance(milestone, dict) and (details or not healthy):
        start = component.get("sync_start_height")
        prefix = f"Blocks from {start} | " if type(start) is int else ""
        row.info.append(f"{prefix}Genesis {milestone['height']}: {milestone['state']}")
        if component.get("sync_max_height") is not None:
            row.info.append(f"Configured maximum target: {component['sync_max_height']}")
        if milestone.get("remaining_blocks"):
            row.info.append(f"To genesis: {milestone['remaining_blocks']} blocks")
        if component.get("sync_target_source") == "last_observed_bitcoin_headers":
            row.info.append("Using last observed Bitcoin headers; current Core RPC unavailable")
    target_source = component.get("sync_target_source")
    target_label = "Target (last observed)" if stale else "Target"
    target_value = f" = {total}" if _height(total) else ""
    if component["id"] == "balance_history" and type(component.get("stable_lag_blocks")) is int:
        source_label = {
            "balance_history_rpc": "balance-history reported sync target",
            "unavailable": "unavailable",
        }.get(target_source)
        if source_label is None:
            headers = "last observed Bitcoin headers" if target_source == "last_observed_bitcoin_headers" else "Bitcoin headers"
            source_label = f"{headers} minus {component['stable_lag_blocks']} confirmation blocks"
        if component.get("sync_max_height") is not None:
            source_label += f", capped at {component['sync_max_height']}"
        row.info.append(f"{target_label}: {source_label}{target_value}")
    elif component["id"] == "usdb_indexer" and target_source == "balance_history_stable_height":
        row.info.append(f"{target_label}: balance-history available stable height{target_value}")
    if component["id"] == "images":
        download = component.get("image_download", {})
        if download.get("image"):
            row.info.append(f"Image {download.get('image_index', '?')}/{download.get('image_count', '?')}: {download['image']}")
        if _height(download.get("downloaded_bytes")):
            total = download.get("download_total_bytes")
            amount = human_size(download["downloaded_bytes"]) + (f" / {human_size(total)}" if _height(total) and total > 0 else " (total not reported)")
            row.info.append("Download observed for current image: " + amount)
        if download.get("layer_count"):
            row.info.append(f"Layers observed: {download.get('completed_layers', 0)}/{download['layer_count']} complete; {download.get('reused_layers', 0)} cached")
        if download.get("image_attempt"):
            row.info.append(f"Image pull attempt: {download['image_attempt']} | Pull retries: {download.get('retry_count', 0)}")
        if download.get("last_error"):
            row.info.append(f"Last pull error ({download.get('last_error_at', 'unknown')}): {download['last_error']}")
        row.info += [f"Stage elapsed={duration_text(component['stage_elapsed_secs'])} | ETA=-- (not estimated)",
                     "Layer progress: usdb-node controller logs --follow"]
    elif (str(progress_phase).startswith("core_") or progress_phase == "initializing") and "stage_elapsed_secs" in component:
        row.info.append(f"Stage elapsed={duration_text(component['stage_elapsed_secs'])} | ETA=-- (not reported by Core)")
    timing = component.get("timing")
    elapsed = component.get("service_elapsed_secs")
    if timing:
        elapsed = timing["elapsed_secs"]
        if details or not healthy:
            label = "Process elapsed" if timing["elapsed_source"] == "process" else "Observed elapsed"
            eta = f"~{duration_text(timing['eta_secs'])}" if timing["eta_secs"] is not None else f"-- ({timing['eta_state']})"
            row.info.append(f"{label}={duration_text(elapsed)} | ETA={eta}")
    elif elapsed is not None and (details or not healthy):
        row.info.append(f"Process elapsed={duration_text(elapsed)}")
    if healthy and elapsed is not None and not details:
        label = "uptime" if not timing or timing["elapsed_source"] == "process" else "observed"
        row.summary += f"{' | ' if row.summary else ''}{label} {duration_text(elapsed)}"
    head = component.get("head")
    if details and isinstance(head, dict):
        row.info.append(f"Latest block #{head['number']} hash: {head['hash']}")
    return row


def _history_row(background: dict[str, Any], *, details: bool, probe_detail: str | None = None) -> _Row:
    if background.get("stale"):
        state = "STALE"
        if background.get("validated"):
            summary = f"STALE: last validated through baseline {background['target']}"
        elif _height(background.get("height")):
            summary = f"STALE {background['height']}/{background['target']} (last observed syncing)"
        else:
            summary = "STALE: last waiting for snapshot activation"
    elif background.get("waiting_for_initialization"):
        state, summary = "WAITING", "WAITING for Core initialization; validation status not yet available"
    elif background.get("waiting_for_start"):
        state, summary = "WAITING", "WAITING for Core startup"
    elif not background.get("available"):
        state, summary = "UNAVAILABLE", "UNAVAILABLE"
    elif background.get("validated"):
        state, summary = "VALIDATED", f"VALIDATED through baseline {background['target']}"
    elif _height(background.get("height")):
        state, summary = "SYNCING", f"SYNCING {background['height']}/{background['target']}"
    else:
        state, summary = "WAITING", "WAITING for snapshot activation"
    info = ["Core background history: " + summary] if details or state != "VALIDATED" else []
    if state in {"STALE", "UNAVAILABLE"} and probe_detail:
        info.append("Latest probe (shared with foreground): " + probe_detail)
    return _Row("Bitcoin background", state, preparation=True,
                summary=f"baseline {background['target']:,}" if state == "VALIDATED" and not details else "",
                info=info,
                percent=_coverage(background.get("height"), background.get("target")) if state == "SYNCING" else None)


def _minting_rows(report: dict[str, Any], *, details: bool) -> list[_Row]:
    minting = report.get("minting", {})
    if not minting.get("enabled"):
        return []
    state = minting["state"]
    # An absent observation is expected only while a live image preparation and
    # the same inventory prove a pending start. Keep fresh failures and raw
    # capability flags intact; these rows never feed readiness or alert policy.
    preparing = (report.get("image_preparation", {}).get("phase") in {"checking", "cached", "pulling"}
                 and state == "UNAVAILABLE" and minting.get("observed_at_ms") is None)
    startup = report.get("minting_startup", {})
    core_wait = preparing and startup.get("bitcoin_not_started") is True
    ord_wait = preparing and startup.get("ord_not_started") is True
    current, target = minting.get("txindex_height"), minting.get("core_height")
    # Height coverage is not proof of readiness, nor a time-completion estimate.
    if core_wait:
        index_state = "WAITING"
    elif state in {"UNAVAILABLE", "STOPPED", "FAILED"}:
        index_state = "UNAVAILABLE"
    elif _height(current) and _height(target):
        index_state = "READY" if minting.get("txindex_synced") is True and current >= target else "INDEXING"
    else:
        index_state = "WAITING"
    index = _Row("Bitcoin txindex", index_state, preparation=True)
    if _height(current) and _height(target):
        index.summary = f"height {current:,} / {target:,}"
    if index_state == "INDEXING":
        index.percent = _coverage(current, target)
        index.info.append("Height coverage only | ETA=-- (unavailable)")
        if minting.get("txindex_synced") is not True and current >= target:
            index.info.append("Waiting for Core to report txindex synced=true")
    elif index_state == "WAITING":
        index.info.append("Waiting for container images before Bitcoin Core startup" if core_wait else
                          "txindex observation deferred with the Ord supervisor during bootstrap" if state == "WAITING_RESOURCES"
                          else "Index height not reported; check that Core adopted BTC_TXINDEX=1")
    ord_row = _Row("Ord (optional)", "WAITING" if ord_wait else state)
    if _height(minting.get("ord_height")):
        ord_row.summary = f"committed height {minting['ord_height']:,}"
        if _height(minting.get("ord_gap")):
            ord_row.summary += f" | gap {minting['ord_gap']:,}"
    if state == "INDEXING":
        ord_row.percent = _coverage(minting.get("ord_height"), target)
        ord_row.info.append("Height coverage only | ETA=-- (unavailable)")
        ord_row.info.append("Committed height updates after a database batch; unchanged height alone does not mean indexing has stalled")
    phase = minting.get("index_phase")
    if phase == "PROCESSING" and _height(minting.get("processing_height")):
        ord_row.info.append(f"Processing block {minting['processing_height']:,} (not yet committed)")
    elif phase == "COMMITTING" and _height(minting.get("commit_target_height")):
        detail = f"Committing database batch through {minting['commit_target_height']:,}"
        if _height(minting.get("commit_elapsed_secs")):
            detail += f" | elapsed {duration_text(minting['commit_elapsed_secs'])}"
        ord_row.info.append(detail)
    elif phase == "RECOVERING":
        ord_row.info.append("Recovering Ord database before indexing resumes")
    read, written, seconds = (minting.get(key) for key in
                              ("sample_read_bytes", "sample_write_bytes", "sample_elapsed_secs"))
    if state in {"INDEXING", "STOPPING", "STARTING"} and all(_height(v) for v in (read, written, seconds)) and seconds:
        ord_row.info.append(f"Recent I/O: read {human_size(read)}, wrote {human_size(written)} over {seconds}s")
    if state == "STOPPING" and _height(minting.get("shutdown_elapsed_secs")):
        ord_row.info.append(f"Shutdown elapsed {duration_text(minting['shutdown_elapsed_secs'])}; waiting for Ord to exit")
    if minting.get("restart_reason") == "cache_profile":
        ord_row.info.append("Restarting only Ord to apply its cache profile; waiting for a clean database commit")
    if minting.get("resource_profile") in {"catchup", "steady"}:
        ord_row.info.append(f"Resource profile: {minting['resource_profile']}; container ceiling includes file-cache headroom")
    if (details or minting.get("resource_profile") in {"catchup", "steady"}) and _height(minting.get("index_cache_bytes")):
        ord_row.info.append(f"Index cache: {human_size(minting['index_cache_bytes'])} | commit interval: {minting.get('commit_interval', 'unknown')} blocks")
    if details or state != "READY":
        ord_row.info.append("Waiting for container images before Ord supervisor startup" if ord_wait
                            else minting.get("guidance", ""))
    if details or state == "BLOCKED_DISK":
        disk = [(label, minting.get(key)) for label, key in
                (("free", "disk_free_bytes"), ("reserve", "disk_required_bytes"), ("index", "index_file_bytes"))]
        values = [f"{label} {human_size(value)}" for label, value in disk if _height(value)]
        if values:
            ord_row.info.append("Disk: " + " | ".join(values))
    return [index, ord_row]


def _bh_bootstrap_context(report: dict[str, Any]) -> tuple[dict, dict] | None:
    """Use recent progress from this BH process, never an old bootstrap journal."""
    bh = next((item for item in report.get("components", []) if item["id"] == "balance_history"), {})
    if (bh.get("state") not in {"IMPORTING", "SYNCING", "VERIFYING"}
            or bh.get("last_observed_at") or bh.get("observation_unavailable")
            or str(bh.get("detail", "")).startswith("STALE")
            or bh.get("display_state", bh.get("state")) not in {"IMPORTING", "SYNCING", "VERIFYING"}):
        return None
    bootstrap = report.get("native_bootstrap", {}).get("balance_history", {})
    phase = bootstrap.get("phase")
    if phase not in {"importing", "replaying", "waiting_for_blocks", "verifying"} or phase != bh.get("progress_phase"):
        return None
    updated = bootstrap.get("observed_file_mtime")
    try:
        started = datetime.fromisoformat(bh["service_started_at"].replace("Z", "+00:00"))
        observed = datetime.fromisoformat(report["observed_at"].replace("Z", "+00:00"))
    except (KeyError, TypeError, ValueError, AttributeError):
        return None
    # A journal left by a previous process cannot explain the current RPC failure.
    if (started.tzinfo is None or observed.tzinfo is None or type(updated) not in (int, float)
            or not math.isfinite(updated) or updated < started.timestamp()
            or not -5 <= observed.timestamp() - updated <= 120):
        return None
    return bh, bootstrap


def _indexer_baseline_details(report: dict[str, Any], component: dict[str, Any]) -> list[str] | None:
    """Label upstream replay counts as BH work, not indexer progress or readiness."""
    if (component["state"] != "WAITING" or component.get("progress_phase") != "waiting_for_upstream"
            or component.get("display_state", "WAITING") != "WAITING"
            or component.get("last_observed_at") or component.get("observation_unavailable")):
        return None
    baseline = component.get("upstream_baseline_height")
    if not _height(baseline):
        return None
    lines = [f"Waiting for balance-history queryable baseline {baseline:,}; indexing has not started"]
    context = _bh_bootstrap_context(report)
    if context is None:
        return lines + ["Current BH replay height unavailable; see Balance history"]
    bh, bootstrap = context
    if bootstrap["phase"] == "importing":
        return lines + ["BH is importing the UTXO snapshot before replay begins"]
    height = bootstrap.get("height")
    start = bh.get("sync_start_height")
    if _height(height) and (not _height(start) or height >= start):
        lines.append(f"BH replay: {height:,} / {baseline:,}; {max(0, baseline - height):,} blocks remaining")
    if bootstrap["phase"] == "verifying" or (_height(height) and height >= baseline):
        lines.append("BH baseline verification and publication are still required")
    return lines


def _chain_bootstrap_detail(report: dict[str, Any], chain: dict[str, Any]) -> str | None:
    """Explain expected pre-RPC bootstrap waits using evidence from the current BH process."""
    detail = chain.get("detail", "")
    marker = "Waiting for balance-history readiness:"
    if (chain["state"] not in {"WAITING", "STARTING"}
            or chain.get("display_state", chain["state"]) not in {"WAITING", "STARTING"}
            or chain.get("last_observed_at")
            or chain.get("observation_unavailable") or marker not in detail
            or not any(error in detail.lower() for error in ("connection reset by peer", "connection refused"))):
        return None
    context = _bh_bootstrap_context(report)
    if context is None:
        return None
    bh, bootstrap = context
    stages = {"importing": "importing the UTXO snapshot", "replaying": "replaying blocks",
              "waiting_for_blocks": "waiting for Bitcoin blocks or undo data", "verifying": "verifying the baseline"}
    baseline = bh.get("genesis_milestone", {}).get("height")
    target = f" {baseline:,}" if _height(baseline) else ""
    return (detail.partition(marker)[0] + f"Waiting for balance-history baseline{target}: {stages[bootstrap['phase']]}; "
            "RPC starts after baseline verification and publication")


def _rows(report: dict[str, Any], *, details: bool) -> list[_Row]:
    rows = []
    incidents = report.get("observations", {}).get("incidents", {})
    if incidents.get("status") == "unavailable":
        rows.append(_Row("Incident records", "UNAVAILABLE", info=[
            "Durable halt records could not be observed; this does not establish recovery."]))
    for event in incidents.get("events", []):
        rows.append(_Row("Chain incident", "BLOCKED", summary=event["code"], info=[
            f"critical | manual intervention | id={event.get('event_id') or 'unknown'}",
            f"Detected: {event.get('detected_at') or 'unknown'} | evidence={event.get('evidence_status', 'unknown')}",
            "Preserve the deep BTC reorg recovery record; restarting does not clear this incident."]))
    controller = report.get("controller", {})
    historical_exit = (controller.get("runtime_state") == "failed"
                       and controller.get("result") == "exit-code" and controller.get("exit_code") == 1
                       and controller.get("exit_status") == 2
                       and controller.get("display_state") in {"idle", "waiting_for_seed"})
    if controller and (details or controller.get("action_required")
                       or (controller.get("runtime_state") == "failed" and not historical_exit)):
        row = _Row("Controller", controller.get("display_state", "unknown").upper(),
                   attention=bool(controller.get("action_required")), info=[controller.get("summary", "")])
        if controller.get("observation_available"):
            runtime = f"systemd={controller['runtime_state']} | last exit={controller.get('exit_status', 'unknown')}"
            if historical_exit:
                if details:
                    row.info.append(f"Historical controller result: {runtime}; exit 2 requested operator action, not a service crash")
            elif details or controller.get("display_state") != "waiting_for_seed":
                row.summary = runtime
        row.info += [f"Action: {action}" for action in controller.get("actions", [])]
        rows.append(row)
    resources = report.get("resources", {})
    if resources.get("error"):
        rows.append(_Row("Resources", "BLOCKED", info=[resources["error"]]))
    elif resources.get("transition_pending") or resources.get("runtime_adopted") is False:
        rows.append(_Row("Resources", "SWITCHING", f"{resources.get('phase', '?')} -> {resources.get('target_phase', '?')}",
                         info=["Waiting for managed service restart / resource adoption"]))
    for component in report.get("components", []):
        if details or component["state"] != "SKIPPED":
            explanations = None
            probes = []
            if component["id"] == "usdb_chain":
                waits = component.get("startup_wait_details")
                if (isinstance(waits, list) and component["state"] in {"WAITING", "STARTING"}
                        and component.get("display_state", component["state"]) in {"WAITING", "STARTING"}
                        and not component.get("last_observed_at") and not component.get("observation_unavailable")):
                    explanations = ["Startup requires Bitcoin Core foreground, balance-history and USDB indexer readiness"]
                    for wait in waits:
                        explanation = _chain_bootstrap_detail(report, {**component, "detail": wait})
                        explanations.append(explanation or wait)
                        if explanation:
                            probes.append("Readiness probe: " + wait)
                else:
                    explanation = _chain_bootstrap_detail(report, component)
                    if explanation:
                        explanations = [explanation]
                        probes.append("Readiness probe: " + component["detail"])
            elif component["id"] == "usdb_indexer":
                explanations = _indexer_baseline_details(report, component)
            row = _component_row({**component, "detail": ""} if explanations else component, details=details)
            if explanations:
                row.info = explanations + row.info
            if details:
                row.info += probes
            if component["id"] == "usdb_chain":
                row.info += _chain_mining_lines(report, component, details=details)
            rows.append(row)
        if isinstance(component.get("background_validation"), dict):
            probe = component.get("latest_probe_detail") or (component.get("detail") if component.get("observation_unavailable") else None)
            rows.append(_history_row(component["background_validation"], details=details, probe_detail=probe))
    rows += _minting_rows(report, details=details)
    mining = report.get("mining")
    if isinstance(mining, dict):
        row = _Row("Mining", mining["state"], attention=bool(mining.get("drift") or mining.get("observation_unavailable")))
        configured = mining.get("configured", {})
        if configured.get("USDB_MINER_THREADS") is not None:
            row.summary = f"workers: {configured['USDB_MINER_THREADS']}"
        if details or row.attention or row.state not in _COMPLETE:
            row.info.append(mining.get("detail", ""))
        candidate = mining.get("eligibility", {}).get("candidate", {})
        profile = candidate.get("pass", {})
        if profile:
            row.info.append(f"Candidate: level {profile.get('level', '?')} | effective energy {profile.get('effective_energy', '?')} "
                            f"| difficulty factor {profile.get('difficulty_factor_bps', '?')} bps")
            height = candidate.get("external_state", {}).get("btc_height")
            if _height(height):
                row.info.append(f"Pass evaluated at BTC height {height:,} | eligible candidates {candidate.get('matching_candidate_count', '?')}")
            if details:
                row.info.append(f"Raw energy {profile.get('raw_energy', '?')} | collaboration {profile.get('collab_contribution', '?')}")
        rows.append(row)
    return rows


def render_node_progress(report: dict[str, Any], *, phase: str = "observe", width: int = 120,
                         details: bool = False, unicode: bool = False) -> str:
    """Render a report without I/O; terminal choices must be explicit inputs."""
    width = max(20, width)
    lines: list[str] = []

    def append(text: str, prefix: str = "", continuation: str = "") -> None:
        # Wrap diagnostics and actions instead of truncating their recovery steps.
        lines.extend(textwrap.wrap(text, width=width, initial_indent=prefix,
                                   subsequent_indent=continuation, break_on_hyphens=False) or [prefix.rstrip()])

    phase = "images" if report.get("image_preparation") else phase
    append(f"USDB node | {report.get('release_id', 'unknown')} | {report.get('node_role', 'unknown')}")
    mining = report.get("mining") or {}
    append(f"Node {report.get('overall_state', 'unknown')} | Mining {mining.get('state', 'unavailable')} | Resources {report.get('resources', {}).get('phase', 'unknown')}")
    resources = report.get("resources", {})
    if resources.get("storage_profile") in {"balanced", "slow-disk"}:
        append(f"Resource profile: {resources['storage_profile']} | Node memory budget: {resources.get('memory_percent', '?')}% (system reserve applies)")
    observed = f"Observed {report.get('observed_at', 'unknown')}"
    if "observation_elapsed_secs" in report:
        observed += f" | Watching {duration_text(report['observation_elapsed_secs'])}"
    append(observed)
    controller = report.get('controller', {}).get('display_state', report.get('controller_state', 'unknown'))
    append(f"phase={phase} | controller={controller}")
    if details:
        for line in network_status_lines(report):
            append(line)
    elif report.get("network"):
        network = report["network"]
        append(f"Network: {network['name']} | Chain ID: {network.get('chain_id')}")

    configured = mining.get("configured", {})
    if configured.get("USDB_NODE_ROLE") == "miner" and configured.get("USDB_MINER_ADDRESS"):
        append(f"Miner address: {configured['USDB_MINER_ADDRESS']}")
        profile = mining.get("eligibility", {}).get("candidate", {}).get("pass", {})
        if profile.get("pass_id"):
            append(f"Candidate Pass: {profile['pass_id']} | {profile.get('state', '?')}/{profile.get('pass_kind', '?')}")
        else:
            append("Candidate Pass: unavailable (see Mining status)")

    rows = _rows(report, details=details)
    for group in ("Attention", "Work in progress", "Node services", "Preparation"):
        members = [row for row in rows if row.group == group]
        if not members:
            continue
        lines.append("")
        append(group, "◆ " if unicode else "== ")
        for row in members:
            if row.state == "FAILED":
                mark = "✗" if unicode else "[FAIL]"
            elif row.group == "Attention":
                mark = "!" if unicode else "[!]"
            elif row.state in {"DISABLED", "SKIPPED"}:
                mark = "-" if unicode else "[-]"
            elif row.state in _COMPLETE:
                mark = "✓" if unicode else "[OK]"
            elif row.state in _ACTIVE:
                mark = "↻" if unicode else "[RUN]"
            else:
                mark = "…" if unicode else "[WAIT]"
            prefix = f"  {mark} " if unicode else f"  {mark:<6} "
            body = f"{row.label:<18} {row.state}"
            info = list(filter(None, row.info))
            if row.percent is not None:
                bar_width = 20
                filled = int(row.percent * bar_width / 100)
                # Rounding must not turn an unfinished height or byte range into 100%.
                displayed_percent = min(row.percent, 99.99) if row.percent < 100 else row.percent
                bar = f"[{'#' * filled}{'-' * (bar_width - filled)}] {displayed_percent:.2f}%"
                if len(body) + len(prefix) + len(bar) + 1 <= width:
                    body += " " + bar
                else:
                    info.insert(0, bar)
                if row.summary:
                    info.insert(1 if info and info[0] == bar else 0, row.summary)
            elif row.summary:
                body += " | " + row.summary
            append(body, prefix, " " * len(prefix))
            for index, detail in enumerate(info):
                branch = ("└─ " if index == len(info) - 1 else "├─ ") if unicode else "- "
                indent = "      " if unicode else "         "
                append(detail, indent + branch, indent + " " * len(branch))
    minting = report.get("minting")
    if minting:
        lines.append("")
        prefix = "Local minting: DISABLED | " if not minting.get("enabled") else ""
        append(prefix + "Wallet transactions: " + ("enabled" if minting.get("transactions_enabled") else "unavailable"))
    return "\n".join(lines)
