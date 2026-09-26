"""Host node monitor: bounded sampling, durable local events and read-only console output."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

import node_monitor_rules as rules
from node_monitor_store import Store, private_directory, private_file
from node_resource_monitor import ResourceObserver

SCHEMA = "usdb-node-monitor:v2"
RELEASE_ENV = "USDB_NODE_MONITOR_RELEASE"


def now_ms():
    return time.time_ns() // 1000000


def root(layout):
    return layout.node_env.resolve().parent / "monitor"


def scope(layout):
    """Bind chain lineage, not release metadata or rotatable snapshot trust keys."""
    fields = ("chain_id", "network_id", "genesis_block_hash", "btc_network_id", "btc_index_origin_height")
    return {"bundle_id": layout.bundle_id, "network": {key: layout.network_identity.get(key) for key in fields}}


def enabled(layout, node):
    value = node.read_env(layout.node_env).get("USDB_MONITOR_ENABLED", "1") if layout.node_env.is_file() else "1"
    if value not in {"0", "1"}:
        raise ValueError("USDB_MONITOR_ENABLED must be 0 or 1")
    return value == "1"


def config(layout):
    """Read bounded local policy, rejecting unknown fields instead of guessing defaults."""
    path = root(layout) / "config.json"
    value = {}
    if path.exists() or path.is_symlink():
        private_directory(path.parent)
        private_file(path)
        if path.stat().st_size > 16384:
            raise ValueError("Monitor configuration exceeds its size limit")
        value = json.loads(path.read_text())
    if not isinstance(value, dict) or value.keys() - rules.DEFAULTS.keys():
        raise ValueError("Unsupported monitor configuration fields")
    return validate_config({**rules.DEFAULTS, **value})


def validate_config(value):
    ranges = dict(interval_secs=(5, 300), sample_timeout_secs=(1, 120), startup_grace_secs=(0, 3600),
                  warning_after_secs=(1, 86400), critical_after_secs=(1, 604800),
                  recovery_after_secs=(1, 3600), stall_after_secs=(60, 86400),
                  retention_days=(1, 3650), max_events=(100, 1000000),
                  resource_interval_secs=(5, 60), resource_raw_days=(1, 30),
                  resource_minute_days=(1, 365), resource_max_mib=(32, 4096))
    for name, (minimum, maximum) in ranges.items():
        if type(value[name]) is not int or not minimum <= value[name] <= maximum:
            raise ValueError(f"Monitor {name} must be an integer between {minimum} and {maximum}")
    if value["critical_after_secs"] < value["warning_after_secs"]:
        raise ValueError("Monitor critical window must not be shorter than warning window")
    return value


def database(layout, *, writable=False):
    path = root(layout) / "events.sqlite3"
    if not writable and not path.exists() and not path.is_symlink():
        raise FileNotFoundError(path)
    return Store(path, scope(layout), writable=writable)


def initialized_process(layout, pid):
    """A running PID alone does not prove the event database was opened successfully."""
    try:
        with database(layout) as store:
            session = store.get("session", {})
            return (store.get("process_id") == int(pid) and session.get("release_id") == layout.release_id
                    and not session.get("stopped_at_ms") and is_running(layout))
    except (OSError, ValueError, sqlite3.Error):
        return False


@contextmanager
def run_lock(layout):
    """One observer per node, including compatibility and foreground entrypoints."""
    directory = root(layout)
    private_directory(directory, create=True)
    descriptor = os.open(directory / "run.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        private_file(directory / "run.lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("Node monitor is already running") from error
        yield
    finally:
        os.close(descriptor)


def is_running(layout):
    path = root(layout) / "run.lock"
    if not path.exists() and not path.is_symlink():
        return False
    private_directory(path.parent)
    private_file(path)
    with path.open("rb") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return False
        except BlockingIOError:
            return True


@contextmanager
def private_creation_mode():
    previous = os.umask(0o077)
    try:
        yield
    finally:
        os.umask(previous)


def summary(store, state, *, at=None):
    """Expose bounded local event evidence without credentials or complete host logs."""
    alerts = sorted(store.alerts(), key=lambda v: (v["state"] != "firing", v["severity"] != "critical", -v["last_seen_ms"]))
    notifications = store.get("notifications", {"state": "not_started"})
    history = store.get("resource_history", {"state": "not_started"})
    if state in {"stopped", "disabled", "failed", "interrupted"}:
        notifications = {**notifications, "state": "unavailable" if state == "failed" else state}
        history = {**history, "state": state}
    elif history.get("state") == "available":
        age_limit = (store.get("policy", {}).get("resource_interval_secs", 10) * 3 + 15) * 1000
        if not -5000 <= (at or now_ms()) - history.get("last_sample_ms", 0) <= age_limit:
            history = {**history, "state": "stale"}
    return dict(schema_version=SCHEMA, state=state, storage="available", node_id=store.get("node_id"),
                session=store.get("session"), updated_at_ms=at or now_ms(),
                notifications=notifications, resource_history=history,
                console_export_available=store.get("console_export_available"),
                alert_count=len(alerts), alerts=alerts[:32], events=store.events(limit=20))


def error_evidence(error):
    return dict(type=type(error).__name__, code=getattr(error, "code", getattr(error, "sqlite_errorname", "UNAVAILABLE")),
                errno=getattr(error, "errno", None))


def publish_report(layout, node, report, *, store=None):
    """A dashboard export failure must not stop the core event journal."""
    import control_plane_monitor as console
    evidence = None
    try:
        console.publish(layout, node, report)
    except (OSError, ValueError) as error:
        evidence = error_evidence(error)
    previous = store.get("console_export_available") if store else None
    available = evidence is None
    if previous != available:
        if evidence:
            print("MONITOR_CONSOLE_EXPORT_FAILED: " + json.dumps(evidence), file=sys.stderr, flush=True)
        if store:
            with store.db:
                store.put("console_export_available", available)
                if evidence or previous is False:
                    store.event(now_ms(), "monitor", "CONSOLE_EXPORT_FAILED" if evidence else "CONSOLE_EXPORT_RECOVERED",
                                "warning" if evidence else "info", evidence=evidence)


def empty_report():
    import control_plane_monitor as console
    return dict(schema_version=console.SCHEMA, observed_at_ms=now_ms(), observation_available=False,
                overall_state="UNAVAILABLE", components=[])


def publish_state(layout, node, state, *, store=None):
    report = empty_report()
    report["monitor"] = summary(store, state) if store else dict(schema_version=SCHEMA, state=state,
        storage="unknown", updated_at_ms=now_ms(), notifications={"state": "not_started"}, alerts=[], events=[])
    if store is None:
        try:
            with database(layout) as reader:
                report["monitor"] = summary(reader, state)
        except (OSError, ValueError, sqlite3.Error):
            pass
    if state == "failed":
        report["monitor"]["storage"] = "unavailable"
    publish_report(layout, node, report, store=store)


def sample(layout, node, timeout, stopped=None, *, incidents_only=False, include_resources=True):
    """Isolate the existing collector; bound both probe time and descendant lifetime."""
    stopped = stopped or threading.Event()
    command = [sys.executable, str(Path(node.__file__).resolve()), "--kit-root", str(layout.kit_root),
               "--node-env", str(layout.node_env), "monitor", "sample"]
    if incidents_only:
        command.append("--incidents-only")
    if not include_resources:
        command.append("--without-resources")
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.DEVNULL, start_new_session=True)
        deadline = time.monotonic() + timeout
        try:
            while process.poll() is None:
                if stopped.wait(min(0.1, max(0, deadline - time.monotonic()))) or time.monotonic() >= deadline:
                    return {**empty_report(), "collection": {"outcome": "stopped" if stopped.is_set() else "timeout"}}
            if process.returncode != 0 or output.tell() > 256 * 1024:
                return empty_report()
            output.seek(0)
            value = json.loads(output.read(256 * 1024 + 1))
            if not isinstance(value, dict) or value.get("schema_version") != "usdb-console-monitor:v1":
                return empty_report()
            return value
        except (ValueError, OSError):
            return empty_report()
        finally:
            # Probe grandchildren can outlive their Python parent on a timeout.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()


def run(layout, node):
    """Persist before publishing; a failed event write is never a healthy sample."""
    if not layout.node_env.is_file():
        raise ValueError("Configure the node before starting its monitor")
    if not enabled(layout, node):
        publish_state(layout, node, "disabled")
        return
    settings = config(layout)
    stopped = threading.Event()
    delivery = None
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: stopped.set())
    with private_creation_mode(), run_lock(layout):
        stage = "initialize"
        try:
            with database(layout, writable=True) as store:
                store.begin(now_ms(), layout.release_id)
                with store.db:
                    store.put("stop_requested", False)
                    store.put("process_id", os.getpid())
                    store.put("policy", settings)
                    store.event(now_ms(), "monitor", "MONITOR_POLICY_SELECTED", evidence=settings)
                publish_state(layout, node, "starting", store=store)
                resources = ResourceObserver(layout, node, stopped, settings["resource_interval_secs"],
                                             settings, store.get("session"), store.get("node_id"))
                import node_notifications
                delivery = node_notifications.Worker(root(layout), scope(layout),
                    store.db.execute("SELECT COALESCE(max(seq),0) FROM events").fetchone()[0])
                delivery.poll()
                last_prune = 0
                while not stopped.is_set() and not store.get("stop_requested"):
                    stage = "sample"
                    started = time.monotonic()
                    incident_report = sample(layout, node, min(5, settings["sample_timeout_secs"]), stopped, incidents_only=True)
                    probe_started = time.monotonic()
                    report = sample(layout, node, settings["sample_timeout_secs"], stopped, include_resources=False)
                    report["collection"] = dict(duration_ms=round((time.monotonic() - probe_started) * 1000),
                                                outcome=report.get("collection", {}).get("outcome") or
                                                ("ok" if report.get("observation_available") else "unavailable"))
                    if stopped.is_set() or store.get("stop_requested"):
                        break
                    at = now_ms()
                    resources.observe(report)
                    report["host_resources"] = resources.snapshot()
                    incident_observation = incident_report.get("observations", {})
                    if (incident_observation.get("incidents", {}).get("status") == "available"
                            and report.get("observations", {}).get("incidents", {}).get("status") != "available"):
                        # Keep the independently read halt even if regular probes
                        # timed out before reaching the chain component.
                        import node_observation
                        observations = node_observation.project(report.get("observations"))
                        if observations["status"] != "available":
                            observations = {**incident_observation, "services": {}}
                        observations["incidents"] = incident_observation["incidents"]
                        report["observations"] = observations
                    stage = "persist"
                    rules.evaluate(store, report, at, settings)
                    notification_status = delivery.poll()
                    with store.db:
                        previous = store.get("notifications", {}).get("state")
                        current = notification_status["state"]
                        store.put("notifications", notification_status)
                        if previous != current:
                            store.event(at, "monitor", "NOTIFICATION_STATUS_CHANGED",
                                        "info" if current == "running" else "warning", evidence={"state": current})
                    if at - last_prune > 3600000:
                        store.prune(at, days=settings["retention_days"], count=settings["max_events"])
                        last_prune = at
                    state = "running" if store.get("last_observation_available") else "degraded"
                    report["monitor"] = summary(store, state, at=at)
                    stage = "publish"
                    publish_report(layout, node, report, store=store)
                    stopped.wait(max(0, settings["interval_secs"] - (time.monotonic() - started)))
                stopped.set()
                delivery.close()
                with store.db:
                    store.put("notifications", {**store.get("notifications", {}), "state": "stopped"})
                stage = "stop"
                store.end(now_ms(), "planned_down" if store.get("stop_requested") else "service_stop")
                publish_state(layout, node, "stopped", store=store)
        except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
            stopped.set()
            # Arbitrary RPC, SQLite and filesystem messages can contain private
            # paths or payloads. Keep the journal diagnostic stable and bounded.
            print(f"MONITOR_PERSISTENCE_OR_COLLECTION_FAILED: stage={stage}; " + json.dumps(error_evidence(error)), file=sys.stderr, flush=True)
            try:
                publish_state(layout, node, "failed")
            except (OSError, ValueError):
                pass
            raise ValueError(f"Node monitor failed (stage={stage}, code={error_evidence(error)['code']}); inspect its journal and private state directory") from None

        finally:
            if delivery is not None:
                delivery.close()


def unit_name(layout):
    return f"usdb-node-monitor-{layout.bundle_id}.service"


def unit_path(layout, node):
    return node.controller_unit_path(layout).with_name(unit_name(layout))


def render_unit(layout, node, context):
    """No Requires=docker: unexpected Docker outages must remain observable."""
    quote = node._systemd_quote
    command = " ".join(quote(str(v)) for v in (context.launcher, "--node-env", layout.node_env, "monitor", "run"))
    return f"""[Unit]
Description=USDB node monitor ({layout.bundle_id})
After=network.target

[Service]
Type=simple
User={context.service_user}
Environment={quote(f'HOME={context.home}')}
Environment=PYTHONDONTWRITEBYTECODE=1
Environment={quote(f'{RELEASE_ENV}={layout.release_id}')}
ExecStart={command}
Restart=on-failure
RestartSec=10s
TimeoutStopSec=15s
KillMode=control-group
UMask=0077

[Install]
WantedBy=multi-user.target
"""


def install(layout, node, context):
    node._install_service_unit(unit_path(layout, node), render_unit(layout, node, context), context.service_user)


def stop(layout, node):
    """Quiesce rules before stopping services; down retains all diagnostic state."""
    if (root(layout) / "events.sqlite3").exists():
        try:
            with database(layout, writable=True) as store, store.db:
                store.put("stop_requested", True)
                store.event(now_ms(), "monitor", "NODE_DOWN_REQUESTED")
        except (OSError, ValueError, sqlite3.Error) as error:
            print(f"MONITOR_STOP_RECORD_FAILED: type={type(error).__name__}; continuing explicit node shutdown", file=sys.stderr)
    if unit_path(layout, node).is_file():
        node._privileged_command(["systemctl", "stop", unit_name(layout)])
    if is_running(layout):
        raise ValueError("A foreground monitor is still stopping; stop it before retrying usdb-node down")
    publish_state(layout, node, "stopped" if enabled(layout, node) else "disabled")


def status(layout, node):
    result = dict(schema_version=SCHEMA, enabled=enabled(layout, node), running=is_running(layout),
                  state="not_started", notifications={"state": "not_started"}, storage="missing", alerts=[], events=[])
    try:
        with database(layout) as store:
            result.update(summary(store, "running" if result["running"] else "stopped"))
            if not result["running"] and not store.get("session", {}).get("stopped_at_ms"):
                result["state"] = "interrupted"
            last = store.get("last_sample_ms")
            result["last_sample_ms"] = last
            if result["running"] and (last is None or now_ms() - last > 120000 or not store.get("last_observation_available")):
                result["state"] = "degraded"
    except FileNotFoundError:
        pass
    except (OSError, ValueError, sqlite3.Error):
        result.update(storage="unavailable", state="degraded")
    if not result["enabled"]:
        result["state"] = "disabled"
    if not result["running"] or not result["enabled"]:
        result["notifications"] = {**result["notifications"], "state": result["state"]}
    return result


def add_parser(subparsers):
    parser = subparsers.add_parser("monitor", help="Node monitoring, local events and persistent alerts")
    actions = parser.add_subparsers(dest="monitor_action", required=True)
    import node_resource_history
    node_resource_history.add_parser(actions)
    for name in ("status", "alerts"):
        action = actions.add_parser(name)
        action.add_argument("--json", action="store_true")
    events = actions.add_parser("events", help="Query local event history without probing services")
    events.add_argument("--json", action="store_true")
    events.add_argument("--service")
    events.add_argument("--severity", choices=("info", "warning", "critical"))
    events.add_argument("--since", help="Timezone-qualified ISO timestamp")
    events.add_argument("--id", help="Show one event by its stable ID")
    events.add_argument("--limit", type=int, default=100)
    configure = actions.add_parser("configure", help="Configure local monitoring while the node is stopped")
    configure.add_argument("--enabled", choices=("on", "off"))
    for key in rules.DEFAULTS:
        configure.add_argument("--" + key.replace("_", "-"), type=int)
    actions.add_parser("run", help="Run the monitor in the foreground (also used by systemd)")
    sample_parser = actions.add_parser("sample", help="Collect one sanitized sample without modifying monitor state")
    sample_parser.add_argument("--incidents-only", action="store_true", help=argparse.SUPPRESS)
    sample_parser.add_argument("--without-resources", action="store_true", help=argparse.SUPPRESS)


def dispatch(args, layout, node):
    action = args.monitor_action
    if action == "resources":
        import node_resource_history
        return node_resource_history.dispatch(args, layout)
    if action == "run":
        return run(layout, node)
    if action == "sample":
        import control_plane_monitor as console
        if args.incidents_only:
            import node_observation
            value = empty_report()
            value["observations"] = dict(schema_version=node_observation.SCHEMA, status="available", observed_at=node_observation.now(),
                                         services={}, incidents=node_observation.observe_incidents(layout, node))
        else:
            value = console.collect(layout, node, include_resources=not args.without_resources)
        print(json.dumps(value, allow_nan=False))
        return
    if action == "configure":
        import usdb_setup
        with node.node_operation_lock(layout, "monitor-configure"):
            usdb_setup._require_stopped(layout, node)
            if is_running(layout):
                raise ValueError("Stop the monitor before changing its configuration")
            settings = validate_config({**config(layout), **{key: getattr(args, key) for key in rules.DEFAULTS if getattr(args, key) is not None}})
            private_directory(root(layout), create=True)
            node._atomic_write_private(root(layout) / "config.json", json.dumps(settings) + "\n")
            if args.enabled is not None:
                node._atomic_write_private(layout.node_env, node.upsert_env(layout.node_env.read_text(),
                    {"USDB_MONITOR_ENABLED": "1" if args.enabled == "on" else "0"}))
        print("Monitor configuration saved; run usdb-node up to apply.")
        return
    if action == "status":
        result = status(layout, node)
    else:
        since = None
        if action == "events" and args.since:
            stamp = datetime.fromisoformat(args.since.replace("Z", "+00:00"))
            if stamp.tzinfo is None:
                raise ValueError("--since requires an explicit timezone")
            since = int(stamp.timestamp() * 1000)
        if not (root(layout) / "events.sqlite3").exists():
            raise ValueError("Monitor event database has not been initialized")
        with database(layout) as store:
            result = store.alerts() if action == "alerts" else store.events(limit=args.limit, service=args.service,
                        severity=args.severity, since=since, identity=args.id)
    if args.json:
        print(json.dumps(result, indent=2, allow_nan=False))
    elif action == "status":
        print(f"Monitor: {result.get('state', 'not_started')}; enabled={result['enabled']}; running={result['running']}; storage={result['storage']}")
        history = result.get("resource_history", {})
        sampled = history.get("last_sample_ms")
        sampled = datetime.fromtimestamp(sampled / 1000, timezone.utc).isoformat() if type(sampled) is int else "unknown"
        print(f"Resource history: {history.get('state', 'not_started')}; last sample={sampled}; "
              f"capacity evictions={history.get('capacity_evictions', 0)}; failed samples={history.get('failed_samples', 0)}")
        delivery = result["notifications"]
        counts = delivery.get("counts", {})
        print(f"Active/pending alerts: {len(result['alerts'])}; notifications: {delivery['state']}; "
              f"pending={counts.get('pending', 0)}; failed={counts.get('failed', 0)}")
        for channel in delivery.get("channels", []):
            latest = channel.get("latest") or {}
            print(f"  {channel['id']}: {latest.get('state', 'no_deliveries')}; "
                  f"result={channel.get('last_error') or latest.get('code') or '-'}; "
                  f"pending={channel.get('pending_count', 0)}")
        if delivery.get("config_error"):
            print(f"Notification configuration: {delivery['config_error']}; last valid settings remain active")
    else:
        for item in result:
            at = item.get("at_ms", item.get("last_seen_ms"))
            stamp = datetime.fromtimestamp(at / 1000, timezone.utc).isoformat()
            print(f"{stamp} {item['severity']} {item['service']} {item['code']} "
                  f"{item.get('state', '')} {item.get('event_id', item.get('alert_id'))}")
            print("  " + json.dumps(item.get("evidence", {}), sort_keys=True))
        if not result:
            print("No matching records.")
