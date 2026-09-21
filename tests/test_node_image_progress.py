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
        env.write_text("SNAPSHOT_MODE=assumeutxo\n" + "\n".join(
            f"{key}=ghcr.io/buckyos/{name}@sha256:" + digit * 64
            for key, name, digit in (("USDB_SERVICES_IMAGE", "services", "1"),
                                     ("USDB_CHAIN_IMAGE", "chain", "2"), ("USDB_BITCOIN_IMAGE", "bitcoin", "3"))) + "\n")
        self.layout = SimpleNamespace(node_env=env, release_id="r-test", bundle_id="test-bundle")

    def test_slow_pull_is_observable_from_another_process_and_clears_after_success(self):
        # Advance the same simulated clock in both processes; a fresh CI runner
        # cannot backdate a real monotonic timestamp by 25 minutes. Replace only
        # this module's clock so subprocess timeouts still use real time.
        for started, elapsed in ((0, 0), (80, 0), (80, 1500), (100000, 1500)):
            with self.subTest(started=started, elapsed=elapsed), \
                 mock.patch.object(images, "time", SimpleNamespace(monotonic=lambda: started)):
                with images.ImagePreparation(self.layout) as preparation:
                    preparation.set_group("bitcoin")
                    preparation.set_group("runtime")
                    self.assertEqual(preparation.record["started_monotonic"], started)
                    result = subprocess.run([sys.executable, "-c",
                        "import json, sys; from pathlib import Path; from types import SimpleNamespace; "
                        "sys.path.insert(0, sys.argv[1]); import node_image_progress as images; "
                        "images.time = SimpleNamespace(monotonic=lambda: float(sys.argv[3])); "
                        "print(json.dumps(images.read_image_preparation(SimpleNamespace("
                        "node_env=Path(sys.argv[2]), release_id='r-test', bundle_id='test-bundle'))))",
                        str(TOOLS), str(self.layout.node_env), str(started + elapsed)],
                        capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    observed = json.loads(result.stdout)
                    self.assertEqual(observed["group"], "runtime")
                    self.assertEqual(observed["elapsed_secs"], elapsed)
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
        failed = images.read_image_preparation(self.layout)
        self.assertEqual(failed["phase"], "failed")
        self.assertEqual(failed["last_error"], "pull failed")
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
            if args == ["up-console"]:
                return
            self.assertEqual(args[0], "pull")
            groups.append((helper, images.read_image_preparation(self.layout)["group"]))

        with mock.patch.object(node, "doctor"), mock.patch.object(node, "_print_startup_phase"), \
             mock.patch.object(images, "image_cached", return_value=False), \
             mock.patch.object(node, "run_helper", side_effect=pull), \
             mock.patch.object(native, "start_native_node") as start:
            monitor = mock.Mock(enabled=False)
            node._start_node(self.layout, sync_timeout_secs=10, pull=True,
                             output_to_stderr=True, progress_monitor=monitor)
            self.assertEqual(groups, [("run_testnet_runtime.sh", "runtime"), ("run_testnet_runtime.sh", "runtime"), ("run_testnet_bitcoin.sh", "bitcoin")])
            self.assertIsNone(images.read_image_preparation(self.layout))
            start.assert_called_once()
            groups.clear()
            node._start_node(self.layout, sync_timeout_secs=10, pull=False,
                             output_to_stderr=True, progress_monitor=monitor)
            self.assertEqual(groups, [])

    def test_progress_write_failure_does_not_prevent_startup(self):
        with mock.patch.object(node, "doctor"), mock.patch.object(node, "_print_startup_phase"), \
             mock.patch.object(images, "image_cached", return_value=False), \
             mock.patch.object(node, "_atomic_write_private", side_effect=PermissionError("read-only")), \
             mock.patch.object(node, "run_helper") as pull, \
             mock.patch.object(native, "start_native_node") as start, redirect_stderr(io.StringIO()) as output:
            node._start_node(self.layout, sync_timeout_secs=10, pull=True,
                             output_to_stderr=True, progress_monitor=mock.Mock(enabled=False))
            self.assertEqual(pull.call_count, 4)
            start.assert_called_once()
            self.assertIn("WARNING Image preparation progress", output.getvalue())

    def test_cache_requires_exact_digest_and_complete_platform_metadata(self):
        reference = node.read_env(self.layout.node_env)["USDB_BITCOIN_IMAGE"]
        complete = dict(RepoDigests=[reference], Os="linux", Architecture="amd64",
                        RootFS={"Layers": ["sha256:" + "4" * 64]})
        # Both classic and containerd stores must qualify without relying on Id.
        for changes, expected in (({}, True), ({"Id": reference.split("@")[1]}, True),
                                  ({"RepoDigests": [reference[:-1] + "0"]}, False),
                                  ({"RepoDigests": None}, False), ({"RootFS": {}}, False),
                                  ({"Architecture": "arm64"}, False), ({"Os": "windows"}, False)):
            with self.subTest(changes=changes), mock.patch.object(images.subprocess, "run",
                    return_value=subprocess.CompletedProcess([], 0, json.dumps([{**complete, **changes}]))):
                self.assertEqual(images.image_cached(reference), expected)
        for result in (subprocess.CompletedProcess([], 1, ""), subprocess.CompletedProcess([], 0, "{"),
                       subprocess.CompletedProcess([], 0, "[]")):
            with mock.patch.object(images.subprocess, "run", return_value=result):
                self.assertFalse(images.image_cached(reference))
        for error in (FileNotFoundError(), subprocess.TimeoutExpired("docker", 15)):
            with mock.patch.object(images.subprocess, "run", side_effect=error):
                self.assertFalse(images.image_cached(reference))
        with mock.patch.object(images.subprocess, "run") as inspect:
            with self.assertRaisesRegex(ValueError, "digest-pinned"):
                images.image_cached("ghcr.io/buckyos/bitcoin:latest")
            inspect.assert_not_called()

    def test_cached_startup_pulls_only_missing_release_images(self):
        for cached, expected in (([True, True, True], []),
                                 ([False, True, True], [("run_testnet_runtime.sh", ["pull", "balance-history"])]),
                                 ([True, False, True], [("run_testnet_runtime.sh", ["pull", "usdb-chain"])]),
                                 ([True, True, False], [("run_testnet_bitcoin.sh", ["pull"])])):
            with self.subTest(cached=cached), mock.patch.object(node, "doctor"), \
                 mock.patch.object(node, "_print_startup_phase"), \
                 mock.patch.object(images, "image_cached", side_effect=cached), \
                 mock.patch.object(node, "run_helper") as helper, \
                 mock.patch.object(native, "start_native_node") as start, redirect_stderr(io.StringIO()):
                node._start_node(self.layout, sync_timeout_secs=10, pull=True,
                                 output_to_stderr=True, progress_monitor=mock.Mock(enabled=True))
                pulls = [(call.args[1], call.args[2]) for call in helper.call_args_list if call.args[2][0] == "pull"]
                self.assertEqual(pulls, expected)
                helper.assert_any_call(self.layout, "run_testnet_runtime.sh", ["up-console"], output_to_stderr=True)
                start.assert_called_once()
                self.assertIsNone(images.read_image_preparation(self.layout))

    def test_layer_downloads_are_deduplicated_and_separate_from_extraction(self):
        with images.ImagePreparation(self.layout) as preparation:
            preparation.output = io.StringIO()
            preparation.set_group("bitcoin")
            preparation.begin_image(node.read_env(self.layout.node_env)["USDB_BITCOIN_IMAGE"], 1, 1)
            preparation.begin_pull()

            def event(layer, text, **values):
                preparation.observe_line(json.dumps(dict(id=layer * 12, text=text, **values)))

            event("a", "Downloading", current=1024, total=4096)
            event("a", "Downloading", current=1024, total=4096)
            event("b", "Pulling fs layer")
            self.assertEqual(preparation.record["downloaded_bytes"], 1024)
            self.assertIsNone(preparation.record["download_total_bytes"])
            event("b", "Downloading", current=2048, total=8192)
            self.assertEqual(preparation.record["downloaded_bytes"], 3072)
            self.assertEqual(preparation.record["download_total_bytes"], 12288)
            event("a", "Download complete")
            event("a", "Extracting", current=65536, total=131072)
            event("c", "Already exists")
            event("c", "Pull complete")
            self.assertEqual(preparation.record["downloaded_bytes"], 6144)
            self.assertEqual(preparation.record["download_total_bytes"], 12288)
            self.assertEqual(preparation.record["reused_layers"], 1)
            self.assertEqual(preparation.record["completed_layers"], 2)
            preparation._publish(force=True)
            report = images.add_image_preparation(self.layout, dict(release_id="r-test", observed_at="now",
                                                                    overall_state="STARTING", components=[]))
            rendered = node.render_node_progress(report, width=200)
            self.assertIn("Download observed for current image: 6.0KiB / 12.0KiB", rendered)
            self.assertIn("Layers observed: 2/3 complete; 1 cached", rendered)
            self.assertIn("Image pull attempt: 1 | Pull retries: 0", rendered)
            preparation.begin_image(node.read_env(self.layout.node_env)["USDB_CHAIN_IMAGE"], 2, 2)
            self.assertIsNone(preparation.record["downloaded_bytes"])
            self.assertEqual(preparation.record["image_attempt"], 0)

    def test_retry_preserves_failure_and_clears_after_success_or_explicit_skip(self):
        reference = node.read_env(self.layout.node_env)["USDB_BITCOIN_IMAGE"]
        with self.assertRaises(subprocess.CalledProcessError), redirect_stderr(io.StringIO()):
            with images.ImagePreparation(self.layout) as preparation:
                preparation.set_group("bitcoin")
                preparation.begin_image(reference, 1, 1)
                preparation.begin_pull()
                preparation.observe_line(json.dumps(dict(id="btc-node", text="Error",
                    status="connection reset at https://user:secret@cdn.invalid/file?token=secret")))
                raise subprocess.CalledProcessError(1, "pull")
        previous = json.loads(images._path(self.layout).read_text())
        self.assertNotIn("secret", json.dumps(previous))
        # Failed observations remain readable after the originating process exits.
        previous["pid"] = 2**31 - 1
        images._path(self.layout).write_text(json.dumps(previous))
        report = images.add_image_preparation(self.layout, dict(release_id="r-test", observed_at="now",
                                                                overall_state="STARTING", components=[]))
        self.assertEqual(report["overall_state"], "FAILED")
        self.assertIn("connection reset", node.render_node_progress(report, width=200))
        frozen = images.read_image_preparation(self.layout)["elapsed_secs"]
        with mock.patch.object(images, "time", SimpleNamespace(monotonic=lambda: previous["finished_monotonic"] + 50)):
            self.assertEqual(images.read_image_preparation(self.layout)["elapsed_secs"], frozen)
        with images.ImagePreparation(self.layout) as preparation:
            preparation.output = io.StringIO()
            preparation.set_group("bitcoin")
            preparation.begin_image(reference, 1, 1)
            preparation.begin_pull()
            observed = images.read_image_preparation(self.layout)
            self.assertEqual(observed["image_attempt"], 2)
            self.assertEqual(observed["retry_count"], 1)
            self.assertIn("connection reset", observed["last_error"])
            self.assertEqual(preparation.record["started_monotonic"], previous["started_monotonic"])
        self.assertIsNone(images.read_image_preparation(self.layout))
        images._path(self.layout).write_text(json.dumps(previous))
        images.clear_failed_preparation(self.layout)
        self.assertFalse(images._path(self.layout).exists())

    def test_streamed_helper_reports_progress_before_exit_and_preserves_failure(self):
        self.layout.kit_root = self.root
        self.layout.bundle_dir = self.root
        helper = self.root / "docker/scripts/tools/pull.sh"
        helper.parent.mkdir(parents=True)
        ack = self.root / "observed"
        helper.write_text(f"#!{sys.executable}\n" + '''import json, os, sys, time
from pathlib import Path
assert os.environ['USDB_IMAGE_PULL_PROGRESS'] == 'json'
print(json.dumps(dict(id='a'*12, text='Downloading', current=1024, total=4096)), flush=True)
deadline = time.monotonic() + 5
while not Path(sys.argv[1]).exists():
    if time.monotonic() > deadline:
        raise RuntimeError('parent did not observe streaming output')
    time.sleep(0.01)
print('connection reset at https://cdn.invalid/blob?token=secret', file=sys.stderr, flush=True)
sys.exit(1)
''')
        helper.chmod(0o700)
        reference = node.read_env(self.layout.node_env)["USDB_BITCOIN_IMAGE"]
        with self.assertRaises(subprocess.CalledProcessError), redirect_stderr(io.StringIO()):
            with images.ImagePreparation(self.layout) as preparation:
                preparation.set_group("bitcoin")
                preparation.begin_image(reference, 1, 1)
                preparation.begin_pull()

                def observe(line):
                    preparation.observe_line(line)
                    if not ack.exists():
                        result = subprocess.run([sys.executable, "-c",
                            "import json,sys;from pathlib import Path;from types import SimpleNamespace;"
                            "sys.path.insert(0,sys.argv[1]);import node_image_progress as p;"
                            "print(json.dumps(p.read_image_preparation(SimpleNamespace(node_env=Path(sys.argv[2]),"
                            "release_id='r-test',bundle_id='test-bundle'))))",
                            str(TOOLS), str(self.layout.node_env)], capture_output=True, text=True, timeout=10)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(json.loads(result.stdout)["downloaded_bytes"], 1024)
                        ack.touch()

                node.run_helper(self.layout, "pull.sh", [str(ack)], on_output=observe)
        failed = images.read_image_preparation(self.layout)
        self.assertTrue(ack.exists())
        self.assertEqual(failed["phase"], "failed")
        self.assertEqual(failed["downloaded_bytes"], 1024)
        self.assertIn("connection reset", failed["last_error"])
        self.assertNotIn("secret", images._path(self.layout).read_text())

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
                    self.assertIn("Checking local images for USDB chain / services", rendered)
                    self.assertIn("Waiting for container images before snapshot download", rendered)
                    self.assertIn("Stage elapsed=", rendered)
                    self.assertIn("controller logs --follow", rendered)
                    self.assertNotIn("timing", observed["components"][0])
                    self.assertNotIn("0.00%", rendered)
                    self.assertNotIn("ETA=~", rendered)
                    self.assertEqual(report["overall_state"], "INSTALLING" if overall == "STARTING" else overall)


if __name__ == "__main__":
    unittest.main()
