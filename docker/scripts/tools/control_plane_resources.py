#!/usr/bin/env python3
"""Bounded host resource observations for the authenticated, private console."""

from __future__ import annotations

import copy
import json
import math
import os
from pathlib import Path
import re
import subprocess
import threading
import time

GIB = 1024**3
DISK_SCAN_INTERVAL = 300
DIRECTORIES = {
    "bitcoin": "BTC_NODE_DATA_HOST_DIR", "balance-history": "BH_DATA_HOST_DIR",
    "usdb-indexer": "USDB_INDEXER_DATA_HOST_DIR", "usdb-chain": "USDB_CHAIN_DATA_HOST_DIR",
    "ord": "ORD_DATA_HOST_DIR", "control-plane": "CONTROL_PLANE_DATA_HOST_DIR",
    "utxo-artifacts": "BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR",
}
SERVICES = {"btc-node", "btc-snapshot-bootstrap", "balance-history", "usdb-indexer",
            "usdb-chain", "ord-server", "usdb-control-plane", "usdb-checkpoint-verify"}
CONTAINER_DATA = {
    "bitcoin": ("btc-node", "/data/bitcoin"), "balance-history": ("balance-history", "/data/balance-history"),
    "usdb-indexer": ("usdb-indexer", "/data/usdb-indexer"), "usdb-chain": ("usdb-chain", "/data/usdb-chain"),
    "ord": ("ord-server", "/data/ord"), "control-plane": ("usdb-control-plane", "/data/usdb-control-plane"),
}


def now_ms():
    return int(time.time() * 1000)


def parse_size(value):
    """Decode Docker's human-readable working-set memory without treating failure as zero."""
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)\s*(B|[KMGTPE]i?B)", value.strip(), re.I)
    if not match:
        raise ValueError("invalid Docker size")
    unit = match[2].upper()
    power = 0 if unit == "B" else "KMGTPE".index(unit[0]) + 1
    result = float(match[1]) * (1024 if "I" in unit else 1000)**power
    if not math.isfinite(result):
        raise ValueError("invalid Docker size")
    return int(result)


def percent(value):
    result = float(value.removesuffix("%"))
    if not math.isfinite(result) or result < 0:
        raise ValueError("invalid utilization")
    return result


def cpu_counters(proc=Path("/proc")):
    values = [int(item) for item in (proc / "stat").read_text().splitlines()[0].split()[1:9]]
    if len(values) < 5 or any(item < 0 for item in values):
        raise ValueError("invalid CPU counters")
    return sum(values), values[3] + values[4]


def cpu_percent(before, after):
    total, idle = after[0] - before[0], after[1] - before[1]
    if total <= 0 or idle < 0 or idle > total:
        return None
    return round(100 * (total - idle) / total, 2)


def host_memory(proc=Path("/proc")):
    values = {}
    for line in (proc / "meminfo").read_text().splitlines():
        key, value = line.split(":", 1)
        if key in {"MemTotal", "MemAvailable", "SwapTotal", "SwapFree"}:
            values[key] = int(value.split()[0]) * 1024
    total, available = values["MemTotal"], values["MemAvailable"]
    if not 0 <= available <= total or total == 0:
        raise ValueError("invalid host memory")
    return dict(memory_total_bytes=total, memory_available_bytes=available,
                memory_used_bytes=total - available, memory_used_percent=round(100 * (total - available) / total, 2),
                swap_total_bytes=values.get("SwapTotal"),
                swap_used_bytes=values["SwapTotal"] - values["SwapFree"] if "SwapFree" in values and "SwapTotal" in values else None)


def command(args, timeout):
    """Commands never contain credentials; raw failures are not sent to the UI."""
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=timeout,
                          env=dict(os.environ, LC_ALL="C")).stdout


def container_stats(bundle_id):
    """Restrict discovery to the node's two Compose projects, including stopped services."""
    containers = {}
    for project in (bundle_id, bundle_id + "-bitcoin"):
        output = command(["docker", "ps", "--all", "--filter", "label=com.docker.compose.project=" + project,
                          "--format", '{{.ID}}\t{{.Label "com.docker.compose.service"}}\t{{.State}}'], 3)
        for line in output.splitlines():
            identifier, service, state = line.split("\t")
            if service not in SERVICES:
                continue
            if not re.fullmatch(r"[0-9a-f]{12,64}", identifier) or len(containers) >= 32:
                raise ValueError("invalid container inventory")
            containers[identifier] = dict(container_id=identifier, service=service, state=state, status="not_running" if state != "running" else "unavailable")
    running = [key for key, value in containers.items() if value["state"] == "running"]
    if running:
        try:
            output = command(["docker", "stats", "--no-stream", "--format", "{{json .}}", *running], 5)
        except (OSError, subprocess.SubprocessError):
            # Keep identities for independent cgroup inspection when stats stalls.
            return dict(status="unavailable", observed_at_ms=now_ms(), items=list(containers.values()))
        for line in output.splitlines():
            value = json.loads(line)
            item = containers.get(value.get("ID"))
            if item is None:
                raise ValueError("unexpected container statistics")
            used, limit = value["MemUsage"].split("/")
            item.update(status="available", cpu_percent=percent(value["CPUPerc"]),
                        memory_used_bytes=parse_size(used), memory_limit_bytes=parse_size(limit))
    return dict(status="available", observed_at_ms=now_ms(), items=list(containers.values()))


def directory_paths(env):
    """Only explicitly allowed data paths are exposed, never general configuration or file contents."""
    result = []
    for service, key in DIRECTORIES.items():
        if service == "ord" and env.get("USDB_MINTING_ENABLED") != "1":
            continue
        value = env.get(key)
        if not value:
            continue
        if len(value) > 4096 or any(ord(c) < 32 for c in value) or not Path(value).is_absolute():
            result.append(dict(service=service, status="invalid_path"))
            continue
        try:
            path = Path(value).resolve()
            if path == Path("/"):
                raise ValueError("refusing filesystem root scan")
            result.append(dict(service=service, path=str(path)))
        except (OSError, ValueError, RuntimeError):
            result.append(dict(service=service, status="invalid_path"))
    return result


def capacity_warning(total, available):
    """Advisory only; this must never drive node readiness or stop services."""
    if available < max(50 * GIB, total * 0.05):
        return "critical"
    if available < max(100 * GIB, total * 0.10):
        return "warning"
    return "ok"


def filesystem_capacity(path):
    """Measure the containing filesystem, even if a configured directory is not created yet."""
    anchor = Path(path)
    while not anchor.exists() and anchor != anchor.parent:
        anchor = anchor.parent
    device = anchor.stat().st_dev
    mount = anchor
    while mount != mount.parent and mount.parent.stat().st_dev == device:
        mount = mount.parent
    stats = os.statvfs(anchor)
    total, available = stats.f_blocks * stats.f_frsize, stats.f_bavail * stats.f_frsize
    return dict(id=str(device), mount_path=str(mount), status="available", total_bytes=total,
                used_bytes=(stats.f_blocks - stats.f_bfree) * stats.f_frsize, available_bytes=available,
                warning=capacity_warning(total, available), observed_at_ms=now_ms())


def directory_size(path):
    """Bound metadata walks; nonzero du exits are partial observations, never valid totals."""
    try:
        if not Path(path).is_dir():
            return dict(status="missing", checked_at_ms=now_ms())
        args = ["du", "-s", "-x", "-B1", "--", path]
        # Give foreground synchronization priority when Linux scheduling tools exist.
        import shutil
        if shutil.which("ionice"):
            args = ["ionice", "-c", "3", *args]
        if shutil.which("nice"):
            args = ["nice", "-n", "19", *args]
        output = command(args, 4)
        size = int(output.split("\t", 1)[0])
        if size < 0:
            raise ValueError("invalid directory size")
        return dict(status="available", used_bytes=size, observed_at_ms=now_ms(), checked_at_ms=now_ms())
    except subprocess.TimeoutExpired:
        return dict(status="timeout", checked_at_ms=now_ms())
    except subprocess.CalledProcessError as error:
        return dict(status="permission_denied" if "Permission denied" in (error.stderr or "") else "unavailable",
                    checked_at_ms=now_ms())
    except (OSError, ValueError, subprocess.SubprocessError):
        return dict(status="unavailable", checked_at_ms=now_ms())


def container_directory_size(path, service, bundle_id):
    """Read a protected data mount as its running service user, after verifying its exact mapping."""
    if service not in CONTAINER_DATA:
        return dict(status="permission_denied", checked_at_ms=now_ms())
    name, destination = CONTAINER_DATA[service]
    project = bundle_id + "-bitcoin" if service == "bitcoin" else bundle_id
    try:
        identifiers = command(["docker", "ps", "--filter", "label=com.docker.compose.project=" + project,
                               "--filter", "label=com.docker.compose.service=" + name, "--format", "{{.ID}}"], 1).split()
        if len(identifiers) != 1 or not re.fullmatch(r"[0-9a-f]{12,64}", identifiers[0]):
            raise ValueError("no unique running data owner")
        mounts = json.loads(command(["docker", "inspect", "--format", "{{json .Mounts}}", identifiers[0]], 1))
        if not any(item.get("Type") == "bind" and item.get("Destination") == destination
                   and str(Path(item.get("Source", "")).resolve()) == path for item in mounts):
            raise ValueError("data mount mismatch")
        # Use the container's configured user, never override it or add a mount.
        output = command(["docker", "exec", identifiers[0], "timeout", "4", "du", "-s", "-k", "-x", "--", destination], 5)
        size = int(output.split("\t", 1)[0]) * 1024
        if size < 0:
            raise ValueError("invalid directory size")
        return dict(status="available", used_bytes=size, observed_at_ms=now_ms(), checked_at_ms=now_ms())
    except subprocess.TimeoutExpired:
        return dict(status="timeout", checked_at_ms=now_ms())
    except subprocess.CalledProcessError as error:
        return dict(status="timeout" if error.returncode in {124, 137, 143} else "permission_denied", checked_at_ms=now_ms())
    except (OSError, ValueError, TypeError, AttributeError, subprocess.SubprocessError):
        return dict(status="permission_denied", checked_at_ms=now_ms())


class ResourceCollector:
    """Keep slow directory walks off the observation heartbeat, with one bounded worker."""

    def __init__(self):
        self.previous_cpu = None
        self.previous_cpu_time = None
        self.disk_cache = {}
        self.disk_identity = None
        self.disk_last_started = None
        self.worker = None
        self.lock = threading.Lock()

    def _scan(self, paths, bundle_id=None, services=None):
        for path in paths:
            result = directory_size(path)
            if result["status"] == "permission_denied" and bundle_id and services:
                result = container_directory_size(path, services.get(path), bundle_id)
            with self.lock:
                previous = self.disk_cache.get(path, {})
                if result["status"] != "available" and "used_bytes" in previous:
                    result.update(used_bytes=previous["used_bytes"], observed_at_ms=previous["observed_at_ms"])
                self.disk_cache[path] = result

    def sample(self, env, bundle_id, *, wait_for_disk=False):
        """Return independent host, container and filesystem results without changing services."""
        result = dict(schema_version="usdb-console-resources:v1", observed_at_ms=now_ms())
        try:
            first = cpu_counters()
            started = time.monotonic()
            if self.previous_cpu is None:
                self.previous_cpu, self.previous_cpu_time = first, started
                time.sleep(0.15)
                first, started = cpu_counters(), time.monotonic()
            result["host"] = dict(status="available", observed_at_ms=now_ms(), **host_memory(),
                                  cpu_percent=cpu_percent(self.previous_cpu, first), cpu_count=os.cpu_count(),
                                  cpu_interval_ms=round((started - self.previous_cpu_time) * 1000))
            self.previous_cpu, self.previous_cpu_time = first, started
        except (OSError, ValueError, KeyError):
            result["host"] = dict(status="unavailable")
        try:
            result["containers"] = container_stats(bundle_id)
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            result["containers"] = dict(status="unavailable", items=[])
        directories = directory_paths(env)
        paths = tuple(sorted({item["path"] for item in directories if "path" in item}))
        if self.worker is None or not self.worker.is_alive():
            if paths != self.disk_identity or self.disk_last_started is None or time.monotonic() - self.disk_last_started >= DISK_SCAN_INTERVAL:
                self.disk_identity, self.disk_last_started = paths, time.monotonic()
                self.worker = threading.Thread(target=self._scan, args=(paths, bundle_id, {item["path"]: item["service"] for item in directories if "path" in item}), daemon=True)
                self.worker.start()
        if wait_for_disk and self.worker:
            self.worker.join(timeout=len(paths) * 11 + 2)
        filesystems = {}
        with self.lock:
            sizes = copy.deepcopy(self.disk_cache)
        for item in directories:
            if "path" not in item:
                continue
            item.update(sizes.get(item["path"], dict(status="pending")))
            try:
                capacity = filesystem_capacity(item["path"])
                filesystems[capacity["id"]] = capacity
                item["filesystem_id"] = capacity["id"]
            except (OSError, ValueError):
                item["filesystem_status"] = "unavailable"
        result.update(directories=directories, filesystems=list(filesystems.values()),
                      disk_scan_in_progress=bool(self.worker and self.worker.is_alive()), disk_scan_interval_secs=DISK_SCAN_INTERVAL)
        return result
