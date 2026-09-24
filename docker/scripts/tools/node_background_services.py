"""Conservative reconciliation of generated host services during background up."""

from contextlib import nullcontext
from pathlib import Path
import shlex
import subprocess
import sys
import time

import control_plane_monitor as monitor
from node_controller_status import _command_options, _directives, PROBE_TIMEOUT_SECS, UNIT_MAX_BYTES


PROPERTIES = ("LoadState", "ActiveState", "SubState", "UnitFileState", "NeedDaemonReload",
              "FragmentPath", "DropInPaths", "MainPID")
PROCESS_RELEASE_WAIT_SECS = 5.0
PROCESS_RELEASE_POLL_SECS = 0.2


class _ProcessReleasePending(ValueError):
    """The PID may still be crossing systemd's fork, credential switch and exec."""


def _review(unit: Path, reason: str) -> ValueError:
    """Keep failed reconciliation actionable without reporting the core node as stopped."""
    return ValueError(
        f"Background service {unit.name}: {reason}. Background startup is incomplete; "
        f"inspect systemctl cat {unit.name} and systemctl status {unit.name}. "
        "Custom settings are preserved; use controller install only after reviewing them."
    )


def _read(unit: Path) -> str | None:
    if unit.is_symlink():
        raise _review(unit, "symlinked or masked unit requires manual review")
    if not unit.exists():
        return None
    with unit.open("rb") as source:
        content = source.read(UNIT_MAX_BYTES + 1)
    if len(content) > UNIT_MAX_BYTES:
        raise _review(unit, "unit exceeds the inspection limit")
    return content.decode("utf-8")


def _normalized(content: str) -> dict:
    """Tolerate quoting and directive ordering, not changes to generated commands."""
    result = _directives(content)
    for key, values in result.items():
        if key[1] in {"ExecStart", "ExecStartPre", "Environment", "Wants", "After", "Requires", "WantedBy"}:
            result[key] = sorted(tuple(shlex.split(value)) for value in values)
        else:
            result[key] = sorted(values)
    return result


def _probe(unit: Path, *, timeout: float = PROBE_TIMEOUT_SECS) -> dict:
    try:
        result = subprocess.run(
            ["systemctl", "show", "--no-pager", "--property=" + ",".join(PROPERTIES), unit.name],
            capture_output=True, text=True, check=False, timeout=timeout,
        )
        values = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        if any(key not in values for key in PROPERTIES):
            raise ValueError("incomplete observation")
        if result.returncode != 0 and values["LoadState"] != "not-found":
            raise ValueError("systemd query failed")
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise _review(unit, "systemd observation unavailable; service readiness could not be confirmed") from error
    if (values["DropInPaths"] or values["UnitFileState"].startswith("masked")
            or values["LoadState"] == "masked"
            or (values["FragmentPath"] and Path(values["FragmentPath"]) != unit)):
        raise _review(unit, "systemd override, mask or alternate unit requires manual review")
    if values["LoadState"] not in {"loaded", "not-found"}:
        raise _review(unit, "systemd could not load the unit")
    if values["ActiveState"] not in {"active", "inactive", "failed", "activating"}:
        raise _review(unit, "service is stopping or its runtime state is unknown; retry after it settles")
    if values["UnitFileState"] not in {"enabled", "disabled", ""}:
        raise _review(unit, "nonstandard enablement requires manual review")
    return values


def _running_release(unit: Path, state: dict, proc: Path = Path("/proc")) -> str | None:
    """Read only the observer's release stamp from its live process, never export other environment values."""
    try:
        pid = int(state["MainPID"])
        if pid <= 0:
            return None
        # systemctl's Environment reflects the loaded unit, not a process that
        # survived controller install + daemon-reload. Inspect the actual PID.
        with (proc / str(pid) / "environ").open("rb") as source:
            data = source.read(128 * 1024 + 1)
        if len(data) > 128 * 1024:
            raise ValueError("process environment exceeds limit")
        prefix = b"USDB_CONSOLE_MONITOR_RELEASE="
        return next((entry[len(prefix):].decode("utf-8") for entry in data.split(b"\0")
                     if entry.startswith(prefix)), None)
    except (FileNotFoundError, ProcessLookupError, PermissionError) as error:
        # Never include environment bytes or arbitrary exception payloads in CLI output.
        raise _ProcessReleasePending(f"PID {pid}: {type(error).__name__} (errno={error.errno})") from error
    except (OSError, ValueError, KeyError) as error:
        detail = f"{type(error).__name__}" + (f" (errno={error.errno})" if isinstance(error, OSError) else "")
        raise _review(unit, f"could not verify the observer process release ({detail}); check access to its /proc entry") from error


def _observe_release(unit: Path, *, after_start: bool = False) -> tuple[dict, str | None]:
    """Wait briefly for a readable process, re-probing systemd instead of pinning a stale PID.

    Type=simple acknowledges fork before the service has switched user and exec'd.
    Only transient process observations are retried; unit policy and query errors
    retain the normal fail-closed path. A readable old release is never accepted
    as the current one, and an existing legacy unstamped process can still migrate.
    """
    deadline = time.monotonic() + PROCESS_RELEASE_WAIT_SECS
    waited = False
    pending = "process identity is not yet available"
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise _review(unit, f"observer process release verification timed out after {PROCESS_RELEASE_WAIT_SECS:g}s "
                          f"({pending}); check the service user and access to its /proc entry")
        state = _probe(unit, timeout=min(PROBE_TIMEOUT_SECS, remaining))
        # A start failure is not a transient process-permission observation.
        if state["LoadState"] != "loaded" or state["ActiveState"] in {"inactive", "failed"}:
            return state, None
        if not after_start and state["ActiveState"] == "activating":
            return state, None
        if state["ActiveState"] == "active" and state["SubState"] == "running":
            try:
                release = _running_release(unit, state)
            except _ProcessReleasePending as error:
                pending = str(error)
            else:
                if release is not None or (not after_start and int(state["MainPID"]) > 0):
                    if waited:
                        print(f"Background service {unit.name}: process release observation available", file=sys.stderr, flush=True)
                    return state, release
                pending = "running process has no release stamp yet"
        else:
            pending = "observer is still starting"
        if not waited:
            print(f"Waiting for background service {unit.name}: process release verification "
                  f"(up to {PROCESS_RELEASE_WAIT_SECS:g}s)", file=sys.stderr, flush=True)
            waited = True
        time.sleep(min(PROCESS_RELEASE_POLL_SECS, max(0, deadline - time.monotonic())))


def _plan(layout, node, context) -> list[dict]:
    """Validate both services before the first write; only known templates may migrate."""
    controller = node.controller_unit_path(layout)
    content = _read(controller)
    if content is None:
        raise ValueError("Bootstrap controller is not installed; run usdb-node controller install "
                         "or use usdb-node up --foreground for an intentional foreground installation")
    try:
        commands = _directives(content).get(("Service", "ExecStart"), [])
        if len(commands) != 1:
            raise ValueError("unexpected command")
        _, timeout, pull = _command_options(commands[0])
        expected = node.render_controller_unit(
            layout, launcher=context.launcher, service_user=context.service_user, home=context.home,
            docker_launcher=context.docker_launcher, sync_timeout_secs=timeout, pull=pull,
        )
        # The pre-console template differs only in this dependency. Other edited
        # values (including paths or restart settings) are never guessed away.
        legacy = expected.replace(" docker.service " + monitor.unit_name(layout), " docker.service")
        current = _normalized(content) == _normalized(expected)
        if not current and _normalized(content) != _normalized(legacy):
            raise ValueError("unrecognized controller template")
    except (ValueError, KeyError) as error:
        raise _review(controller, "controller configuration does not match a supported generated template") from error
    plan = [dict(unit=controller, content=content, expected=expected, changed=not current, state=_probe(controller))]
    unit = monitor.unit_path(layout, node)
    content = _read(unit)
    expected = monitor.render_unit(layout, node, context)
    try:
        installed, wanted = _normalized(content or ""), _normalized(expected)
        current = installed == wanted
        # Release stamps trigger observer restart without changing the bootstrap
        # command. Both unstamped legacy units and earlier stamps are recognized.
        env_key = ("Service", "Environment")
        for values in (installed, wanted):
            values[env_key] = [value for value in values.get(env_key, [])
                               if not (len(value) == 1 and value[0].startswith("USDB_CONSOLE_MONITOR_RELEASE="))]
        if content is not None and installed != wanted:
            raise ValueError("unrecognized observer template")
    except ValueError as error:
        raise _review(unit, "observer configuration does not match a supported generated template") from error
    plan.append(dict(unit=unit, content=content, expected=expected, changed=not current, state=_probe(unit)))
    for item in plan:
        state = item["state"]
        if item["content"] is None and (state["LoadState"] != "not-found" or state["FragmentPath"]):
            raise _review(item["unit"], "unit exists only in systemd memory; review it before reinstalling")
        if item["content"] is not None and state["UnitFileState"] not in {"enabled", "disabled"}:
            raise _review(item["unit"], "could not determine existing enablement; no automatic replacement was attempted")
    return plan


def ensure(layout, *, node) -> dict:
    """Repair known units and run the observer, preserving deliberate autostart settings."""
    if not layout.node_env.is_file():
        raise ValueError("Configure the node before background startup")
    context = node._controller_install_context()
    plan = _plan(layout, node, context)
    needs_update = any(item["changed"] or item["state"]["NeedDaemonReload"] == "yes" for item in plan)
    actions = []
    # An already current monitor can be checked while the controller owns the
    # node operation lock. Template updates serialize with activation and setup.
    with node.node_operation_lock(layout, "up-background-services") if needs_update else nullcontext():
        if needs_update:
            plan = _plan(layout, node, context)
        try:
            for item in plan:
                if item["changed"]:
                    print(f"Refreshing background service: {item['unit'].name}", file=sys.stderr, flush=True)
                    node._install_service_unit(item["unit"], item["expected"], context.service_user)
                    actions.append("install:" + item["unit"].name)
            if needs_update:
                node._privileged_command(["systemctl", "daemon-reload"])
            observer = plan[1]
            # A new observer inherits the existing controller's boot policy;
            # disabled existing services remain disabled after manual up.
            if observer["content"] is None and plan[0]["state"]["UnitFileState"] == "enabled":
                node._privileged_command(["systemctl", "enable", observer["unit"].name])
            monitor.prepare(layout, node)
            # Reconcile against a fresh, readable process observation; a running
            # unit may have restarted since the configuration plan was collected.
            refresh = observer["changed"] or observer["state"]["NeedDaemonReload"] == "yes"
            if refresh:
                current = _probe(observer["unit"])
            else:
                current, running_release = _observe_release(observer["unit"])
                refresh = current["ActiveState"] == "active" and running_release != layout.release_id
            active = current["ActiveState"] == "active"
            if not active or refresh:
                verb = "restart" if active else "start"
                node._privileged_command(["systemctl", "reset-failed", observer["unit"].name], check=False)
                # Wait for systemd's start job, then verify the process state;
                # accepting an asynchronous request is not evidence of startup.
                node._privileged_command(["systemctl", verb, observer["unit"].name])
                actions.append(verb + ":" + observer["unit"].name)
            final, running_release = _observe_release(observer["unit"], after_start=True)
            if final["LoadState"] != "loaded" or final["ActiveState"] != "active" or final["SubState"] != "running":
                raise _review(observer["unit"], "observer did not reach running state; inspect its journal")
            if running_release != layout.release_id:
                raise _review(observer["unit"], "observer process is not running the selected release; inspect its journal")
        except (OSError, subprocess.SubprocessError) as error:
            raise ValueError("Background service preparation failed; check the preceding systemd/sudo error and "
                             "retry usdb-node up from the node operator's terminal. Existing core services were "
                             "not restarted by this preparation; console monitoring is not confirmed ready.") from error
    return dict(state="ready", monitor="running", actions=actions,
                controller_autostart=plan[0]["state"]["UnitFileState"], monitor_autostart=final["UnitFileState"])
