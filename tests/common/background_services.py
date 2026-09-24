"""Temporary generated units with a stateful fake systemd for startup tests."""

import os
from pathlib import Path
import pwd
import shutil
import subprocess
from types import SimpleNamespace
from unittest import mock

import control_plane_monitor as MONITOR
import node_background_services as SERVICES
import usdb_node as NODE
from common.controller_status import ControllerStatusFixture


class BackgroundServicesFixture(ControllerStatusFixture):
    """Exercise file migrations, enablement and start verification without host mutation."""

    def __enter__(self):
        super().__enter__()
        account = pwd.getpwuid(os.getuid())
        self.context = SimpleNamespace(launcher=self.launcher, docker_launcher=self.docker,
                                       service_user=account.pw_name, home=Path(account.pw_dir))
        self.stack.enter_context(mock.patch.object(NODE, "_controller_install_context", return_value=self.context))
        self.observer = MONITOR.unit_path(self.layout, NODE)
        self.observer.write_text(MONITOR.render_unit(self.layout, NODE, self.context))
        self.states = {self.unit.name: self.properties,
                       self.observer.name: {**self.properties, "FragmentPath": str(self.observer)}}
        for state in self.states.values():
            state["MainPID"] = "0"
        self.proc = self.root / "proc"
        self.proc.mkdir()
        self.elapsed = 0.0
        self.waits = []
        self.on_wait = lambda: None
        self.on_start = lambda: None
        self.stack.enter_context(mock.patch.object(SERVICES, "time", SimpleNamespace(
            monotonic=lambda: self.elapsed, sleep=self.advance)))
        live_release = SERVICES._running_release
        self.stack.enter_context(mock.patch.object(SERVICES, "_running_release",
                                                  side_effect=lambda unit, state: live_release(unit, state, self.proc)))
        self.commands = []
        self.privileged = self.stack.enter_context(mock.patch.object(NODE, "_privileged_command", side_effect=self.mutate))
        self.helper = self.stack.enter_context(mock.patch.object(NODE, "run_helper"))
        self.ord = self.stack.enter_context(mock.patch.object(NODE, "_start_optional_ord"))
        return self

    def advance(self, seconds):
        """Drive startup races deterministically without real sleeps or host uptime assumptions."""
        self.elapsed += seconds
        self.waits.append(seconds)
        self.on_wait()

    def process_read_errors(self, errors):
        """Inject kernel-style read failures only for the fixture's process environment."""
        errors = iter(errors)
        original = Path.open

        def open_file(path, *args, **kwargs):
            mode = args[0] if args else kwargs.get("mode", "r")
            if path.name == "environ" and path.parent.parent == self.proc and mode == "rb":
                error = next(errors, None)
                if error is not None:
                    raise error
            return original(path, *args, **kwargs)

        self.stack.enter_context(mock.patch.object(Path, "open", autospec=True, side_effect=open_file))

    def running_observer(self, release=None, *, pid=321):
        self.states[self.observer.name].update(ActiveState="active", SubState="running", MainPID=str(pid))
        path = self.proc / str(pid)
        path.mkdir(exist_ok=True)
        path.joinpath("environ").write_bytes(b"UNRELATED=SECRET\0USDB_CONSOLE_MONITOR_RELEASE="
                                            + (release or self.layout.release_id).encode() + b"\0")

    def remove_observer(self):
        self.observer.unlink()
        self.states[self.observer.name].update(LoadState="not-found", ActiveState="inactive", SubState="dead",
                                               UnitFileState="", FragmentPath="")

    def probe(self, command, **kwargs):
        if command[:2] != ["systemctl", "show"]:
            raise AssertionError(f"Unexpected host command: {command}")
        properties = self.states[command[-1]]
        return subprocess.CompletedProcess(command, 0, "\n".join(f"{key}={value}" for key, value in properties.items()), "")

    def mutate(self, command, **kwargs):
        self.commands.append(command)
        if command[0] == "install":
            shutil.copyfile(command[-2], command[-1])
        elif command == ["systemctl", "daemon-reload"]:
            for unit in (self.unit, self.observer):
                state = self.states[unit.name]
                state.update(LoadState="loaded", FragmentPath=str(unit), NeedDaemonReload="no")
                if not state["UnitFileState"]:
                    state["UnitFileState"] = "disabled"
        elif command[1] in {"start", "restart"}:
            if command[-1] == self.observer.name:
                self.running_observer()
                self.on_start()
            else:
                self.states[command[-1]].update(ActiveState="active", SubState="running")
        elif command[1] == "enable":
            self.states[command[-1]]["UnitFileState"] = "enabled"
        elif command[1] != "reset-failed":
            raise AssertionError(f"Unexpected mutation: {command}")
        return subprocess.CompletedProcess(command, 0)
