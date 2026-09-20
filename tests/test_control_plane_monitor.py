"""Private observer projection, credentials, and resource boundaries."""

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
