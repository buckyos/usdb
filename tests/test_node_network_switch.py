"""Cross-network preview and autostart handoff must preserve data and avoid double startup."""

from contextlib import ExitStack, redirect_stdout, redirect_stderr
from dataclasses import replace
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_background_services as background
import node_network_switch as switch
import node_rebuild as core
import node_upgrade as upgrade
import usdb_node as node
from common.network_switch import NetworkSwitchFixture


class NetworkSwitchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.f = NetworkSwitchFixture(Path(temporary.name))
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.out, self.err = io.StringIO(), io.StringIO()
        self.stack.enter_context(redirect_stdout(self.out))
        self.stack.enter_context(redirect_stderr(self.err))
        self.stack.enter_context(mock.patch.object(Path, "home", return_value=self.f.home))
        self.stack.enter_context(mock.patch.object(node, "controller_unit_path", side_effect=self.f.controller))
        self.stack.enter_context(mock.patch.object(node, "_controller_install_context", return_value=self.f.context))
        self.stack.enter_context(mock.patch.object(node, "_systemd_available", return_value=True))
        self.stack.enter_context(mock.patch.object(background.subprocess, "run", side_effect=self.f.probe))
        self.privileged = self.stack.enter_context(mock.patch.object(node, "_privileged_command", side_effect=self.f.mutate))
        self.containers = self.stack.enter_context(mock.patch.object(core, "containers", return_value=[]))

    def reconcile(self, **kwargs):
        return switch.reconcile(self.f.target, node, data_root=self.f.data, **kwargs)

    def test_missing_target_preview_discovers_old_config_and_exact_kit_without_writes(self):
        before = {str(p): p.read_bytes() for p in self.f.root.rglob("*") if p.is_file()}
        args = node.build_parser().parse_args(["--kit-root", str(self.f.target_kit), "upgrade-plan", "--json"])
        self.assertEqual(upgrade.dispatch(args, self.f.target, node), 2)
        value = json.loads(self.out.getvalue())
        self.assertTrue(value["cross_bundle"])
        self.assertFalse(value["executable"])
        self.assertEqual(value["source_kit"], str(self.f.source_kit))
        self.assertEqual(value["source_bundle"], "usdb-testnet-v0")
        self.assertEqual([c["action"] for c in value["components"]], ["reuse", "reuse", "rebuild", "rebuild", "rebuild"])
        self.assertNotIn("private-", self.out.getvalue())
        self.assertEqual(before, {str(p): p.read_bytes() for p in self.f.root.rglob("*") if p.is_file()})
        self.containers.assert_not_called()
        self.privileged.assert_not_called()
        self.out.seek(0); self.out.truncate()
        upgrade.show(value)
        upgrade.show_next_steps(value)
        self.assertIn("SEPARATE_SETUP_REQUIRED", self.out.getvalue())
        self.assertIn(str(self.f.source_kit), self.out.getvalue())
        self.assertIn("setup", self.out.getvalue())
        self.assertNotIn("--node-env " + str(self.f.env_path) + " setup", self.out.getvalue())

    def test_activation_explains_network_switch_and_does_not_touch_services(self):
        with self.assertRaisesRegex(ValueError, "existing network configuration: usdb-testnet-v0") as error:
            node.activate_release(self.f.target)
        self.assertIn("upgrade-plan", str(error.exception))
        self.assertIn("setup", str(error.exception))
        self.privileged.assert_not_called()

    def test_multiple_sources_and_explicit_missing_config_require_selection(self):
        extra = self.f.config.with_name("usdb-testnet-v2")
        extra.mkdir()
        (extra / "node.env").write_bytes(self.f.env_path.read_bytes())
        with self.assertRaisesRegex(ValueError, "Multiple existing networks"):
            switch.preview_source(self.f.target, node)
        args = node.build_parser().parse_args(["--node-env", str(self.f.target.node_env), "upgrade-plan"])
        with self.assertRaisesRegex(ValueError, "Selected node configuration does not exist"):
            upgrade.dispatch(args, self.f.target, node)

    def test_retire_all_old_units_without_altering_configuration_or_data_is_idempotent(self):
        saved = {p: p.read_bytes() for p in self.f.root.rglob("*") if p.is_file()}
        self.f.states[f"usdb-node-monitor-{self.f.source.bundle_id}.service"].update(ActiveState="active", SubState="running", MainPID="123")
        self.reconcile()
        self.assertEqual(len(self.f.commands), 3)
        self.assertTrue(all(s["UnitFileState"] == "disabled" and s["ActiveState"] == "inactive" for s in self.f.states.values()))
        for path, data in saved.items():
            self.assertEqual(path.read_bytes(), data, str(path))
        self.reconcile()
        self.assertEqual(len(self.f.commands), 3)
        self.assertFalse(self.f.target.node_env.exists())

    def test_running_old_controller_or_shared_container_blocks_every_disable(self):
        unit = self.f.controller(self.f.source).name
        self.f.states[unit].update(ActiveState="activating")
        with self.assertRaisesRegex(ValueError, "still running"):
            self.reconcile()
        self.f.states[unit].update(ActiveState="inactive")
        self.containers.return_value = [dict(id="a"*64, state="running", labels={"com.docker.compose.project":"another-project"},
                                            mounts=[dict(Source=str(self.f.paths["BTC_NODE_DATA_HOST_DIR"]))])]
        with self.assertRaisesRegex(ValueError, "still uses"):
            self.reconcile()
        self.privileged.assert_not_called()

    def test_customized_unit_or_dropin_cannot_disable_related_or_unrelated_services(self):
        path = self.f.units / f"usdb-console-monitor-{self.f.source.bundle_id}.service"
        original = path.read_text()
        path.write_text(original + "\n[Install]\nAlso=unrelated.service\n")
        with self.assertRaisesRegex(ValueError, "customized"):
            self.reconcile()
        path.write_text(original)
        self.f.states[path.name]["DropInPaths"] = "/etc/systemd/system/custom.conf"
        with self.assertRaisesRegex(ValueError, "manual review"):
            self.reconcile()
        self.privileged.assert_not_called()

    def test_failed_disable_is_reported_and_retry_finishes_without_reenable(self):
        def fail(command, **kwargs):
            if len(self.f.commands) == 1:
                raise subprocess.CalledProcessError(1, command)
            return self.f.mutate(command, **kwargs)
        self.privileged.side_effect = fail
        with self.assertRaisesRegex(ValueError, "retry target setup/up"):
            self.reconcile()
        self.assertEqual(len(self.f.commands), 1)
        self.privileged.side_effect = self.f.mutate
        self.reconcile()
        self.assertEqual(len(self.f.commands), 3)
        self.assertTrue(all("disable" in command for command in self.f.commands))

    def test_successful_systemctl_exit_still_requires_observed_disabled_state(self):
        self.privileged.side_effect = lambda command, **kwargs: subprocess.CompletedProcess(command, 0)
        with self.assertRaisesRegex(ValueError, "retirement incomplete"):
            self.reconcile()
        self.assertFalse(self.f.target.node_env.exists())

    def test_unknown_systemd_state_or_reload_requirement_blocks_all_changes(self):
        state = self.f.states[self.f.controller(self.f.source).name]
        state["NeedDaemonReload"] = "yes"
        with self.assertRaisesRegex(ValueError, "unacknowledged changes"):
            self.reconcile()
        state["NeedDaemonReload"] = "no"
        state.pop("ActiveState")
        with self.assertRaisesRegex(ValueError, "observation unavailable"):
            self.reconcile()
        self.privileged.assert_not_called()

    def test_quiet_check_and_preview_are_read_only_and_never_request_sudo(self):
        with self.assertRaisesRegex(ValueError, "OLD_NETWORK_AUTOSTART"):
            self.reconcile(mode="check")
        self.reconcile(mode="preview")
        self.assertIn("would disable", self.err.getvalue())
        self.privileged.assert_not_called()
        self.assertFalse((self.f.config / ".usdb-node-operation.lock").exists())

    def test_unrelated_data_root_and_network_are_left_alone(self):
        switch.reconcile(self.f.target, node, data_root=self.f.root / "separate-data")
        self.privileged.assert_not_called()
        self.containers.assert_not_called()

    def test_shared_launcher_cannot_boot_or_monitor_another_network_config(self):
        layout = replace(self.f.target, node_env=self.f.env_path)
        for command in (["controller", "run"], ["monitor", "run"], ["console", "monitor"], ["up"], ["setup"]):
            with self.subTest(command=command):
                args = node.build_parser().parse_args(command)
                with self.assertRaisesRegex(ValueError, "NETWORK_SELECTION_MISMATCH"):
                    node._execute_command(layout, args)
        self.privileged.assert_not_called()

    def test_setup_configuration_integrates_retirement_and_reuses_bitcoin_and_bh(self):
        old = self.f.env_path.read_bytes()
        with mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(self.f.target, data_root=self.f.data, role="full", miner_address="", miner_threads=1,
                                bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private")
        self.assertEqual(len(self.f.commands), 3)
        env = node.read_env(self.f.target.node_env)
        for key in ("BTC_NODE_DATA_HOST_DIR", "BH_DATA_HOST_DIR"):
            self.assertEqual(env[key], str(self.f.paths[key]))
            self.assertTrue((Path(env[key]) / "opaque.db").is_file())
        self.assertEqual(old, self.f.env_path.read_bytes())
        self.assertNotEqual(env["USDB_CHAIN_DATA_HOST_DIR"], str(self.f.paths["USDB_CHAIN_DATA_HOST_DIR"]))

    def test_up_retires_old_autostart_before_submitting_target_controller(self):
        self.f.target.node_env.parent.mkdir()
        # An already configured target with shared upstream directories.
        self.f.target.node_env.write_bytes(self.f.env_path.read_bytes())
        def submit(*args, **kwargs):
            self.assertTrue(all(s["UnitFileState"] == "disabled" for s in self.f.states.values()))
            return {"outcome": "controller_started"}, 0
        with mock.patch.object(node, "submit_up_to_controller", side_effect=submit), mock.patch.object(node, "print_up_result"):
            args = node.build_parser().parse_args(["up", "--no-watch"])
            self.assertEqual(node._execute_command(self.f.target, args), 0)
        self.assertEqual(len(self.f.commands), 3)

    def test_boot_controller_only_checks_and_never_runs_sudo_for_handoff(self):
        self.f.target.node_env.parent.mkdir()
        self.f.target.node_env.write_bytes(self.f.env_path.read_bytes())
        with mock.patch.object(node, "run_bootstrap_controller") as start:
            args = node.build_parser().parse_args(["controller", "run"])
            with self.assertRaisesRegex(ValueError, "OLD_NETWORK_AUTOSTART"):
                node._execute_command(self.f.target, args)
            start.assert_not_called()
        self.privileged.assert_not_called()


if __name__ == "__main__":
    unittest.main()
