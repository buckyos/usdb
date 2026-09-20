#!/usr/bin/env python3
"""Supervise the optional, private Ord index without gating USDB startup."""

from __future__ import annotations

import base64
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import threading
import time
import urllib.request

SCHEMA = "usdb-ord-progress:v1"
STATES = {"WAITING_CORE", "WAITING_HISTORY", "WAITING_TXINDEX", "BLOCKED_DISK",
          "BLOCKED_CONFIG", "STARTING", "INDEXING", "READY", "UNAVAILABLE", "FAILED", "STOPPED"}
FIELDS = ("state", "observed_at_ms", "core_height", "history_height", "history_validated",
          "txindex_height", "txindex_synced", "ord_height", "ord_gap", "anchor_height",
          "canonical", "disk_free_bytes", "disk_required_bytes", "index_file_bytes")


def height(value):
    """Reject booleans and malformed RPC quantities rather than implying progress."""
    if type(value) is not int or value < 0:
        raise ValueError("invalid height")
    return value


def block_hash(value):
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
        raise ValueError("invalid block hash")
    return value


def prerequisites(info, chains, indexes, *, now, max_tip_age=7200):
    """A synced index may still follow AssumeUTXO's historical chainstate."""
    if not all(isinstance(item, dict) for item in (info, chains, indexes)):
        raise ValueError("invalid Core response")
    tip = height(info["blocks"])
    head = block_hash(info["bestblockhash"])
    states = chains["chainstates"]
    if not isinstance(states, list) or not states or not all(isinstance(item, dict) for item in states):
        raise ValueError("missing chainstates")
    validated = [item for item in states if item.get("validated") is True]
    history = max((height(item["blocks"]) for item in validated), default=None)
    complete = any(item.get("bestblockhash") == head and item["blocks"] == tip for item in validated)
    index = indexes.get("txindex", {})
    if not isinstance(index, dict):
        raise ValueError("invalid txindex response")
    indexed = height(index["best_block_height"]) if index else None
    result = dict(core_height=tip, history_height=history, history_validated=complete,
                  txindex_height=indexed, txindex_synced=index.get("synced") is True)
    if info.get("chain") != "main" or info.get("pruned") is not False:
        return dict(result, state="BLOCKED_CONFIG")
    if (info.get("initialblockdownload") is not False or tip < height(info["headers"])
            or now - height(info["time"]) > max_tip_age):
        return dict(result, state="WAITING_CORE")
    if not complete:
        return dict(result, state="WAITING_HISTORY")
    if indexed is None or index.get("synced") is not True or indexed < tip:
        return dict(result, state="WAITING_TXINDEX")
    return dict(result, state="STARTING", anchor_height=tip, anchor_hash=head)


def read_json(request):
    """Bound every local dependency probe, and never publish its raw error text."""
    with urllib.request.urlopen(request, timeout=4) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("RPC response too large")
    return json.loads(data)


def rpc(method, params=None):
    token = base64.b64encode(f"{os.environ['BTC_RPC_USER']}:{os.environ['BTC_RPC_PASSWORD']}".encode()).decode()
    request = urllib.request.Request(os.environ.get("BTC_RPC_URL", "http://btc-node:8332"),
        data=json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params or [])).encode(),
        headers={"Authorization": "Basic " + token, "Content-Type": "application/json"})
    result = read_json(request)
    if not isinstance(result, dict) or result.get("error") or result.get("id") != 1 or "result" not in result:
        raise ValueError("RPC request failed")
    return result["result"]


def observe_core():
    report = prerequisites(rpc("getblockchaininfo"), rpc("getchainstates"), rpc("getindexinfo"),
                           now=time.time(), max_tip_age=int(os.environ.get("BTC_MAX_TIP_AGE_SECS", "7200")))
    if (report["state"] == "STARTING" and height(rpc("getnetworkinfo")["connections"])
            < int(os.environ.get("BTC_MIN_CONNECTIONS", "1"))):
        report["state"] = "WAITING_CORE"
    return report


def ord_heights(core, fetch=read_json):
    """Continue reporting indexing progress while upstream readiness is revoked."""
    result = dict(core)
    count = height(fetch("http://127.0.0.1:28030/blockcount"))
    result.update(ord_height=count - 1 if count else None, canonical=False)
    if core.get("core_height") is not None:
        result["ord_gap"] = max(0, core["core_height"] + 1 - count)
    return result


def observe_ord(core, fetch=read_json, core_rpc=rpc):
    """Prove coverage of a canonical sampled anchor; blockcount includes genesis."""
    result = dict(ord_heights(core, fetch), state="INDEXING")
    if result["ord_height"] is None or result["ord_height"] < core["anchor_height"]:
        return result
    anchor = core["anchor_height"]
    observed = block_hash(fetch(f"http://127.0.0.1:28030/r/blockhash/{anchor}"))
    canonical = block_hash(core_rpc("getblockhash", [anchor]))
    if observed == canonical == core["anchor_hash"]:
        result.update(canonical=True, state="READY")
    return result


def ord_command(root):
    """Only inscriptions and addresses are indexed; no redundant transaction/sat index."""
    return ["/opt/ord/bin/ord", "--chain", "mainnet", "--data-dir", str(root),
            "--index-addresses", "--index-cache-size", os.environ.get("ORD_INDEX_CACHE_BYTES", "1073741824"),
            "server", "--address", "0.0.0.0", "--http", "--http-port", "28030"]


def publish(root, report):
    report = {key: value for key, value in report.items() if key in FIELDS}
    report.update(schema_version=SCHEMA, observed_at_ms=int(time.time() * 1000))
    temporary = root / "progress.json.tmp"
    temporary.write_text(json.dumps(report) + "\n", encoding="utf-8")
    temporary.replace(root / "progress.json")


def stop_child(child):
    """Allow Ord to flush its database on dependency loss and operator shutdown."""
    if child is not None and child.poll() is None:
        child.send_signal(signal.SIGINT)
        child.wait()


def supervise():
    root = Path(os.environ.get("ORD_DATA_DIR", "/data/ord"))
    root.mkdir(parents=True, exist_ok=True)
    required = int(os.environ.get("ORD_MIN_FREE_BYTES", str(50 * 1024**3)))
    stopped = threading.Event()
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda *_: stopped.set())
    child = None
    previous = None
    failed = False
    # Credentials stay out of process arguments and progress/log projections.
    child_env = dict(os.environ, ORD_BITCOIN_RPC_URL=os.environ.get("BTC_RPC_URL", "http://btc-node:8332"),
                     ORD_BITCOIN_RPC_USERNAME=os.environ["BTC_RPC_USER"],
                     ORD_BITCOIN_RPC_PASSWORD=os.environ["BTC_RPC_PASSWORD"])
    try:
        while not stopped.is_set():
            report = dict(state="UNAVAILABLE")
            try:
                try:
                    report = observe_core()
                except (OSError, ValueError, KeyError, TypeError):
                    pass
                free = shutil.disk_usage(root).free
                index = root / "index.redb"
                report.update(disk_free_bytes=free, disk_required_bytes=required,
                              index_file_bytes=index.stat().st_size if index.exists() else 0)
                if free < required:
                    report["state"] = "BLOCKED_DISK"
                if child is not None and child.poll() is not None:
                    report["state"] = "FAILED"
                    publish(root, report)
                    failed = True
                    return 1
                if report["state"] == "STARTING":
                    if child is None:
                        child = subprocess.Popen(ord_command(root), env=child_env)
                    try:
                        report = observe_ord(report)
                    except (OSError, ValueError, KeyError, TypeError):
                        report["state"] = "STARTING"
                elif report["state"] in {"BLOCKED_DISK", "BLOCKED_CONFIG"}:
                    publish(root, report)
                    stop_child(child)
                    child = None
                elif child is not None:
                    # A new block, transient RPC outage or Core restart must not
                    # repeatedly discard a long-running Ord indexing batch.
                    try:
                        report = ord_heights(report)
                    except (OSError, ValueError, KeyError, TypeError):
                        pass
            except (OSError, ValueError, KeyError, TypeError):
                report["state"] = "UNAVAILABLE"
            if report["state"] != previous:
                print(f"Ord state: {previous or 'INIT'} -> {report['state']}", flush=True)
                previous = report["state"]
            publish(root, report)
            stopped.wait(10)
    finally:
        if not failed:
            publish(root, dict(state="STOPPED"))
        stop_child(child)
    return 0


if __name__ == "__main__":
    raise SystemExit(supervise())
