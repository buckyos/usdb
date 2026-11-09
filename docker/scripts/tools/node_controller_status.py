"""Read-only controller configuration and systemd observations for node status."""

from __future__ import annotations

from pathlib import Path
import pwd
import shlex
import shutil
import subprocess


PROBE_TIMEOUT_SECS = 5
UNIT_MAX_BYTES = 64 * 1024
PROPERTIES = (
    "LoadState", "ActiveState", "SubState", "UnitFileState", "Result",
    "ExecMainCode", "ExecMainStatus", "NeedDaemonReload", "FragmentPath", "DropInPaths",
)
STARTABLE_STATES = {"ACTIVATION_REQUIRED", "SNAPSHOT_INCOMPLETE", "PREPARATION_REQUIRED", "READY_TO_START", "STARTING"}


def _directives(content: str) -> dict[tuple[str, str], list[str]]:
    """Parse the generated unit's simple directives; never interpret arbitrary unit code."""
    section = ""
    values: dict[tuple[str, str], list[str]] = {}
    for raw in content.splitlines():
        line = raw.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
        elif section and "=" in line and not line.endswith("\\"):
            key, value = line.split("=", 1)
            values.setdefault((section, key.strip()), []).append(value.strip())
        else:
            raise ValueError("Controller unit uses unsupported syntax")
    return values


def _command_options(command: str) -> tuple[list[str], int, bool]:
    """Keep supported installed timeout/pull settings when suggesting a unit refresh."""
    tokens = [value.replace("%%", "%") for value in shlex.split(command)]
    if len(tokens) < 5 or tokens[1] != "--node-env" or tokens[3:5] != ["controller", "run"]:
        raise ValueError("Controller command needs manual review")
    remaining = tokens[5:]
    timeout = None
    pull = True
    while remaining:
        flag = remaining.pop(0)
        if flag == "--sync-timeout-secs" and remaining and timeout is None:
            timeout = int(remaining.pop(0))
        elif flag == "--skip-pull" and pull:
            pull = False
        else:
            raise ValueError("Controller options need manual review")
    if timeout is None or timeout <= 0:
        raise ValueError("Controller timeout needs manual review")
    return tokens, timeout, pull


def _inspect_controller(layout, *, node) -> dict:
    """Collect bounded observations without sudo, mutation, or node readiness decisions."""
    unit = node.controller_unit_path(layout)
    report = dict(state="installed", configuration_state="unknown", display_state="unknown",
                  unit=unit.name, summary="Controller observation unavailable",
                  action_required=False, actions=[], guidance=[])
    try:
        if unit.is_symlink():
            report.update(configuration_state="custom", display_state="review_required",
                          summary="Controller unit is symlinked or masked; inspect its systemd configuration")
        elif not unit.is_file():
            return {**report, "state": "missing", "configuration_state": "missing", "display_state": "missing",
                    "summary": "Controller is not installed; background startup is unavailable"}
        else:
            with unit.open("rb") as source:
                content = source.read(UNIT_MAX_BYTES + 1)
            if len(content) > UNIT_MAX_BYTES:
                raise ValueError("Controller unit exceeds the inspection limit")
            installed = _directives(content.decode("utf-8"))
            commands = installed.get(("Service", "ExecStart"), [])
            if len(commands) != 1:
                raise ValueError("Controller command needs manual review")
            tokens, timeout, pull = _command_options(commands[0])
            report["install_command"] = "usdb-node controller install"
            if timeout != node.DEFAULT_SYNC_TIMEOUT_SECS:
                report["install_command"] += f" --sync-timeout-secs {timeout}"
            if not pull:
                report["install_command"] += " --skip-pull"
            # The configuration owner is the operator even when status is read by root.
            account = pwd.getpwuid(layout.node_env.stat().st_uid)
            if installed.get(("Service", "User")) != [account.pw_name]:
                raise ValueError("Controller operator differs from the node configuration owner")
            launcher = node._controller_launcher_path()
            docker = shutil.which("docker")
            if docker is None:
                raise OSError("Docker launcher is unavailable")
            expected = _directives(node.render_controller_unit(
                layout, launcher=launcher, service_user=account.pw_name, home=Path(account.pw_dir),
                docker_launcher=Path(docker).absolute(), sync_timeout_secs=timeout, pull=pull,
            ))
            # Compare command tokens separately to tolerate harmless quoting changes.
            expected_tokens, _, _ = _command_options(expected[("Service", "ExecStart")][0])
            del installed[("Service", "ExecStart")]
            del expected[("Service", "ExecStart")]
            current = tokens == expected_tokens and {
                key: sorted(values) for key, values in installed.items()
            } == {key: sorted(values) for key, values in expected.items()}
            if installed.keys() - expected.keys() or any(
                len(values) > len(expected.get(key, [])) for key, values in installed.items()
            ):
                report.update(configuration_state="custom", display_state="review_required",
                              summary="Controller has additional unit settings; review them before reinstalling")
            else:
                report["configuration_state"] = "current" if current else "update_required"
    except (OSError, ValueError, KeyError, UnicodeError):
        report.update(configuration_state="unverified", display_state="review_required",
                      summary="Controller configuration could not be matched; inspect it before reinstalling")

    return _observe_unit(report, unit, "Controller")


def _observe_unit(report, unit, label):
    """Read bounded systemd properties for either generated background service."""
    try:
        result = subprocess.run(
            ["systemctl", "show", "--no-pager", "--property=" + ",".join(PROPERTIES), unit.name],
            capture_output=True, text=True, check=False, timeout=PROBE_TIMEOUT_SECS,
        )
        if result.returncode != 0:
            raise ValueError("systemd query failed")
        properties = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        if any(key not in properties for key in PROPERTIES):
            raise ValueError("Incomplete systemd observation")
        report.update(runtime_state=properties["ActiveState"], substate=properties["SubState"],
                      load_state=properties["LoadState"], unit_file_state=properties["UnitFileState"],
                      result=properties["Result"], needs_daemon_reload=properties["NeedDaemonReload"] == "yes")
        report["exit_code"] = int(properties["ExecMainCode"])
        report["exit_status"] = int(properties["ExecMainStatus"])
        report["autostart"] = ("enabled" if properties["UnitFileState"] == "enabled" else
                               "disabled" if properties["UnitFileState"] == "disabled" else properties["UnitFileState"])
        if properties["DropInPaths"] or (properties["FragmentPath"] and Path(properties["FragmentPath"]) != unit):
            report.update(configuration_state="custom", display_state="review_required",
                          summary=f"{label} has systemd overrides; review effective settings before reinstalling")
        if properties["LoadState"] == "masked" or properties["UnitFileState"].startswith("masked"):
            report.update(configuration_state="custom", display_state="review_required",
                          summary=f"{label} is masked; review this deliberate systemd restriction before startup")
        elif properties["LoadState"] != "loaded":
            report.update(display_state="unavailable", summary=f"systemd could not load {label.lower()}; inspect its systemd status")
        report["observation_available"] = True
    except (OSError, ValueError, subprocess.SubprocessError):
        report.update(observation_available=False, runtime_state="unknown", autostart="unknown")
        if report["configuration_state"] == "current":
            report.update(display_state="unavailable", summary=f"{label} unit is current; systemd status is unavailable")
    return report


def inspect_monitor(layout, *, node):
    """Verify the paired monitor without enabling it or overriding an opt-out."""
    from types import SimpleNamespace
    import node_monitor
    from node_background_services import _normalized
    unit = node_monitor.unit_path(layout, node)
    report = dict(unit=unit.name, enabled=True, configuration_state="unverified",
                  observation_available=False, autostart="unknown", runtime_state="unknown")
    try:
        report["enabled"] = node_monitor.enabled(layout, node)
        if not report["enabled"]:
            return {**report, "configuration_state": "disabled", "summary": "Monitoring is disabled in node configuration"}
        if unit.is_symlink():
            report["configuration_state"] = "custom"
        elif not unit.is_file():
            return {**report, "configuration_state": "missing", "summary": "Monitor unit is not installed"}
        else:
            with unit.open("rb") as source:
                content = source.read(UNIT_MAX_BYTES + 1)
            if len(content) > UNIT_MAX_BYTES:
                raise ValueError("Monitor unit exceeds the inspection limit")
            account = pwd.getpwuid(layout.node_env.stat().st_uid)
            context = SimpleNamespace(launcher=node._controller_launcher_path(), service_user=account.pw_name,
                                      home=Path(account.pw_dir))
            installed = _normalized(content.decode("utf-8"))
            expected = _normalized(node_monitor.render_unit(layout, node, context))
            current = installed == expected
            # Only a legacy/mismatched release stamp may be refreshed automatically.
            for values in (installed, expected):
                key = ("Service", "Environment")
                values[key] = [v for v in values.get(key, []) if not
                               (len(v) == 1 and v[0].startswith(node_monitor.RELEASE_ENV + "="))]
            report["configuration_state"] = "current" if current else "update_required" if installed == expected else "custom"
    except (OSError, ValueError, KeyError, UnicodeError):
        report["configuration_state"] = "unverified"
    return _observe_unit(report, unit, "Monitor")


def inspect_controller(layout, *, node) -> dict:
    """Keep controller runtime and paired monitor boot policy independently visible."""
    report = _inspect_controller(layout, node=node)
    report["monitor"] = inspect_monitor(layout, node=node)
    return report


def command_prefix(args, node):
    """Keep explicitly selected kit/configuration paths in copyable recommendations."""
    result = ["usdb-node"]
    if args.kit_root != node.KIT_ROOT:
        result += ["--kit-root", str(args.kit_root)]
    if args.node_env is not None:
        result += ["--node-env", str(args.node_env)]
    return result


def _controller_autostart_notice(controller: dict, *, command=("usdb-node",)) -> dict:
    """Describe reboot behavior without changing intentional foreground/disabled operation."""
    status = shlex.join([*command, "controller", "status"])
    install = shlex.join([*command, *shlex.split(controller.get(
        "install_command", "usdb-node controller install"))[1:]])
    configuration = controller.get("configuration_state")
    if configuration == "missing":
        state = "MISSING"
        summary = ("Controller is not installed; automatic startup after reboot is unavailable. "
                   f"For background operation, run {install}. Intentional up --foreground remains supported.")
    elif configuration in {"custom", "unverified"}:
        state = "REVIEW_REQUIRED"
        summary = f"Controller settings need manual review; run {status} before replacing or enabling the unit."
    elif configuration == "update_required" or controller.get("needs_daemon_reload"):
        state = "UPDATE_REQUIRED"
        summary = (f"Controller configuration needs refreshing; run {install} for background operation. "
                   "This also enables automatic startup after reboot.")
    elif not controller.get("observation_available") or controller.get("load_state") != "loaded":
        state = "UNAVAILABLE"
        summary = f"Automatic startup after reboot could not be verified; inspect {status}."
    elif controller.get("autostart") == "enabled":
        state = "ENABLED"
        summary = "Automatic startup after reboot is enabled; this does not prove current service readiness."
    elif controller.get("autostart") == "disabled":
        state = "DISABLED"
        summary = ("Automatic startup after reboot is disabled. Manual up, down and activate-release preserve this setting. "
                   f"If reboot recovery is intended, run {install}; otherwise leave it disabled.")
    else:
        state = "UNAVAILABLE"
        summary = f"Persistent automatic startup after reboot is not confirmed; inspect {status}."
    return dict(state=state, severity="PASS" if state == "ENABLED" else "WARN", summary=summary)


def autostart_notice(controller: dict, *, command=("usdb-node",)) -> dict:
    """Reboot recovery includes the monitor unless monitoring was explicitly disabled."""
    notice = _controller_autostart_notice(controller, command=command)
    monitor = controller.get("monitor")
    if monitor is None:  # Older observations retain their controller-only semantics.
        return notice
    configuration = monitor.get("configuration_state")
    if notice["state"] == "REVIEW_REQUIRED":
        return {**notice, "summary": notice["summary"] + " Review the paired monitor configuration as well."}
    install = shlex.join([*command, *shlex.split(controller.get("install_command", "usdb-node controller install"))[1:]])
    unit = shlex.quote(monitor["unit"])
    if monitor.get("enabled") is False:
        detail, state = "Monitor is intentionally disabled in node configuration.", None
    elif configuration in {"custom", "unverified"}:
        # Do not recommend reinstalling even a normal controller over a custom monitor.
        return dict(state="REVIEW_REQUIRED", severity="WARN", summary=
                    f"Monitor configuration needs manual review: systemctl cat {unit}; systemctl status {unit}. "
                    "Review paired background services before replacing or enabling units.")
    elif configuration == "missing":
        detail, state = f"Monitor unit is missing; background monitoring cannot resume after reboot. Run {install} for reboot recovery.", "INCOMPLETE"
    elif configuration == "update_required" or monitor.get("needs_daemon_reload"):
        detail, state = (f"Monitor configuration needs refreshing. Manual up refreshes supported units while preserving boot policy; "
                         f"run {install} if automatic startup after reboot is intended."), "UPDATE_REQUIRED"
    elif not monitor.get("observation_available") or monitor.get("load_state") != "loaded":
        detail, state = f"Monitor startup after reboot could not be verified; inspect systemctl status {unit}.", "UNAVAILABLE"
    elif monitor.get("autostart") == "enabled":
        detail, state = "Monitor automatic startup after reboot is enabled.", None
    elif monitor.get("autostart") == "disabled":
        detail, state = f"Monitor automatic startup after reboot is disabled. Run {install} if reboot recovery is intended.", "INCOMPLETE"
    else:
        detail, state = f"Persistent monitor startup after reboot is not confirmed; inspect systemctl status {unit}.", "UNAVAILABLE"
    return dict(state=state if state and notice["severity"] == "PASS" else notice["state"],
                severity="WARN" if state else notice["severity"], summary=notice["summary"] + " " + detail)


def apply_controller_guidance(report: dict) -> None:
    """Add operator actions while keeping runtime readiness and foreground startup intact."""
    controller = report["checks"].get("controller")
    if not controller:
        return
    overall = report["overall_state"]
    configuration = controller.get("configuration_state", controller["state"])
    actions, guidance = [], []
    install = controller.get("install_command", "usdb-node controller install")
    monitor_configuration = controller.get("monitor", {}).get("configuration_state")
    if configuration == "missing":
        if overall in STARTABLE_STATES:
            actions.append(install)
        guidance.append("For background startup, run usdb-node controller install; explicit foreground operation uses usdb-node up --foreground.")
    elif configuration in {"custom", "unverified"}:
        actions.append("usdb-node controller status")
        guidance.append("Review the effective systemd unit and custom settings before replacing the controller configuration.")
    elif configuration == "update_required" or controller.get("needs_daemon_reload"):
        controller.update(display_state="update_required", summary="Controller configuration needs refreshing before the next background startup")
        actions.append(install)
        guidance.append("Refresh the controller with the command shown; installed timeout and image-pull settings are preserved.")
    elif not controller.get("observation_available") or controller.get("load_state") != "loaded":
        actions.append("usdb-node controller status")
    else:
        active = controller.get("runtime_state")
        manual = (controller.get("result") == "exit-code" and controller.get("exit_code") == 1
                  and controller.get("exit_status") == 2)
        membership = report["checks"].get("network_membership", {})
        needs_seed = (membership.get("state") == "WAITING" and membership.get("reason") == "SEED_REQUIRED"
                      and overall in {"AWAITING_PEERS", "WAITING", "SYNCING", "STARTING"})
        if needs_seed and (active == "inactive" or (active == "failed" and manual)):
            controller.update(display_state="waiting_for_seed",
                              summary="Chain is running; add a seed to join the network")
            actions.extend(["usdb-node peers add ENODE", "usdb-node peers status --watch"])
            guidance.append("Use a reachable enode from the same network. Full nodes can sync and validate blocks without enabling mining.")
        elif active == "failed" and manual:
            controller.update(display_state="manual_action", summary="Last controller run requested operator action (exit 2); follow the current node checks")
            if overall == "READY":
                controller.update(display_state="idle", summary="Earlier controller run exited with code 2; the node is currently ready")
            elif overall == "AWAITING_PEERS":
                actions.extend(report["next_actions"])
            else:
                actions.append("usdb-node controller logs --follow")
        elif active == "failed":
            controller.update(display_state="failed", summary="Controller run failed; inspect its logs before retrying startup")
            actions.append("usdb-node controller logs --follow")
        elif active in {"active", "activating", "reloading"}:
            controller.update(display_state="running", summary="Controller is running; startup and operation progress is available in its logs")
        elif active == "deactivating":
            controller.update(display_state="stopping", summary="Controller is stopping; wait for the current shutdown operation")
        elif active == "inactive":
            detail = "node services are ready" if overall == "READY" else "no background orchestration is running"
            controller.update(display_state="idle", summary=f"Controller is stopped; {detail}")
        else:
            controller.update(display_state="unavailable", summary="Controller runtime state is unknown; inspect controller status")
            actions.append("usdb-node controller status")
    if monitor_configuration in {"custom", "unverified"}:
        actions = [action for action in actions if action != install]
        guidance = [line for line in guidance if "controller install" not in line and not line.startswith("Refresh the controller")]
        actions.append("usdb-node controller status")
        guidance.append(autostart_notice(controller)["summary"])
    if controller.get("observation_available"):
        if controller.get("autostart") == "disabled":
            controller["summary"] += "; automatic startup after reboot is disabled"
        elif controller.get("autostart") == "enabled":
            controller["summary"] += "; automatic startup after reboot is enabled"
        else:
            controller["summary"] += "; persistent automatic startup is not confirmed"
            guidance.append("Inspect controller status if persistent automatic startup after reboot is required.")
    startup = autostart_notice(controller)
    controller["startup"] = startup
    controller["startup_warning"] = startup["severity"] == "WARN"
    if controller["startup_warning"]:
        guidance.append(startup["summary"])
    monitor = controller.get("monitor", {})
    if monitor.get("enabled") and monitor.get("configuration_state") == "current":
        controller["summary"] += f"; monitor autostart={monitor.get('autostart', 'unknown')}"
        if monitor.get("observation_available") and monitor.get("runtime_state") in {"inactive", "failed"} and overall in {"READY", "SYNCING"}:
            controller["startup_warning"] = True
            guidance.append("Node services are running but the monitor is stopped; run usdb-node up to restore background observation. "
                            "This preserves existing autostart settings.")
            controller["summary"] += "; monitor is stopped while node services are running"
    actions, guidance = list(dict.fromkeys(actions)), list(dict.fromkeys(guidance))
    controller.update(action_required=bool(actions), actions=actions, guidance=guidance)
    report["next_actions"] = list(dict.fromkeys([*actions, *report["next_actions"]]))
    report["operator_guidance"] = list(dict.fromkeys([*report["operator_guidance"], *guidance]))
