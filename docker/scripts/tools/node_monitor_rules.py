"""Conservative rules over sanitized observations, with explicit unknown evidence."""
from __future__ import annotations

from datetime import datetime
import node_observation

DEFAULTS = dict(interval_secs=30, sample_timeout_secs=25, startup_grace_secs=120,
                warning_after_secs=120, critical_after_secs=600, recovery_after_secs=60,
                stall_after_secs=900, retention_days=90, max_events=20000,
                resource_interval_secs=10, resource_raw_days=7, resource_minute_days=30, resource_max_mib=256)


def fresh(timestamp, at, max_age):
    try:
        value = int(datetime.fromisoformat(timestamp).timestamp() * 1000)
        return -5000 <= at - value <= max_age
    except (ValueError, TypeError, OverflowError):
        return False


def evaluate(store, report, at, config):
    """Commit one sample's state and transitions together; gaps never imply recovery."""
    with store.db:
        last = store.get("last_sample_ms")
        gap_ms = (config["interval_secs"] + config["sample_timeout_secs"] + 10) * 2000
        reset = last is None or not 0 <= at - last <= gap_ms
        session = store.get("session", {})
        warming = at - session.get("started_at_ms", at) < config["startup_grace_secs"] * 1000
        observation = node_observation.project(report.get("observations"))
        sample_at = report.get("observed_at_ms")
        available = (report.get("observation_available") is True and type(sample_at) is int
                     and -5000 <= at - sample_at <= gap_ms and observation["status"] == "available")

        def condition(key, service, code, bad, **options):
            windows = dict(warning_ms=config["warning_after_secs"] * 1000,
                           critical_ms=config["critical_after_secs"] * 1000,
                           recovery_ms=config["recovery_after_secs"] * 1000)
            store.condition(key, service, code, bad, at, reset=reset, **{**windows, **options})

        condition("collector", "monitor", "OBSERVATION_UNAVAILABLE", not available)
        incidents = observation["incidents"]
        # Durable evidence can survive an unrelated service collection failure.
        incidents_known = (type(sample_at) is int and -5000 <= at - sample_at <= gap_ms
                           and incidents["status"] == "available")
        condition("incidents-unavailable", "monitor", "INCIDENT_OBSERVATION_UNAVAILABLE",
                  None if not available else incidents["status"] == "unavailable")
        active_keys = set()
        if incidents_known:
            for event in incidents["events"]:
                # An unreadable ID still represents a known halt; a deterministic
                # local key prevents every poll from becoming a new incident.
                key = "incident:" + (event["event_id"] or "deep-reorg-unknown")
                active_keys.add(key)
                condition(key, "usdb_chain", event["code"], True, latched=True, evidence=event)
        for row in store.db.execute("SELECT key FROM alerts WHERE key LIKE 'incident:%'").fetchall():
            if row[0] not in active_keys:
                # The durable guard owns protection. Only fresh READY observations
                # with a readable, empty source prove recovery; absence alone does not.
                recovered = (available and incidents_known and not incidents["events"]
                             and report.get("overall_state") == "READY")
                condition(row[0], "usdb_chain", "DEEP_REORG_HALTED", False if recovered else None, latched=True)

        components = {c.get("id"): c for c in report.get("components", []) if isinstance(c, dict)}
        for service in node_observation.SERVICES:
            item = observation["services"].get(service, {})
            runtime = item.get("runtime", {})
            readiness = item.get("readiness", {})
            known = (available and readiness.get("status") == "available"
                     and fresh(readiness.get("observed_at"), at, gap_ms))
            component = components.get(service, {})
            phase = component.get("state")
            if available and phase in {"READY", "SYNCING", "WAITING", "SKIPPED", "FAILED", "BLOCKED", "UNAVAILABLE", "STARTING"}:
                previous_phase = store.baseline(service + ":phase")
                if previous_phase != phase:
                    store.event(at, service, "SERVICE_STATE_CHANGED", "warning" if phase in {"FAILED", "BLOCKED"} else "info",
                                evidence={"from": previous_phase, "to": phase})
                    store.set_baseline(service + ":phase", phase)
            evidence = {"state": runtime.get("state"), "health": runtime.get("health"),
                        "exit_code": runtime.get("exit_code"), "probe_status": item.get("probe_status")}
            if available and phase == "READY":
                store.put("ever_running:" + service, True)
            health_bad = None
            if (available and not warming and phase == "WAITING" and store.get("ever_running:" + service)
                    and report.get("controller", {}).get("runtime_state") in {"inactive", "failed"}):
                health_bad = True
            if available and not warming and phase not in {"WAITING", "SKIPPED"}:
                if runtime.get("state") in {"exited", "dead", "restarting", "paused"} or runtime.get("health") == "unhealthy":
                    health_bad = True
                elif item.get("probe_status") == "unavailable":
                    health_bad = True
                elif readiness.get("rpc_alive") is False:
                    health_bad = True
                elif (known and readiness.get("rpc_alive") is True) or (not readiness and (item.get("probe_status") == "available" or phase == "READY")):
                    health_bad = False
                elif phase in {"FAILED", "BLOCKED"}:
                    health_bad = True
            condition(service + ":health", service, "SERVICE_UNAVAILABLE", health_bad, evidence=evidence)

            ready = readiness.get("consensus_ready") if known else None
            if ready is True:
                store.put("ever_ready:" + service, True)
            regression = None if warming or ready is None else (not ready if store.get("ever_ready:" + service) else None)
            condition(service + ":readiness", service, "CONSENSUS_READINESS_LOST", regression,
                      evidence={"blockers": readiness.get("blockers")})

            # Count actual per-container restart increments, never lifetime counts
            # or a new container's reset as events from the previous container.
            baseline = store.baseline(service + ":runtime")
            current = runtime.get("container_id")
            count = runtime.get("restart_count")
            runtime_known = available and runtime.get("details_available") and current and count is not None
            if runtime_known:
                same = baseline and baseline["container_id"] == current and not reset and count >= baseline["count"]
                restarts = [v for v in baseline["restarts"] if 0 <= at - v[0] <= 600000] if same else []
                if same and count > baseline["count"]:
                    delta = count - baseline["count"]
                    restarts.append([at, delta])
                    store.event(at, service, "CONTAINER_RESTARTED", "warning", evidence={"container_id": current, "restart_delta": delta})
                oom_identity = {"container_id": current, "finished_at": runtime.get("finished_at"), "started_at": runtime.get("started_at")}
                if runtime.get("oom_killed") and store.get("last_oom:" + service) != oom_identity:
                    store.event(at, service, "CONTAINER_OOM_OBSERVED", "warning", evidence=oom_identity)
                    store.put("last_oom:" + service, oom_identity)
                store.set_baseline(service + ":runtime", dict(container_id=current, count=count,
                                   oom=runtime.get("oom_killed") is True, restarts=restarts))
                condition(service + ":restarts", service, "CONTAINER_RESTART_LOOP", sum(v[1] for v in restarts) >= 3)
            else:
                store.set_baseline(service + ":runtime", None)
                condition(service + ":restarts", service, "CONTAINER_RESTART_LOOP", None)

            # A stall requires measured upstream advancement, not just an old tip.
            stall = None
            local = readiness.get("synced_block_height")
            if local is None:
                local = readiness.get("stable_height", readiness.get("current"))
            if local is None:
                local = readiness.get("current")
            target = readiness.get("balance_history_stable_height")
            if target is None:
                target = readiness.get("total")
            previous = store.baseline(service + ":progress")
            if known and local is not None and target is not None:
                epoch = readiness.get("upstream_reorg_epoch")
                commitment = readiness.get("latest_block_commit") or readiness.get("local_state_commit")
                unchanged = (not reset and previous and previous["local"] == local and target >= previous["target"]
                             and previous["epoch"] == epoch and previous["commitment"] == commitment)
                advanced = unchanged and (previous["advanced"] or target > previous["target"])
                progressed = (not reset and previous and previous["epoch"] == epoch and
                              (local > previous["local"] or (unchanged and previous.get("progressed", False))))
                store.set_baseline(service + ":progress", dict(local=local, target=target, epoch=epoch,
                                   commitment=commitment, advanced=bool(advanced), progressed=bool(progressed)))
                if not warming:
                    if local >= target or ready is True:
                        stall = False
                    elif advanced:
                        stall = True
                    elif progressed:
                        stall = False
            else:
                store.set_baseline(service + ":progress", None)
            condition(service + ":stall", service, "SYNC_STALLED", stall,
                      warning_ms=config["stall_after_secs"] * 1000,
                      critical_ms=max(config["critical_after_secs"], config["stall_after_secs"] * 2) * 1000,
                      evidence={"local_height": local, "upstream_height": target})

        # Bootstrap completion is normal. Only explicit failure is an unhealthy
        # controller; optional minting is evaluated only when it is enabled.
        controller = report.get("controller", {})
        controller_state = controller.get("runtime_state")
        # Guidance distinguishes a historical manual exit from a current crash.
        historical_manual = (controller.get("display_state") in {"idle", "waiting_for_seed"}
                             and controller.get("result") == "exit-code"
                             and controller.get("exit_code") == 1 and controller.get("exit_status") == 2)
        condition("controller:health", "controller", "CONTROLLER_FAILED",
                  None if not available or warming or controller_state not in {"failed", "active", "inactive"}
                  else controller_state == "failed" and not historical_manual,
                  evidence={"exit_code": node_observation.quantity(controller.get("exit_code"))})
        for service, component, active in (("control_plane", components.get("control_plane", {}), True),
                                          ("ord", report.get("minting", {}), report.get("minting", {}).get("enabled") is True)):
            state = component.get("state")
            condition(service + ":health", service, "SERVICE_UNAVAILABLE",
                      None if not available or warming or not active or state not in {"FAILED", "BLOCKED", "READY"}
                      else state != "READY", evidence={"state": state if state in {"FAILED", "BLOCKED", "READY"} else None})

        store.put("last_sample_ms", at)
        store.put("last_observation_available", available)
        import node_resource_monitor
        node_resource_monitor.evaluate(store, report.get("host_resources", {}), at, config)
