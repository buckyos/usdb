"""Unattended firewall checks, narrow sudo grants, and lifecycle regression tests."""

from contextlib import nullcontext, redirect_stderr
import io
import os
from pathlib import Path
import stat
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_firewall as firewall
import usdb_node as node
from common.firewall import FirewallFixture
from common.background_services import BackgroundServicesFixture


class FirewallPermissionTests(unittest.TestCase):
    def test_rule_has_only_literal_read_only_command_and_account_scope(self):
        content = firewall.rule_content("usdb-testnet-v0", "node-user")
        self.assertIn("node-user ALL=(root) NOPASSWD: NOSETENV: /usr/sbin/ufw status verbose\n", content)
        self.assertNotIn("*", content)
        self.assertNotIn("python", content)
        self.assertNotIn(".sh", content)
        self.assertNotEqual(firewall.rule_path("usdb-testnet-v0", 1000), firewall.rule_path("usdb-testnet-v1", 1000))
        self.assertNotEqual(firewall.rule_path("usdb-testnet-v0", 1000), firewall.rule_path("usdb-testnet-v0", 1001))
        self.assertNotIn(".", firewall.rule_path("usdb-testnet-v0", 1000).name)
        for user in ("root ALL", "x\nroot", "#123", "1000", "x:ALL", "x,y"):
            with self.assertRaises(ValueError):
                firewall.rule_content("usdb-testnet-v0", user)
        with self.assertRaises(ValueError):
            firewall.rule_path("../sudoers", 1000)

    @unittest.skipIf(os.getuid() == 0, "root does not need a sudo permission")
    def test_install_is_atomic_validated_and_idempotent(self):
        with FirewallFixture() as f:
            firewall.install_rule(f.bundle, f.user)
            self.assertEqual(firewall.installed_rule(f.bundle, f.uid), f.path)
            self.assertEqual(stat.S_IMODE(f.path.stat().st_mode), 0o440)
            self.assertEqual(f.path.stat().st_nlink, 1)
            self.assertEqual(f.validate.call_count, 3)
            original = f.path.stat().st_ino
            firewall.install_rule(f.bundle, f.user)
            self.assertEqual(f.path.stat().st_ino, original)
            self.assertEqual(list(f.directory.iterdir()), [f.path])

    @unittest.skipIf(os.getuid() == 0, "root does not need a sudo permission")
    def test_rejected_rule_or_global_policy_leaves_no_grant(self):
        for failure_call in (0, 1, 2):
            with self.subTest(failure_call=failure_call), FirewallFixture() as f:
                f.validate.side_effect = [None] * failure_call + [ValueError("invalid policy")]
                with self.assertRaisesRegex(ValueError, "invalid policy"):
                    firewall.install_rule(f.bundle, f.user)
                self.assertFalse(f.path.exists())
                self.assertEqual(list(f.directory.iterdir()), [])

    def test_custom_symlink_hardlink_policy_is_preserved(self):
        for kind in ("custom", "symlink", "hardlink"):
            with self.subTest(kind=kind), FirewallFixture() as f:
                other = f.root / "unrelated"
                other.write_text("preserve")
                if kind == "custom":
                    f.path.write_text("preserve")
                elif kind == "symlink":
                    f.path.symlink_to(other)
                else:
                    os.link(other, f.path)
                with self.assertRaises(ValueError):
                    firewall.installed_rule(f.bundle, f.uid)
                self.assertEqual(other.read_text(), "preserve")
                self.assertTrue(f.path.exists())

    def test_root_boundary_rejects_user_owned_or_writable_paths(self):
        for uid, mode in ((1000, 0o755), (0, 0o775), (0, 0o777)):
            with self.subTest(uid=uid, mode=mode), mock.patch.object(Path, "stat", return_value=SimpleNamespace(st_uid=uid, st_mode=mode)):
                with self.assertRaisesRegex(ValueError, "root-owned"):
                    firewall._secure(Path("/etc/sudoers.d"))

    def test_background_upgrade_prepares_permission_without_restarting_core(self):
        with BackgroundServicesFixture() as f:
            f.layout.node_env.write_text("USDB_NODE_ROLE=full\nUSDB_FIREWALL_MODE=managed\n")
            f.running_observer()
            with mock.patch.object(firewall, "ensure") as prepare:
                result = node.ensure_background_services(f.layout)
            prepare.assert_called_once_with(f.layout, node, f.context)
            self.assertEqual(result["actions"], [])
            self.assertEqual(f.commands, [])

    def test_first_controller_install_prepares_permission_before_enablement(self):
        with BackgroundServicesFixture() as f:
            f.layout.node_env.write_text("USDB_NODE_ROLE=full\nUSDB_FIREWALL_MODE=managed\n")
            def ensure(layout, frontend, context):
                self.assertFalse(any(command[:2] == ["systemctl", "enable"] for command in f.commands))
                self.assertEqual(context.service_user, f.context.service_user)
            with mock.patch.object(firewall, "ensure", side_effect=ensure) as prepare:
                node.install_controller_unit(f.layout)
            prepare.assert_called_once_with(f.layout, node, f.context)
            self.assertIn(["systemctl", "enable", f.unit.name], f.commands)
            self.assertFalse(any("start" in command for command in f.commands))

    def test_helper_exit_code_preserves_specific_permission_diagnostic(self):
        layout = SimpleNamespace(node_env=Path("/unused/node.env"))
        env = {"USDB_OPERATOR_SSH_PORT": "22", "BTC_P2P_BIND_ADDRESS": "127.0.0.1"}
        with mock.patch.object(node, "configured_firewall_mode", return_value="managed"), \
                mock.patch.object(node, "read_env", return_value=env), \
                mock.patch.object(node, "run_helper", side_effect=subprocess.CalledProcessError(78, "firewall-check")):
            with self.assertRaisesRegex(firewall.FirewallInspectionRequired, "usdb-node up"):
                node.run_firewall_action(layout, "check")

    def test_permission_failure_prevents_background_submission(self):
        with BackgroundServicesFixture() as f, mock.patch.object(firewall, "ensure", side_effect=firewall.FirewallInspectionRequired("repair UFW access")):
            with self.assertRaisesRegex(firewall.FirewallInspectionRequired, "repair UFW access"):
                node.ensure_background_services(f.layout)
            self.assertEqual(f.commands, [])

    def test_frontend_uses_isolated_installer_and_ignores_cached_sudo_credentials(self):
        layout = SimpleNamespace(bundle_id="usdb-testnet-v0")
        frontend = SimpleNamespace(configured_firewall_mode=lambda layout: "managed", _privileged_command=mock.Mock())
        context = SimpleNamespace(service_user="node-user")
        with mock.patch.object(firewall.os, "geteuid", return_value=1000), mock.patch.object(firewall.subprocess, "run", return_value=SimpleNamespace(returncode=0)) as probe:
            firewall.ensure(layout, frontend, context)
        command = frontend._privileged_command.call_args.args[0]
        self.assertEqual(command[:2], ["/usr/bin/python3", "-I"])
        self.assertEqual(command[-4:], ["--bundle", layout.bundle_id, "--user", context.service_user])
        self.assertEqual(probe.call_args.args[0], ["sudo", "-k", "-n", "--", "/usr/sbin/ufw", "status", "verbose"])
        frontend.configured_firewall_mode = lambda layout: "external"
        frontend._privileged_command.reset_mock()
        firewall.ensure(layout, frontend, context)
        frontend._privileged_command.assert_not_called()

    def test_ignored_or_overridden_sudo_policy_is_not_success(self):
        frontend = SimpleNamespace(configured_firewall_mode=lambda layout: "managed", _privileged_command=mock.Mock())
        with mock.patch.object(firewall.subprocess, "run", return_value=SimpleNamespace(returncode=1)):
            with self.assertRaisesRegex(firewall.FirewallInspectionRequired, "FIREWALL_INSPECTION_REQUIRED"):
                firewall.ensure(SimpleNamespace(bundle_id="usdb-testnet-v0"), frontend, SimpleNamespace(service_user="node-user"))

    def test_controller_permission_failure_is_manual_action_not_restart_loop(self):
        with mock.patch.object(sys, "argv", ["usdb-node", "controller", "run"]), \
                mock.patch.object(node, "load_release_layout", return_value=SimpleNamespace()), \
                mock.patch.object(node, "node_operation_lock", return_value=nullcontext()), \
                mock.patch.object(node, "_execute_command", side_effect=firewall.FirewallInspectionRequired("FIREWALL_INSPECTION_REQUIRED")), \
                redirect_stderr(io.StringIO()) as output:
            self.assertEqual(node.main(), node.CONTROLLER_MANUAL_EXIT_CODE)
        self.assertIn("FIREWALL_INSPECTION_REQUIRED", output.getvalue())


if __name__ == "__main__":
    unittest.main()
