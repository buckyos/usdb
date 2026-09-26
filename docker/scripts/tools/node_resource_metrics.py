"""Read-only Linux resource evidence; missing counters never become zero usage."""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
from functools import wraps
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time

from control_plane_resources import command, now_ms

PROBES = ContextVar("monitor_resource_probes", default=None)
PROBE_ACTIONS = {("run_testnet_bitcoin.sh", "progress"): "bitcoin",
                 ("run_testnet_bitcoin.sh", "data-progress"): "bitcoin",
                 ("run_testnet_runtime.sh", "data-status"): "balance_history",
                 ("run_testnet_runtime.sh", "indexer-status"): "usdb_indexer"}


@contextmanager
def capture_probes():
    """Measure existing helper calls, including process overhead, without extra RPCs."""
    values = []
    token = PROBES.set(values)
    try:
        yield values
    finally:
        PROBES.reset(token)


def trace_helper(function):
    """Only collect allowlisted operations inside an explicit monitor sample."""
    @wraps(function)
    def measured(layout, helper, arguments, **kwargs):
        values = PROBES.get()
        service = PROBE_ACTIONS.get((helper, arguments[0] if arguments else ""))
        if values is None or service is None:
            return function(layout, helper, arguments, **kwargs)
        started, at, outcome = time.monotonic(), now_ms(), "failed"
        try:
            result = function(layout, helper, arguments, **kwargs)
            if result.returncode == 0:
                try:
                    payload = json.loads(result.stdout or "")
                    if isinstance(payload, dict) and not payload.get("error") and payload.get("rpc_available") is not False:
                        outcome = "ok"
                except (ValueError, TypeError):
                    pass
            return result
        except subprocess.TimeoutExpired:
            outcome = "timeout"
            raise
        finally:
            if len(values) < 32:
                values.append(dict(service=service, operation=arguments[0], started_at_ms=at,
                                   duration_ms=round((time.monotonic() - started) * 1000), outcome=outcome))
    return measured


def trace_chain_rpc(function):
    """Measure a chain RPC batch without retaining URLs, methods' arguments or results."""
    @wraps(function)
    def measured(*args, **kwargs):
        values = PROBES.get()
        if values is None:
            return function(*args, **kwargs)
        started, at, outcome = time.monotonic(), now_ms(), "failed"
        try:
            result = function(*args, **kwargs)
            outcome = "ok"
            return result
        finally:
            if len(values) < 32:
                values.append(dict(service="usdb_chain", operation="rpc-batch", started_at_ms=at,
                                   duration_ms=round((time.monotonic() - started) * 1000), outcome=outcome))
    return measured


def pairs(path):
    return {line.split()[0]: int(line.split()[1]) for line in path.read_text().splitlines()}


def pressure(path, prefix):
    """PSI totals are microseconds; avg10 is percent of wall time stalled."""
    result = {}
    for line in path.read_text().splitlines():
        kind, *fields = line.split()
        if kind not in {"some", "full"}:
            continue
        fields = dict(field.split("=", 1) for field in fields)
        average, total = float(fields["avg10"]), int(fields["total"])
        if not math.isfinite(average) or not 0 <= average <= 100 or total < 0:
            raise ValueError("invalid pressure counters")
        result[f"{prefix}_{kind}_avg10"] = average
        result[f"{prefix}_{kind}_total_usec"] = total
    return result


def number(path):
    value = path.read_text().strip()
    result = None if value == "max" else int(value)
    if result is not None and result < 0:
        raise ValueError("invalid memory counter")
    return result


def attempt(item, label, read):
    """One unsupported kernel field must not discard other valid observations."""
    try:
        item["metrics"].update(read())
    except (OSError, ValueError, KeyError, IndexError):
        item["missing"].append(label)


class Collector:
    """Counter deltas are scoped to boot/container/start/cgroup identity."""

    def __init__(self, proc=Path("/proc"), cgroup=Path("/sys/fs/cgroup")):
        self.proc, self.cgroup = proc, cgroup
        self.previous = {}

    def host(self, summary):
        try:
            boot = (self.proc / "sys/kernel/random/boot_id").read_text().strip()
        except OSError:
            boot = None
        item = dict(service="host", identity=boot, status=summary.get("status", "unavailable"), metrics={}, missing=[])
        item["metrics"].update({key: value for key, value in summary.items()
                                if key in {"memory_total_bytes", "memory_available_bytes", "memory_used_bytes",
                                           "swap_total_bytes", "swap_used_bytes", "cpu_percent"}})
        for kind in ("memory", "io", "cpu"):
            attempt(item, kind + "_pressure", lambda k=kind: pressure(self.proc / "pressure" / k, k + "_psi"))
        attempt(item, "swap_counters", lambda: {key: value * os.sysconf("SC_PAGE_SIZE")
                for key, value in pairs(self.proc / "vmstat").items() if key in {"pswpin", "pswpout"}})
        def cpu():
            values = [int(value) for value in (self.proc / "stat").read_text().splitlines()[0].split()[1:9]]
            return dict(cpu_ticks=sum(values), iowait_ticks=values[4])
        attempt(item, "iowait", cpu)
        return item

    def container(self, item, details):
        result = dict(service=item["service"], identity=None, status=item["status"], metrics={}, missing=[])
        result["metrics"].update({key: value for key, value in item.items()
                                  if key in {"cpu_percent", "memory_limit_bytes"}})
        # Docker's displayed working set excludes some cache; retain its distinct name.
        result["metrics"]["working_set_bytes"] = item.get("memory_used_bytes")
        if details is None:
            result["missing"].append("container_inspect")
            return result
        identifier, pid, restarts, limit, swap_limit, started = details
        result["identity"] = identifier + ":" + started
        result["metrics"].update(restart_count=int(restarts), memory_limit_bytes=int(limit) or None,
                                  memory_swap_limit_bytes=int(swap_limit))
        if int(pid) <= 0:
            result["status"] = "not_running"
            return result
        try:
            entries = (self.proc / pid / "cgroup").read_text().splitlines()
            relative = next(line[3:] for line in entries if line.startswith("0::"))
            directory = (self.cgroup / relative.lstrip("/")).resolve()
            if not directory.is_relative_to(self.cgroup.resolve()):
                raise ValueError("invalid cgroup path")
        except (OSError, ValueError, StopIteration):
            result["missing"].append("cgroup_v2")
            return result
        for file, key in (("memory.current", "memory_current_bytes"), ("memory.max", "memory_limit_bytes"),
                          ("memory.swap.current", "swap_used_bytes"), ("memory.swap.max", "swap_limit_bytes")):
            attempt(result, file, lambda f=file, k=key: {k: number(directory / f)})
        attempt(result, "memory.stat", lambda: {key + "_bytes": value for key, value in pairs(directory / "memory.stat").items()
                                                  if key in {"anon", "file", "inactive_file", "slab"}})
        attempt(result, "memory.events", lambda: {"events_" + key: value for key, value in pairs(directory / "memory.events").items()
                                                    if key in {"high", "max", "oom", "oom_kill"}})
        for kind in ("memory", "io"):
            attempt(result, kind + ".pressure", lambda k=kind: pressure(directory / (k + ".pressure"), k + "_psi"))
        if "memory_current_bytes" in result["metrics"]:
            result["status"] = "available"
        # Confirm the process still belongs to the observed cgroup after reading it.
        try:
            if entries != (self.proc / pid / "cgroup").read_text().splitlines():
                raise ValueError("container changed during collection")
        except (OSError, ValueError):
            result.update(status="unavailable", metrics={}, missing=["container_changed"])
        return result

    def sample(self, resources):
        at = now_ms()
        host = self.host(resources.get("host", {}))
        containers = resources.get("containers", {}).get("items", [])
        details = {}
        identifiers = [item["container_id"] for item in containers if re.fullmatch(r"[0-9a-f]{12,64}", item.get("container_id", ""))]
        if identifiers:
            try:
                output = command(["docker", "inspect", "--format", "{{.Id}}\t{{.State.Pid}}\t{{.RestartCount}}\t{{.HostConfig.Memory}}\t{{.HostConfig.MemorySwap}}\t{{.State.StartedAt}}", *identifiers], 3)
                for line in output.splitlines():
                    fields = line.split("\t")
                    if len(fields) != 6 or not re.fullmatch(r"[0-9a-f]{64}", fields[0]):
                        raise ValueError("invalid container details")
                    for value in fields[1:5]:
                        int(value)
                    details[fields[0][:12]] = fields
            except (OSError, ValueError, subprocess.SubprocessError):
                details = {}
        items = [host] + [self.container(item, details.get(item.get("container_id", "")[:12])) for item in containers]
        current = {}
        for item in items:
            metrics = item["metrics"]
            identity = (host["identity"], item["service"], item["identity"])
            previous = self.previous.get(identity) if all(identity) else None
            current[identity] = (at, dict(metrics))
            if previous and 0 < at - previous[0] <= 120000:
                elapsed = (at - previous[0]) / 1000
                item["interval_ms"] = at - previous[0]
                for key, value in list(metrics.items()):
                    if key.startswith("events_") or key.endswith("_total_usec") or key in {"pswpin", "pswpout", "cpu_ticks", "iowait_ticks"}:
                        before = previous[1].get(key)
                        if type(value) is int and type(before) is int and value >= before:
                            metrics[key + "_delta"] = value - before
                for key, target in (("pswpin", "swap_in_bytes_per_sec"), ("pswpout", "swap_out_bytes_per_sec")):
                    if key + "_delta" in metrics:
                        metrics[target] = metrics[key + "_delta"] / elapsed
                total = metrics.get("cpu_ticks_delta", 0)
                if total > 0 and "iowait_ticks_delta" in metrics:
                    metrics["iowait_percent"] = 100 * metrics["iowait_ticks_delta"] / total
        self.previous = current
        return dict(at_ms=at, boot_id=host["identity"], items=items,
                    containers_status=resources.get("containers", {}).get("status", "unavailable"))


def configuration(env):
    """Explicitly allowlist budgets and caches; never persist environment contents."""
    from resource_policy import SERVICE_MEMORY_KEYS, memory_bytes
    result = {"phase": env.get("USDB_RESOURCE_PHASE") if env.get("USDB_RESOURCE_PHASE") in {"bitcoin", "overlap", "steady"} else None,
              "mode": env.get("USDB_RESOURCE_MODE") if env.get("USDB_RESOURCE_MODE") in {"auto", "manual"} else None,
              "limits": {}, "caches": {}, "invalid": []}
    for service, key in SERVICE_MEMORY_KEYS.items():
        if key in env:
            try:
                result["limits"][service] = memory_bytes(env[key], key)
            except ValueError:
                result["invalid"].append(key)
    for key in ("BTC_DBCACHE_MB", "BH_SYNC_UTXO_MAX_CACHE_BYTES", "BH_SYNC_BALANCE_MAX_CACHE_BYTES"):
        if env.get(key, "").isdigit():
            result["caches"][key] = int(env[key])
    for key in ("USDB_RESOURCE_HOST_MEMORY_BYTES", "USDB_EXTERNAL_MEMORY_BUDGET", "BTC_MEMORY_SWAP_LIMIT", "BH_MEMORY_SWAP_LIMIT"):
        if key in env:
            try:
                result[key] = int(env[key]) if env[key] in {"0", "-1"} else memory_bytes(env[key], key)
            except ValueError:
                result["invalid"].append(key)
    return result
