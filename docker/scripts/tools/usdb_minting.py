#!/usr/bin/env python3
"""Operator-local configuration and read-only capability checks for optional Ord."""

from __future__ import annotations

import json
from pathlib import Path
import time

from ord_runtime import FIELDS, SCHEMA, STATES
from ord_release import IDENTITY, LEGACY_IDENTITY, LEGACY_VERSION, VERSION
from resource_policy import GIB, memory_bytes

GUIDANCE = {
    "DISABLED": "Run usdb-node down, then set-minting --enabled on and up to enable the local backend.",
    "WAITING_CORE": "Waiting for the Bitcoin foreground chain to catch up; USDB startup remains independent.",
    "WAITING_HISTORY": "Waiting for Bitcoin historical validation; do not restart or reload the snapshot.",
    "WAITING_TXINDEX": "Waiting for txindex to cover the foreground tip. If absent, check that Core adopted BTC_TXINDEX=1.",
    "BLOCKED_CONFIG": "Ord requires unpruned Bitcoin mainnet; check the Core configuration.",
    "BLOCKED_DISK": "Free space is below the Ord reserve. Add capacity or free unrelated files; existing indexes are preserved.",
    "STARTING": "Dependencies passed; waiting for the Ord HTTP server.",
    "INDEXING": "Ord is catching up or reconciling the canonical chain. Existing indexes are reused.",
    "READY": "Local indexing backend is ready. Production wallet signing and broadcast are not enabled yet.",
    "UNAVAILABLE": "Cannot establish current readiness; inspect usdb-node logs ord-server and Bitcoin logs.",
    "FAILED": "Ord exited unexpectedly; inspect its logs and container memory/disk limits.",
    "STOPPED": "Ord is stopped; run usdb-node up to start configured services.",
}


def enabled(env):
    value = env.get("USDB_MINTING_ENABLED", "0")
    if value not in {"0", "1"}:
        raise ValueError("USDB_MINTING_ENABLED must be 0 or 1")
    return value == "1"


def data_path(root, version=VERSION):
    return Path(root) / "datasets" / "ord" / "btc-mainnet" / f"ord-{version}"


def environment(root, active, *, legacy_txindex="0"):
    """Keep optional data independent of the USDB network and retain it when disabled."""
    return {"USDB_MINTING_ENABLED": str(int(active)), "BTC_TXINDEX": "1" if active else legacy_txindex,
            "ORD_DATA_HOST_DIR": str(data_path(root)), "ORD_MEMORY_LIMIT": str(4 * GIB),
            "ORD_INDEX_CACHE_BYTES": str(GIB), "ORD_MIN_FREE_BYTES": str(50 * GIB)}


def validate(env, *, require_current=False):
    """Accept the recognized older path for activation, but never for startup."""
    if not enabled(env):
        return
    if env.get("BTC_TXINDEX") != "1":
        raise ValueError("local minting requires BTC_TXINDEX=1 before Bitcoin startup")
    if any(arg.lstrip("-").split("=", 1)[0].removeprefix("no") in {"txindex", "prune"}
           for arg in env.get("BTC_EXTRA_ARGS", "").split()):
        raise ValueError("BTC_EXTRA_ARGS cannot override txindex or pruning for local minting")
    configured = Path(env.get("ORD_DATA_HOST_DIR", ""))
    allowed = {data_path(env["USDB_DATA_ROOT"]), data_path(env["USDB_DATA_ROOT"], LEGACY_VERSION)}
    if configured not in allowed:
        raise ValueError("ORD_DATA_HOST_DIR must use the versioned Bitcoin mainnet dataset")
    if require_current and configured != data_path(env["USDB_DATA_ROOT"]):
        raise ValueError(f"Ord {VERSION} requires a separate index; run usdb-node activate-release while stopped. The old Ord dataset is preserved.")
    limit = memory_bytes(env.get("ORD_MEMORY_LIMIT", "4g"), "ORD_MEMORY_LIMIT")
    for key, default in (("ORD_INDEX_CACHE_BYTES", GIB), ("ORD_MIN_FREE_BYTES", 50 * GIB)):
        if not env.get(key, str(default)).isascii() or not env.get(key, str(default)).isdigit():
            raise ValueError(f"{key} must be positive decimal bytes")
    cache = memory_bytes(env.get("ORD_INDEX_CACHE_BYTES", str(GIB)), "ORD_INDEX_CACHE_BYTES")
    if limit < 2 * GIB or cache > limit // 2:
        raise ValueError("Ord requires at least 2 GiB RAM and cache no larger than half its memory limit")
    memory_bytes(env.get("ORD_MIN_FREE_BYTES", str(50 * GIB)), "ORD_MIN_FREE_BYTES")


def prepare(env):
    """Claim only a new or matching dataset; never replace an existing index."""
    validate(env, require_current=True)
    if not enabled(env):
        return
    root = data_path(env["USDB_DATA_ROOT"])
    if any(path.is_symlink() for path in (root, *root.parents)):
        raise ValueError("Refusing symlinked Ord data directory")
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    marker = root / "identity.json"
    if marker.exists():
        if marker.is_symlink() or json.loads(marker.read_text()) != IDENTITY:
            raise ValueError("Ord dataset identity differs; existing data was preserved")
    else:
        if any(root.iterdir()):
            raise ValueError("Refusing nonempty unmarked Ord dataset; existing data was preserved")
        with marker.open("x", encoding="utf-8") as output:
            json.dump(IDENTITY, output)


def activation_updates(env):
    """Select a fresh versioned index; never open, move or delete the older DB."""
    value = env.get("ORD_DATA_HOST_DIR")
    if not value or Path(value) == data_path(env["USDB_DATA_ROOT"]):
        return {}
    legacy = data_path(env["USDB_DATA_ROOT"], LEGACY_VERSION)
    if Path(value) != legacy:
        raise ValueError("Unknown Ord dataset path; release activation will not migrate it")
    if any(path.is_symlink() for path in (legacy, *legacy.parents)):
        raise ValueError("Refusing symlinked Ord data directory")
    if legacy.exists():
        marker = legacy / "identity.json"
        if marker.is_symlink() or (marker.exists() and json.loads(marker.read_text()) != LEGACY_IDENTITY):
            raise ValueError("Legacy Ord dataset identity differs; existing data was preserved")
        if not marker.exists() and any(legacy.iterdir()):
            raise ValueError("Refusing nonempty unmarked legacy Ord dataset; existing data was preserved")
    return {"ORD_DATA_HOST_DIR": str(data_path(env["USDB_DATA_ROOT"]))}


def progress(env, *, now_ms=None):
    """Old successful observations cannot authorize a capability after a restart."""
    active = enabled(env)
    result = dict(enabled=active, state="UNAVAILABLE" if active else "DISABLED",
                  backend_ready=False, transactions_enabled=False)
    if active:
        if Path(env.get("ORD_DATA_HOST_DIR", "")) != data_path(env["USDB_DATA_ROOT"]):
            return dict(result, state="BLOCKED_CONFIG", guidance=f"Run usdb-node down, then activate-release to select the Ord {VERSION} dataset; the older index is retained.")
        now_ms = int(time.time() * 1000) if now_ms is None else now_ms
        try:
            path = data_path(env["USDB_DATA_ROOT"]) / "progress.json"
            if path.is_symlink() or path.stat().st_size > 8192:
                raise ValueError("invalid Ord observation")
            report = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(report, dict):
                raise ValueError("invalid Ord observation")
            observed = report.get("observed_at_ms")
            if (report.get("schema_version") != SCHEMA or report.get("state") not in STATES
                    or type(observed) is not int or not 0 <= now_ms - observed <= 60_000):
                raise ValueError("stale or invalid Ord observation")
            result.update({key: value for key, value in report.items()
                           if key in FIELDS and (value is None or type(value) in {int, bool})})
            result["state"] = report["state"]
            result["backend_ready"] = (report["state"] == "READY" and report.get("canonical") is True
                                       and report.get("history_validated") is True
                                       and report.get("txindex_synced") is True)
            if report["state"] == "READY" and not result["backend_ready"]:
                result["state"] = "UNAVAILABLE"
        except (OSError, ValueError, TypeError, KeyError):
            pass
    result["guidance"] = GUIDANCE[result["state"]]
    return result


def configure(layout, active, node):
    """Change optional services only while stopped, recalculating all phase budgets."""
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run setup first")
    if any(item.get("state") not in {"exited", "dead", "created"}
           for item in node._collect_compose_services(layout).values()):
        raise ValueError("stop the node with usdb-node down before changing minting support")
    original = layout.node_env.read_text(encoding="utf-8")
    env = node.read_env(layout.node_env)
    # Legacy readiness still requires txindex even without Ord. Native bootstrap
    # can disable it independently without changing BH/indexer startup semantics.
    updates = environment(env["USDB_DATA_ROOT"], active,
                          legacy_txindex="0" if env.get("SNAPSHOT_MODE") == "assumeutxo" else "1")
    updates.update({key: env[key] for key in ("ORD_MEMORY_LIMIT", "ORD_INDEX_CACHE_BYTES", "ORD_MIN_FREE_BYTES") if key in env})
    candidate = {**env, **updates}
    updates.update(node._resource_policy_updates(node.resource_mode(env), candidate))
    prepare({**candidate, **updates})
    try:
        node._atomic_write_private(layout.node_env, node.upsert_env(original, updates))
        node._validate_node_config(layout, require_runtime=False, require_bitcoin_runtime=True)
        node.validate_resource_environment(node.read_env(layout.node_env), node.effective_memory_bytes())
    except BaseException:
        node._atomic_write_private(layout.node_env, original)
        raise
    node._resource_state_path(layout).unlink(missing_ok=True)
