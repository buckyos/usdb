#!/usr/bin/env python3
"""Export sanitized host observations without giving the web server host control."""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
import secrets
import subprocess
import tempfile
import time

from control_plane_resources import ResourceCollector
import node_observation

SCHEMA = "usdb-console-monitor:v1"
# Deliberate projection: never export node.env, credentials, raw RPC errors,
# artifacts, wallet secrets, or Docker inspect output. The separate resource
# collector exposes only allowlisted service data paths requested by operators.
COMPONENT_FIELDS = ("id", "label", "state", "display_state", "progress_phase", "current", "total",
                    "progress_percent", "unit", "observation_unavailable", "verification_progress",
                    "query_ready", "consensus_ready", "stage_elapsed_secs")


def select(source, fields):
    """Copy only bounded scalar observations from the versioned progress report."""
    if not isinstance(source, dict):
        return {}
    return {key: value[:256] if isinstance(value, str) else value for key in fields
            if (value := source.get(key)) is not None and isinstance(value, (str, bool, int, float))}


def project(report: dict, now_ms: int) -> dict:
    """Keep completion milestones and background validation independent of readiness."""
    result = dict(schema_version=SCHEMA, observed_at_ms=now_ms, observation_available=True,
                  **select(report, ("release_id", "bundle_id", "overall_state", "node_role", "observed_at")))
    result["network"] = select(report.get("network"), ("name", "chain_id", "network_id", "genesis_hash", "bitcoin_network"))
    result["controller"] = select(report.get("controller"), (
        "state", "runtime_state", "display_state", "observation_available", "restart_count",
        "result", "exit_code", "exit_status",
    ))
    result["resources"] = select(report.get("resources"), ("mode", "phase", "target_phase", "transition_pending", "runtime_adopted", "host_memory_bytes", "external_services_bytes"))
    result["mining"] = select(report.get("mining"), ("state", "enabled", "drift", "observation_unavailable", "role", "configured_role", "runtime_role"))
    from ord_runtime import FIELDS
    result["minting"] = select(report.get("minting"), (*FIELDS, "enabled", "backend_ready", "transactions_enabled"))
    result["components"] = []
    for component in report.get("components", [])[:32]:
        item = select(component, COMPONENT_FIELDS)
        item["background_validation"] = select(component.get("background_validation"), ("height", "target", "validated", "available", "waiting_for_start"))
        item["file_preparation"] = select(component.get("file_preparation"), ("state", "download_complete", "sha256_verified", "size_bytes", "completed_bytes", "total_bytes", "base_height"))
        result["components"].append(item)
    if report.get("control_plane"):
        result["components"].append(select(report["control_plane"], COMPONENT_FIELDS))
    result["observations"] = node_observation.project(report.get("observations"))
    return result


def data_root(layout, node) -> Path:
    """Resolve the operator-owned directory shared only with the control plane."""
    # Keep new observer files operator-owned even when an older Docker deployment
    # created the service data directory as root. The container mounts this read-only.
    return layout.node_env.resolve().parent / "console"



def prepare(layout, node) -> Path:
    """Create credentials as the node operator before Docker can create root-owned files."""
    root = data_root(layout, node)
    if root.is_symlink():
        raise ValueError("Refusing symlinked console state directory")
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    token = root / "access-token"
    if not token.exists():
        descriptor, temporary_name = tempfile.mkstemp(prefix=".access-token-", dir=root)
        try:
            with os.fdopen(descriptor, "w", encoding="ascii") as stream:
                stream.write(secrets.token_hex(32) + "\n")
                stream.flush()
                os.fsync(stream.fileno())
            try:
                # Publish complete bytes without overwriting a concurrent creator.
                os.link(temporary_name, token)
            except FileExistsError:
                pass
        finally:
            Path(temporary_name).unlink(missing_ok=True)
    read_token(token)
    return root


def read_token(path: Path) -> str:
    """Read only a valid owner-private token, never follow a redirected secret file."""
    if path.is_symlink() or not path.is_file():
        raise ValueError("Refusing unsafe console access-token path")
    metadata = path.stat()
    if metadata.st_size > 128 or metadata.st_mode & 0o077:
        raise ValueError("Console access-token must be a small private file with mode 0600")
    value = path.read_text(encoding="ascii").strip()
    if len(value) != 64 or any(character not in "0123456789abcdefABCDEF" for character in value):
        raise ValueError("Invalid console access token")
    return value


def collect(layout, node, collector=None, *, include_resources=True) -> dict:
    """Collect a sanitized snapshot; the core monitor owns persistence and publication."""
    observed = int(time.time() * 1000)
    env = {}
    try:
        report = project(node.collect_node_progress(layout), observed)
        # Expose configured ceilings, never the rest of the private environment.
        if layout.node_env.is_file():
            from resource_policy import SERVICE_MEMORY_KEYS, memory_bytes
            env = node.read_env(layout.node_env)
            # Public configuration only; this does not attest to a wallet owner or runtime coinbase.
            miner_address = env.get("USDB_MINER_ADDRESS", "").strip()
            if re.fullmatch(r"0x[0-9a-fA-F]{40}", miner_address):
                report["node_identity"] = {"configured_miner_address": miner_address}
            report["resources"]["configured_limits_bytes"] = {
                service: memory_bytes(env[key], key) for service, key in SERVICE_MEMORY_KEYS.items() if key in env and (service != "ord-server" or env.get("USDB_MINTING_ENABLED") == "1")
            }
            host = env.get("USDB_RESOURCE_HOST_MEMORY_BYTES", "")
            if host.isdigit():
                report["resources"]["host_memory_bytes"] = int(host)
    except (OSError, ValueError, subprocess.SubprocessError):
        report = dict(schema_version=SCHEMA, observed_at_ms=observed, observation_available=False,
                      overall_state="UNAVAILABLE", components=[])
    # A daemon uses a persistent resource sampler; isolated service probes must
    # not start a new expensive directory scan on every iteration.
    if not include_resources:
        return report
    # Resource observation failures do not erase node readiness or minting progress.
    try:
        report["host_resources"] = (collector or ResourceCollector()).sample(
            env, layout.bundle_id, wait_for_disk=collector is None)
    except (OSError, ValueError, subprocess.SubprocessError):
        report["host_resources"] = dict(schema_version="usdb-console-resources:v1", status="unavailable")
    return report


def publish(layout, node, report) -> None:
    """Publish a complete, bounded snapshot on the existing read-only console mount."""
    root = prepare(layout, node)
    payload = json.dumps(report, allow_nan=False) + "\n"
    if len(payload.encode()) > 256 * 1024:
        raise ValueError("Monitor console snapshot exceeds its size limit")
    node._atomic_write_private(root / "node-progress.json", payload)


def export(layout, node, collector=None) -> dict:
    """Keep one-shot export compatible without replacing the core daemon's snapshot."""
    report = collect(layout, node, collector)
    import node_monitor
    if not node_monitor.is_running(layout):
        report["monitor"] = node_monitor.status(layout, node)
        publish(layout, node, report)
    return report


def run(layout, node) -> None:
    """Compatibility entrypoint; all persistent observation uses the core monitor."""
    import node_monitor
    node_monitor.run(layout, node)


def legacy_unit_name(layout) -> str:
    return f"usdb-console-monitor-{layout.bundle_id}.service"


def unit_path(layout, node) -> Path:
    return node.controller_unit_path(layout).with_name(unit_name(layout))


def legacy_render_unit(layout, node, context) -> str:
    """Include the release identity so upgrades reload long-running observer code."""
    quote = node._systemd_quote
    command = " ".join(quote(str(value)) for value in (context.launcher, "--node-env", layout.node_env, "console", "monitor"))
    return f"""[Unit]
Description=USDB private console observer ({layout.bundle_id})
After=docker.service
Requires=docker.service

[Service]
Type=simple
User={context.service_user}
Environment={quote(f'HOME={context.home}')}
Environment=PYTHONDONTWRITEBYTECODE=1
Environment={quote(f'USDB_CONSOLE_MONITOR_RELEASE={layout.release_id}')}
ExecStart={command}
Restart=on-failure
RestartSec=10s
TimeoutStopSec=100s
UMask=0077

[Install]
WantedBy=multi-user.target
"""

def install(layout, node, context) -> None:
    """Compatibility wrapper for the core monitor service."""
    import node_monitor
    node_monitor.install(layout, node, context)


def start(layout, node) -> bool:
    """Request observer startup; return False when its systemd unit is missing."""
    import node_monitor
    if not node_monitor.enabled(layout, node):
        node_monitor.publish_state(layout, node, "disabled")
        return False
    prepare(layout, node)
    if unit_path(layout, node).is_file():
        node._privileged_command(["systemctl", "start", "--no-block", unit_name(layout)])
        return True
    return False


def stop(layout, node) -> None:
    import node_monitor
    node_monitor.stop(layout, node)


# Keep old Python callers on the same service, without running a second collector.
def unit_name(layout):
    import node_monitor
    return node_monitor.unit_name(layout)


def render_unit(layout, node, context):
    import node_monitor
    return node_monitor.render_unit(layout, node, context)


def add_parser(subparsers) -> None:
    parser = subparsers.add_parser("console", help="Private monitoring console and host observer")
    actions = parser.add_subparsers(dest="console_action", required=True)
    token = actions.add_parser("token", help="Print the private login token (keep it secret)")
    token.add_argument("--raw", action="store_true", help="Print only the token on stdout for scripts")
    actions.add_parser("start", help="Start only the private console and installed observer")
    actions.add_parser("export", help="Export one sanitized node progress snapshot")
    actions.add_parser("monitor", help="Continuously export progress; normally managed by systemd")


def dispatch(args, layout, node) -> None:
    if args.console_action == "token":
        token = data_root(layout, node) / "access-token"
        if not token.is_file():
            raise ValueError("Console has not started; run usdb-node console start first")
        value = read_token(token)
        print(value if args.raw else f"Console access token: {value}")
    elif args.console_action == "export":
        print(json.dumps(export(layout, node), indent=2))
    elif args.console_action == "monitor":
        run(layout, node)
    elif args.console_action == "start":
        monitoring = start(layout, node)
        node.run_helper(layout, "run_testnet_runtime.sh", ["up-console"])
        print("Private console started. Use an SSH tunnel to localhost:28040 and usdb-node console token to sign in.")
        if not monitoring:
            import node_monitor
            if not node_monitor.enabled(layout, node):
                print("Node monitoring is disabled by configuration; local event history is retained.")
                return
            print("WARNING: host monitoring is not installed; without a separate node monitor process, "
                  "node and Ord observations will be missing or become stale. "
                  "Run usdb-node controller install, then usdb-node console start; "
                  "or keep usdb-node monitor run (legacy: usdb-node console monitor) running in a separate terminal.")
