"""Observe Core warmup without turning logs or Docker health into readiness gates."""

import math
import re

from bitcoin_import_progress import read_log_progress, timestamp


PHASES = {
    "starting": "initializing (phase not reported)",
    "loading_block_index": "loading block index",
    "loading_wallet": "loading wallet",
    "validating_snapshot": "validating AssumeUTXO snapshot (computing UTXO statistics)",
    "reinitializing_chainstate": "reinitializing validated chainstate",
    "verifying_blocks": "verifying recent blocks",
}


def _observe(line, state, since, now):
    """Only a phase transition or a growing counter advances the progress clock."""
    parts = line.split(" ", 1)
    observed = timestamp(parts[0])
    if len(parts) != 2 or observed is None or not since <= observed <= now:
        return
    message = parts[1]
    phase = None
    if "init message: Loading block index" in message:
        phase = "loading_block_index"
    elif "init message: Loading wallet" in message:
        phase = "loading_wallet"
    elif "[snapshot] computing UTXO stats for background chainstate to validate snapshot" in message:
        phase = "validating_snapshot"
    elif "[snapshot]" in message and any(value in message for value in (
            "has been fully validated", "cleaning up unneeded background chainstate",
            "deleting background chainstate directory", "moving snapshot chainstate")):
        phase = "reinitializing_chainstate"
    elif "init message: Verifying blocks" in message:
        phase = "verifying_blocks"
    elif "init message: Done loading" in message:
        phase = "completed"
    counter = re.search(r"Verification progress: (\d+)%", message)
    percent = int(counter[1]) if counter and int(counter[1]) <= 100 else None
    if percent is not None:
        phase = "verifying_blocks"
    if phase and observed >= state.get("updated_at", since):
        if phase != state.get("phase"):
            state.clear()
            state.update(phase=phase, stage_started_at=observed, updated_at=observed)
        if percent is not None and percent > state.get("progress_percent", -1):
            state.update(progress_percent=percent, updated_at=observed)


def project(value):
    """Sanitize startup evidence before persistence or web export; omit raw log text."""
    if not isinstance(value, dict) or value.get("phase") not in PHASES or value.get("rpc_code") != -28:
        return {}
    fields = ("process_started_at_ms", "stage_started_at_ms", "last_progress_at_ms")
    if any(type(value.get(key)) is not int or value[key] <= 0 for key in fields):
        return {}
    if not value[fields[0]] <= value[fields[1]] <= value[fields[2]]:
        return {}
    result = {"phase": value["phase"], "rpc_code": -28, **{key: value[key] for key in fields}}
    percent = value.get("progress_percent")
    if value["phase"] == "verifying_blocks" and type(percent) is int and 0 <= percent <= 100:
        result["progress_percent"] = percent
    return result


def running_since(runtime):
    """Require an identifiable live run without exit/OOM evidence."""
    if (runtime.get("state") != "running" or runtime.get("details_available") is not True
            or not runtime.get("container_id") or runtime.get("oom_killed") is not False
            or runtime.get("exit_code") not in (None, 0)):
        return None
    started = timestamp(runtime.get("started_at"))
    return started if started is not None and math.isfinite(started) and started > 0 else None


def observe(core, runtime, path, now):
    """Read a bounded current-run log tail only for an explicit RPC warmup response."""
    failure = core.get("rpc_failure") or {}
    started = running_since(runtime)
    if (started is None or started > now or core.get("error_kind") != "rpc_unavailable"
            or not isinstance(failure, dict)
            or failure.get("kind") != "warmup" or failure.get("code") != -28):
        return {}
    progress = read_log_progress(path, started, ("startup",),
                                 lambda line, state, since: _observe(line, state, since, now))
    if progress.get("phase") == "completed":
        return {}
    # Core logs have second precision; never move before the container start.
    stage = max(started, progress.get("stage_started_at", started))
    updated = max(stage, progress.get("updated_at", stage))
    return project(dict(phase=progress.get("phase", "starting"), rpc_code=-28,
                        process_started_at_ms=int(started * 1000), stage_started_at_ms=int(stage * 1000),
                        last_progress_at_ms=int(updated * 1000), progress_percent=progress.get("progress_percent")))


def detail(progress):
    """A safe stage label for CLI summaries and downstream dependency waits."""
    return "Bitcoin Core initializing: " + PHASES[progress["phase"]] + "; RPC warmup (-28)"
