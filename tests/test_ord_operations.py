"""Regressions for the node1 slow address-index commit and shutdown incident."""

import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import ord_observation as observation
import ord_runtime as runtime
import ord_shutdown as shutdown
import resource_policy as policy
import usdb_minting as minting
import node_progress_render as render
import control_plane_monitor as monitor
from common.minting import Child


class OrdOperationsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        output = contextlib.redirect_stdout(io.StringIO())
        output.__enter__()
        self.addCleanup(output.__exit__, None, None, None)

    def test_node1_and_boundary_hosts_budget_ord_in_every_phase(self):
        for memory in (32_000_000_000, 32 * policy.GIB, 65_720_652 * 1024, 64 * policy.GIB, 256 * policy.GIB):
            for cap in (4, 8, 16):
                for phase in policy.PHASES:
                    env = dict(USDB_MINTING_ENABLED="1", SNAPSHOT_MODE="assumeutxo", USDB_ORD_MEMORY_CAP=f"{cap}g")
                    plan = policy.build_resource_plan(memory, phase, env)
                    with self.subTest(memory=memory, cap=cap, phase=phase):
                        self.assertLessEqual(plan.total_bytes, memory)
                        self.assertLessEqual(plan.limits["ORD_MEMORY_LIMIT"], cap * policy.GIB)
                        self.assertLessEqual(plan.ord_cache_bytes, plan.limits["ORD_MEMORY_LIMIT"] // 2)
                        self.assertGreaterEqual(plan.limits["BH_MEMORY_LIMIT"], 4 * policy.GIB)
                        policy.validate_resource_environment({**env, **plan.environment()}, memory)
        node1 = policy.build_resource_plan(65_720_652 * 1024, "steady", dict(
            USDB_MINTING_ENABLED="1", SNAPSHOT_MODE="assumeutxo", USDB_ORD_MEMORY_CAP="16g"))
        self.assertGreater(node1.limits["ORD_MEMORY_LIMIT"], 15 * policy.GIB)
        self.assertGreater(node1.ord_cache_bytes, 7 * policy.GIB)

    def test_legacy_custom_ord_limits_are_not_silently_changed(self):
        for limit in ("4g", "6g"):
            env = dict(USDB_MINTING_ENABLED="1", ORD_MEMORY_LIMIT=limit, ORD_INDEX_CACHE_BYTES=str(policy.GIB))
            plan = policy.build_resource_plan(64 * policy.GIB, "steady", env)
            self.assertEqual(plan.limits["ORD_MEMORY_LIMIT"], policy.memory_bytes(limit, "limit"))
            self.assertNotIn("ORD_INDEX_CACHE_BYTES", plan.environment())
            policy.validate_resource_environment({**env, **plan.environment()}, 64 * policy.GIB)

    def test_external_budget_reduces_ord_and_disabled_ord_reserves_nothing(self):
        env = dict(USDB_MINTING_ENABLED="1", USDB_ORD_MEMORY_CAP="16g", USDB_EXTERNAL_MEMORY_BUDGET="16g")
        plan = policy.build_resource_plan(64 * policy.GIB, "steady", env)
        self.assertEqual(plan.limits["ORD_MEMORY_LIMIT"], 12 * policy.GIB)
        self.assertLessEqual(plan.total_bytes, 64 * policy.GIB)
        self.assertNotIn("ORD_MEMORY_LIMIT", policy.build_resource_plan(64 * policy.GIB, "steady", dict(env, USDB_MINTING_ENABLED="0")).limits)
        with self.assertRaisesRegex(ValueError, "at least 2 GiB"):
            policy.build_resource_plan(64 * policy.GIB, "steady", dict(env, USDB_ORD_MEMORY_CAP="1g"))

    def test_batch_progress_is_independent_from_committed_height(self):
        observer = observation.OrdObservation(self.root)
        self.assertTrue(observer.consume("[INFO ord::index::updater] Block 590123 at 2019-08-13 with 2000 transactions…"))
        values = observer.snapshot(committed=589999)
        self.assertEqual(values["processing_height"], 590123)
        self.assertEqual(values["index_phase"], "PROCESSING")
        self.assertTrue(observer.consume("[INFO ord::index::updater] Committing at block height 595000, 8000000 outputs traversed, 200000 in map, 50 cached"))
        values = observer.snapshot(committed=589999)
        self.assertEqual(values["index_phase"], "COMMITTING")
        self.assertEqual(values["commit_target_height"], 594999)
        observer.snapshot(committed=594999)
        observer.snapshot(committed=594999)
        records = [json.loads(line) for line in (self.root / "ord-events.jsonl").read_text().splitlines()]
        self.assertEqual(sum(r["event"] == "commit_finished" for r in records), 1)
        self.assertEqual(records[-1]["height"], 594999)

    def test_unknown_log_text_does_not_enter_durable_records_and_files_rotate(self):
        observer = observation.OrdObservation(self.root)
        self.assertFalse(observer.consume("error connecting: secret-password"))
        self.assertFalse((self.root / "ord-events.jsonl").exists())
        with mock.patch.object(observation, "MAX_LOG_BYTES", 1):
            for index in range(8):
                observer.event("test", height=index)
        paths = sorted(self.root.glob("ord-events*"))
        self.assertEqual(len(paths), 3)
        self.assertEqual(json.loads((self.root / "ord-events.jsonl").read_text())["height"], 7)

    def test_replay_after_commit_error_does_not_imply_success(self):
        observer = observation.OrdObservation(self.root)
        observer.consume("Committing at block height 595000, 100 outputs traversed, 50 in map, 0 cached")
        observer.consume("Block 590000 at date with 1 transactions")
        records = [json.loads(line) for line in (self.root / "ord-events.jsonl").read_text().splitlines()]
        self.assertFalse(any(r["event"] == "commit_finished" for r in records))
        self.assertEqual(records[-1]["event"], "commit_observation_reset")

    def test_real_log_pipe_is_drained_without_persisting_per_block_output(self):
        observer = observation.OrdObservation(self.root)
        script = "for i in range(3000): print(f'[INFO ord::index::updater] Block {i} at date with 1 transactions')"
        child = subprocess.Popen([sys.executable, "-c", script], stdout=subprocess.PIPE, text=True)
        try:
            observer.attach(child)
            child.wait(timeout=10)
            observer.reader.join(timeout=5)
            self.assertFalse(observer.reader.is_alive())
            self.assertEqual(observer.snapshot()["processing_height"], 2999)
            self.assertLess((self.root / "ord-events.jsonl").stat().st_size, 1024)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
            child.stdout.close()

    def test_shutdown_preserves_height_and_only_marks_stopping_while_waiting(self):
        child = Child()
        observer = observation.OrdObservation(self.root)
        waits = []
        def wait(timeout):
            waits.append(json.loads((self.root / "progress.json").read_text()))
            if len(waits) < 3:
                raise subprocess.TimeoutExpired("ord", timeout)
            child.code = 0
        child.wait = wait
        code = runtime.stop_child(child, self.root, dict(ord_height=589999, canonical=True), observer)
        self.assertEqual(code, 0)
        self.assertEqual(child.signals, [signal.SIGINT])
        self.assertTrue(all(r["state"] == "STOPPING" and r["ord_height"] == 589999 and r["canonical"] is False for r in waits))

    def test_rendering_and_projection_keep_commit_and_io_observations(self):
        report = dict(enabled=True, state="INDEXING", ord_height=589999, core_height=968997,
                      index_phase="COMMITTING", commit_target_height=594999, commit_elapsed_secs=75,
                      sample_read_bytes=policy.MIB, sample_write_bytes=2 * policy.MIB, sample_elapsed_secs=10)
        projected = monitor.project(dict(minting=report), 1000)["minting"]
        self.assertEqual(projected["index_phase"], "COMMITTING")
        row = render._minting_rows(report, details=False)[-1]
        self.assertIn("committed height 589,999", row.summary)
        self.assertIn("Committing database batch through 594,999 | elapsed 00:01:15", row.info)
        self.assertTrue(any("wrote 2.0MiB" in line for line in row.info))

    def test_shutdown_progress_write_failure_does_not_send_second_interrupt(self):
        child = Child()
        with mock.patch.object(runtime, "publish", side_effect=OSError("disk full")):
            self.assertEqual(runtime.stop_child(child, self.root, dict(ord_height=589999)), 0)
        self.assertEqual(child.signals, [signal.SIGINT])

    def test_new_phase_fields_survive_safe_status_reading(self):
        env = dict(USDB_DATA_ROOT=str(self.root), **minting.environment(self.root, True))
        path = minting.data_path(self.root)
        path.mkdir(parents=True)
        report = dict(schema_version=runtime.SCHEMA, state="STOPPING", observed_at_ms=1000,
                      ord_height=589999, index_phase="COMMITTING", commit_target_height=594999,
                      shutdown_elapsed_secs=600, password="SECRET")
        (path / "progress.json").write_text(json.dumps(report))
        actual = minting.progress(env, now_ms=1001)
        self.assertEqual(actual["index_phase"], "COMMITTING")
        self.assertEqual(actual["shutdown_elapsed_secs"], 600)
        self.assertFalse(actual["backend_ready"])
        self.assertNotIn("SECRET", json.dumps(actual))

    def test_stop_uses_infinite_docker_grace_and_detects_nonclean_exit(self):
        for code in (0, 137):
            child = mock.Mock()
            child.wait.side_effect = [subprocess.TimeoutExpired("docker", 15), 0]
            child.poll.return_value = 0
            states = [dict(Status="running", Running=True, ExitCode=0), dict(Status="exited", Running=False, ExitCode=code)]
            with mock.patch.object(shutdown.subprocess, "Popen", return_value=child) as start, \
                    mock.patch.object(shutdown.subprocess, "run", side_effect=[SimpleNamespace(stdout=json.dumps(s)) for s in states]):
                if code:
                    with self.assertRaisesRegex(ValueError, "did not exit cleanly"):
                        shutdown.stop_ord("test-container", self.root)
                else:
                    shutdown.stop_ord("test-container", self.root)
                self.assertEqual(start.call_args.args[0], ["docker", "stop", "--timeout", "-1", "test-container"])
                child.terminate.assert_not_called()

    def test_cancel_only_terminates_docker_client_and_keeps_old_status_honest(self):
        child = mock.Mock()
        child.wait.side_effect = [KeyboardInterrupt(), 0]
        child.poll.return_value = None
        with mock.patch.object(shutdown.subprocess, "Popen", return_value=child), \
                mock.patch.object(shutdown.subprocess, "run", return_value=SimpleNamespace(stdout='{"Running":true}')):
            with self.assertRaises(KeyboardInterrupt):
                shutdown.stop_ord("test-container", self.root)
        child.terminate.assert_called_once()
        child.kill.assert_not_called()
        (self.root / "progress.json").write_text(json.dumps(dict(state="STOPPED", observed_at_ms=1)))
        self.assertIn("container exit is still pending", shutdown.shutdown_status(self.root, 600))

    def test_shell_down_waits_for_ord_and_aborts_before_other_services_on_failure(self):
        # Exercise the actual shell -> Python -> Docker-client path without a daemon.
        fake = self.root / "docker"
        fake.write_text(f"#!{sys.executable}\n" + '''
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
root = Path(os.environ['ORD_STOP_TEST_ROOT'])
with (root / 'calls').open('a') as log: log.write(json.dumps(args) + '\\n')
if args[0] == 'compose':
    if 'ps' in args: print('ord-test')
    elif 'down' not in args: raise SystemExit(5)
elif args[0] == 'inspect':
    stopped = (root / 'stopped').exists()
    print(json.dumps(dict(Status='exited' if stopped else 'running', Running=not stopped,
                         ExitCode=int(os.environ['ORD_STOP_TEST_EXIT']) if stopped else 0)))
elif args[0] == 'stop':
    assert args == ['stop', '--timeout', '-1', 'ord-test']
    (root / 'stopped').touch()
else: raise SystemExit(6)
''')
        fake.chmod(0o755)
        config = self.root / "node.env"
        config.write_text(f"USDB_MINTING_ENABLED=1\nORD_DATA_HOST_DIR={self.root}\n")
        runner = Path(runtime.__file__).with_name("run_testnet_runtime.sh")
        for code in (0, 137):
            (self.root / "stopped").unlink(missing_ok=True)
            (self.root / "calls").unlink(missing_ok=True)
            env = {**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}",
                   "USDB_TESTNET_NODE_ENV": str(config), "ORD_STOP_TEST_ROOT": str(self.root),
                   "ORD_STOP_TEST_EXIT": str(code)}
            result = subprocess.run(["bash", str(runner), "down"], env=env,
                                    capture_output=True, text=True, timeout=10)
            calls = [json.loads(line) for line in (self.root / "calls").read_text().splitlines()]
            down = [i for i, args in enumerate(calls) if args[0] == "compose" and "down" in args]
            if code:
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(down)
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(down)
                self.assertLess(next(i for i, args in enumerate(calls) if args[0] == "stop"), down[0])


if __name__ == "__main__":
    unittest.main()
