"""Bounded, read-only process inventory, independent of startup and RPC readiness."""

from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
from types import SimpleNamespace

from control_plane_resources import parse_size
from node_observation import quantity, timestamp


SCHEMA = "usdb-node-top:v1"
TIMEOUT = 5
SERVICES = ("btc-node", "balance-history", "usdb-indexer", "usdb-chain", "usdb-control-plane", "ord-server")
JOBS = ("btc-snapshot-bootstrap", "snapshot-loader", "script-registry-installer", "usdb-chain-init",
        "paired-checkpoint-recovery")
LIVE = {"RUNNING", "PAUSED", "RESTARTING", "STARTING", "STOPPING", "REMOVING"}
PROPERTIES = ("Id", "LoadState", "ActiveState", "SubState", "MainPID", "UnitFileState", "Result",
              "ExecMainCode", "ExecMainStatus", "ExecMainStartTimestampMonotonic", "NRestarts",
              "MemoryCurrent", "MemoryMax")
LIST_FORMAT = ('{"id":{{json .ID}},"service":{{json (.Label "com.docker.compose.service")}},'
               '"project":{{json (.Label "com.docker.compose.project")}}}')
INSPECT_FORMAT = ('{"id":{{json .Id}},"service":{{json (index .Config.Labels "com.docker.compose.service")}},'
                  '"project":{{json (index .Config.Labels "com.docker.compose.project")}},'
                  '"state":{{json .State.Status}},"pid":{{json .State.Pid}},'
                  '"started_at":{{json .State.StartedAt}},"finished_at":{{json .State.FinishedAt}},'
                  '"exit_code":{{json .State.ExitCode}},"oom_killed":{{json .State.OOMKilled}},'
                  '"restart_count":{{json .RestartCount}},"memory_limit_bytes":{{json .HostConfig.Memory}},'
                  '"health":{{if .State.Health}}{{json .State.Health.Status}}{{else}}null{{end}}}')


def _command(args):
    """Never invoke Compose interpolation, shell evaluation, sudo or service mutations."""
    return subprocess.run(args, capture_output=True, text=True, check=False, timeout=TIMEOUT,
                          env={**os.environ, "LC_ALL": "C"})


def _row(service, manager, *, enabled=None, kind="service", **values):
    return dict(service=service, manager=manager, kind=kind, configured_enabled=enabled,
                state="UNKNOWN", observation="unavailable", host_pid=None, uptime_secs=None,
                restart_count=None, exit_code=None, exit_signal=None, oom_killed=None,
                started_at=None, finished_at=None, memory_used_bytes=None, memory_limit_bytes=None,
                memory_source=None, **values)


def _number(value):
    """systemd uses UINT64_MAX / [not set] for unavailable accounting values."""
    return int(value) if isinstance(value, str) and value.isascii() and value.isdigit() and int(value) < 2**64 - 1 else None


def _systemd(service, unit, enabled):
    row = _row(service, "systemd", enabled=enabled, unit=unit)
    try:
        result = _command(["systemctl", "show", "--no-pager", "--property=" + ",".join(PROPERTIES), unit])
        props = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        if props.get("Id") != unit or not all(key in props for key in ("LoadState", "ActiveState", "SubState", "MainPID")):
            raise ValueError("incomplete systemd observation")
        if props["LoadState"] == "not-found":
            row.update(state="NOT_INSTALLED", observation="available")
            return [row], []
        if result.returncode or props["LoadState"] not in {"loaded", "masked", "error", "bad-setting"}:
            raise ValueError("systemd query failed")
        active, sub = props["ActiveState"], props["SubState"]
        pid = _number(props["MainPID"])
        if active not in {"active", "inactive", "failed", "activating", "deactivating", "reloading", "maintenance"} or pid is None:
            raise ValueError("invalid systemd state")
        state = {"inactive": "STOPPED", "failed": "FAILED", "activating": "STARTING",
                 "deactivating": "STOPPING", "reloading": "RELOADING", "maintenance": "MAINTENANCE"}.get(active, "ACTIVE")
        if sub == "auto-restart":
            state = "RESTARTING"
        elif active == "active":
            state = "RUNNING" if pid else "COMPLETED" if sub == "exited" else "ACTIVE"
        if state == "STOPPED" and enabled is False:
            state = "DISABLED"
        row.update(state=state, observation="available", host_pid=pid or None,
                   active_state=active, sub_state=sub, load_state=props["LoadState"],
                   unit_file_state=props.get("UnitFileState"), result=props.get("Result"),
                   restart_count=_number(props.get("NRestarts")))
        exit_kind, exit_status = _number(props.get("ExecMainCode")), _number(props.get("ExecMainStatus"))
        # A signal is not an exit code; a default zero before the first start is not a successful run.
        if not pid and exit_kind in (1, 2, 3):
            row["exit_code" if exit_kind == 1 else "exit_signal"] = exit_status
        if props.get("Result") == "oom-kill":
            row["oom_killed"] = True
        if pid:
            start = _number(props.get("ExecMainStartTimestampMonotonic"))
            now = time.monotonic_ns() // 1000
            if start and start <= now:
                row["uptime_secs"] = (now - start) // 1_000_000
            row.update(memory_used_bytes=_number(props.get("MemoryCurrent")),
                       memory_limit_bytes=_number(props.get("MemoryMax")), memory_source="systemd-cgroup")
        return [row], []
    except (OSError, ValueError, subprocess.SubprocessError):
        return [row], [f"{service}: systemd observation unavailable; inspect usdb-node {service} status."]


def _enabled(service, env):
    if env is None:
        return None
    if service == "ord-server":
        return env.get("USDB_MINTING_ENABLED", "0") == "1"
    if service == "monitor":
        return env.get("USDB_MONITOR_ENABLED", "1") == "1"
    if service == "script-registry-installer":
        return env.get("SNAPSHOT_MODE") != "assumeutxo" and env.get("BH_SCRIPT_REGISTRY_ENABLED", "1") == "1"
    if service in {"snapshot-loader", "paired-checkpoint-recovery"}:
        return env.get("SNAPSHOT_MODE") != "assumeutxo"
    if service == "btc-snapshot-bootstrap":
        return env.get("SNAPSHOT_MODE") == "assumeutxo"
    return True


def _container(row, value, observed_at):
    """Project only process evidence; never include environment, command lines or health logs."""
    state = value.get("state", "").upper()
    if state not in {"CREATED", "RUNNING", "PAUSED", "RESTARTING", "REMOVING", "EXITED", "DEAD"}:
        raise ValueError("invalid Docker state")
    pid = quantity(value.get("pid"))
    if pid is None:
        raise ValueError("invalid Docker PID")
    row.update(state=state, observation="available", host_pid=pid or None,
               started_at=timestamp(value.get("started_at")), finished_at=timestamp(value.get("finished_at")),
               restart_count=quantity(value.get("restart_count")),
               oom_killed=value.get("oom_killed") if type(value.get("oom_killed")) is bool else None,
               memory_limit_bytes=quantity(value.get("memory_limit_bytes")),
               health=value.get("health") if value.get("health") in {"healthy", "unhealthy", "starting"} else None)
    if state in {"EXITED", "DEAD", "RESTARTING"}:
        row["exit_code"] = quantity(value.get("exit_code"))
    if pid and row["started_at"]:
        age = (observed_at - datetime.fromisoformat(row["started_at"])).total_seconds()
        if age >= 0:
            row["uptime_secs"] = int(age)


def _docker(project, expected, env, observed_at):
    """Keep project failures independent and retain duplicate/replaced containers as distinct rows."""
    rows = []
    errors = []
    try:
        result = _command(["docker", "ps", "--all", "--no-trunc", "--filter",
                           "label=com.docker.compose.project=" + project, "--format", LIST_FORMAT])
        if result.returncode:
            raise ValueError("Docker listing failed")
        inventory = {}
        for line in result.stdout.splitlines():
            item = json.loads(line)
            identifier, service = item["id"], item["service"]
            if (item.get("project") != project or not isinstance(identifier, str)
                    or not re.fullmatch(r"[0-9a-f]{64}", identifier) or identifier in inventory
                    or not isinstance(service, str) or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}", service)
                    or len(inventory) >= 64):
                raise ValueError("invalid Docker inventory")
            inventory[identifier] = _row(service, "docker", enabled=_enabled(service, env),
                kind="service" if service in SERVICES else "job", project=project, container_id=identifier)
        rows = list(inventory.values())
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.SubprocessError):
        return [_row(name, "docker", enabled=_enabled(name, env), project=project,
                     kind="service" if name in SERVICES else "job") for name in expected], [
                         f"{project}: Docker inventory unavailable; check the daemon and socket permissions."]
    found = {row["service"] for row in rows}
    for service in expected:
        if service not in found:
            row = _row(service, "docker", enabled=_enabled(service, env), project=project,
                       kind="service" if service in SERVICES else "job")
            row.update(state="DISABLED" if row["configured_enabled"] is False else "NOT_CREATED", observation="available")
            rows.append(row)
    if not inventory:
        return rows, errors
    # A removal between ps and inspect can give nonzero status with valid results for other IDs.
    try:
        result = _command(["docker", "inspect", "--format", INSPECT_FORMAT, *inventory])
        for line in result.stdout.splitlines():
            try:
                value = json.loads(line)
                row = inventory[value["id"]]
                if value.get("project") != project or value.get("service") != row["service"]:
                    continue
                _container(row, value, observed_at)
            except (ValueError, KeyError, TypeError, AttributeError):
                continue
    except (OSError, subprocess.SubprocessError):
        pass
    if any(row["observation"] != "available" for row in inventory.values()):
        errors.append(f"{project}: some container details are unavailable or changed during observation.")
    running = {identifier: row for identifier, row in inventory.items() if row["state"] == "RUNNING"}
    if running:
        try:
            result = _command(["docker", "stats", "--no-stream", "--no-trunc", "--format", "{{json .}}", *running])
            for line in result.stdout.splitlines():
                try:
                    item = json.loads(line)
                    row = running[item["ID"]]
                    row.update(memory_used_bytes=parse_size(item["MemUsage"].split("/", 1)[0]),
                               memory_source="docker-working-set")
                except (ValueError, KeyError, TypeError, AttributeError):
                    continue
        except (OSError, subprocess.SubprocessError):
            pass  # Missing optional memory accounting never hides known process state.
    return rows, errors


def _settings(path, node):
    """A broken or absent node.env must not prevent host process inspection."""
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > 1024 * 1024:
            raise ValueError("unsupported configuration")
        return node.read_env(path), "available"
    except FileNotFoundError:
        return None, "missing"
    except (OSError, ValueError):
        return None, "unavailable"


def collect(layout, node, *, include_all=False):
    """Observe only this bundle's exact systemd units and two Compose projects."""
    import node_monitor
    observed_at = datetime.now(timezone.utc)
    env, config_state = _settings(layout.node_env, node)
    tasks = [(_systemd, ("controller", node.controller_unit_name(layout), None)),
             (_systemd, ("monitor", node_monitor.unit_name(layout), _enabled("monitor", env))),
             (_docker, (layout.bundle_id + "-bitcoin", ("btc-node", "btc-snapshot-bootstrap"), env, observed_at)),
             (_docker, (layout.bundle_id, (*SERVICES[1:], *JOBS[1:]), env, observed_at))]
    rows, errors = [], []
    # Slow Docker must not suppress independent systemd observations (and vice versa).
    with ThreadPoolExecutor(max_workers=4) as pool:
        pending = [pool.submit(function, *args) for function, args in tasks]
        for future in pending:
            items, failures = future.result()
            rows.extend(items)
            errors.extend(failures)
    order = {name: i for i, name in enumerate(("controller", "monitor", *SERVICES, *JOBS))}
    rows.sort(key=lambda row: (order.get(row["service"], len(order)), row.get("container_id", "")))
    visible = [row for row in rows if include_all or row["kind"] == "service" or row["state"] in LIVE
               or row["state"] in {"UNKNOWN", "DEAD"} and row.get("container_id")
               or row["exit_code"] not in (None, 0) or row["oom_killed"] is True]
    notes = ["Process state only; use usdb-node status for synchronization and readiness.",
             "HOST PID is the service main process on its host; containers may also run child processes."]
    if config_state != "available":
        notes.append("node.env is missing or unreadable; configured enablement is unknown. Process observations remain independent.")
    return dict(schema_version=SCHEMA, observed_at=observed_at.isoformat(timespec="seconds"),
                release_id=layout.release_id, network_bundle_id=layout.bundle_id,
                configuration=config_state, observation="partial" if errors else "available",
                services=visible, hidden_jobs=len(rows) - len(visible), errors=errors, notes=notes)


def _safe(value):
    return "".join(char if char.isprintable() else "?" for char in str(value))


def _duration(seconds):
    if seconds is None:
        return "-"
    if seconds >= 86400:
        return f"{seconds // 86400}d{seconds % 86400 // 3600}h"
    if seconds >= 3600:
        return f"{seconds // 3600}h{seconds % 3600 // 60}m"
    return f"{seconds // 60}m{seconds % 60}s"


def _size(value):
    if value is None:
        return "-"
    for unit in ("B", "KiB", "MiB", "GiB", "TiB", "PiB"):
        if value < 1024 or unit == "PiB":
            return f"{value:.1f}{unit}"
        value /= 1024


def render(report):
    """Keep stopped/missing/unknown processes visible without dependency interpretations."""
    table = [("SERVICE", "MANAGER", "STATE", "HOST PID", "UPTIME", "RESTARTS", "EXIT", "MEM USED / LIMIT")]
    notes = []
    for row in report["services"]:
        limit = "none" if row["memory_limit_bytes"] == 0 else _size(row["memory_limit_bytes"])
        exit_value = str(row["exit_code"]) if row["exit_code"] is not None else f"sig:{row['exit_signal']}" if row["exit_signal"] else "-"
        table.append((row["service"], row["manager"], row["state"], str(row["host_pid"] or "-"),
                      _duration(row["uptime_secs"]), str(row["restart_count"]) if row["restart_count"] is not None else "-",
                      exit_value, f"{_size(row['memory_used_bytes'])} / {limit}"))
        reasons = []
        if row["oom_killed"] is True:
            reasons.append("OOMKilled=true")
        if row["configured_enabled"] is False and row["state"] in LIVE:
            reasons.append("configured disabled, but process is present")
        if row.get("health"):
            reasons.append("container health=" + row["health"])
        if row.get("result") and row["result"] != "success":
            reasons.append("systemd result=" + _safe(row["result"]))
        if row.get("load_state") in {"masked", "error", "bad-setting"}:
            reasons.append("systemd load=" + row["load_state"])
        if reasons:
            notes.append(f"{row['service']}: " + "; ".join(reasons))
    widths = [max(len(_safe(row[i])) for row in table) for i in range(len(table[0]))]
    lines = [f"USDB processes | network={report['network_bundle_id']} | tool={report['release_id']}",
             f"Observed {report['observed_at']} | observation={report['observation']}", ""]
    lines += ["  ".join(_safe(value).ljust(width) for value, width in zip(row, widths)).rstrip() for row in table]
    lines += ["", *notes, *report["errors"], *report["notes"],
              "Memory: Docker working set / configured container limit; systemd cgroup usage / limit. '-' means unavailable."]
    if report["hidden_jobs"]:
        lines.append(f"{report['hidden_jobs']} inactive auxiliary tasks hidden; use --all to include them.")
    return "\n".join(lines)


def show(kit_root, node_env, node, *, watch=False, json_output=False, include_all=False, refresh_secs=5):
    """Read package identity only: invalid budgets/snapshots cannot block process diagnosis."""
    if not math.isfinite(refresh_secs) or refresh_secs <= 0:
        raise ValueError("top refresh interval must be positive and finite")
    if watch and json_output:
        raise ValueError("top --watch and --json cannot be combined")
    manifest = node._load_release_manifest(kit_root.expanduser().resolve())
    bundle_id = manifest["network_bundle"]["bundle_id"]
    layout = SimpleNamespace(bundle_id=bundle_id, release_id=manifest["release_id"],
        node_env=(node_env or Path.home() / ".config/usdb" / bundle_id / "node.env").expanduser().absolute())
    if not watch:
        report = collect(layout, node, include_all=include_all)
        print(json.dumps(report, indent=2, sort_keys=True) if json_output else render(report))
        return 0 if report["observation"] == "available" else 1
    display = node.TerminalProgressDisplay(sys.stderr)
    try:
        display.start()
        while True:
            display.render(render(collect(layout, node, include_all=include_all)))
            time.sleep(refresh_secs)
    except KeyboardInterrupt:
        return 0
    finally:
        display.close()
