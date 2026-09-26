"""Independent resource recording and conservative, evidence-based pressure rules."""
from __future__ import annotations

import copy
import json
import sqlite3
import subprocess
import sys
import threading
import time
import uuid

from control_plane_resources import now_ms
from node_resource_history import History
from node_resource_metrics import Collector, configuration


class ResourceObserver:
    """Persist without waiting for service RPC probes or notification delivery."""

    def __init__(self, layout, node, stopped, interval, settings=None, session=None, node_id=None):
        import node_monitor_rules
        self.settings = settings or node_monitor_rules.DEFAULTS
        self.session = session or {"id": uuid.uuid4().hex}
        self.node_id = node_id
        self.lock = threading.Lock()
        self.value = {"schema_version": "usdb-console-resources:v1", "status": "unavailable",
                      "history": {"state": "starting", "at_ms": now_ms()}}
        self.observation = {}
        self.worker = threading.Thread(target=self._run, args=(layout, node, stopped, interval), daemon=True)
        self.worker.start()

    def observe(self, report):
        """Keep source timestamps: repeating the last probe is not a fresh RPC success."""
        import control_plane_monitor
        value = control_plane_monitor.project(report, report.get("observed_at_ms", now_ms()))
        value["observation_available"] = report.get("observation_available") is True
        value["probes"] = report.get("probes", [])
        value["collection"] = report.get("collection", {})
        with self.lock:
            self.observation = {key: value[key] for key in ("observed_at_ms", "observation_available", "components", "resources", "controller", "probes", "collection")}

    def _run(self, layout, node, stopped, interval):
        import node_monitor
        # Resolve here so test and compatibility callers can substitute the collector.
        from control_plane_resources import ResourceCollector
        collector, metrics = ResourceCollector(), Collector()
        last_prune, failure, lost, last_observation = 0, None, 0, None
        pressure_windows = {}
        while not stopped.is_set():
            started = time.monotonic()
            value = {"schema_version": "usdb-console-resources:v1", "status": "unavailable"}
            try:
                env = node.read_env(layout.node_env)
                value = collector.sample(env, layout.bundle_id)
                evidence = metrics.sample(value)
                for item in evidence["items"]:
                    key = (evidence["boot_id"], item["identity"], pressure_state(item))
                    previous = pressure_windows.get(item["service"])
                    if (previous is None or previous[0] != key
                            or not 0 < evidence["at_ms"] - previous[1] <= (interval * 3 + 15) * 1000):
                        window = uuid.uuid4().hex
                    else:
                        window = previous[2]
                    item["pressure_window"] = window
                    pressure_windows[item["service"]] = (key, evidence["at_ms"], window)
                with self.lock:
                    observation = copy.deepcopy(self.observation)
                evidence.update(session_id=self.session.get("id", self.session.get("session_id", "")),
                                release_id=layout.release_id, configuration=configuration(env), observation=observation)
                evidence["observation_is_new"] = observation.get("observed_at_ms") != last_observation
                value["diagnostics"] = evidence
                with History(node_monitor.root(layout) / "resources.sqlite3", node_monitor.scope(layout),
                             writable=True, max_mib=self.settings["resource_max_mib"]) as history:
                    if evidence["at_ms"] - last_prune >= 60000:
                        history.prune(evidence["at_ms"], self.settings)
                        last_prune = evidence["at_ms"]
                    history.append(evidence, node_id=self.node_id, failed_samples=lost)
                    last_observation = observation.get("observed_at_ms")
                    value["history"] = dict(state="available", at_ms=now_ms(), last_sample_ms=evidence["at_ms"],
                        capacity_evictions=history.get("capacity_evictions", 0), failed_samples=history.get("failed_samples", 0))
                if failure:
                    print("RESOURCE_HISTORY_RECOVERED", file=sys.stderr, flush=True)
                failure, lost = None, 0
            except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
                lost += 1
                code = node_monitor.error_evidence(error)
                if failure != code:
                    print("RESOURCE_HISTORY_FAILED: " + json.dumps(code), file=sys.stderr, flush=True)
                failure = code
                # Retain this sample's independent resource evidence if persistence failed.
                value["history"] = dict(state="unavailable", at_ms=now_ms(), error=code, failed_samples=lost)
            with self.lock:
                self.value = value
            stopped.wait(max(0, interval - (time.monotonic() - started)))

    def snapshot(self):
        with self.lock:
            return copy.deepcopy(self.value)


def pressure_state(item):
    """Cache occupancy alone is healthy; pressure needs reclaim/swap evidence."""
    metrics = item.get("metrics", {})
    full = metrics.get("memory_psi_full_avg10")
    if item.get("status") != "available" or type(full) not in {int, float}:
        return None
    if item["service"] == "host":
        available, total = metrics.get("memory_available_bytes"), metrics.get("memory_total_bytes")
        if type(available) is not int or type(total) is not int or total <= 0:
            return None
        swap = metrics.get("swap_out_bytes_per_sec")
        if full >= 10 or (full >= 1 and available / total < 0.05):
            return True
        if swap is None:
            return None
        return full >= 1 and swap > 1024 * 1024
    high, maximum = metrics.get("events_high_delta"), metrics.get("events_max_delta")
    swap = metrics.get("swap_used_bytes")
    if full >= 10 or (full >= 1 and (any(type(v) is int and v > 0 for v in (high, maximum, swap)))):
        return True
    if high is None or maximum is None or swap is None:
        return None
    return False


def evaluate(store, resources, at, settings):
    """Use distinct fresh resource samples, even when service probes are unavailable."""
    resource_at = resources.get("diagnostics", {}).get("at_ms")
    fresh = type(resource_at) is int and 0 <= at - resource_at <= (settings["resource_interval_secs"] * 3 + 15) * 1000
    previous = store.get("last_resource_sample_ms")
    # The worker's pressure window covers intermediate good/unknown samples;
    # slow service-probe intervals must not reset continuously observed pressure.
    reset = previous is None or not fresh or resource_at <= previous
    windows = dict(warning_ms=settings["warning_after_secs"] * 1000, critical_ms=settings["critical_after_secs"] * 1000,
                   recovery_ms=settings["recovery_after_secs"] * 1000)
    history = resources.get("history", {})
    history_at = history.get("at_ms")
    known = type(history_at) is int and 0 <= at - history_at <= (settings["resource_interval_secs"] * 3 + 15) * 1000
    storage_bad = None if type(history_at) is not int else not known or history.get("state") != "available"
    store.condition("resource-history", "monitor", "RESOURCE_HISTORY_UNAVAILABLE", storage_bad,
                    at, evidence={"state": history.get("state"), "error": history.get("error")}, **windows)
    previous_history = store.get("resource_history", {}).get("state")
    if known and history.get("state") != previous_history and (history.get("state") == "unavailable" or previous_history in {"unavailable", "stale"}):
        store.event(at, "monitor", "RESOURCE_HISTORY_FAILED" if history.get("state") == "unavailable" else "RESOURCE_HISTORY_RECOVERED",
                    "warning" if history.get("state") == "unavailable" else "info", evidence={"error": history.get("error")})
    store.put("resource_history", {**history, "state": history.get("state", "not_started") if known else "stale"})
    if fresh and resource_at == previous:
        return
    items = {item["service"]: item for item in resources.get("diagnostics", {}).get("items", [])} if fresh else {}
    existing = {row[0].removeprefix("resource-pressure:") for row in store.db.execute("SELECT key FROM alerts WHERE key LIKE 'resource-pressure:%'")}
    for service in items.keys() | existing:
        item = items.get(service, {})
        identity = (resources.get("diagnostics", {}).get("boot_id"), item.get("identity"), item.get("pressure_window"))
        identity_reset = list(identity) != store.baseline("resource-identity:" + service)
        evidence = {"sample_at_ms": resource_at, "identity": item.get("identity"), "metrics": item.get("metrics", {})}
        known_identity = bool(identity[0] and identity[1])
        store.condition("resource-pressure:" + service, service, "MEMORY_PRESSURE", pressure_state(item) if item and known_identity else None,
                        resource_at if fresh else at, reset=reset or identity_reset, evidence=evidence, **windows)
        if item:
            store.set_baseline("resource-identity:" + service, list(identity))
    if fresh:
        store.put("last_resource_sample_ms", resource_at)


def transition(layout, node, code, operation_id, source, target, *, error=None, observed=None, desired=None, plan_id=None):
    """Persist controller boundaries directly; monitoring never controls resource policy."""
    import node_monitor
    try:
        if not node_monitor.enabled(layout, node):
            return
        settings = node_monitor.config(layout)
        payload = dict(release_id=layout.release_id, operation_id=operation_id, from_phase=source, to_phase=target,
                       configuration=configuration(node.read_env(layout.node_env)), session_id="", plan_id=plan_id)
        if node_monitor.is_running(layout):
            with node_monitor.database(layout) as events:
                session = events.get("session", {})
                if not session.get("stopped_at_ms"):
                    payload["session_id"] = session.get("id", "")
        payload["desired"] = configuration(desired or {})
        payload["observed"] = {service: {**{key: item[key] for key in ("state", "memory", "swap") if key in item},
                                        "configuration": configuration(item.get("environment", {}))}
                               for service, item in (observed or {}).items()}
        if error is not None:
            payload["error"] = node_monitor.error_evidence(error)
        with History(node_monitor.root(layout) / "resources.sqlite3", node_monitor.scope(layout), writable=True,
                     max_mib=settings["resource_max_mib"]) as history:
            history.transition(now_ms(), code, payload)
    except (OSError, ValueError, sqlite3.Error) as failure:
        print("RESOURCE_TRANSITION_RECORD_FAILED: " + json.dumps(node_monitor.error_evidence(failure)), file=sys.stderr, flush=True)
