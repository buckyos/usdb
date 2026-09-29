"""Isolated sudo policy files and validation; never invoke host sudo or UFW."""

from contextlib import ExitStack
import os
from pathlib import Path
import pwd
import tempfile
from unittest import mock

import node_firewall as firewall


class FirewallFixture:
    def __enter__(self):
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.directory = self.root / "sudoers.d"
        self.directory.mkdir()
        self.ufw = self.root / "ufw"
        self.ufw.write_text("fixture executable")
        self.visudo = self.root / "visudo"
        self.visudo.write_text("fixture parser")
        self.bundle = "usdb-testnet-v0"
        self.uid = os.getuid()
        self.user = pwd.getpwuid(self.uid).pw_name
        for name, value in (("SUDOERS_DIR", self.directory), ("UFW", self.ufw), ("VISUDO", self.visudo)):
            self.stack.enter_context(mock.patch.object(firewall, name, value))
        self.secure = self.stack.enter_context(mock.patch.object(firewall, "_secure"))
        self.root_user = self.stack.enter_context(mock.patch.object(firewall.os, "geteuid", return_value=0))
        self.stack.enter_context(mock.patch.object(firewall.os, "fchown"))
        self.validate = self.stack.enter_context(mock.patch.object(firewall, "_validate_policy"))
        return self

    @property
    def path(self):
        return firewall.rule_path(self.bundle, self.uid)

    def write_rule(self):
        self.path.write_text(firewall.rule_content(self.bundle, self.user))
        self.path.chmod(0o440)
        return self.path

    def __exit__(self, *args):
        return self.stack.__exit__(*args)
