"""Versioned, sanitized evidence for monitoring; no alert policy or recovery actions.

Availability describes an observation, not service health. Missing fields in old
releases remain unknown. A fresh sample never resolves a durable halt marker.
"""
from __future__ import annotations

from datetime import datetime, timezone
import re
import subprocess

SCHEMA = "usdb-node-observation:v1"
SERVICES = {"bitcoin": "btc-node", "balance_history": "balance-history",
            "usdb_indexer": "usdb-indexer", "usdb_chain": "usdb-chain"}
BLOCKERS = frozenset({
    "RpcNotListening", "Initializing", "Loading", "CatchingUp", "RollbackInProgress",
    "ShutdownRequested", "StableBlockHashMissing", "LatestBlockCommitMissing",
    "SnapshotInstallUnverified", "NativeBootstrapNotReady", "BlockProcessingPending",
    "SyncedHeightMissing", "HistoryBackfillPending", "UpstreamReadinessUnknown",
    "UpstreamConsensusNotReady", "UpstreamSnapshotMissing", "UpstreamSnapshotHeightMismatch",
    "ReorgRecoveryPending", "LocalStateCommitMissing", "SystemStateMissing", "UnknownBlocker",
})
HEIGHTS = ("current", "total", "stable_height", "synced_block_height",
           "balance_history_stable_height", "snapshot_history_ready_height",
           "snapshot_history_pending_from", "block_processing_pending_height", "upstream_reorg_epoch")
HASHES = ("stable_block_hash", "latest_block_commit", "upstream_snapshot_id",
          "local_state_commit", "system_state_id")


def now() -> str:
    """Timestamp this probe, rather than manufacturing a last-success time."""
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds")


def timestamp(value):
    """Accept only bounded, timezone-qualified timestamps and normalize their text."""
    try:
        if not isinstance(value, str) or len(value) > 40:
            return None
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.isoformat() if parsed.tzinfo and parsed.year > 1 else None
    except ValueError:
        return None


def quantity(value):
    return value if type(value) is int and 0 <= value <= 2**64 - 1 else None


def readiness(value, *, observed_at=None) -> dict:
    """Preserve exact service blockers without classifying them as permanent errors."""
    result = {"status": "unavailable", "observed_at": observed_at or now(),
              "rpc_alive": None, "query_ready": None, "consensus_ready": None,
              "blockers": None, "failure": {"code": "READINESS_UNAVAILABLE",
                  "severity": "unknown", "recovery": "unknown"}}
    if not isinstance(value, dict):
        return result
    if "status" in value:
        # Re-serialization is not a new probe and cannot refresh old timestamps.
        result["observed_at"] = timestamp(value.get("observed_at"))
    # Re-project already sanitized observations at the console boundary too.
    if value.get("status") in ("unavailable", "invalid"):
        result["status"] = value["status"]
        if result["status"] == "invalid":
            result["failure"]["code"] = "READINESS_INVALID"
        return result
    if type(value.get("consensus_ready")) is not bool:
        result.update(status="invalid", failure={"code": "READINESS_INVALID",
                      "severity": "unknown", "recovery": "unknown"})
        return result
    result.update(status="available", failure=None)
    result["observed_at"] = timestamp(value.get("observed_at")) or result["observed_at"]
    for key in ("rpc_alive", "query_ready", "consensus_ready"):
        result[key] = value.get(key) if type(value.get(key)) is bool else None
    blockers = value.get("blockers")
    if isinstance(blockers, list):
        result["blockers"] = list(dict.fromkeys(
            item if isinstance(item, str) and item in BLOCKERS else "UnknownBlocker"
            for item in blockers[:64]))
    for key in HEIGHTS:
        result[key] = quantity(value.get(key))
    for key in HASHES:
        v = value.get(key)
        result[key] = v.lower() if isinstance(v, str) and re.fullmatch(r"(?:0x)?[0-9a-fA-F]{64}", v) else None
    return result


def runtime(value) -> dict:
    """Expose container evidence, excluding inspect errors and environment variables."""
    value = value if isinstance(value, dict) else {}
    status = value.get("status") if value.get("status") in ("not_observed", "unavailable") else "available" if value else "not_observed"
    result = {"status": status,
              "details_available": value.get("details_available") is True}
    result["state"] = value.get("state") if value.get("state") in (
        "created", "running", "paused", "restarting", "removing", "exited", "dead") else "unknown"
    result["health"] = value.get("health") if value.get("health") in ("healthy", "unhealthy", "starting", "") else None
    result["exit_code"] = quantity(value.get("exit_code"))
    result["restart_count"] = quantity(value.get("restart_count"))
    result["oom_killed"] = value.get("oom_killed") if type(value.get("oom_killed")) is bool else None
    for key in ("started_at", "finished_at"):
        result[key] = timestamp(value.get(key))
    identity = value.get("container_id")
    result["container_id"] = identity if isinstance(identity, str) and re.fullmatch(r"[0-9a-f]{12,64}", identity) else None
    return result


def incidents(value) -> dict:
    """Project only the supported durable incident contract, never raw marker JSON."""
    if not isinstance(value, dict) or value.get("status") not in ("available", "unavailable", "not_configured"):
        return {"status": "unavailable", "events": []}
    result = {"status": value["status"], "events": []}
    if result["status"] != "available":
        return result
    events = value.get("events")
    if not isinstance(events, list) or len(events) > 1:
        return {"status": "unavailable", "events": []}
    for event in events:
        if not isinstance(event, dict) or event.get("code") != "DEEP_REORG_HALTED" or event.get("latched") is not True:
            return {"status": "unavailable", "events": []}
        identity = event.get("event_id")
        item = {"event_id": identity if isinstance(identity, str) and re.fullmatch(r"(?:[0-9a-f]{32}|legacy-sha256:[0-9a-f]{64})", identity) else None,
                "service": "usdb_chain", "code": "DEEP_REORG_HALTED", "severity": "critical",
                "recovery": "manual_intervention", "latched": True, "source": "deep_btc_reorg_marker",
                "detected_at": timestamp(event.get("detected_at")),
                "evidence_status": event.get("evidence_status") if event.get("evidence_status") in ("available", "unavailable") else "invalid"}
        if event.get("reason") in ("upstream_reorg_epoch_advanced", "upstream_reorg_epoch_regressed"):
            item["reason"] = event["reason"]
        for key in ("baseline_epoch", "observed_epoch"):
            item[key] = quantity(event.get(key))
        result["events"].append(item)
    return result


def observe_incidents(layout, node) -> dict:
    """Read the marker even when Docker, geth or the mining observer is unavailable."""
    if not layout.node_env.is_file():
        return {"status": "not_configured", "events": []}
    try:
        env = node.read_env(layout.node_env)
        if not env.get("USDB_CHAIN_DATA_HOST_DIR"):
            return {"status": "not_configured", "events": []}
        import usdb_mining
        return incidents(usdb_mining._read_chain_files(layout, env, "incidents"))
    except (OSError, ValueError, subprocess.SubprocessError):
        return {"status": "unavailable", "events": []}


def project(value) -> dict:
    """Enforce the same allowlist when the web-facing snapshot is serialized."""
    value = value if isinstance(value, dict) else {}
    if value.get("schema_version") != SCHEMA or value.get("status") in ("not_observed", "invalid"):
        status = "not_observed" if not value or (value.get("schema_version") == SCHEMA and value.get("status") == "not_observed") else "invalid"
        return {"schema_version": SCHEMA, "status": status,
                "observed_at": None, "services": {}, "incidents": {"status": "unavailable", "events": []}}
    result = {"schema_version": SCHEMA, "status": "available", "observed_at": timestamp(value.get("observed_at")),
              "services": {}, "incidents": incidents(value.get("incidents"))}
    services = value.get("services")
    for name, source in (services.items() if isinstance(services, dict) else []):
        if name not in SERVICES or not isinstance(source, dict):
            continue
        item = {}
        if "readiness" in source:
            item["readiness"] = readiness(source["readiness"], observed_at=result["observed_at"])
        if "runtime" in source:
            item["runtime"] = runtime(source["runtime"])
        if source.get("probe_status") in ("available", "unavailable", "not_observed"):
            item["probe_status"] = source["probe_status"]
        item["peer_count"] = quantity(source.get("peer_count"))
        head = source.get("head")
        if isinstance(head, dict) and quantity(head.get("number")) is not None and re.fullmatch(r"0x[0-9a-fA-F]{64}", str(head.get("hash", ""))):
            item["head"] = {"number": head["number"], "hash": head["hash"].lower(),
                            "timestamp": quantity(head.get("timestamp"))}
        result["services"][name] = item
    return result


def attach(report, layout, node) -> dict:
    """Add current evidence to either report without repeating service RPC probes.

    The lifecycle report may not probe readiness; absence there is explicitly
    not_observed. The progress report is the complete service observation API.
    """
    services = {}
    inventory = report.get("checks", {}).get("runtime", {}).get("services", {})
    inventory_unavailable = report.get("checks", {}).get("runtime", {}).get("state") == "unavailable"
    components = {c["id"]: c for c in report.get("components", [])}
    for name, service in SERVICES.items():
        component = components.get(name, {})
        item = {"probe_status": "not_observed"}
        if "readiness" in component:
            item["readiness"] = component["readiness"]
            item["probe_status"] = "available" if component["readiness"]["status"] == "available" else "unavailable"
        if "runtime" in component or service in inventory:
            item["runtime"] = component.get("runtime", inventory.get(service))
        elif inventory_unavailable:
            item["runtime"] = {"status": "unavailable"}
        if "head" in component:
            item.update(head=component["head"], peer_count=component.get("peer_count"), probe_status="available")
        if component.get("observation_unavailable"):
            item["probe_status"] = "unavailable"
        services[name] = item
    report["observations"] = project({"schema_version": SCHEMA, "observed_at": now(), "services": services,
                                      "incidents": observe_incidents(layout, node)})
    return report
