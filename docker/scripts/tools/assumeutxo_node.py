#!/usr/bin/env python3
"""Native node lifecycle coordination using Core and service readiness, never legacy markers."""

from __future__ import annotations

from datetime import datetime, timezone
import json
import math
from pathlib import Path
import subprocess
import time

import usdb_node as node
import bitcoin_startup_progress
from bitcoin_import_progress import read_import_progress, timestamp
from bitcoin_release import UTXO_SIZE


class CoreProbeError(ValueError):
    """The helper response cannot be interpreted as a native readiness report."""


class BootstrapPreparationRequired(ValueError):
    """A stopped preparation job needs operator action, not a systemd restart loop."""


def _download_identity_matches(download, expected):
    """Only the release-pinned journal can authorize resuming a stopped download job."""
    identity = download.get("identity", {})
    snapshot = identity.get("snapshot", {}) if isinstance(identity, dict) else {}
    return (download.get("schema_version") == "usdb-bitcoin-assumeutxo:v1"
            and isinstance(snapshot, dict) and bool(expected)
            and all(snapshot.get(key) == expected.get(key) for key in ("base_height", "base_hash", "file_sha256"))
            and snapshot.get("chain") == "main"
            and snapshot.get("size_bytes") == UTXO_SIZE)


def _preparation_retryable(layout, env, loader, *, allow_exhausted=True):
    """Resume file preparation only; never automatically repeat an uncertain Core load."""
    if loader.get("state") != "exited":
        return False
    artifact_dir, state_dir = env.get("BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR"), env.get("BTC_ASSUMEUTXO_STATE_HOST_DIR")
    if not artifact_dir or not state_dir:
        return False
    activation = read_progress(Path(state_dir) / "activation.json")
    if activation and activation.get("phase") not in {"new", "waiting_for_core", "waiting_for_rpc", "waiting_for_headers"}:
        return False
    download = read_progress(Path(artifact_dir) / "mainnet-935000-utxos.dat.download" / "progress.json")
    expected = layout.snapshot["contract"]["snapshot"]
    if not _download_identity_matches(download, expected):
        return False
    phase, details = download.get("phase"), download.get("details", {})
    if phase == "download_failed":
        return (isinstance(details, dict) and details.get("retryable") is True
                and (allow_exhausted or details.get("retry_exhausted") is not True))
    return (phase in {"checking_file", "downloading", "download_retry", "verifying_file"}
            and (loader.get("oom_killed") is True or loader.get("exit_code") in {137, 143}))


def _preparation_failure(loader):
    """Explain the observed container failure without confusing it with Core's state."""
    code = loader.get("exit_code")
    if loader.get("oom_killed") is True:
        memory = loader.get("memory", loader.get("memory_limit_bytes"))
        limit = f" ({memory // 1024**2} MiB)" if type(memory) is int and memory > 0 else ""
        return f"Snapshot preparation exceeded its container memory limit{limit}; OOM killed (exit {code})"
    return f"Snapshot preparation container stopped: state={loader.get('state', 'unknown')}, exit={code}"


def core_progress(layout, *, command_timeout_secs: float = 20) -> dict:
    """An unsuccessful probe is pending; a reported identity error is a hard failure."""
    # The native helper shares a 12s RPC budget; include Docker/process overhead
    # without consuming the monitor's entire default 25s collection window.
    result = node.run_helper(layout, "run_testnet_bitcoin.sh", ["progress"], check=False,
                             capture_output=True, command_timeout_secs=command_timeout_secs)
    try:
        value = json.loads(result.stdout)
    except ValueError:
        return dict(bootstrap_ready=False, tip_ready=False, error_kind="probe_failed",
                    error="Native Bitcoin readiness helper returned invalid JSON", probe_exit_code=result.returncode)
    if not isinstance(value, dict) or value.get("schema_version") != "usdb-bitcoin-assumeutxo:v1":
        raise CoreProbeError("Core native probe returned an incompatible response")
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
    preparation_retries = 0
    while time.monotonic() < deadline:
        env = node.read_env(layout.node_env)
        containers = node._resource_containers(layout)
        if managed:
            node._check_running_resource_budget(env, containers)
        if containers.get("btc-node", {}).get("state") in {"dead", "exited", "restarting", "paused"}:
            raise ValueError("Core stopped during native bootstrap; inspect its persistent log before retrying")
        loader = containers.get("btc-snapshot-bootstrap")
        if loader and loader["state"] == "running":
            started.add("btc-snapshot-bootstrap")
        if loader is None or loader["state"] == "created":
            node.run_helper(layout, "run_testnet_bitcoin.sh", ["bootstrap-start"], output_to_stderr=output_to_stderr)
            started.add("btc-snapshot-bootstrap")
        elif loader["state"] in {"dead", "restarting", "paused"} or (loader["state"] == "exited" and loader["exit_code"] != 0):
            # A new explicit up may resume an earlier exhausted job. Do not
            # multiply its long no-progress budget within this controller run.
            if preparation_retries < 1 and _preparation_retryable(layout, env, loader,
                    allow_exhausted="btc-snapshot-bootstrap" not in started):
                preparation_retries += 1
                node._print_startup_phase("snapshot-resume", _preparation_failure(loader) +
                    "; resuming retained file, preparation retry=1/1; Core data preserved",
                    output_to_stderr=output_to_stderr)
                node.run_helper(layout, "run_testnet_bitcoin.sh", ["bootstrap-start"], output_to_stderr=output_to_stderr)
                started.add("btc-snapshot-bootstrap")
                continue
            raise BootstrapPreparationRequired("Core bootstrap preparation failed: " + _preparation_failure(loader) +
                ". Automatic preparation retries stopped; inspect usdb-node controller logs --follow and "
                "usdb-node logs --bitcoin btc-snapshot-bootstrap. After correcting the cause, run usdb-node up; "
                "an uncertain Core import requires explicit recovery, not another load request.")
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
            if any(containers.get(service, {}).get("state") != "running" for service in ("usdb-chain",)):
                if "usdb-chain" in started:
                    raise ValueError("USDB chain exited after native startup")
                if managed:
                    node._resource_prepare_restart(layout, "usdb-chain")
                node.run_helper(layout, "run_testnet_runtime.sh", ["up-chain"], sync_timeout_secs=0, output_to_stderr=output_to_stderr)
                if managed:
                    node._resource_service_started(layout, "usdb-chain")
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


def _download_retry_details(details, *, waiting):
    """Render persisted retry facts; countdowns never authorize another attempt."""
    lines = []
    attempt = details.get("attempt")
    if type(attempt) is int:
        if "max_attempts" in details:  # Older image journals remain readable.
            lines.append(f"Download attempt: {attempt}/{details['max_attempts']}")
        else:
            lines.append(f"Download attempt: {attempt} | Retries: {details.get('retry_count', max(0, attempt - 1))}")
    def instant(value):
        if type(value) in (int, float) and math.isfinite(value):
            try:
                return datetime.fromtimestamp(value, timezone.utc).isoformat()
            except (OverflowError, OSError, ValueError):
                pass
        return None
    next_at = details.get("next_retry_at")
    formatted = instant(next_at)
    if waiting and formatted:
        remaining = max(0, math.ceil(next_at - time.time()))
        lines.append(f"Next download retry: {formatted} (in {remaining}s)" if remaining else
                     f"Scheduled download retry: {formatted}; awaiting progress observation")
    timeout = details.get("no_progress_timeout_seconds")
    if type(timeout) in (int, float) and math.isfinite(timeout) and timeout > 0:
        lines.append(f"Automatic retry pauses after {timeout / 3600:g}h without new download bytes")
    if details.get("last_error") or details.get("error"):
        when = instant(details.get("last_error_at"))
        lines.append(f"Last download error{f' ({when})' if when else ''}: " +
                     str(details.get("last_error") or details["error"]))
    return lines


def _snapshot_component(phase, latest, imported, *, failed, complete, activated, base_height=None):
    """Show phase-specific work; reading all coins is not snapshot activation."""
    details = latest.get("details", {})
    details = details if isinstance(details, dict) else {}
    state = {"downloading": "INSTALLING", "download_retry": "INSTALLING", "download_failed": "FAILED",
             "verifying_file": "VERIFYING", "loading": "IMPORTING",
             "load_requested": "IMPORTING", "load_uncertain": "BLOCKED", "load_failed": "FAILED"}.get(phase, "WAITING")
    current = total = percent = None
    unit = "bytes"
    detail = f"Core UTXO preparation: {phase}"
    if phase in {"downloading", "download_retry", "download_failed", "verifying_file"}:
        current, total = details.get("bytes"), details.get("total_bytes")
        detail = {"downloading": "Downloading snapshot", "verifying_file": "Verifying downloaded file SHA-256",
                  "download_retry": "Download interrupted; waiting to resume retained partial file",
                  "download_failed": "Snapshot file preparation failed"}[phase]
    elif phase in {"loading", "load_requested"}:
        unit = "utxos"
        stage = imported.get("phase", "loading")
        phase = "core_" + stage
        detail = {"reading": "Reading UTXOs (Core log)",
                  "flushing_cache": "Writing UTXO batch to disk",
                  "flushing": "Writing snapshot chainstate to disk; UTXO reading finished",
                  "verifying": "Verifying snapshot UTXO hash",
                  "activating": "Waiting for RPC confirmation of snapshot activation",
                  "loading": "Core import in progress; detailed progress unavailable"}[stage]
        current = imported.get("imported_coins")
        if stage == "reading":
            total, percent = imported.get("total_coins"), imported.get("progress_percent")
        elif stage in {"verifying", "activating"}:
            state = "VERIFYING"
        # Do not turn a completed byte/coin counter into readiness or a 100% hash check.
    elif phase in {"file_verified", "file_published"}:
        detail = "Download verified; waiting for Core snapshot activation"
    elif phase == "waiting_for_core":
        detail = "Waiting for Bitcoin Core startup"
    elif phase == "waiting_for_headers":
        detail = "Waiting for baseline block header before Core import"
    elif phase == "waiting_for_rpc":
        detail = "Waiting for Core RPC before snapshot activation"
    if complete:
        state, phase, detail, percent = "READY", "ready", "Snapshot baseline and raw file ready", 100.0
        report = details.get("report")
        if isinstance(report, dict) and report.get("snapshot_file_reused") is True:
            detail = "Existing snapshot baseline reused; no file rescan needed"
        current = total = None
    elif activated and not failed and state not in {"FAILED", "BLOCKED"}:
        if phase.startswith("core_") or phase in {"snapshot_active", "fully_validated_chain"}:
            state, phase, detail = "STARTING", "core_activating", "Snapshot active; confirming readiness"
            current = total = percent = None
    item = node._component_progress("snapshot", "FAILED" if failed else state, detail,
                                   current=current, total=total, progress_percent=percent, unit=unit)
    item.update(label="UTXO snapshot", progress_phase=phase)
    if phase == "waiting_for_headers" and type(base_height) is int:
        item["baseline_header_height"] = base_height
    if phase.startswith("core_"):
        start = imported.get("stage_started_at", latest.get("phase_started_at"))
        if type(start) in (int, float) and math.isfinite(start):
            item["stage_elapsed_secs"] = int(max(0, time.time() - start))
        item["progress_source"] = "core_log" if imported else "activation_journal"
    return item


def _snapshot_file_milestone(download, artifact, expected):
    """Retain file completion across activation phases without rescanning its bytes."""
    phase = download.get("phase")
    if phase not in {"verifying_file", "file_verified", "file_published"}:
        return None
    identity = download.get("identity", {})
    snapshot = identity.get("snapshot", {}) if isinstance(identity, dict) else {}
    details = download.get("details", {})
    if (download.get("schema_version") != "usdb-bitcoin-assumeutxo:v1"
            or not isinstance(snapshot, dict) or not isinstance(details, dict)
            or any(snapshot.get(key) != expected[key] for key in ("base_height", "base_hash", "file_sha256"))
            or snapshot.get("size_bytes") != UTXO_SIZE or details.get("total_bytes") != UTXO_SIZE):
        return None
    if phase != "verifying_file" and details.get("bytes") != UTXO_SIZE:
        return None
    candidates = [artifact]
    if phase != "file_published":
        candidates.append(artifact.with_name(artifact.name + ".download") / "snapshot.part")
    for path in candidates:
        try:
            if not path.is_symlink() and path.is_file() and path.stat().st_size == UTXO_SIZE:
                return dict(state="VERIFYING" if phase == "verifying_file" else "VERIFIED", size_bytes=UTXO_SIZE)
        except OSError:
            continue
    return None


def _height(value):
    """Read optional nonnegative block heights without treating booleans as integers."""
    return value if type(value) is int and value >= 0 else None


def _recorded_preparation_complete(activation, expected):
    """Recognize a completed preparation job for display, never for live readiness."""
    identity = activation.get("identity", {})
    snapshot = identity.get("snapshot", {}) if isinstance(identity, dict) else {}
    details = activation.get("details", {})
    report = details.get("report", {}) if isinstance(details, dict) else {}
    return (activation.get("schema_version") == "usdb-bitcoin-assumeutxo:v1"
            and activation.get("phase") in {"snapshot_active", "fully_validated_chain"}
            and isinstance(snapshot, dict)
            and all(snapshot.get(key) == expected[key] for key in ("base_height", "base_hash", "file_sha256"))
            and isinstance(report, dict) and report.get("bootstrap_ready") is True)


def _balance_history_progress(item, readiness, bootstrap, core, activation, *, base, origin, stable_lag, max_height=0xFFFFFFFF):
    """Use one block range across origin replay and live sync, with an origin milestone."""
    if item["state"] in {"FAILED", "BLOCKED"}:
        return item
    phase = bootstrap.get("phase", "starting") if readiness is None else readiness.get("phase")
    height = _height(bootstrap.get("height")) if readiness is None else _height(readiness.get("stable_height"))
    if readiness is not None and height is None and phase in {"Indexing", "Synced"}:
        height = _height(readiness.get("current"))
    if readiness is not None and height is not None and height >= origin:
        milestone = "available" if readiness.get("query_ready") else "initializing"
    elif phase == "verifying":
        milestone = "verifying"
    elif phase == "sealed":
        milestone = "sealed; waiting for service RPC"
    else:
        milestone = "replaying" if height is not None else "pending"
    item["genesis_milestone"] = dict(height=origin, state=milestone,
        remaining_blocks=max(0, origin - height) if height is not None else None)
    item["sync_start_height"] = base
    item["stable_lag_blocks"] = stable_lag
    if max_height != 0xFFFFFFFF:
        item["sync_max_height"] = max_height
    # Once published, the journal only describes the baseline. It cannot report
    # the current service height during an RPC outage after normal sync begins.
    if readiness is None and phase == "sealed":
        item.update(current=None, total=None, progress_percent=None,
                    detail="Baseline sealed; waiting for service RPC")
        return item
    if height is None or height < base:
        return item
    target, source = _height(core.get("headers")), "bitcoin_headers"
    if target is not None:
        target = max(0, target - stable_lag)
    if target is None or target < origin:
        target = _height(readiness.get("total")) if readiness is not None and phase in {"Indexing", "Synced"} else None
        source = "balance_history_rpc"
    if target is None or target < origin:
        details = activation.get("details", {})
        report = details.get("report", {}) if isinstance(details, dict) else {}
        target = _height(report.get("headers")) if isinstance(report, dict) else None
        if target is not None:
            target = max(0, target - stable_lag)
        source = "last_observed_bitcoin_headers"
    if target is not None:
        target = min(target, max_height)
    if target is None or target < max(origin, height):
        target, source = None, "unavailable"
    percent = (height - base) * 100 / (target - base) if target is not None and target > base else None
    item.update(current=height, total=target, progress_percent=percent, unit="blocks", sync_target_source=source)
    if item["state"] == "READY" and target is not None and height < target:
        item.update(state="WAITING", detail="Caught up with available blocks; waiting for Bitcoin")
    if readiness is None:
        item["detail"] = {"replaying": "Replaying blocks toward genesis baseline",
                          "waiting_for_blocks": "Waiting for Bitcoin blocks or undo data",
                          "verifying": "Verifying genesis baseline before publication",
                          "sealed": "Baseline sealed; waiting for service RPC"}.get(phase, item["detail"])
        if phase == "verifying":
            verification = node.node_observation.bootstrap_progress(bootstrap)
            stage = {"utxos_and_balance_aggregation": "scanning UTXOs and rebuilding balances",
                     "origin_utxos": "scanning stored UTXOs", "origin_balances": "scanning stored balances",
                     "aggregate_utxos": "rebuilding balances from UTXOs", "compare_balances": "checking rebuilt balances"}.get(
                         verification.get("verification_stage"))
            scanned, total = verification.get("verification_scanned"), verification.get("verification_total")
            if stage and scanned is not None:
                count = f"{scanned:,}" + (f" / {total:,}" if total is not None else "")
                item["detail"] = f"Verifying genesis baseline: {stage}; records {count}"
    return item


def _chain_wait_details(core, loader, readiness, startup=None):
    """Describe every unmet startup gate; an unknown Core probe must not hide data waits."""
    details = []
    if startup:
        details.append("Waiting for " + bitcoin_startup_progress.detail(startup))
    elif core.get("error") or core.get("rpc_available") is False:
        label = ("Bitcoin Core readiness check failed" if core.get("error_kind") == "identity_or_configuration"
                 else "Bitcoin Core readiness unknown")
        details.append(label + ": " + (core.get("error") or "Core RPC unavailable"))
    elif core.get("bootstrap_ready") is not True:
        details.append("Waiting for Bitcoin snapshot baseline activation")
    if loader.get("state") != "exited" or loader.get("exit_code") != 0:
        details.append("Waiting for UTXO snapshot preparation to complete")
    if (not core.get("error") and core.get("rpc_available") is not False
            and core.get("bootstrap_ready") is True and core.get("tip_ready") is not True):
        current, target = _height(core.get("active_height")), _height(core.get("headers"))
        if current is not None and target is not None and current < target:
            remaining = target - current
            unit = "block" if remaining == 1 else "blocks"
            details.append(f"Waiting for Bitcoin foreground: {remaining} {unit} remaining ({current}/{target})")
        else:
            details.append("Waiting for Bitcoin foreground readiness: checking minimum height, tip freshness and peer connections")
    for service in ("balance-history", "usdb-indexer"):
        report, error = readiness[service]
        if report is None:
            details.append(f"Waiting for {service} readiness: {error or 'RPC unavailable'}")
        elif report.get("consensus_ready") is not True:
            blockers = ", ".join(str(value) for value in (report.get("blockers") or []))
            details.append(f"Waiting for {service} readiness: {blockers or report.get('message') or 'synchronization in progress'}")
    return details


def _chain_wait_detail(core, loader, readiness):
    """Keep the first-gate summary for existing consumers of the detail field."""
    details = _chain_wait_details(core, loader, readiness)
    return details[0] if details else "Upstream ready; waiting for controller to start USDB chain"


def collect_native_progress(layout, *, controller_state: str | None = None) -> dict:
    """Expose import/replay plus independent Core foreground/background observations."""
    env = node.read_env(layout.node_env)
    services_available = True
    try:
        services = node._collect_compose_services(layout, command_timeout_secs=8, include_started_at=True)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        services = {}
        services_available = False
    btc_service = services.get("btc-node")
    not_started = services_available and (btc_service is None or (
        btc_service.get("state") == "created" and not node._container_start_failed(btc_service)))
    try:
        # Missing containers are a startup wait, not a malformed RPC response.
        # A failed inventory is not evidence that the container is absent.
        core = dict(bootstrap_ready=False, tip_ready=False) if not_started else core_progress(layout)
    except (OSError, subprocess.TimeoutExpired) as error:
        core = dict(error=str(error), error_kind="rpc_unavailable", rpc_available=False)
    except CoreProbeError as error:
        core = dict(error=str(error), error_kind="probe_failed", rpc_available=False)
    except ValueError as error:
        core = dict(error=str(error), error_kind="identity_or_configuration")
    unavailable = (core.get("error_kind") != "identity_or_configuration"
                   and (core.get("error_kind") in {"rpc_unavailable", "probe_failed"} or core.get("rpc_available") is False))
    observed_time = time.time()
    startup = bitcoin_startup_progress.observe(core, node.node_observation.runtime(btc_service),
        Path(env["BTC_NODE_DATA_HOST_DIR"]) / "debug.log", observed_time)
    artifact = Path(env["BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR"]) / "mainnet-935000-utxos.dat"
    download = read_progress(artifact.with_name(artifact.name + ".download") / "progress.json")
    activation = read_progress(Path(env["BTC_ASSUMEUTXO_STATE_HOST_DIR"]) / "activation.json")
    bootstrap = read_progress(Path(env["BH_DATA_HOST_DIR"]) / "bootstrap-progress.json")
    loader = services.get("btc-snapshot-bootstrap", {})
    latest = download if download.get("updated_at", 0) > activation.get("updated_at", 0) else activation
    phase = latest.get("phase", "waiting_for_core")
    failed = loader.get("state") in {"dead", "restarting", "paused"} or (loader.get("state") == "exited" and loader.get("exit_code") not in (None, 0))
    # RPC probes may straddle two tips after activation. Keep the observed chain
    # identity distinct from the stricter readiness result of that probe. Snapshot
    # preparation can stay ready; Bitcoin and controller gates still use readiness.
    activated = core.get("snapshot_active") is True or core.get("bootstrap_ready") is True
    recorded = latest is activation and _recorded_preparation_complete(activation, layout.snapshot["contract"]["snapshot"])
    complete = (activated or (unavailable and recorded)) and loader.get("state") == "exited" and loader.get("exit_code") == 0
    imported = {}
    if phase in {"loading", "load_requested"} and not failed and not core.get("bootstrap_ready"):
        process_start = timestamp(services.get("btc-node", {}).get("started_at"))
        attempt_start = activation.get("phase_started_at", activation.get("started_at"))
        if process_start is not None and type(attempt_start) in (int, float) and math.isfinite(attempt_start):
            imported = read_import_progress(Path(env["BTC_NODE_DATA_HOST_DIR"]) / "debug.log",
                layout.snapshot["contract"]["snapshot"]["base_hash"], max(process_start, attempt_start))
    expected = layout.snapshot["contract"]["snapshot"]
    snapshot = _snapshot_component(phase, latest, imported, failed=failed, complete=complete, activated=activated,
                                   base_height=expected["base_height"])
    download_details = download.get("details", {})
    if not isinstance(download_details, dict):
        download_details = {}
    if phase in {"downloading", "download_retry"} and not failed:
        snapshot["preparation_details"] = _download_retry_details(download_details, waiting=phase == "download_retry")
    if failed:
        snapshot["detail"] = _preparation_failure(loader)
        lines = [f"Interrupted phase: {phase}"]
        if phase == "download_failed":
            lines.extend(_download_retry_details(download_details, waiting=False))
            if download_details.get("retry_exhausted") is True:
                lines.append("Automatic download retry paused: no-progress time limit reached")
        if phase == "download_failed" and download_details.get("error"):
            lines.append("Last file preparation error: " + str(download_details["error"]))
        # The last progress journal can lag SIGKILL. File length is resumable bytes,
        # never proof that the contents have passed the pinned SHA-256 check.
        if _download_identity_matches(download, expected):
            part = artifact.with_name(artifact.name + ".download") / "snapshot.part"
            try:
                size = part.stat().st_size if part.is_file() and not part.is_symlink() else None
            except OSError:
                size = None
            if type(size) is int and 0 <= size <= UTXO_SIZE:
                lines.append(f"Retained partial file: {node._human_bytes(size)}; SHA-256 not yet verified")
                if phase in {"checking_file", "downloading", "download_retry", "download_failed"}:
                    snapshot.update(current=size, total=UTXO_SIZE, unit="bytes", progress_percent=None)
        lines.append("Logs: usdb-node logs --bitcoin btc-snapshot-bootstrap")
        lines.append("Action: correct the failure, then usdb-node up to resume file preparation; preserve Bitcoin data"
                     if _preparation_retryable(layout, env, loader) else
                     "Action: inspect the failure before explicit recovery; automatic retry is disabled")
        if phase in {"loading", "load_requested", "load_uncertain", "load_failed"}:
            lines[-1] = "Action: inspect Core import before explicit recovery; automatic load retry is disabled"
        snapshot["preparation_details"] = lines
    file_milestone = _snapshot_file_milestone(download, artifact, expected)
    if file_milestone:
        snapshot["file_preparation"] = file_milestone
    # Preparation is a completed job; current Core health is shown separately.
    # Require both a matching completion record and a successful loader exit when
    # RPC is unavailable. Neither observation authorizes startup or mining.
    if not failed and core.get("error_kind") == "identity_or_configuration":
        snapshot.update(state="BLOCKED", detail=core["error"], progress_percent=None)
    elif complete and unavailable:
        snapshot.update(detail="Snapshot preparation completed; live Core status shown below",
                        completion_source="bootstrap_job", completion_observed_at=activation.get("updated_at"))
    elif not failed and unavailable:
        if phase in {"snapshot_active", "fully_validated_chain"}:
            snapshot.update(state="STARTING", display_state="UNAVAILABLE", progress_phase="observation_unavailable",
                            observation_unavailable=True, detail="Snapshot completion not confirmed; Core probe unavailable",
                            current=None, total=None, progress_percent=None)
        if startup and snapshot["state"] in {"WAITING", "STARTING"}:
            snapshot.update(state="WAITING", detail="Waiting for Core initialization before confirming snapshot baseline")
            snapshot.pop("display_state", None)
            snapshot.pop("observation_unavailable", None)
    snapshot["observation_identity"] = [layout.release_id, services.get("btc-node", {}).get("started_at"),
                                        activation.get("started_at")]
    components = [snapshot, node._component_progress("script_registry", "SKIPPED", "Native observed-script registry is maintained by balance-history")]
    pre_snapshot = not activated and core.get("rpc_available") is not False and not core.get("error") and type(core.get("active_height")) is int
    sync_phase = "not_started" if not_started else "foreground" if activated else "pre_snapshot_ibd" if pre_snapshot else "unavailable"
    sync_detail = (f"foreground={core.get('active_height')}; background={core.get('background_height')}; history_validated={core.get('history_validated', False)}"
                   if activated else "Before snapshot activation: ordinary block sync" if pre_snapshot else "Waiting for Core chainstate observation")
    bitcoin = node._component_progress("bitcoin", "STARTING" if unavailable else "BLOCKED" if core.get("error") else "READY" if core.get("tip_ready") else "SYNCING",
                      core.get("error") or sync_detail,
                      current=core.get("active_height"), total=core.get("headers"))
    bitcoin["progress_phase"] = sync_phase
    if not_started:
        bitcoin.update(state="WAITING", detail="Bitcoin Core container has not started")
    bitcoin["observation_identity"] = [layout.release_id, env["BTC_NODE_DATA_HOST_DIR"],
                                        services.get("btc-node", {}).get("started_at")]
    bitcoin["rpc_available"] = False if unavailable else True if not not_started and not core.get("error") else None
    if startup:
        bitcoin.update(state="STARTING", progress_phase="initializing", startup_progress=startup,
                       detail=bitcoin_startup_progress.detail(startup),
                       current=None, total=None, progress_percent=startup.get("progress_percent"),
                       stage_elapsed_secs=max(0, int(observed_time - startup["stage_started_at_ms"] / 1000)))
    elif unavailable:
        bitcoin.update(observation_unavailable=True, display_state="UNAVAILABLE")
    if pre_snapshot:
        bitcoin["label"] = "Bitcoin (IBD)"
    chains = core.get("chainstates", [])
    if chains and isinstance(chains[-1], dict):
        bitcoin["verification_progress"] = chains[-1].get("verificationprogress")
    # This observation is separate from foreground readiness and never gates startup.
    bitcoin["background_validation"] = dict(height=core.get("background_height"),
        target=int(env["BH_ASSUMEUTXO_BASE_HEIGHT"]), validated=core.get("history_validated") is True,
        available=not not_started and not core.get("error") and core.get("rpc_available") is not False,
        waiting_for_start=not_started, waiting_for_initialization=bool(startup))
    if node._container_start_failed(btc_service) or (btc_service or {}).get("state") in {"dead", "exited", "restarting", "paused"}:
        bitcoin.update(state="FAILED", detail="Core is not running; inspect its persistent log")
        bitcoin.pop("display_state", None)
        bitcoin.pop("observation_unavailable", None)
    components.append(bitcoin)
    readiness_reports = {}
    for component, service, action in (("balance_history", "balance-history", "data-status"), ("usdb_indexer", "usdb-indexer", "indexer-status")):
        readiness, error = node._read_service_readiness(layout, "run_testnet_runtime.sh", [action], service)
        readiness_reports[service] = (readiness, error)
        item = node._indexed_service_component(component, services.get(service), readiness, error, "waiting for native service startup")
        if component == "usdb_indexer" and item.get("progress_phase") == "waiting_for_upstream":
            item["upstream_baseline_height"] = int(env["USDB_GENESIS_BLOCK_HEIGHT"])
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
            # The service starts RPC only after native bootstrap. Preserve the
            # journal's real update time; collecting it again is not progress.
            modified = bootstrap.get("observed_file_mtime")
            item["bootstrap_progress"] = node.node_observation.bootstrap_progress({
                **bootstrap, "updated_at_ms": int(modified * 1000)
                if type(modified) in (int, float) and math.isfinite(modified) else None})
        if component == "balance_history" and services.get(service, {}).get("state") == "running":
            item = _balance_history_progress(item, readiness, bootstrap, core, activation,
                base=int(env["BH_ASSUMEUTXO_BASE_HEIGHT"]), origin=int(env["USDB_GENESIS_BLOCK_HEIGHT"]),
                stable_lag=node.btc_registry_stable_lag_blocks(layout.network_identity["btc_activation_registry_id"]),
                max_height=int(env.get("BH_SYNC_MAX_SYNC_BLOCK_HEIGHT", 0xFFFFFFFF)))
        item["readiness"] = node.node_observation.readiness(readiness)
        if startup and item["state"] == "WAITING" and services.get(service) is None:
            item["detail"] = ("Waiting for Bitcoin Core initialization before native service startup" if component == "balance_history"
                              else "Waiting for balance-history after Bitcoin Core initialization")
        components.append(item)
    chain = node._chain_component(layout, env, services.get("usdb-chain"))
    waiting_details = (_chain_wait_details(core, loader, readiness_reports, startup)
                       if chain["state"] == "WAITING" and services.get("usdb-chain") is None else None)
    gate = node._chain_startup_gate_component(services)
    if gate and (gate["state"] == "FAILED" or chain["state"] not in {"FAILED", "BLOCKED"}):
        chain.update(gate)
    components.append(chain)
    resources, resource_waiting = node._resource_progress(layout, env, services, components)
    if waiting_details is not None and gate is None and chain["state"] in {"WAITING", "STARTING"}:
        # Managed restart annotations must not hide the upstream gate still pending.
        prefix = chain["detail"] + "; " if chain["state"] == "STARTING" else ""
        chain["detail"] = prefix + (waiting_details[0] if waiting_details else "Upstream ready; waiting for controller to start USDB chain")
        if waiting_details:
            chain["startup_wait_details"] = [prefix + waiting_details[0], *waiting_details[1:]]
    overall = node._overall_progress_state(components)
    if resources.get("error"):
        overall = "BLOCKED"
    elif overall == "READY" and resource_waiting:
        overall = "STARTING"
    mining, overall = node._mining_progress(layout, services, chain, components, overall)
    observed_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    for item in components:
        service = services.get({"bitcoin": "btc-node", "balance_history": "balance-history", "usdb_indexer": "usdb-indexer", "usdb_chain": "usdb-chain"}.get(item["id"]), {})
        item["runtime"] = node.node_observation.runtime(service if services_available else {"status": "unavailable"})
        elapsed = node.service_elapsed(service.get("started_at"), observed_at)
        if service.get("state") == "running" and elapsed is not None:
            item.update(service_started_at=service["started_at"], service_elapsed_secs=elapsed)
    return dict(schema_version=node.NODE_PROGRESS_SCHEMA_VERSION, release_id=layout.release_id, network_bundle_id=layout.bundle_id,
                observed_at=observed_at, controller_state=(controller_state if controller_state is not None else node.controller_observed_state(layout)),
                overall_state=overall, auxiliary_state="READY", components=components, resources=resources, mining=mining,
                control_plane=node._control_plane_progress(services, observation_available=services_available),
                native_bootstrap=dict(core=core, download=download, activation=activation, import_progress=imported, balance_history=bootstrap))
