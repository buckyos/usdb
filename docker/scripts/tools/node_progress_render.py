"""Pure text presentation of collected node progress.

The caller owns observations, timing, terminal capabilities and control flow.
Rendering never probes services, reads configuration, or changes the report.
The public progress JSON remains the observation contract; rows are disposable
presentation objects, not inputs to readiness or resource decisions.
"""

from __future__ import annotations

from dataclasses import dataclass, field
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
_ACTIVE = {"SYNCING", "INDEXING", "IMPORTING", "VERIFYING", "INSTALLING", "DOWNLOADING", "STARTING", "SWITCHING", "RUNNING"}


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
    row = _Row(component["label"], state, preparation=component["id"] in {"snapshot", "script_registry"})
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
    elif str(progress_phase).startswith("core_") and "stage_elapsed_secs" in component:
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


def _history_row(background: dict[str, Any], *, details: bool) -> _Row:
    if background.get("stale"):
        state = "STALE"
        if background.get("validated"):
            summary = f"STALE: last validated through baseline {background['target']}"
        elif _height(background.get("height")):
            summary = f"STALE {background['height']}/{background['target']} (last observed syncing)"
        else:
            summary = "STALE: last waiting for snapshot activation"
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
    return _Row("Bitcoin history", state, preparation=True,
                summary=f"baseline {background['target']:,}" if state == "VALIDATED" and not details else "",
                info=["Core background history: " + summary] if details or state != "VALIDATED" else [],
                percent=_coverage(background.get("height"), background.get("target")) if state == "SYNCING" else None)


def _minting_rows(minting: dict[str, Any], *, details: bool) -> list[_Row]:
    if not minting.get("enabled"):
        return []
    state = minting["state"]
    current, target = minting.get("txindex_height"), minting.get("core_height")
    # Height coverage is not proof of readiness, nor a time-completion estimate.
    if state in {"UNAVAILABLE", "STOPPED", "FAILED"}:
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
        index.info.append("Index height not reported; check that Core adopted BTC_TXINDEX=1")
    ord_row = _Row("Ord (optional)", state)
    if _height(minting.get("ord_height")):
        ord_row.summary = f"height {minting['ord_height']:,}"
        if _height(minting.get("ord_gap")):
            ord_row.summary += f" | gap {minting['ord_gap']:,}"
    if state == "INDEXING":
        ord_row.percent = _coverage(minting.get("ord_height"), target)
        ord_row.info.append("Height coverage only | ETA=-- (unavailable)")
    if details or state != "READY":
        ord_row.info.append(minting.get("guidance", ""))
    if details or state == "BLOCKED_DISK":
        disk = [(label, minting.get(key)) for label, key in
                (("free", "disk_free_bytes"), ("reserve", "disk_required_bytes"), ("index", "index_file_bytes"))]
        values = [f"{label} {human_size(value)}" for label, value in disk if _height(value)]
        if values:
            ord_row.info.append("Disk: " + " | ".join(values))
    return [index, ord_row]


def _rows(report: dict[str, Any], *, details: bool) -> list[_Row]:
    rows = []
    controller = report.get("controller", {})
    if controller and (details or controller.get("action_required") or controller.get("runtime_state") == "failed"):
        row = _Row("Controller", controller.get("display_state", "unknown").upper(),
                   attention=bool(controller.get("action_required")), info=[controller.get("summary", "")])
        if controller.get("observation_available"):
            row.summary = f"systemd={controller['runtime_state']} | last exit={controller.get('exit_status', 'unknown')}"
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
            rows.append(_component_row(component, details=details))
        if isinstance(component.get("background_validation"), dict):
            rows.append(_history_row(component["background_validation"], details=details))
    rows += _minting_rows(report.get("minting", {}), details=details)
    mining = report.get("mining")
    if isinstance(mining, dict):
        row = _Row("Mining", mining["state"], attention=bool(mining.get("drift") or mining.get("observation_unavailable")))
        configured = mining.get("configured", {})
        if configured.get("USDB_MINER_THREADS") is not None:
            row.summary = f"workers: {configured['USDB_MINER_THREADS']}"
        if details or row.attention or row.state not in _COMPLETE:
            row.info.append(mining.get("detail", ""))
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
            body = f"{row.label:<17} {row.state}"
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
