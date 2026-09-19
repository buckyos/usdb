"""Exercise slow image preparation across startup and independent observers."""

from contextlib import redirect_stderr
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

TOOLS = Path(__file__).resolve().parents[1] / "docker/scripts/tools"
sys.path.insert(0, str(TOOLS))
import assumeutxo_node as native
import node_image_progress as images
import usdb_node as node


class ImageProgressTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        env = self.root / "node.env"
        env.write_text("SNAPSHOT_MODE=assumeutxo\n")
        self.layout = SimpleNamespace(node_env=env, release_id="r-test", bundle_id="test-bundle")

    def test_slow_pull_is_observable_from_another_process_and_clears_after_success(self):
        with images.ImagePreparation(self.layout) as preparation:
            preparation.set_group("bitcoin")
            started = preparation.record["started_monotonic"]
            # More than 24 minutes is still a valid preparation observation.
            preparation.record["started_monotonic"] = max(0, started - 1500)
            preparation.set_group("runtime")
            result = subprocess.run([sys.executable, "-c",
                "import json, sys; from pathlib import Path; from types import SimpleNamespace; "
                "sys.path.insert(0, sys.argv[1]); import node_image_progress as images; "
                "print(json.dumps(images.read_image_preparation(SimpleNamespace("
                "node_env=Path(sys.argv[2]), release_id='r-test', bundle_id='test-bundle'))))",
                str(TOOLS), str(self.layout.node_env)], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            observed = json.loads(result.stdout)
            self.assertEqual(observed["group"], "runtime")
            self.assertGreaterEqual(observed["elapsed_secs"], 1500)
            self.assertEqual(images._path(self.layout).stat().st_mode & 0o777, 0o600)
        self.assertIsNone(images.read_image_preparation(self.layout))
        self.assertFalse(images._path(self.layout).exists())

    def test_obsolete_or_invalid_records_never_report_pulling(self):
        with images.ImagePreparation(self.layout) as preparation:
            preparation.set_group("bitcoin")
            valid = images._path(self.layout).read_text()
            for changes in (dict(pid=0), dict(pid=2**31 - 1), dict(process_identity=["old-boot", "old-pid"]),
                            dict(binding=["old-release"]), dict(started_monotonic=float("nan")),
                            dict(group=["invalid"]), dict(schema_version="old")):
                with self.subTest(changes=changes):
                    images._path(self.layout).write_text(json.dumps({**json.loads(valid), **changes}))
                    self.assertIsNone(images.read_image_preparation(self.layout))
            for content in ("{", "[]", " " * 8193):
                images._path(self.layout).write_text(content)
                self.assertIsNone(images.read_image_preparation(self.layout))
            images._path(self.layout).write_text(valid)
            self.layout.node_env.write_text("SNAPSHOT_MODE=none\n")
            self.assertIsNone(images.read_image_preparation(self.layout))

    def test_failure_cleanup_preserves_the_pull_error_and_newer_attempt(self):
        with self.assertRaisesRegex(RuntimeError, "pull failed"):
            with images.ImagePreparation(self.layout) as preparation:
                preparation.set_group("bitcoin")
                raise RuntimeError("pull failed")
        self.assertIsNone(images.read_image_preparation(self.layout))
        with images.ImagePreparation(self.layout) as preparation:
            preparation.set_group("bitcoin")
            record = {**preparation.record, "attempt": "new-attempt"}
            images._path(self.layout).write_text(json.dumps(record))
        self.assertEqual(json.loads(images._path(self.layout).read_text())["attempt"], "new-attempt")

    def test_abrupt_process_exit_does_not_leave_dashboard_installing(self):
        result = subprocess.run([sys.executable, "-c",
            "import os, sys; from pathlib import Path; from types import SimpleNamespace; "
            "sys.path.insert(0, sys.argv[1]); import node_image_progress as images; "
            "layout=SimpleNamespace(node_env=Path(sys.argv[2]), release_id='r-test', bundle_id='test-bundle'); "
            "preparation=images.ImagePreparation(layout).__enter__(); preparation.set_group('bitcoin'); os._exit(1)",
            str(TOOLS), str(self.layout.node_env)], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(images._path(self.layout).is_file())
        self.assertIsNone(images.read_image_preparation(self.layout))

    def test_startup_publishes_each_group_even_without_a_terminal(self):
        groups = []

        def pull(_layout, helper, args, **kwargs):
            self.assertEqual(args, ["pull"])
            groups.append((helper, images.read_image_preparation(self.layout)["group"]))

        with mock.patch.object(node, "doctor"), mock.patch.object(node, "_print_startup_phase"), \
             mock.patch.object(node, "run_helper", side_effect=pull), \
             mock.patch.object(native, "start_native_node") as start:
            monitor = mock.Mock(enabled=False)
            node._start_node(self.layout, sync_timeout_secs=10, pull=True,
                             output_to_stderr=True, progress_monitor=monitor)
            self.assertEqual(groups, [("run_testnet_bitcoin.sh", "bitcoin"), ("run_testnet_runtime.sh", "runtime")])
            self.assertIsNone(images.read_image_preparation(self.layout))
            start.assert_called_once()
            groups.clear()
            node._start_node(self.layout, sync_timeout_secs=10, pull=False,
                             output_to_stderr=True, progress_monitor=monitor)
            self.assertEqual(groups, [])

    def test_progress_write_failure_does_not_prevent_startup(self):
        with mock.patch.object(node, "doctor"), mock.patch.object(node, "_print_startup_phase"), \
             mock.patch.object(node, "_atomic_write_private", side_effect=PermissionError("read-only")), \
             mock.patch.object(node, "run_helper") as pull, \
             mock.patch.object(native, "start_native_node") as start, redirect_stderr(io.StringIO()) as output:
            node._start_node(self.layout, sync_timeout_secs=10, pull=True,
                             output_to_stderr=True, progress_monitor=mock.Mock(enabled=False))
            self.assertEqual(pull.call_count, 2)
            start.assert_called_once()
            self.assertIn("WARNING Image preparation progress", output.getvalue())

    def test_dashboard_exposes_image_group_and_elapsed_without_masking_failures(self):
        with images.ImagePreparation(self.layout) as preparation:
            preparation.set_group("runtime")
            for overall in ("STARTING", "FAILED", "BLOCKED"):
                with self.subTest(overall=overall):
                    bitcoin = node._component_progress("bitcoin", "WAITING", "not started")
                    bitcoin["progress_phase"] = "not_started"
                    snapshot = node._component_progress("snapshot", "WAITING", "waiting")
                    snapshot["progress_phase"] = "waiting_for_core"
                    initial = dict(release_id="r-test", observed_at="now", overall_state=overall,
                                   components=[snapshot, bitcoin])
                    with mock.patch.object(node, "_collect_node_progress", return_value=initial):
                        report = node.collect_node_progress(self.layout)
                    observed = node.NodeProgressHistory().apply(report, observed_monotonic=0)
                    rendered = node.render_node_progress(observed, phase="bootstrap-controller", width=160)
                    self.assertIn("phase=images", rendered)
                    self.assertIn("Pulling USDB chain / services", rendered)
                    self.assertIn("Waiting for container images before snapshot download", rendered)
                    self.assertIn("Stage elapsed=", rendered)
                    self.assertIn("controller logs --follow", rendered)
                    self.assertNotIn("timing", observed["components"][0])
                    self.assertNotIn("0.00%", rendered)
                    self.assertNotIn("ETA=~", rendered)
                    self.assertEqual(report["overall_state"], "INSTALLING" if overall == "STARTING" else overall)


if __name__ == "__main__":
    unittest.main()
