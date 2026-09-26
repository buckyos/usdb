"""Resource history survives RPC stalls and preserves pressure/identity semantics."""
import contextlib
import base64
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_monitor
import node_resource_history as history
import node_resource_metrics as metrics
import node_resource_monitor as monitor
import usdb_node as node
from common.node_monitor import BASE, MonitorFixture, report
from common.resource_history import evidence, kernel_tree, resource_report


class MetricTests(unittest.TestCase):
    def test_cgroup_cache_swap_pressure_deltas_and_restart(self):
        with tempfile.TemporaryDirectory() as root:
            proc, cg, resources, inspect = kernel_tree(root)
            collector = metrics.Collector(proc, cg)
            with mock.patch.object(metrics, "command", return_value=inspect), mock.patch.object(metrics, "now_ms", return_value=BASE):
                first = collector.sample(resources)["items"][1]
            self.assertEqual(first["metrics"]["memory_current_bytes"], 1024)
            self.assertEqual(first["metrics"]["working_set_bytes"], 624)
            self.assertEqual(first["metrics"]["file_bytes"], 896)
            self.assertIsNone(first["metrics"]["swap_limit_bytes"])
            self.assertNotIn("events_max_delta", first["metrics"])
            (cg / "docker/a/memory.events").write_text("high 0\nmax 9\noom 0\noom_kill 0\n")
            (proc / "vmstat").write_text("pswpin 3\npswpout 6\n")
            with mock.patch.object(metrics, "command", return_value=inspect), mock.patch.object(metrics, "now_ms", return_value=BASE + 10000):
                second = collector.sample(resources)
            self.assertEqual(second["items"][1]["metrics"]["events_max_delta"], 4)
            self.assertEqual(second["items"][0]["metrics"]["swap_out_bytes_per_sec"], 4 * os.sysconf("SC_PAGE_SIZE") / 10)
            with mock.patch.object(metrics, "command", return_value=inspect.replace("00:00:00", "00:00:10")), mock.patch.object(metrics, "now_ms", return_value=BASE + 20000):
                restarted = collector.sample(resources)["items"][1]
            self.assertNotIn("events_max_delta", restarted["metrics"])
            (proc / "sys/kernel/random/boot_id").write_text("boot-b\n")
            with mock.patch.object(metrics, "command", return_value=inspect), mock.patch.object(metrics, "now_ms", return_value=BASE + 30000):
                self.assertNotIn("swap_out_bytes_per_sec", collector.sample(resources)["items"][0]["metrics"])

    def test_missing_v2_or_permission_is_not_zero_pressure(self):
        with tempfile.TemporaryDirectory() as root:
            proc, cg, resources, inspect = kernel_tree(root)
            (proc / "42/cgroup").write_text("1:memory:/docker/a\n")
            with mock.patch.object(metrics, "command", return_value=inspect):
                item = metrics.Collector(proc, cg).sample(resources)["items"][1]
            self.assertIn("cgroup_v2", item["missing"])
            self.assertIsNone(monitor.pressure_state(item))
            (proc / "42/cgroup").write_text("0::/../../outside\n")
            with mock.patch.object(metrics, "command", return_value=inspect):
                self.assertIn("cgroup_v2", metrics.Collector(proc, cg).sample(resources)["items"][1]["missing"])

    def test_probe_instrumentation_is_opt_in_and_never_records_rpc_payloads(self):
        @metrics.trace_helper
        def helper(*args, **kwargs):
            return subprocess.CompletedProcess([], 0, '{"rpc_available":true,"password":"SECRET"}')
        with metrics.capture_probes() as values:
            helper(None, "run_testnet_bitcoin.sh", ["progress"])
            helper(None, "run_testnet_bitcoin.sh", ["up"])
        self.assertEqual(len(values), 1)
        self.assertEqual(values[0]["outcome"], "ok")
        self.assertNotIn("SECRET", json.dumps(values))
        @metrics.trace_helper
        def timeout(*args, **kwargs):
            raise subprocess.TimeoutExpired("SECRET", 5)
        with metrics.capture_probes() as values, self.assertRaises(subprocess.TimeoutExpired):
            timeout(None, "run_testnet_bitcoin.sh", ["progress"])
        self.assertEqual(values[0]["outcome"], "timeout")
        self.assertNotIn("SECRET", json.dumps(values))


class HistoryTests(unittest.TestCase):
    def test_minute_probe_failures_and_peaks_are_not_overwritten_or_counted_twice(self):
        with MonitorFixture() as f:
            with history.History(node_monitor.root(f.layout)/"resources.sqlite3", node_monitor.scope(f.layout), writable=True) as db:
                for offset, fresh, outcome, duration in ((0, True, "timeout", 5000), (1000, False, "timeout", 5000), (2000, True, "ok", 20)):
                    value = evidence(BASE + offset)
                    value["observation_is_new"] = fresh
                    value["observation"].update(collection={"outcome": outcome}, probes=[dict(service="bitcoin", operation="progress", duration_ms=duration, outcome=outcome)])
                    db.append(value)
                record = db.query(resolution="minute")["records"][0]
                self.assertEqual(record["collection_outcomes"], {"timeout": 1, "ok": 1})
                probe = record["probes"]["bitcoin:progress"]
                self.assertEqual(probe["count"], 2)
                self.assertEqual(probe["max_ms"], 5000)
                self.assertEqual(probe["mean_ms"], 2510)
                self.assertEqual(probe["outcomes"], {"timeout": 1, "ok": 1})

    def test_capacity_eviction_and_corruption_are_visible(self):
        with MonitorFixture() as f:
            path = node_monitor.root(f.layout)/"resources.sqlite3"
            # A tiny synthetic quota exercises the production pruning path cheaply.
            with history.History(path, node_monitor.scope(f.layout), writable=True, max_mib=9) as db:
                for index in range(50):
                    value = evidence(BASE + index * 1000)
                    value["observation"]["fixture_padding"] = base64.b64encode(os.urandom(32000)).decode()
                    db.append(value)
                    db.prune(BASE + index * 1000, f.settings)
                self.assertGreater(db.get("capacity_evictions"), 0)
                self.assertLess(path.stat().st_size, 9 * history.MIB)
                self.assertEqual(db.query()["records"][0]["at_ms"], BASE + 49000)
                with db.db:
                    db.db.execute("UPDATE raw SET payload=?", (b"corrupted",))
                with self.assertRaisesRegex(ValueError, "Corrupt resource history"):
                    db.query()

    def test_roundtrip_rollup_extremes_identity_filters_and_pagination(self):
        with MonitorFixture() as f:
            path = node_monitor.root(f.layout) / "resources.sqlite3"
            with history.History(path, node_monitor.scope(f.layout), writable=True) as db:
                db.append(evidence(BASE, full=2))
                db.append(evidence(BASE + 1000, full=8))
                db.append(evidence(BASE + 2000, full=4, identity="new-container"))
                rollups = db.query(resolution="minute")["records"]
                self.assertEqual(len(rollups), 2)
                stats = rollups[1]["items"][0]["metrics"]["memory_psi_full_avg10"]
                self.assertEqual(stats, dict(count=2, min=2, max=8, sum=10, mean=5))
                first = db.query(limit=2)
                second = db.query(limit=2, before_id=first["next_before_id"])
                self.assertEqual(len(first["records"]) + len(second["records"]), 3)
                self.assertEqual(len(db.query(since=BASE+1500, until=BASE+2500, session="session-a")["records"]), 1)
                self.assertEqual(db.query(session="missing")["records"], [])
            with history.History(path, node_monitor.scope(f.layout)) as db:
                self.assertEqual(len(db.query()["records"]), 3)
                with self.assertRaises(sqlite3.OperationalError):
                    db.put("forbidden", True)
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_retention_preserves_minute_history_after_raw_expires(self):
        with MonitorFixture() as f:
            with history.History(node_monitor.root(f.layout)/"resources.sqlite3", node_monitor.scope(f.layout), writable=True) as db:
                db.append(evidence())
                db.prune(BASE + 8 * history.DAY, f.settings)
                self.assertEqual(db.query()["records"], [])
                self.assertEqual(len(db.query(resolution="minute")["records"]), 1)
                db.prune(BASE + 31 * history.DAY, f.settings)
                self.assertEqual(db.query(resolution="minute")["records"], [])

    def test_network_version_and_redirected_file_are_rejected(self):
        with MonitorFixture() as f:
            path = node_monitor.root(f.layout)/"resources.sqlite3"
            with self.assertRaises(FileNotFoundError):
                history.History(path, node_monitor.scope(f.layout))
            self.assertFalse(path.exists())
            with history.History(path, node_monitor.scope(f.layout), writable=True) as db:
                db.append(evidence())
            with self.assertRaisesRegex(ValueError, "different network"):
                history.History(path, {})
            with sqlite3.connect(path) as db:
                db.execute("PRAGMA user_version=99")
            with self.assertRaisesRegex(ValueError, "Unsupported resource history"):
                history.History(path, node_monitor.scope(f.layout), writable=True)
            moved = path.with_name("saved.sqlite3")
            path.rename(moved)
            path.symlink_to(moved)
            with self.assertRaisesRegex(ValueError, "private regular file"):
                history.History(path, node_monitor.scope(f.layout))

    def test_cli_query_never_calls_rpc_and_supports_export(self):
        with MonitorFixture() as f:
            with history.History(node_monitor.root(f.layout)/"resources.sqlite3", node_monitor.scope(f.layout), writable=True) as db:
                db.append(evidence())
            args = node.build_parser().parse_args(["monitor", "resources", "--service", "btc-node", "--json"])
            with mock.patch.object(node, "run_helper", side_effect=AssertionError("unexpected probe")), contextlib.redirect_stdout(io.StringIO()) as output:
                node_monitor.dispatch(args, f.layout, node)
            self.assertEqual(json.loads(output.getvalue())["records"][0]["items"][0]["service"], "btc-node")
            with self.assertRaises(ValueError):
                history.timestamp("2026-09-25T00:00:00")


class PressureTests(unittest.TestCase):
    def test_cache_occupancy_alone_is_not_a_fault(self):
        self.assertFalse(monitor.pressure_state(evidence(full=0, maximum=0)["items"][0]))
        self.assertTrue(monitor.pressure_state(evidence()["items"][0]))

    def test_pressure_works_during_rpc_failure_and_unknown_never_recovers(self):
        with MonitorFixture() as f:
            for seconds in (0, 2, 4):
                at = BASE + seconds * 1000
                value = report(at, available=False)
                value["host_resources"] = resource_report(evidence(at))
                node_monitor.rules.evaluate(f.store, value, at, f.settings)
            alert = next(v for v in f.store.alerts() if v["code"] == "MEMORY_PRESSURE")
            self.assertEqual(alert["severity"], "critical")
            with f.store.db:
                monitor.evaluate(f.store, {}, BASE + 100000, f.settings)
            alert = next(v for v in f.store.alerts() if v["code"] == "MEMORY_PRESSURE")
            self.assertEqual(alert["condition"], "unknown")
            for seconds in (101, 103):
                at = BASE + seconds * 1000
                with f.store.db:
                    monitor.evaluate(f.store, resource_report(evidence(at, full=0, maximum=0, window="good")), at, f.settings)
            self.assertNotIn("MEMORY_PRESSURE", [v["code"] for v in f.store.alerts()])

    def test_short_recovery_between_rpc_samples_resets_pressure_window(self):
        with MonitorFixture() as f, f.store.db:
            monitor.evaluate(f.store, resource_report(evidence()), BASE, f.settings)
            monitor.evaluate(f.store, resource_report(evidence(BASE+3000, window="bad-after-recovery")), BASE+3000, f.settings)
            alerts = [v for v in f.store.alerts() if v["code"] == "MEMORY_PRESSURE"]
            self.assertEqual(alerts[0]["state"], "pending")

    def test_slow_rpc_poll_cadence_keeps_independently_observed_pressure(self):
        with MonitorFixture() as f, f.store.db:
            monitor.evaluate(f.store, resource_report(evidence()), BASE, f.settings)
            monitor.evaluate(f.store, resource_report(evidence(BASE+300000)), BASE+300000, f.settings)
            alert = next(v for v in f.store.alerts() if v["code"] == "MEMORY_PRESSURE")
            self.assertEqual(alert["state"], "firing")
            self.assertEqual(alert["severity"], "critical")


class WorkerTests(unittest.TestCase):
    def test_history_failure_preserves_metrics_and_event_journal_then_recovers(self):
        import control_plane_resources
        with MonitorFixture() as f:
            stopped, failed = threading.Event(), threading.Event()
            original = history.History.append
            calls = []
            def write(db, value, **kwargs):
                calls.append(1)
                if len(calls) == 1:
                    failed.set()
                    raise sqlite3.OperationalError("PRIVATE_SECRET")
                original(db, value, **kwargs)
                stopped.set()
            with mock.patch.object(control_plane_resources, "ResourceCollector") as collector, \
                    mock.patch.object(monitor, "Collector") as metric_collector, mock.patch.object(history.History, "append", write), \
                    contextlib.redirect_stderr(io.StringIO()) as output:
                collector.return_value.sample.side_effect = lambda *_: {"host": {"status": "available"}}
                metric_collector.return_value.sample.side_effect = lambda _: evidence(metrics.now_ms())
                observer = monitor.ResourceObserver(f.layout, node, stopped, .1, f.settings, {"id": "session-a"})
                self.assertTrue(failed.wait(1))
                # The event database still accepts independent lifecycle events.
                with f.store.db:
                    f.store.event(BASE, "monitor", "EVENT_DURING_RESOURCE_FAILURE")
                observer.worker.join(2)
                self.assertFalse(observer.worker.is_alive())
                self.assertIn("EVENT_DURING_RESOURCE_FAILURE", f.codes())
                self.assertEqual(observer.snapshot()["history"]["state"], "available")
                self.assertEqual(observer.snapshot()["history"]["failed_samples"], 1)
                self.assertIn("RESOURCE_HISTORY_FAILED", output.getvalue())
                self.assertIn("RESOURCE_HISTORY_RECOVERED", output.getvalue())
                self.assertNotIn("PRIVATE_SECRET", output.getvalue())

    def test_resource_writes_continue_while_rpc_sample_is_blocked(self):
        import control_plane_resources
        with MonitorFixture() as f:
            stopped = threading.Event()
            entered, release = threading.Event(), threading.Event()
            def stalled_probe():
                entered.set()
                release.wait(2)
            probe = threading.Thread(target=stalled_probe)
            probe.start()
            entered.wait(1)
            with mock.patch.object(control_plane_resources, "ResourceCollector") as collector, mock.patch.object(monitor, "Collector") as metrics_collector:
                collector.return_value.sample.return_value = {"host": {"status": "available"}}
                metrics_collector.return_value.sample.side_effect = lambda _: evidence(metrics.now_ms())
                observer = monitor.ResourceObserver(f.layout, node, stopped, .02, f.settings, {"id": "session-a"})
                path = node_monitor.root(f.layout)/"resources.sqlite3"
                try:
                    deadline = time.monotonic() + 2
                    count = 0
                    while time.monotonic() < deadline:
                        if path.exists():
                            try:
                                with history.History(path, node_monitor.scope(f.layout)) as db:
                                    count = len(db.query()["records"])
                            except (ValueError, sqlite3.Error):
                                pass
                        if count >= 3:
                            break
                        time.sleep(.01)
                    self.assertGreaterEqual(count, 3)
                    self.assertTrue(probe.is_alive())
                finally:
                    stopped.set()
                    observer.worker.join(2)
                    release.set()
                    probe.join(2)

    def test_transition_evidence_is_redacted_and_failure_does_not_block_controller(self):
        with MonitorFixture() as f:
            monitor.transition(f.layout, node, "RESOURCE_TRANSITION_STARTED", "operation-a", "bitcoin", "overlap",
                               observed={"btc-node": {"memory": 123, "secret": "SECRET"}})
            monitor.transition(f.layout, node, "RESOURCE_TRANSITION_FAILED", "operation-a", "bitcoin", "overlap",
                               error=ValueError("SECRET"))
            with history.History(node_monitor.root(f.layout)/"resources.sqlite3", node_monitor.scope(f.layout)) as db:
                records = db.query(resolution="transitions")["records"]
            self.assertEqual(len(records), 2)
            self.assertEqual(records[0]["operation_id"], records[1]["operation_id"])
            self.assertNotIn("SECRET", json.dumps(records))
            with mock.patch.object(monitor, "History", side_effect=sqlite3.OperationalError("SECRET")), contextlib.redirect_stderr(io.StringIO()) as output:
                monitor.transition(f.layout, node, "RESOURCE_TRANSITION_APPLIED", "operation-a", "bitcoin", "overlap")
            self.assertIn("RESOURCE_TRANSITION_RECORD_FAILED", output.getvalue())
            self.assertNotIn("SECRET", output.getvalue())


if __name__ == "__main__":
    unittest.main()
