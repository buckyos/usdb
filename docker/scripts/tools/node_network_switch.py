"""Discover network changes and retire stopped, overlapping deployments safely."""

from contextlib import ExitStack
from dataclasses import replace
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

import node_background_services as background
import node_rebuild as core
from runtime_compatibility import PERSISTENT_DATA_SERVICES, build_persistent_data_paths

BUNDLE = re.compile(r"usdb-(?:testnet|mainnet)-v[0-9]+")


def configurations(layout, node):
    """Read only this operator's standard bundle configs; never expose env values in errors."""
    roots = {Path.home() / ".config/usdb"}
    if BUNDLE.fullmatch(layout.node_env.parent.name):
        roots.add(layout.node_env.parent.parent)
    result = []
    for root in sorted(roots):
        if not root.exists():
            continue
        core.safe_path(root)
        for directory in sorted(root.iterdir()):
            if not BUNDLE.fullmatch(directory.name) or directory.name == layout.bundle_id:
                continue
            path = directory / "node.env"
            if not path.exists() and not path.is_symlink():
                continue
            try:
                core.safe_path(path)
                info = path.stat()
                if not path.is_file() or info.st_uid != os.getuid() or info.st_size > 1024 * 1024:
                    raise ValueError("unsupported configuration")
                env = node.read_env(path)
                if not env.get("USDB_DATA_ROOT"):
                    raise ValueError("missing data root")
            except (OSError, ValueError):
                raise ValueError(f"Cannot inspect network configuration {path}; check access and syntax before switching networks. Values were not printed.") from None
            result.append(dict(bundle=directory.name, path=path, env=env))
    return result


def unconfigured_message(layout, node):
    """Distinguish a new network deployment from an ordinary compatible release update."""
    previous = configurations(layout, node)
    if previous:
        names = ", ".join(item["bundle"] for item in previous)
        return (f"Target network {layout.bundle_id} is not configured; existing network configuration: {names}.\n"
                "Installing a release selects the tool, not the running node or its configured images.\n"
                "activate-release only activates compatible releases of an already configured network.\n"
                "Run 'usdb-node upgrade-plan' to inspect cross-network data reuse, then 'usdb-node setup' for the target network.\n"
                "Select the original Host data root to reuse compatible datasets. Stop the old node first; "
                "setup retires supported old autostart services that share data before saving the new configuration.")
    return ("node is not configured; run 'usdb-node setup' first.\n"
            "activate-release upgrades an already configured node.\n"
            "For a first installation or after uninstall --purge-data, run:\n"
            "  usdb-node setup\n  usdb-node doctor\n  usdb-node up\n"
            "If reusing retained data, select the original Host data root during setup.")


def preview_source(layout, node):
    """Select an unambiguous prior config for read-only planning, never for execution."""
    previous = configurations(layout, node)
    if len(previous) == 1:
        return replace(layout, node_env=previous[0]["path"])
    if not previous:
        raise ValueError(unconfigured_message(layout, node))
    commands = ["usdb-node --node-env " + shlex.quote(str(item["path"])) + " upgrade-plan" for item in previous]
    raise ValueError("Multiple existing networks found; select the source explicitly for this read-only comparison:\n  " + "\n  ".join(commands))


def require_selected_network(layout):
    """A shared launcher must not run a newly selected bundle with another bundle's config."""
    configured = layout.node_env.parent.name
    if BUNDLE.fullmatch(configured) and configured != layout.bundle_id:
        raise ValueError(f"NETWORK_SELECTION_MISMATCH: tool selects {layout.bundle_id}, configuration belongs to {configured}. "
                         "No services were started. Use the original kit to manage the old node; complete the target network's setup before startup.")


def conflicts(layout, node, data_root=None):
    """Scope retirement to shared data, not every other network installed on the host."""
    previous = configurations(layout, node)
    if not previous:
        return []
    if data_root is None:
        if not layout.node_env.is_file():
            return []
        env = node.read_env(layout.node_env)
        data_root = Path(env["USDB_DATA_ROOT"])
        paths = [Path(env[key]).resolve() for key in PERSISTENT_DATA_SERVICES if env.get(key)]
    else:
        paths = list(build_persistent_data_paths(data_root, layout.network_identity, layout.runtime_compatibility).values())
    root = data_root.expanduser().resolve()
    result = []
    for item in previous:
        env = item["env"]
        old_paths = [Path(env[key]).expanduser().resolve() for key in PERSISTENT_DATA_SERVICES if env.get(key)]
        if Path(env["USDB_DATA_ROOT"]).expanduser().resolve() == root or any(core.overlap(a, b) for a in old_paths for b in paths):
            result.append({**item, "paths": old_paths})
    return result


def _templates(layout, node, context):
    """Accept generated units only, including legacy observers and release stamps."""
    import node_monitor
    import control_plane_monitor
    controller = node.controller_unit_path(layout)
    return [(controller, None),
            (controller.with_name(node_monitor.unit_name(layout)), node_monitor.render_unit(layout, node, context)),
            (controller.with_name(control_plane_monitor.legacy_unit_name(layout)), control_plane_monitor.legacy_render_unit(layout, node, context))]


def _normalized(content):
    """Ignore observer release stamps while retaining every service/install directive."""
    value = background._normalized(content)
    key = ("Service", "Environment")
    value[key] = [v for v in value.get(key, []) if not (len(v) == 1 and v[0].startswith(
        ("USDB_NODE_MONITOR_RELEASE=", "USDB_CONSOLE_MONITOR_RELEASE=")))]
    return value


def _units(layout, node, previous):
    """Validate the entire retirement set before the first sudo/systemd mutation."""
    if not node._systemd_available():
        for item in previous:
            controller = node.controller_unit_path(replace(layout, bundle_id=item["bundle"], node_env=item["path"]))
            for prefix in ("usdb-node-bootstrap", "usdb-node-monitor", "usdb-console-monitor"):
                path = controller.with_name(f"{prefix}-{item['bundle']}.service")
                core.require(not path.exists() and not path.is_symlink(),
                             f"Cannot verify old autostart without systemd: {path}; inspect it before switching")
        return []
    context = node._controller_install_context()
    result = []
    for item in previous:
        old = replace(layout, bundle_id=item["bundle"], node_env=item["path"])
        for unit, expected in _templates(old, node, context):
            content = background._read(unit)
            state = background._probe(unit)
            if content is None:
                core.require(state["LoadState"] == "not-found", f"Review old service {unit.name}: its definition is unavailable")
                continue
            core.require(state["NeedDaemonReload"] == "no", f"Old service {unit.name} has unacknowledged changes; review it and reload systemd before switching")
            core.require(state["MainPID"].isdigit(), f"Old service process observation is invalid: {unit.name}; inspect systemctl status before switching")
            core.require(not Path(str(unit) + ".d").exists(), f"Custom service overrides require manual review: {unit}.d")
            if expected is None:
                commands = background._directives(content).get(("Service", "ExecStart"), [])
                core.require(len(commands) == 1, f"Review old controller command: {unit.name}")
                _, timeout, pull = background._command_options(commands[0])
                expected = node.render_controller_unit(old, launcher=context.launcher, service_user=context.service_user,
                    home=context.home, docker_launcher=context.docker_launcher, sync_timeout_secs=timeout, pull=pull)
                legacy = expected.replace("Wants=network-online.target docker.service\n",
                    f"Wants=network-online.target docker.service usdb-console-monitor-{old.bundle_id}.service\n")
                valid = _normalized(content) in (_normalized(expected), _normalized(legacy))
                core.require(state["ActiveState"] in {"inactive", "failed"} and state["MainPID"] == "0",
                             f"Old controller {unit.name} is still running; use the original kit's usdb-node down first")
            else:
                valid = _normalized(content) == _normalized(expected)
            core.require(valid, f"Old service {unit.name} is customized; automatic network switching requires manual review")
            result.append(dict(unit=unit, content=content, state=state))
    return result


def _stopped(layout, previous):
    """Check all container mounts, including unrelated projects sharing old datasets."""
    roots = [p for item in previous for p in item["paths"]]
    projects = {name for item in previous for name in (item["bundle"], item["bundle"] + "-bitcoin")}
    own = {layout.bundle_id, layout.bundle_id + "-bitcoin"} if layout.node_env.is_file() else set()
    for container in core.containers():
        project = (container.get("labels") or {}).get("com.docker.compose.project")
        if project in own:
            continue  # A repeated up may inspect an already running target node.
        shared = any(core.overlap(root, Path(m["Source"]).resolve()) for root in roots
                     for m in container["mounts"] if m.get("Source", "").startswith("/"))
        core.require(not (shared or project in projects) or container["state"] in {"exited", "dead", "created"},
                     f"Container {container['id'][:12]} still uses the previous network or shared data; stop it with the original node's down before switching")


def reconcile(layout, node, *, data_root=None, mode="apply"):
    """Retire stopped overlapping autostart in setup/up; background startup only checks.

    Unit files, configuration and datasets remain intact. Partial failures leave
    any already retired units disabled; rerunning setup/up safely finishes them.
    """
    previous = conflicts(layout, node, data_root)
    if not previous:
        return
    with ExitStack() as stack:
        if mode == "apply":
            for item in previous:
                stack.enter_context(node.node_operation_lock(replace(layout, node_env=item["path"], bundle_id=item["bundle"]), "network-switch"))
        # Re-read after acquiring old bundle locks; never disable from a stale path selection.
        core.require(conflicts(layout, node, data_root) == previous,
                     "Network configuration changed during switch; rerun setup/up")
        units = _units(layout, node, previous)
        _stopped(layout, previous)
        pending = [item for item in units if item["state"]["UnitFileState"] == "enabled"
                   or item["state"]["ActiveState"] not in {"inactive", "failed"} or item["state"]["MainPID"] != "0"]
        if mode == "check":
            core.require(not pending, "OLD_NETWORK_AUTOSTART: shared data still has old background services enabled/running; "
                         "run the target network's usdb-node up from the operator terminal to retire them (sudo may be needed)")
            return
        for item in pending:
            unit = item["unit"]
            if mode == "preview":
                print(f"Network switch would disable old autostart: {unit.name}", file=sys.stderr)
                continue
            core.require(background._read(unit) == item["content"], f"Old service changed during network switch: {unit.name}; retry after review")
            print(f"Network switch: disabling old autostart {unit.name}; configuration and data are retained", file=sys.stderr, flush=True)
            try:
                node._privileged_command(["systemctl", "disable", "--now", unit.name])
            except (OSError, subprocess.SubprocessError):
                raise ValueError(f"Network switch could not disable {unit.name}; check sudo/systemctl access, "
                                 "then retry target setup/up. Previously disabled units stay disabled; configuration and data are retained.") from None
        if mode == "apply":
            for item in _units(layout, node, previous):
                state = item["state"]
                core.require(state["UnitFileState"] == "disabled" and state["ActiveState"] in {"inactive", "failed"} and state["MainPID"] == "0",
                             f"Old autostart retirement incomplete: {item['unit'].name}; retry target setup/up before reboot")
            _stopped(layout, previous)
