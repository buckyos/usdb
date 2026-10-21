"""Versioned runtime budgets and recoverable, foreground-independent transitions."""

from __future__ import annotations

from datetime import datetime
import subprocess
import sys
import time
import uuid

from resource_policy import (MIN_HOST_MEMORY_BYTES, POLICY_VERSION_KEY, build_resource_plan,
                             chain_resources_ready, resource_mode,
                             runtime_policy, validate_resource_environment)

ACTIVE = {"running", "restarting", "paused"}
ORDER = ("usdb-chain", "usdb-indexer", "balance-history", "ord-server", "usdb-control-plane")
JOBS = {"snapshot-loader", "script-registry-installer", "paired-checkpoint-recovery", "btc-snapshot-bootstrap", "usdb-chain-init"}


def prepare_for_up(layout, node):
    """Acquire an operation lock only for configured automatic resume candidates."""
    if not layout.node_env.is_file():
        return False
    env = node.read_env(layout.node_env)
    if resource_mode(env) != "auto" or not chain_resources_ready(env):
        return False
    with node.node_operation_lock(layout, "resources"):
        return _prepare_for_up_locked(layout, node)


def _prepare_for_up_locked(layout, node):
    """Rebudget a stopped resumed node before old allocations block preflight.

    An old steady marker is only permission to resume existing data, never proof
    of completed validation. Existing identity/bootstrap gates still run in up.
    """
    if not layout.node_env.is_file():
        return False
    env = node.read_env(layout.node_env)
    if resource_mode(env) != "auto" or not chain_resources_ready(env):
        return False
    memory = node.effective_memory_bytes()
    migrate_compact = (runtime_policy(env) and env.get("USDB_RESOURCE_PHASE") == "catchup"
                       and env.get("USDB_RESOURCE_RUNTIME_BUDGET") == "compact")
    needs = (not runtime_policy(env) or env.get("USDB_RESOURCE_HOST_MEMORY_BYTES") != str(memory)
             or env.get("USDB_RESOURCE_PHASE") == "steady" or migrate_compact
             or env.get("USDB_RESOURCE_RUNTIME_BUDGET") == "runtime-ready")
    if not needs:
        return False
    # Unavailable Docker observations must never masquerade as an empty host.
    observed = node._resource_containers(layout)
    state = node._read_resource_state(layout)
    if state.get("pending") or state.get("recover_services"):
        if env.get("USDB_RESOURCE_HOST_MEMORY_BYTES") == str(memory):
            return False  # Resume the journal's original plan before considering migration.
        raise ValueError("finish the pending resource transition before changing the host memory budget")
    if any(item["state"] not in {"exited", "dead", "created"} for item in observed.values()):
        if int(env.get("USDB_RESOURCE_HOST_MEMORY_BYTES", "0")) > memory:
            raise ValueError("effective host memory decreased while containers are active; run usdb-node down before rebudgeting")
        if migrate_compact and env.get("USDB_RESOURCE_HOST_MEMORY_BYTES") == str(memory):
            apply_plan(layout, node, "catchup", runtime_budget="core-catchup", output_to_stderr=True)
            print("RESOURCE_REBUDGETED: Core catch-up receives unused runtime capacity", file=sys.stderr)
            return True
        return False
    settings = {**env, POLICY_VERSION_KEY: "2", "USDB_RESOURCE_PHASE": "catchup"}
    if memory < MIN_HOST_MEMORY_BYTES or env.get("USDB_RESOURCE_RUNTIME_BUDGET") in {"compact", "core-catchup", "runtime-ready"}:
        settings["USDB_RESOURCE_RUNTIME_BUDGET"] = "core-catchup"
    # Preserve the explicitly selected Ord policy across hardware changes and
    # cold starts. Older nodes retain their previously released allocations.
    if settings.get("USDB_ORD_RESOURCE_POLICY") != "adaptive-v2":
        settings["USDB_ORD_RESOURCE_POLICY"] = "adaptive-v1"
    desired = {**settings, **build_resource_plan(memory, "catchup", settings).environment()}
    validate_resource_environment(desired, memory)
    original = layout.node_env.read_text(encoding="utf-8")
    node._atomic_write_private(layout.node_env, node.upsert_env(original, desired))
    node._resource_state_path(layout).unlink(missing_ok=True)
    print("RESOURCE_REBUDGETED: stopped node adopts current host capacity; synchronization will be checked live", file=sys.stderr)
    return True


def affected_services(observed, desired, node):
    """Stop only changed services and their write-dependent consumers."""
    changed = {name for name, item in observed.items() if item["state"] in ACTIVE
               and name not in JOBS and name != "usdb-chain-init"
               and not node._resource_container_matches(item, desired, name)}
    dependents = set()
    if "btc-node" in changed:
        dependents.update(("balance-history", "usdb-indexer", "usdb-chain"))
    if "balance-history" in changed:
        dependents.update(("usdb-indexer", "usdb-chain"))
    if "usdb-indexer" in changed:
        dependents.add("usdb-chain")
    return changed | {name for name in dependents if observed.get(name, {}).get("state") in ACTIVE}


def apply_plan(layout, node, target, *, runtime_budget=None, output_to_stderr=False):
    """Journal before stopping; never lower a running database's hard ceiling."""
    env = node.read_env(layout.node_env)
    validate_resource_environment(env, node.effective_memory_bytes())
    current = env["USDB_RESOURCE_PHASE"]
    backwards = node.RESOURCE_PHASES.index(target) < node.RESOURCE_PHASES.index(current)
    if backwards and (current, target) != ("steady", "catchup"):
        raise ValueError("runtime resource phases cannot return to exclusive bootstrap")
    state = node._read_resource_state(layout)
    if state.get("pending"):
        runtime_budget = state.get("runtime_budget", runtime_budget)
    elif target == "catchup" and current == "steady" and runtime_budget is None:
        runtime_budget = "core-catchup"
    settings = dict(env)
    if runtime_budget is not None:
        settings["USDB_RESOURCE_RUNTIME_BUDGET"] = runtime_budget
    plan = build_resource_plan(int(env["USDB_RESOURCE_HOST_MEMORY_BYTES"]), target, settings)
    desired = {**env, **plan.environment()}
    observed = node._resource_containers(layout)
    if any(observed.get(name, {}).get("state") in ACTIVE for name in JOBS):
        raise ValueError("RESOURCE_JOB_ACTIVE: resource transition waits for the active preparation/recovery job")
    if target == "bitcoin":
        node._check_running_resource_budget(desired, {**observed,
            "btc-node": dict(state="running", memory=plan.limits["BTC_MEMORY_LIMIT"])})
    plan_id = node._resource_plan_id(settings, target)
    resuming = state.get("pending") is True
    if resuming and (state["phase"] != target or state.get("plan_id") != plan_id):
        raise ValueError("resource policy or release changed during a pending transition")
    affected = affected_services(observed, desired, node)
    if not resuming:
        state = dict(schema_version="usdb-resource-state:v1", bundle_id=layout.bundle_id,
                     phase=target, plan_id=plan_id, pending=True,
                     runtime_budget=plan.environment()["USDB_RESOURCE_RUNTIME_BUDGET"],
                     recover_services=sorted(set(state.get("recover_services", [])) | (affected & set(ORDER))))
        node._write_resource_state(layout, state)
    import node_resource_monitor
    operation = uuid.uuid4().hex
    def record(code, error=None):
        node_resource_monitor.transition(layout, node, code, operation, current, target,
            error=error, observed=observed, desired=plan.environment(), plan_id=plan_id)
    record("RESOURCE_TRANSITION_RESUMED" if resuming else "RESOURCE_TRANSITION_STARTED")
    try:
        services = [name for name in ORDER if name in affected]
        if services:
            node.run_helper(layout, "run_testnet_runtime.sh", ["quiesce-services", *services], output_to_stderr=output_to_stderr)
        bitcoin_matches = node._resource_container_matches(observed.get("btc-node"), desired, "btc-node")
        if not bitcoin_matches and "btc-node" in observed:
            node.run_helper(layout, "run_testnet_bitcoin.sh", ["down"], output_to_stderr=output_to_stderr)
        if any(env.get(k) != v for k, v in plan.environment().items()):
            node._atomic_write_private(layout.node_env, node.upsert_env(layout.node_env.read_text(), plan.environment()))
        if not bitcoin_matches:
            node.run_helper(layout, "run_testnet_bitcoin.sh", ["start"], output_to_stderr=output_to_stderr)
        observed = node._resource_containers(layout)
        if not node._resource_container_matches(observed.get("btc-node"), desired, "btc-node"):
            raise ValueError("Bitcoin did not adopt the runtime resource plan")
        node._check_running_resource_budget(desired, observed)
        state["pending"] = False
        node._write_resource_state(layout, state)
        record("RESOURCE_CONFIG_COMMITTED" if state.get("recover_services") else "RESOURCE_TRANSITION_APPLIED")
        node._start_optional_ord(layout, output_to_stderr=output_to_stderr)
    except Exception as error:
        record("RESOURCE_TRANSITION_FAILED", error)
        raise


def observations(layout, node):
    """Read fresh upstream facts; optional Ord gates resource demotion only."""
    env = node.read_env(layout.node_env)
    containers = node._resource_containers(layout)
    if containers.get("btc-node", {}).get("state") != "running":
        return dict(known=False, reason="Bitcoin is not running", containers=containers)
    if env.get("SNAPSHOT_MODE") == "assumeutxo":
        from assumeutxo_node import core_progress
        core = core_progress(layout)
        if core.get("rpc_available") is not True:
            return dict(known=False, reason="Bitcoin observation unavailable", containers=containers)
        core_done = (core.get("tip_ready") is True and core.get("history_validated") is True
                     and (env.get("BTC_TXINDEX") != "1" or core.get("indexes_ready") is True))
        lag = max(0, core.get("headers", 0) - core.get("active_height", 0))
        for key in ("background_height", "txindex_height"):
            height = core.get(key)
            relevant = core.get("history_validated") is not True if key == "background_height" else env.get("BTC_TXINDEX") == "1"
            if relevant and type(height) is int:
                lag = max(lag, core.get("headers", 0) - height)
    else:
        core = node._bitcoin_startup_progress(layout, command_timeout_secs=20)
        if not isinstance(core, dict):
            return dict(known=False, reason="Bitcoin observation unavailable", containers=containers)
        core_done = core.get("ready") is True
        status = core.get("status") or {}
        lag = max(0, status.get("headers", 0) - status.get("blocks", 0))
    complete = core_done
    for service, command in (("balance-history", "data-status"), ("usdb-indexer", "indexer-status")):
        report, _ = node._read_service_readiness(layout, "run_testnet_runtime.sh", [command], service)
        if not report or containers.get(service, {}).get("state") != "running":
            return dict(known=False, reason=f"{service} observation unavailable", containers=containers)
        current, total = report.get("current"), report.get("total")
        caught = type(current) is int and type(total) is int and current >= total
        complete = complete and report.get("consensus_ready") is True and caught
        if type(current) is int and type(total) is int:
            lag = max(lag, total - current)
    chain = node._chain_component(layout, env, containers.get("usdb-chain"))
    if chain.get("observation_unavailable") or chain.get("state") in {"STARTING", "FAILED", "BLOCKED"}:
        return dict(known=False, reason="USDB chain observation unavailable", containers=containers)
    complete = complete and chain.get("state") == "READY"
    if env.get("USDB_MINTING_ENABLED") == "1":
        import usdb_minting
        ord_report = usdb_minting.progress(env)
        container = containers.get("ord-server", {})
        try:
            started = datetime.fromisoformat(container["started_at"].replace("Z", "+00:00")).timestamp() * 1000
            fresh = ord_report.get("observed_at_ms", 0) >= started
        except (KeyError, ValueError, TypeError):
            fresh = False
        if container.get("state") != "running" or not fresh or ord_report["state"] == "UNAVAILABLE":
            return dict(known=False, reason="Ord observation unavailable", containers=containers)
        complete = complete and ord_report.get("backend_ready") is True
        gap = ord_report.get("ord_gap")
        if type(gap) is int:
            lag = max(lag, gap)
    if any(containers.get(name, {}).get("state") in ACTIVE for name in JOBS):
        complete = False
    return dict(known=True, complete=bool(complete), core_complete=bool(core_done), lag=lag, containers=containers,
                identity=tuple(sorted((name, item.get("started_at")) for name, item in containers.items() if item["state"] == "running")))


class Stability:
    """Unknown/stale samples reset evidence; ordinary block arrival cannot churn budgets."""
    def __init__(self):
        self.since = self.previous = self.identity = self.candidate = None

    def target(self, phase, sample, now, budget="bootstrap"):
        target = None
        if sample.get("known"):
            if phase == "catchup" and sample.get("complete"):
                target = "steady"
            elif phase == "catchup" and budget == "bootstrap" and sample.get("core_complete"):
                target = "core-ready"
            elif phase == "catchup" and budget == "core-catchup" and sample.get("core_complete"):
                target = "runtime-ready"
            elif (phase == "catchup" and budget in {"core-ready", "runtime-ready"}
                  and sample.get("core_complete") is False and sample.get("lag", 0) >= 1000):
                target = "core-catchup"
            elif phase == "steady" and sample.get("lag", 0) >= 1000:
                target = "catchup"
        identity = sample.get("identity")
        if (target != self.candidate or identity != self.identity or self.previous is None
                or not 0 < now - self.previous <= 60):
            self.since = now
        self.previous, self.identity, self.candidate = now, identity, target
        if target and now - self.since >= (300 if target in {"catchup", "core-catchup"} else 60):
            return target
        return None


def run(layout, node, *, interval=15, sleep=time.sleep):
    """Continue after foreground startup, releasing the operation lock between polls."""
    import usdb_mining
    import usdb_peers
    stability = Stability()
    last_error = None
    while True:
        if not layout.node_env.is_file():
            return 0
        env = node.read_env(layout.node_env)
        if resource_mode(env) != "auto" or not runtime_policy(env):
            return 0
        if usdb_mining.pending(layout) or usdb_peers.pending(layout):
            return 1  # Let systemd dispatch the already authorized queued operation.
        try:
            state = node._read_resource_state(layout)
            sample = observations(layout, node)
            if not (state.get("pending") or state.get("recover_services")) and not any(item["state"] in ACTIVE for item in sample["containers"].values()):
                return 0
            target = state.get("phase") if state.get("pending") else stability.target(
                env["USDB_RESOURCE_PHASE"], sample, time.monotonic(), env.get("USDB_RESOURCE_RUNTIME_BUDGET", "bootstrap"))
            if target or state.get("recover_services"):
                with node.node_operation_lock(layout, "resources"):
                    # Recheck under the same lock as configuration and service changes.
                    if node.read_env(layout.node_env) != env:
                        stability = Stability()
                        continue
                    if target in {"steady", "core-ready", "runtime-ready"} and not state.get("pending"):
                        key = "complete" if target == "steady" else "core_complete"
                        if not observations(layout, node).get(key):
                            stability = Stability()
                            continue
                    if target:
                        budget_target = target in {"core-ready", "runtime-ready", "core-catchup"}
                        apply_plan(layout, node, "catchup" if budget_target else target,
                                   runtime_budget=target if budget_target else None,
                                   output_to_stderr=True)
                    node.start_node(layout, sync_timeout_secs=node.DEFAULT_SYNC_TIMEOUT_SECS,
                                    pull=False, output_to_stderr=True)
                    # Console/Ord are independent consumers; their recovery must
                    # not be lost when the native startup clears its journal.
                    node.run_helper(layout, "run_testnet_runtime.sh", ["up-console"], output_to_stderr=True)
                    node._start_optional_ord(layout, output_to_stderr=True)
                    updated = node.read_env(layout.node_env)
                    actual = node._resource_containers(layout)
                    state = node._read_resource_state(layout)
                    recovered = [name for name in state.get("recover_services", [])
                                 if node._resource_container_matches(actual.get(name), updated, name)]
                    node._resource_service_started(layout, *recovered)
                    if node._read_resource_state(layout).get("recover_services"):
                        raise ValueError("resource consumers have not adopted the committed plan; recovery will retry")
                    node._check_running_resource_budget(updated, actual)
                    import node_resource_monitor
                    node_resource_monitor.transition(layout, node, "RESOURCE_TRANSITION_APPLIED", uuid.uuid4().hex,
                        env["USDB_RESOURCE_PHASE"], updated["USDB_RESOURCE_PHASE"], observed=actual,
                        desired=updated, plan_id=state.get("plan_id"))
                stability = Stability()
            if last_error:
                print("RESOURCE_OBSERVATION_RECOVERED", file=sys.stderr)
            last_error = None
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            stability = Stability()
            message = str(error)
            if message != last_error:
                print("RESOURCE_RECONCILE_WAITING: " + message, file=sys.stderr)
            last_error = message
        sleep(interval)
