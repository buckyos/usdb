"""Generated controller units and isolated systemd observations for status tests."""

from contextlib import ExitStack
import os
from pathlib import Path
import pwd
import subprocess
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import mock

import node_controller_status as CONTROLLER
import usdb_node as NODE


class ControllerStatusFixture:
    """Exercise the installed unit contract without touching systemd or node services."""

    def __enter__(self):
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(TemporaryDirectory(prefix="usdb-controller-status-")))
        self.layout = SimpleNamespace(node_env=self.root / "private/node.env", bundle_id="usdb-testnet-v0", release_id="r26")
        self.layout.node_env.parent.mkdir()
        self.layout.node_env.write_text("USDB_NODE_ROLE=full\n")
        self.launcher = self.root / "bin/usdb-node"
        self.launcher.parent.mkdir()
        self.launcher.write_text("#!/bin/sh\n")
        self.launcher.chmod(0o755)
        self.docker = self.root / "bin/docker"
        self.docker.write_text("#!/bin/sh\n")
        self.docker.chmod(0o755)
        self.stack.enter_context(mock.patch.dict(os.environ, {
            "USDB_SYSTEMD_UNIT_DIR": str(self.root), "USDB_NODE_LAUNCHER": str(self.launcher),
        }))
        self.unit = NODE.controller_unit_path(self.layout)
        self.write_unit()
        self.properties = dict(LoadState="loaded", ActiveState="inactive", SubState="dead", UnitFileState="enabled",
                               Result="success", ExecMainCode="0", ExecMainStatus="0", NeedDaemonReload="no",
                               FragmentPath=str(self.unit), DropInPaths="")
        self.run = self.stack.enter_context(mock.patch.object(CONTROLLER.subprocess, "run", side_effect=self.probe))
        self.stack.enter_context(mock.patch.object(CONTROLLER.shutil, "which", return_value=str(self.docker)))
        return self

    def write_unit(self, **options):
        account = pwd.getpwuid(os.getuid())
        self.unit.write_text(NODE.render_controller_unit(
            self.layout, launcher=self.launcher, service_user=account.pw_name,
            home=Path(account.pw_dir), docker_launcher=self.docker, **options,
        ))

    def probe(self, command, **kwargs):
        if command[:2] != ["systemctl", "show"]:
            raise AssertionError(f"Unexpected mutation or probe: {command}")
        return subprocess.CompletedProcess(command, 0, "\n".join(f"{key}={value}" for key, value in self.properties.items()), "")

    def report(self, overall="READY"):
        controller = CONTROLLER.inspect_controller(self.layout, node=NODE)
        return NODE._finish_node_status({"release_id": self.layout.release_id,
                                        "checks": {"controller": controller}}, overall)

    def __exit__(self, *args):
        return self.stack.__exit__(*args)
