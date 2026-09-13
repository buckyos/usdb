#!/usr/bin/env python3
"""Native node lifecycle coordination using Core and service readiness, never legacy markers."""

from __future__ import annotations

from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import time

import usdb_node as node


def core_progress(layout) -> dict:
    """An unsuccessful probe is pending; a reported identity error is a hard failure."""
    result = node.run_helper(layout, "run_testnet_bitcoin.sh", ["progress"], check=False, capture_output=True, command_timeout_secs=45)
    try:
        value = json.loads(result.stdout)
    except ValueError:
        return dict(rpc_available=False, bootstrap_ready=False, tip_ready=False)
    if not isinstance(value, dict) or value.get("schema_version") != "usdb-bitcoin-assumeutxo:v1":
        raise ValueError("Core native probe returned an incompatible response")
    if value.get("error") and value.get("error_kind") != "rpc_unavailable":
        raise ValueError("Core native probe rejected the configured chain: " + value["error"])
    return value


def start_native_node(layout, *, sync_timeout_secs: int, output_to_stderr: bool, progress_monitor) -> None:
    """Start BH/indexer after activation and file preparation; commit budgets before overlap."""
    env = node.read_env(layout.node_env)
    managed = node.resource_mode(env) == "auto"
    if managed:
        state = node._read_resource_state(layout)
        target = state["phase"] if state.get("pending") else env["USDB_RESOURCE_PHASE"]
        observed = node._resource_containers(layout)
        stale_bh = observed.get("balance-history", {}).get("state") == "running" and not node._resource_container_matches(observed["balance-history"], env, "balance-history")
        if state.get("pending") or stale_bh or not node._resource_container_matches(observed.get("btc-node"), env, "btc-node"):
            node._transition_resources(layout, target, output_to_stderr=output_to_stderr)
    else:
        node.run_helper(layout, "run_testnet_bitcoin.sh", ["start"], output_to_stderr=output_to_stderr)
    deadline, heartbeat = time.monotonic() + sync_timeout_secs, 0.0
    started = set()
    while time.monotonic() < deadline:
        env = node.read_env(layout.node_env)
        containers = node._resource_containers(layout)
        if managed:
            node._check_running_resource_budget(env, containers)
        if containers.get("btc-node", {}).get("state") in {"dead", "exited", "restarting", "paused"}:
            raise ValueError("Core stopped during native bootstrap; inspect its persistent log before retrying")
        loader = containers.get("btc-snapshot-bootstrap")
        if loader is None or loader["state"] == "created":
            node.run_helper(layout, "run_testnet_bitcoin.sh", ["bootstrap-start"], output_to_stderr=output_to_stderr)
            started.add("btc-snapshot-bootstrap")
        elif loader["state"] in {"dead", "restarting", "paused"} or (loader["state"] == "exited" and loader["exit_code"] != 0):
            raise ValueError("Core bootstrap preparation failed; inspect its download/load journal before an explicit retry")
        try:
            core = core_progress(layout)
        except (OSError, subprocess.TimeoutExpired):
            core = dict(bootstrap_ready=False, tip_ready=False)
        prepared = loader is not None and loader["state"] == "exited" and loader["exit_code"] == 0
        baseline = core.get("bootstrap_ready") is True
        tip_ready = core.get("tip_ready") is True
        phase = env.get("USDB_RESOURCE_PHASE", "manual")
        progress_monitor.set_phase("native-" + phase)
        if baseline and prepared and managed and phase == "bitcoin":
            node._transition_resources(layout, "overlap", output_to_stderr=output_to_stderr)
            started.clear()
            continue
        if baseline and prepared:
            need_start = False
            for service in ("balance-history", "usdb-indexer"):
                current = containers.get(service)
                if current is None or current["state"] in {"created", "exited"}:
                    if service in started:
                        raise ValueError(f"{service} exited after native startup; inspect its persistent log")
                    need_start = True
                elif current["state"] != "running":
                    raise ValueError(f"{service} is not running normally during native bootstrap")
                elif managed and not node._resource_container_matches(current, env, service):
                    raise ValueError(f"{service} is using a stale native resource allocation")
            if need_start:
                node.run_helper(layout, "run_testnet_runtime.sh", ["native-start-data"], output_to_stderr=output_to_stderr)
                if managed:
                    node._resource_service_started(layout, "balance-history", "usdb-indexer")
                started.update(("balance-history", "usdb-indexer"))
        bh, _ = node._read_service_readiness(layout, "run_testnet_runtime.sh", ["data-status"], "balance-history")
        indexer, _ = node._read_service_readiness(layout, "run_testnet_runtime.sh", ["indexer-status"], "usdb-indexer")
        consensus = bh and indexer and bh["consensus_ready"] and indexer["consensus_ready"]
        if baseline and prepared and tip_ready and consensus:
            if managed and phase != "steady":
                node._transition_resources(layout, "steady", output_to_stderr=output_to_stderr)
                started.clear()
                continue
            if any(containers.get(service, {}).get("state") != "running" for service in ("usdb-chain", "usdb-control-plane")):
                if "usdb-chain" in started:
                    raise ValueError("USDB chain or control-plane exited after native startup")
                if managed:
                    node._resource_prepare_restart(layout, "usdb-chain", "usdb-control-plane")
                node.run_helper(layout, "run_testnet_runtime.sh", ["up-chain"], sync_timeout_secs=0, output_to_stderr=output_to_stderr)
                if managed:
                    node._resource_service_started(layout, "usdb-chain", "usdb-control-plane")
                started.add("usdb-chain")
            if node._runtime_lifecycle_status(layout)["state"] == "ready":
                if managed:
                    state = node._read_resource_state(layout)
                    if state:
                        state["recover_services"] = []
                        node._write_resource_state(layout, state)
                progress_monitor.set_phase("ready")
                return
        if time.monotonic() >= heartbeat:
            node._print_startup_phase("native-" + phase,
                f"Core baseline={baseline}, file prepared={prepared}, foreground ready={tip_ready}, history validated={core.get('history_validated', False)}, services consensus ready={bool(consensus)}",
                output_to_stderr=output_to_stderr)
            heartbeat = time.monotonic() + 60
        time.sleep(5)
    raise ValueError("Native startup observation timed out; Core import and durable service progress are retained")


def read_progress(path: Path) -> dict:
    """Read bounded display-only journals; readiness is always queried from services."""
    if not path.exists():
        return {}
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 1024 * 1024:
        return dict(error="Invalid progress file")
    try:
        value = node._load_json(path)
        if not isinstance(value, dict):
            return dict(error="Invalid progress object")
        value["observed_file_mtime"] = path.stat().st_mtime
        now = time.time()
        for start, elapsed in (("started_at", "elapsed_seconds"), ("phase_started_at", "phase_elapsed_seconds")):
            if type(value.get(start)) in (int, float):
                value[elapsed] = round(max(0, now - value[start]), 1)
        return value
    except (OSError, ValueError):
        return dict(error="Progress file is unavailable or invalid")


def collect_native_progress(layout) -> dict:
    """Expose import/replay plus independent Core foreground/background observations."""
    env = node.read_env(layout.node_env)
    try:
        services = node._collect_compose_services(layout, command_timeout_secs=8, include_started_at=True)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        services = {}
    try:
        core = core_progress(layout)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        core = dict(error=str(error))
    artifact = Path(env["BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR"]) / "mainnet-935000-utxos.dat"
    download = read_progress(artifact.with_name(artifact.name + ".download") / "progress.json")
    activation = read_progress(Path(env["BTC_ASSUMEUTXO_STATE_HOST_DIR"]) / "activation.json")
    bootstrap = read_progress(Path(env["BH_DATA_HOST_DIR"]) / "bootstrap-progress.json")
    loader = services.get("btc-snapshot-bootstrap", {})
    latest = download if download.get("updated_at", 0) > activation.get("updated_at", 0) else activation
    phase = latest.get("phase", "waiting_for_core")
    details = latest.get("details", {})
    details = details if isinstance(details, dict) else {}
    failed = loader.get("state") in {"dead", "restarting", "paused"} or (loader.get("state") == "exited" and loader.get("exit_code") not in (None, 0))
    complete = core.get("bootstrap_ready") and loader.get("state") == "exited" and loader.get("exit_code") == 0
    preparation_state = {"downloading": "INSTALLING", "verifying_file": "VERIFYING", "loading": "IMPORTING", "load_requested": "IMPORTING",
                         "load_uncertain": "BLOCKED", "load_failed": "FAILED"}.get(phase, "WAITING")
    # Download completion is not Core import progress; never reuse its 100% bar.
    byte_phase = phase in {"downloading", "verifying_file"}
    snapshot = node._component_progress("snapshot", "FAILED" if failed else "READY" if complete else preparation_state,
                                       f"Core UTXO preparation: {phase}; elapsed_seconds={latest.get('elapsed_seconds')}",
                                       current=details.get("bytes") if byte_phase else None,
                                       total=details.get("total_bytes") if byte_phase else None, unit="bytes")
    snapshot["label"] = "UTXO snapshot"
    snapshot["progress_phase"] = phase
    components = [snapshot, node._component_progress("script_registry", "SKIPPED", "Native observed-script registry is maintained by balance-history")]
    bitcoin = node._component_progress("bitcoin", "STARTING" if core.get("error_kind") == "rpc_unavailable" or core.get("rpc_available") is False else "BLOCKED" if core.get("error") else "READY" if core.get("tip_ready") else "SYNCING",
                      core.get("error") or f"foreground={core.get('active_height')}; background={core.get('background_height')}; history_validated={core.get('history_validated', False)}",
                      current=core.get("active_height"), total=core.get("headers"))
    # This observation is separate from foreground readiness and never gates startup.
    bitcoin["background_validation"] = dict(height=core.get("background_height"),
        target=int(env["BH_ASSUMEUTXO_BASE_HEIGHT"]), validated=core.get("history_validated") is True,
        available=not core.get("error") and core.get("rpc_available") is not False)
    if services.get("btc-node", {}).get("state") in {"dead", "exited", "restarting", "paused"}:
        bitcoin.update(state="FAILED", detail="Core is not running; inspect its persistent log")
    components.append(bitcoin)
    for component, service, action in (("balance_history", "balance-history", "data-status"), ("usdb_indexer", "usdb-indexer", "indexer-status")):
        readiness, error = node._read_service_readiness(layout, "run_testnet_runtime.sh", [action], service)
        item = node._indexed_service_component(component, services.get(service), readiness, error, "waiting for native service startup")
        if component == "balance_history" and not readiness and services.get(service, {}).get("state") == "running":
            phase = bootstrap.get("phase", "starting")
            height, target = bootstrap.get("height"), bootstrap.get("target", int(env["USDB_GENESIS_BLOCK_HEIGHT"]))
            base = int(env["BH_ASSUMEUTXO_BASE_HEIGHT"])
            replay = phase in {"replaying", "waiting_for_blocks"}
            percent = (height - base) * 100 / (target - base) if replay and type(height) is int and type(target) is int and target > base else None
            item = node._component_progress(component, "IMPORTING" if phase == "importing" else "VERIFYING" if phase == "verifying" else "SYNCING" if replay else "STARTING",
                f"native phase={phase}; imported_coins={bootstrap.get('imported_coins')}; elapsed_seconds={bootstrap.get('elapsed_seconds')}",
                current=bootstrap.get("imported_coins") if phase == "importing" else height if replay else None,
                total=target if replay else None, progress_percent=percent,
                unit="utxos" if phase == "importing" else "blocks")
            item["progress_phase"] = phase
        components.append(item)
    chain = node._chain_component(layout, env, services.get("usdb-chain"))
    gate = node._chain_startup_gate_component(services)
    if gate and (gate["state"] == "FAILED" or chain["state"] not in {"FAILED", "BLOCKED"}):
        chain.update(gate)
    components.append(chain)
    resources, resource_waiting = node._resource_progress(layout, env, services, components)
    control = services.get("usdb-control-plane", {})
    planned = "usdb-control-plane" in resources.get("recover_services", [])
    if node._container_start_failed(control) or (control.get("state") == "exited" and not planned):
        chain.update(state="FAILED", detail="control-plane is not running; inspect its service log")
    elif chain["state"] == "READY" and (control.get("state") != "running" or control.get("health") != "healthy"):
        chain.update(state="STARTING", detail="waiting for control-plane health")
    overall = node._overall_progress_state(components)
    if resources.get("error"):
        overall = "BLOCKED"
    elif overall == "READY" and resource_waiting:
        overall = "STARTING"
    mining, overall = node._mining_progress(layout, services, chain, components, overall)
    observed_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    for item in components:
        service = services.get({"bitcoin": "btc-node", "balance_history": "balance-history", "usdb_indexer": "usdb-indexer", "usdb_chain": "usdb-chain"}.get(item["id"]), {})
        elapsed = node.service_elapsed(service.get("started_at"), observed_at)
        if service.get("state") == "running" and elapsed is not None:
            item.update(service_started_at=service["started_at"], service_elapsed_secs=elapsed)
    return dict(schema_version=node.NODE_PROGRESS_SCHEMA_VERSION, release_id=layout.release_id, network_bundle_id=layout.bundle_id,
                observed_at=observed_at, controller_state=node.controller_observed_state(layout),
                overall_state=overall, auxiliary_state="READY", components=components, resources=resources, mining=mining,
                native_bootstrap=dict(core=core, download=download, activation=activation, balance_history=bootstrap))
