"""Private observer projection, credentials, and resource boundaries."""

import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import control_plane_monitor as monitor
import resource_policy as policy
import usdb_node as node


class PrivateMonitorTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = SimpleNamespace(node_env=self.root / "private/node.env", bundle_id="test")

    def test_token_label_and_explicit_raw_output_preserve_the_existing_secret(self):
        root = monitor.prepare(self.layout, node)
        token = monitor.read_token(root / "access-token")
        for raw in (False, True):
            args = node.build_parser().parse_args(["console", "token", *(["--raw"] if raw else [])])
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                monitor.dispatch(args, self.layout, node)
            self.assertEqual(stdout.getvalue(), (token if raw else f"Console access token: {token}") + "\n")
            self.assertEqual(monitor.read_token(root / "access-token"), token)

    def test_console_start_reports_missing_observer_and_starts_an_installed_one(self):
        for installed in (False, True):
            unit = self.root / "observer.service"
            if installed:
                unit.write_text("fixture")
            stdout = io.StringIO()
            with mock.patch.object(monitor, "unit_path", return_value=unit), \
                    mock.patch.object(node, "_privileged_command") as privileged, \
                    mock.patch.object(node, "run_helper") as helper, \
                    contextlib.redirect_stdout(stdout):
                monitor.dispatch(SimpleNamespace(console_action="start"), self.layout, node)
            helper.assert_called_once_with(self.layout, "run_testnet_runtime.sh", ["up-console"])
            if installed:
                privileged.assert_called_once_with(["systemctl", "start", "--no-block", monitor.unit_name(self.layout)])
                self.assertNotIn("WARNING", stdout.getvalue())
            else:
                privileged.assert_not_called()
                self.assertIn("host monitoring is not installed", stdout.getvalue())
                self.assertIn("usdb-node controller install", stdout.getvalue())
                self.assertIn("usdb-node console monitor", stdout.getvalue())

    def test_projection_preserves_independent_milestones_without_exporting_secrets(self):
        report = dict(overall_state="SYNCING", release_id="r-test", node_role="full",
                      node_env={"password": "SECRET"}, native_bootstrap={"error": "SECRET"},
                      network=dict(name="test", chain_id=1, rpc_url="http://SECRET"),
                      components=[dict(id="bitcoin", state="SYNCING", detail="http://user:SECRET@rpc",
                                       current=935000, total=960000,
                                       background_validation=dict(height=100, target=935000, validated=False, available=True),
                                       file_preparation=dict(state="VERIFIED", size_bytes=123))])
        exported = monitor.project(report, 1234)
        self.assertNotIn("SECRET", json.dumps(exported))
        bitcoin = exported["components"][0]
        self.assertEqual(bitcoin["file_preparation"]["state"], "VERIFIED")
        self.assertFalse(bitcoin["background_validation"]["validated"])
        self.assertEqual(bitcoin["current"], 935000)
        self.assertEqual(exported["overall_state"], "SYNCING")

    def test_export_failure_replaces_previous_ready_and_keeps_token_private(self):
        with mock.patch.object(node, "collect_node_progress", return_value=dict(overall_state="READY", components=[])):
            monitor.export(self.layout, node)
        root = monitor.data_root(self.layout, node)
        token = (root / "access-token").read_text()
        self.assertEqual(len(token.strip()), 64)
        self.assertEqual((root / "access-token").stat().st_mode & 0o777, 0o600)
        with mock.patch.object(node, "collect_node_progress", side_effect=ValueError("SECRET")):
            monitor.export(self.layout, node)
        exported = json.loads((root / "node-progress.json").read_text())
        self.assertFalse(exported["observation_available"])
        self.assertEqual(exported["overall_state"], "UNAVAILABLE")
        self.assertNotIn("SECRET", json.dumps(exported))
        self.assertEqual((root / "access-token").read_text(), token)
        self.assertEqual((root / "node-progress.json").stat().st_mode & 0o777, 0o600)

    def test_refuses_symlinked_token_without_changing_the_target(self):
        root = monitor.data_root(self.layout, node)
        root.mkdir(parents=True)
        target = self.root / "protected"
        target.write_text("preserved")
        (root / "access-token").symlink_to(target)
        with self.assertRaisesRegex(ValueError, "unsafe"):
            monitor.prepare(self.layout, node)
        self.assertEqual(target.read_text(), "preserved")

    def test_exports_only_valid_public_configured_miner_identity(self):
        self.layout.node_env.parent.mkdir(parents=True)
        for address in ("0x" + "a" * 40, "http://user:SECRET@rpc", "not-an-address"):
            self.layout.node_env.write_text(f"USDB_MINER_ADDRESS={address}\nUSDB_MINER_PRIVATE_KEY=SECRET\n")
            with mock.patch.object(node, "collect_node_progress", return_value=dict(overall_state="READY", components=[])):
                exported = monitor.export(self.layout, node)
            self.assertNotIn("SECRET", json.dumps(exported))
            if address.startswith("0x"):
                self.assertEqual(exported["node_identity"], dict(configured_miner_address=address))
            else:
                self.assertNotIn("node_identity", exported)

    def test_private_console_fits_all_automatic_phases_and_stale_limits_are_rejected(self):
        for memory in (32_000_000_000, 32 * policy.GIB, 64 * policy.GIB, 128 * policy.GIB):
            for phase in policy.PHASES:
                env = dict(SNAPSHOT_MODE="assumeutxo")
                plan = policy.build_resource_plan(memory, phase, env)
                env.update(plan.environment())
                containers = {service: dict(state="running", memory=plan.limits[policy.SERVICE_MEMORY_KEYS[service]])
                              for service in ("btc-node", "btc-snapshot-bootstrap", "usdb-control-plane")}
                self.assertLessEqual(plan.total_bytes, memory)
                node._check_running_resource_budget(env, containers)
                containers["usdb-control-plane"]["memory"] += 1
                with self.assertRaisesRegex(ValueError, "stale"):
                    node._check_running_resource_budget(env, containers)

    def test_console_failure_is_not_a_chain_initialization_failure(self):
        services = {"usdb-chain": dict(state="running"),
                    "usdb-control-plane": dict(state="exited", exit_code=1)}
        self.assertIsNone(node._chain_startup_gate_component(services))
        self.assertEqual(node._control_plane_progress(services)["state"], "FAILED")
        self.assertEqual(node._control_plane_progress({}, observation_available=False)["state"], "UNAVAILABLE")

    def test_unconfigured_progress_remains_observable(self):
        self.layout.release_id = "test-release"
        report = node._collect_node_progress(self.layout, controller_state="uninstalled")
        self.assertEqual(report["control_plane"]["state"], "UNAVAILABLE")
        self.assertEqual(report["overall_state"], "WAITING")


if __name__ == "__main__":
    unittest.main()
