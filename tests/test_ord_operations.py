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
from ord_resources import CachePolicy
import resource_policy as policy
import usdb_minting as minting
import node_progress_render as render
import control_plane_monitor as monitor
from common.minting import Child, Loop, core_observation, disk_space


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
        report = dict(minting=dict(enabled=True, state="INDEXING", ord_height=589999, core_height=968997,
                      index_phase="COMMITTING", commit_target_height=594999, commit_elapsed_secs=75,
                      sample_read_bytes=policy.MIB, sample_write_bytes=2 * policy.MIB, sample_elapsed_secs=10))
        projected = monitor.project(report, 1000)["minting"]
        for field in ("ord_height", "index_phase", "commit_target_height", "commit_elapsed_secs",
                      "sample_read_bytes", "sample_write_bytes", "sample_elapsed_secs"):
            self.assertEqual(projected[field], report["minting"][field], field)
        for details in (False, True):
            with self.subTest(details=details):
                rendered = render.render_node_progress(report, details=details)
                self.assertIn("Ord (optional)", rendered)
                self.assertIn("committed height 589,999", rendered)
                self.assertIn("Committing database batch through 594,999 | elapsed 00:01:15", rendered)
                self.assertIn("Recent I/O: read 1.0MiB, wrote 2.0MiB over 10s", rendered)

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
                      shutdown_elapsed_secs=600, resource_profile="catchup", restart_reason="cache_profile", password="SECRET")
        (path / "progress.json").write_text(json.dumps(report))
        actual = minting.progress(env, now_ms=1001)
        self.assertEqual(actual["index_phase"], "COMMITTING")
        self.assertEqual(actual["shutdown_elapsed_secs"], 600)
        self.assertEqual(actual["resource_profile"], "catchup")
        self.assertEqual(actual["restart_reason"], "cache_profile")
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

    def test_shell_down_stops_consumers_before_ord_and_preserves_core_on_failure(self):
        # Exercise the actual shell -> Python -> Docker-client path without a daemon.
        fake = self.root / "docker"
        fake.write_text(f"#!{sys.executable}\n" + '''
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
root = Path(os.environ['ORD_STOP_TEST_ROOT'])
with (root / 'calls').open('a') as log: log.write(json.dumps(args) + '\\n')
if args[0] == 'compose':
    if 'ps' in args: print(args[-1])
    elif 'down' not in args: raise SystemExit(5)
elif args[0] == 'inspect':
    stopped = (root / ('stopped-' + args[-1])).exists()
    if args[2] == '{{.State.Status}}': print('exited' if stopped else 'running')
    else: print(json.dumps(dict(Status='exited' if stopped else 'running', Running=not stopped,
                              ExitCode=int(os.environ['ORD_STOP_TEST_EXIT']) if stopped else 0)))
elif args[0] == 'update':
    assert args[1] == '--restart=no'
elif args[0] == 'kill':
    assert args[1] == '--signal=SIGTERM' and args[-1] != 'ord-server'
    (root / ('stopped-' + args[-1])).touch()
elif args[0] == 'stop':
    assert args == ['stop', '--timeout', '-1', 'ord-server']
    (root / ('stopped-' + args[-1])).touch()
else: raise SystemExit(6)
''')
        fake.chmod(0o755)
        config = self.root / "node.env"
        config.write_text(f"USDB_MINTING_ENABLED=1\nORD_DATA_HOST_DIR={self.root}\n")
        runner = Path(runtime.__file__).with_name("run_testnet_runtime.sh")
        for code in (0, 137):
            for marker in self.root.glob("stopped-*"):
                marker.unlink()
            (self.root / "calls").unlink(missing_ok=True)
            env = {**os.environ, "PATH": f"{self.root}:{os.environ['PATH']}",
                   "USDB_TESTNET_NODE_ENV": str(config), "ORD_STOP_TEST_ROOT": str(self.root),
                   "ORD_STOP_TEST_EXIT": str(code)}
            result = subprocess.run(["bash", str(runner), "down"], env=env,
                                    capture_output=True, text=True, timeout=10)
            calls = [json.loads(line) for line in (self.root / "calls").read_text().splitlines()]
            down = [i for i, args in enumerate(calls) if args[0] == "compose" and "down" in args]
            stopped = [args[-1] for args in calls if args[0] == "kill"]
            self.assertEqual(stopped, ["usdb-control-plane", "usdb-chain", "usdb-indexer", "balance-history"])
            ord_stop = next(i for i, args in enumerate(calls) if args[0] == "stop")
            self.assertTrue(all(i < ord_stop for i, args in enumerate(calls) if args[0] == "kill"))
            self.assertFalse(any("btc-node" in args for args in calls))
            if code:
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(down)
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(down)
                self.assertLess(next(i for i, args in enumerate(calls) if args[0] == "stop"), down[0])

    def test_adaptive_budgets_keep_core_and_system_reserve_and_old_policies(self):
        for host in (32_000_000_000, 65_720_652 * 1024, 128 * policy.GIB):
            for storage in (None, "balanced", "slow-disk"):
                for cap in ("4g", "16g", "32g"):
                    env = dict(USDB_MINTING_ENABLED="1", SNAPSHOT_MODE="assumeutxo", USDB_ORD_MEMORY_CAP=cap)
                    if storage:
                        env["USDB_STORAGE_PROFILE"] = storage
                    for phase in policy.PHASES:
                        before = policy.build_resource_plan(host, phase, env)
                        adaptive = dict(env, USDB_ORD_RESOURCE_POLICY="adaptive-v1")
                        after = policy.build_resource_plan(host, phase, adaptive)
                        self.assertLessEqual(after.total_bytes, host)
                        self.assertEqual(after.reserve_bytes, before.reserve_bytes)
                        self.assertEqual(after.limits["BTC_MEMORY_LIMIT"], before.limits["BTC_MEMORY_LIMIT"])
                        self.assertGreaterEqual(after.limits["BH_MEMORY_LIMIT"], 4 * policy.GIB)
                        self.assertLessEqual(after.limits["ORD_MEMORY_LIMIT"], policy.memory_bytes(cap, "cap"))
                        self.assertLessEqual(after.ord_cache_bytes, after.limits["ORD_MEMORY_LIMIT"] // 4)
                        policy.validate_resource_environment(dict(adaptive, **after.environment()), host)
                        minting.validate({**minting.environment(self.root, True), **adaptive,
                                          **after.environment(), "USDB_DATA_ROOT": str(self.root)})
        node1 = policy.build_resource_plan(65_720_652 * 1024, "steady", dict(
            USDB_MINTING_ENABLED="1", SNAPSHOT_MODE="assumeutxo", USDB_ORD_MEMORY_CAP="32g",
            USDB_STORAGE_PROFILE="balanced", USDB_ORD_RESOURCE_POLICY="adaptive-v1"))
        self.assertGreater(node1.limits["ORD_MEMORY_LIMIT"], 27 * policy.GIB)
        self.assertEqual(node1.ord_steady_cache_bytes, policy.GIB)

    def test_cache_tiers_require_sustained_canonical_readiness_and_large_backlog(self):
        tier = CachePolicy(self.root, 8 * policy.GIB, policy.GIB, True)
        for at in range(0, 100, 10):
            self.assertIsNone(tier.target(dict(state="READY", canonical=False), at))
        for at in range(100, 160, 10):
            self.assertIsNone(tier.target(dict(state="READY", canonical=True), at))
        self.assertEqual(tier.target(dict(state="READY", canonical=True), 160), "steady")
        tier.apply("steady")
        self.assertEqual(CachePolicy(self.root, 8 * policy.GIB, policy.GIB, True).cache, policy.GIB)
        for at in range(170, 510, 10):
            self.assertIsNone(tier.target(dict(state="INDEXING", ord_gap=1), at))
        for at in range(510, 810, 10):
            self.assertIsNone(tier.target(dict(state="INDEXING", ord_gap=1000), at))
        self.assertEqual(tier.target(dict(state="INDEXING", ord_gap=1000), 810), "catchup")

    def test_unknown_and_missing_probes_reset_cache_transition_window(self):
        for gap in (dict(state="UNAVAILABLE"), dict(state="WAITING_CORE")):
            tier = CachePolicy(self.root, 8 * policy.GIB, policy.GIB, True)
            for at in range(0, 60, 10):
                self.assertIsNone(tier.target(dict(state="READY", canonical=True), at))
            self.assertIsNone(tier.target(gap, 60))
            self.assertIsNone(tier.target(dict(state="READY", canonical=True), 70))
            self.assertIsNone(tier.target(dict(state="READY", canonical=True), 140))

    def test_adaptive_restart_waits_for_clean_exit_and_operator_stop_wins(self):
        for exit_code, operator_stop in ((0, False), (1, False), (0, True)):
            with self.subTest(exit_code=exit_code, operator_stop=operator_stop):
                (self.root / "resource-profile.json").unlink(missing_ok=True)
                loop, children = Loop(3), []
                core = runtime.prerequisites(*core_observation(), now=1001)
                def start(*args, **kwargs):
                    self.assertTrue(all(c.poll() is not None for c in children))
                    child = Child()
                    def wait(timeout=None):
                        child.code = exit_code
                        if operator_stop:
                            loop.set()
                        return exit_code
                    child.wait = wait
                    children.append(child)
                    return child
                with mock.patch.dict(runtime.os.environ, dict(ORD_DATA_DIR=str(self.root), BTC_RPC_USER="user",
                        BTC_RPC_PASSWORD="secret", USDB_ORD_RESOURCE_POLICY="adaptive-v1",
                        ORD_INDEX_CACHE_BYTES=str(8 * policy.GIB), ORD_STEADY_INDEX_CACHE_BYTES=str(policy.GIB)), clear=True), \
                        mock.patch.object(runtime.threading, "Event", return_value=loop), \
                        mock.patch.object(runtime.signal, "signal"), \
                        mock.patch.object(runtime, "observe_core", return_value=core), \
                        mock.patch.object(runtime, "observe_ord", return_value=dict(core, state="READY", canonical=True)), \
                        mock.patch.object(runtime.shutil, "disk_usage", return_value=disk_space()), \
                        mock.patch.object(runtime.subprocess, "Popen", side_effect=start) as spawned, \
                        mock.patch.object(CachePolicy, "target", side_effect=["steady", None, None]):
                    self.assertEqual(runtime.supervise(), exit_code)
                self.assertEqual(len(children), 1 if exit_code or operator_stop else 2)
                self.assertTrue(all(c.signals == [signal.SIGINT] for c in children))
                if len(children) == 2:
                    args = spawned.call_args_list[-1].args[0]
                    self.assertEqual(args[args.index("--index-cache-size") + 1], str(policy.GIB))
                    self.assertEqual(args[args.index("--commit-interval") + 1], "100")
                if exit_code:
                    self.assertFalse((self.root / "resource-profile.json").exists())

    def test_shutdown_displays_original_wait_and_commit_target(self):
        with mock.patch.object(shutdown.time, "time", return_value=1000):
            (self.root / "progress.json").write_text(json.dumps(dict(observed_at_ms=999000,
                ord_height=494999, processing_height=499865, index_phase="COMMITTING",
                commit_target_height=499865, commit_elapsed_secs=600,
                shutdown_started_at_ms=400000, shutdown_elapsed_secs=600)))
            line = shutdown.shutdown_status(self.root, 15)
        for expected in ("client wait=15s", "shutdown elapsed=600s", "commit target=499865",
                         "commit elapsed (s)=600", "last committed height=494999", "requested="):
            self.assertIn(expected, line)


if __name__ == "__main__":
    unittest.main()
