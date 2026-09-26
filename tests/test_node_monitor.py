"""Event durability, rule continuity, CLI and bounded monitor execution."""
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_monitor as monitor
import node_monitor_rules as rules
from control_plane_monitor import project
import usdb_node as node
from common.node_monitor import BASE, MonitorFixture, incident, report


class StoreTests(unittest.TestCase):
    def test_release_metadata_and_trust_key_rotation_preserve_node_identity(self):
        with MonitorFixture() as f:
            identity = f.store.get("node_id")
            f.layout.network_identity.update(network_json_sha256="changed", snapshot_trusted_keys_sha256="rotated",
                                             btc_activation_registry_id="new-revision")
            with monitor.database(f.layout, writable=True) as reopened:
                self.assertEqual(reopened.get("node_id"), identity)
            f.layout.network_identity["genesis_block_hash"] = "b" * 64
            with self.assertRaisesRegex(ValueError, "different node network"):
                monitor.database(f.layout, writable=True)

    def test_incident_survives_restart_and_requires_continuous_proven_recovery(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            alert = next(v for v in f.store.alerts() if v["latched"])
            identity = alert["alert_id"]
            f.store.end(BASE + 2000, "planned_down")
            with monitor.database(f.layout, writable=True) as reopened:
                reopened.begin(BASE + 1000000, "r2")
                rules.evaluate(reopened, report(BASE + 1000001, incidents=dict(status="unavailable", events=[])), BASE + 1000001, f.settings)
                rules.evaluate(reopened, report(BASE + 1000002), BASE + 1000002, f.settings)
                active = next(v for v in reopened.alerts() if v["alert_id"] == identity)
                self.assertEqual(active["state"], "firing")
                self.assertEqual(active["condition"], "good")
                rules.evaluate(reopened, report(BASE + 1002002), BASE + 1002002, f.settings)
                self.assertNotIn(identity, [v["alert_id"] for v in reopened.alerts()])
                self.assertIn("ALERT_RESOLVED", [v["code"] for v in reopened.events()])

    def test_stable_incident_deduplication_even_without_source_id(self):
        for identity in ("b" * 32, None):
            with self.subTest(identity=identity), MonitorFixture() as f:
                for at in range(10):
                    f.tick(at, incidents=incident(identity))
                self.assertEqual(f.codes().count("ALERT_FIRING"), 1)
                self.assertEqual(len([v for v in f.store.alerts() if v["latched"]]), 1)
                self.assertEqual(f.store.alerts()[0]["occurrences"], 10)

    def test_transaction_failure_rolls_back_events_and_alert_state(self):
        with MonitorFixture() as f:
            with self.assertRaises(RuntimeError), f.store.db:
                f.store.condition("broken", "usdb_chain", "DEEP_REORG_HALTED", True, BASE, latched=True)
                raise RuntimeError("injected crash")
            with monitor.database(f.layout) as reopened:
                self.assertEqual(reopened.alerts(), [])
                self.assertNotIn("ALERT_FIRING", [v["code"] for v in reopened.events()])

    def test_network_and_schema_mismatch_never_reinitialize_history(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            f.layout.network_identity = {"chain_id": 13}
            with self.assertRaisesRegex(ValueError, "different node network"):
                monitor.database(f.layout, writable=True)
            f.store.db.execute("PRAGMA user_version=99")
            with self.assertRaisesRegex(ValueError, "Unsupported monitor database"):
                monitor.database(f.layout, writable=True)
            self.assertEqual(f.codes().count("ALERT_FIRING"), 1)

    def test_retention_preserves_active_incidents_and_limits_ordinary_history(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            with f.store.db:
                for i in range(20):
                    f.store.event(BASE + i, "monitor", "TEST_EVENT")
            f.store.prune(BASE + 2000, days=1, count=5)
            events = f.store.events()
            self.assertEqual(len([v for v in events if v["code"] == "TEST_EVENT"]), 5)
            self.assertIn("ALERT_FIRING", [v["code"] for v in events])

    def test_queries_do_not_create_database_or_change_node(self):
        with MonitorFixture() as f:
            path = monitor.root(f.layout) / "events.sqlite3"
            f.store.close()
            path.unlink()
            result = monitor.status(f.layout, node)
            self.assertFalse(path.exists())
            self.assertEqual(result["storage"], "missing")

    def test_private_paths_reject_symlinks_and_shared_database(self):
        with MonitorFixture() as f:
            path = monitor.root(f.layout) / "events.sqlite3"
            path.chmod(0o644)
            with self.assertRaisesRegex(ValueError, "private regular"):
                monitor.database(f.layout)
            path.chmod(0o600)
            sidecar = monitor.root(f.layout) / "events.sqlite3-journal"
            sidecar.symlink_to(f.layout.node_env)
            with self.assertRaisesRegex(ValueError, "private regular"):
                monitor.database(f.layout, writable=True)


class RuleTests(unittest.TestCase):
    def test_controller_manual_seed_wait_does_not_fire_failure_alert(self):
        for display, exit_status, expected in (("waiting_for_seed", 2, False), ("idle", 2, False),
                                              ("failed", 1, True), ("manual_action", 2, True),
                                              ("waiting_for_seed", 1, True)):
            with self.subTest(display=display, exit_status=exit_status), MonitorFixture() as f:
                for seconds in (0, 1, 3, 5):
                    at = BASE + seconds * 1000
                    value = report(at)
                    value["controller"] = dict(runtime_state="failed", display_state=display,
                                               result="exit-code", exit_code=1, exit_status=exit_status)
                    rules.evaluate(f.store, project(value, at), at, f.settings)
                self.assertEqual(any(v["code"] == "CONTROLLER_FAILED" for v in f.store.alerts()), expected)

    def test_unready_initial_sync_never_becomes_readiness_regression(self):
        with MonitorFixture() as f:
            for at in range(10):
                f.tick(at, ready=False, phase="SYNCING", target=100)
            self.assertEqual(f.store.alerts(), [])

    def test_regression_escalation_unknown_and_consecutive_recovery(self):
        with MonitorFixture() as f:
            f.tick(0)
            for at in (1, 2, 3, 5):
                f.tick(at, ready=False)
            alerts = [v for v in f.store.alerts() if v["code"] == "CONSENSUS_READINESS_LOST"]
            self.assertEqual(len(alerts), 4)
            self.assertTrue(all(v["severity"] == "critical" for v in alerts))
            ids = [v["alert_id"] for v in alerts]
            f.tick(6, available=False)
            f.tick(7)
            f.tick(8, available=False)
            f.tick(9)
            f.tick(10)
            self.assertTrue(all(v["state"] == "firing" for v in f.store.alerts() if v["alert_id"] in ids))
            f.tick(11)
            self.assertFalse(any(v["alert_id"] in ids for v in f.store.alerts()))
            f.tick(12, ready=False)
            f.tick(14, ready=False)
            self.assertFalse(set(ids) & {v["alert_id"] for v in f.store.alerts()})

    def test_sampling_gap_restart_and_clock_jump_do_not_count_as_continuity(self):
        for mode in ("gap", "restart", "clock_back"):
            with self.subTest(mode=mode), MonitorFixture() as f:
                f.tick(0)
                f.tick(1, ready=False)
                if mode == "restart":
                    f.store.begin(BASE + 200000, "r2")
                at = -10 if mode == "clock_back" else 200
                f.tick(at, ready=False)
                self.assertNotIn("ALERT_FIRING", f.codes())

    def test_critical_incident_is_not_delayed_by_startup_grace_or_rpc_failure(self):
        with MonitorFixture() as f:
            f.settings["startup_grace_secs"] = 600
            f.tick(0, available=False, incidents=incident())
            self.assertEqual([v["severity"] for v in f.store.alerts() if v["latched"]], ["critical"])

    def test_stale_sample_never_resolves_firing_alert(self):
        with MonitorFixture() as f:
            f.tick(0)
            f.tick(1, ready=False)
            f.tick(3, ready=False)
            rules.evaluate(f.store, report(BASE), BASE + 1000000, f.settings)
            self.assertTrue(any(v["state"] == "firing" for v in f.store.alerts()))

    def test_stall_requires_upstream_advancement_and_resets_on_local_progress(self):
        with MonitorFixture() as f:
            for at in range(8):
                f.tick(at, ready=False, phase="SYNCING", current=10, target=20)
            self.assertEqual(f.store.alerts(), [])
            for at in range(8, 15):
                f.tick(at, ready=False, phase="SYNCING", current=10, target=21 + at)
            self.assertTrue(any(v["code"] == "SYNC_STALLED" and v["state"] == "firing" for v in f.store.alerts()))
            for at in range(15, 18):
                f.tick(at, ready=False, phase="SYNCING", current=30, target=40)
            self.assertFalse(any(v["code"] == "SYNC_STALLED" for v in f.store.alerts()))

    def test_runtime_history_tracks_delta_and_does_not_invent_oom_history(self):
        with MonitorFixture() as f:
            f.tick(0, restarts=100)
            self.assertNotIn("CONTAINER_RESTARTED", f.codes())
            f.tick(1, restarts=101)
            self.assertEqual(f.codes().count("CONTAINER_RESTARTED"), 4)
            value = report(BASE + 2000, restarts=0)
            for item in value["observations"]["services"].values():
                item["runtime"].update(container_id="c" * 64, oom_killed=True)
            rules.evaluate(f.store, value, BASE + 2000, f.settings)
            rules.evaluate(f.store, value, BASE + 3000, f.settings)
            self.assertEqual(f.codes().count("CONTAINER_RESTARTED"), 4)
            self.assertEqual(f.codes().count("CONTAINER_OOM_OBSERVED"), 4)

    def test_raw_rpc_errors_and_unknown_fields_never_enter_event_database(self):
        with MonitorFixture() as f:
            value = report(BASE, incidents=incident())
            value["observations"]["incidents"]["events"][0]["error"] = "PRIVATE_SECRET"
            value["observations"]["services"]["bitcoin"]["runtime"]["env"] = "PRIVATE_SECRET"
            rules.evaluate(f.store, value, BASE, f.settings)
            self.assertNotIn("PRIVATE_SECRET", json.dumps(monitor.summary(f.store, "running")))


class CommandTests(unittest.TestCase):
    def test_resource_sampler_reuses_cache_and_does_not_block_the_monitor(self):
        import control_plane_resources
        with MonitorFixture() as f:
            stopped = threading.Event()
            samples = []

            def sample(*_):
                samples.append(1)
                if len(samples) == 2:
                    stopped.set()
                return {"sample": len(samples)}

            with mock.patch.object(control_plane_resources, "ResourceCollector") as collector:
                collector.return_value.sample.side_effect = sample
                observer = monitor.ResourceObserver(f.layout, node, stopped, 0.001)
                observer.worker.join(timeout=1)
                self.assertFalse(observer.worker.is_alive())
                collector.assert_called_once()
                self.assertEqual(observer.snapshot()["sample"], 2)
                self.assertEqual(observer.snapshot()["history"]["state"], "available")

    def test_independent_incident_probe_survives_regular_collection_failure(self):
        import control_plane_monitor as console
        with MonitorFixture() as f:
            stop = []

            def sample(layout, node, timeout, stopped, *, incidents_only=False, **_):
                stop[:] = [stopped]
                return report(BASE, incidents=incident(), available=False) if incidents_only else monitor.empty_report()

            def publish(layout, node, value):
                if value.get("observations", {}).get("incidents", {}).get("events"):
                    stop[0].set()

            with mock.patch.object(monitor, "sample", side_effect=sample), \
                    mock.patch.object(monitor, "ResourceObserver") as resources, \
                    mock.patch.object(monitor, "now_ms", return_value=BASE), \
                    mock.patch.object(monitor.signal, "signal"), \
                    mock.patch.object(console, "publish", side_effect=publish):
                resources.return_value.snapshot.return_value = {}
                monitor.run(f.layout, node)
            self.assertEqual(f.codes().count("ALERT_FIRING"), 1)
            self.assertFalse(f.store.get("last_observation_available"))
            self.assertTrue(f.store.alerts()[0]["latched"])

    def test_console_export_failure_does_not_interrupt_event_recording(self):
        import control_plane_monitor as console
        with MonitorFixture() as f:
            with mock.patch.object(console, "publish", side_effect=PermissionError(13, "PRIVATE_SECRET")):
                for at in (0, 1):
                    f.tick(at, incidents=incident())
                    monitor.publish_state(f.layout, node, "running", store=f.store)
            self.assertEqual(f.codes().count("ALERT_FIRING"), 1)
            self.assertEqual(f.codes().count("CONSOLE_EXPORT_FAILED"), 1)
            self.assertFalse(f.store.get("console_export_available"))
            self.assertNotIn("PRIVATE_SECRET", json.dumps(f.store.events()))
            monitor.publish_state(f.layout, node, "running", store=f.store)
            self.assertTrue(f.store.get("console_export_available"))
            self.assertIn("CONSOLE_EXPORT_RECOVERED", f.codes())

    def test_real_daemon_restarts_preserve_incident_and_exit_without_live_services(self):
        with MonitorFixture() as f:
            script = f.root / "fake-node.py"
            tools_dir = str(Path(node.__file__).parent)
            tests_dir = str(Path(__file__).parent)
            script.write_text(f'''import json, sys
from pathlib import Path
from types import SimpleNamespace
sys.path[:0] = [{tools_dir!r}, {tests_dir!r}]
import node_monitor as monitor
import usdb_node as node
from common.node_monitor import report, incident
if "sample" in sys.argv:
    print(json.dumps(report(monitor.now_ms(), incidents=incident())))
else:
    layout = SimpleNamespace(node_env=Path({str(f.layout.node_env)!r}), kit_root=Path({str(f.root)!r}),
        release_id=sys.argv[-1], bundle_id="test", network_identity={f.layout.network_identity!r})
    node.__file__ = __file__
    monitor.ResourceObserver = lambda *args: SimpleNamespace(snapshot=lambda: {{"status": "unavailable"}}, observe=lambda report: None)
    monitor.run(layout, node)
''')
            for release in ("r1", "r2"):
                process = subprocess.Popen([sys.executable, str(script), "run", release], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                try:
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        with monitor.database(f.layout) as store:
                            if store.get("process_id") == process.pid and store.get("last_sample_ms"):
                                break
                        if process.poll() is not None:
                            self.fail(process.stderr.read().decode())
                        time.sleep(0.05)
                    else:
                        self.fail("Daemon did not persist a completed sample")
                    f.layout.release_id = release
                    self.assertTrue(monitor.initialized_process(f.layout, process.pid))
                    self.assertEqual(f.codes().count("ALERT_FIRING"), 1)
                finally:
                    process.terminate()
                    _, errors = process.communicate(timeout=5)
                self.assertEqual(process.returncode, 0, errors.decode())
                self.assertFalse(monitor.is_running(f.layout))
                self.assertEqual(monitor.status(f.layout, node)["state"], "stopped")
            self.assertEqual(f.codes().count("MONITOR_STOPPED"), 2)

    def test_initialization_requires_current_live_process_and_event_store(self):
        with MonitorFixture() as f, monitor.run_lock(f.layout):
            self.assertFalse(monitor.initialized_process(f.layout, os.getpid()))
            with f.store.db:
                f.store.put("process_id", os.getpid())
            self.assertTrue(monitor.initialized_process(f.layout, os.getpid()))
            f.store.end(BASE + 10, "service_stop")
            self.assertFalse(monitor.initialized_process(f.layout, os.getpid()))

    def test_planned_down_retains_incidents_and_stops_monitor_before_services(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            unit = f.root / "monitor.service"
            unit.write_text("fixture")
            with mock.patch.object(monitor, "unit_path", return_value=unit), \
                    mock.patch.object(node, "_privileged_command") as privileged:
                monitor.stop(f.layout, node)
            privileged.assert_called_once_with(["systemctl", "stop", monitor.unit_name(f.layout)])
            self.assertTrue(f.store.get("stop_requested"))
            self.assertIn("NODE_DOWN_REQUESTED", f.codes())
            self.assertEqual(f.store.alerts()[0]["state"], "firing")
            value = json.loads((f.layout.node_env.parent / "console/node-progress.json").read_text())
            self.assertEqual(value["monitor"]["state"], "stopped")
            self.assertEqual(len(value["monitor"]["alerts"]), 1)

    def test_event_query_filters_without_live_probes(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            output = io.StringIO()
            args = node.build_parser().parse_args(["monitor", "events", "--service", "usdb_chain", "--severity", "critical", "--json"])
            with mock.patch.object(node, "collect_node_progress", side_effect=AssertionError("live probe")), redirect_stdout(output):
                monitor.dispatch(args, f.layout, node)
            events = json.loads(output.getvalue())
            self.assertEqual(len(events), 1)
    def test_incident_marker_absence_without_ready_does_not_resolve(self):
        with MonitorFixture() as f:
            f.tick(0, incidents=incident())
            for at in (2, 4, 8):
                f.tick(at, phase="FAILED")
            self.assertTrue(f.store.alerts()[0]["latched"])
            f.tick(9)
            f.tick(11)
            self.assertFalse(any(v["latched"] for v in f.store.alerts()))

    def test_configuration_is_bounded_and_enabled_choice_persists(self):
        with MonitorFixture() as f:
            args = node.build_parser().parse_args(["monitor", "configure", "--enabled", "off", "--interval-secs", "15"])
            with mock.patch.object(node, "_collect_compose_services", return_value={}), redirect_stdout(io.StringIO()):
                monitor.dispatch(args, f.layout, node)
            self.assertFalse(monitor.enabled(f.layout, node))
            self.assertEqual(monitor.config(f.layout)["interval_secs"], 15)
            with self.assertRaisesRegex(ValueError, "critical window"):
                monitor.validate_config({**rules.DEFAULTS, "warning_after_secs": 1000})

    def test_single_instance_lock_and_no_second_console_publisher(self):
        import control_plane_monitor as console
        with MonitorFixture() as f, monitor.run_lock(f.layout):
            self.assertTrue(monitor.is_running(f.layout))
            with self.assertRaisesRegex(ValueError, "already running"), monitor.run_lock(f.layout):
                pass
            with mock.patch.object(console, "collect", return_value=report()), mock.patch.object(console, "publish") as publish:
                console.export(f.layout, node)
            publish.assert_not_called()

    def test_sample_timeout_and_stop_kill_probe_process_group(self):
        with MonitorFixture() as f:
            script = f.root / "fake-node.py"
            script.write_text("import time\ntime.sleep(30)\n")
            fake_node = mock.Mock(__file__=str(script))
            started = time.monotonic()
            value = monitor.sample(f.layout, fake_node, 0.1)
            self.assertFalse(value["observation_available"])
            self.assertLess(time.monotonic() - started, 2)
            stop = threading.Event()
            stop.set()
            value = monitor.sample(f.layout, fake_node, 30, stop)
            self.assertFalse(value["observation_available"])
            self.assertLess(time.monotonic() - started, 2)

    def test_failed_persistence_publishes_failure_and_leaves_existing_history(self):
        import control_plane_monitor as console
        with MonitorFixture() as f:
            with mock.patch.object(monitor, "sample", return_value=report()), \
                    mock.patch.object(monitor, "ResourceObserver"), \
                    mock.patch.object(rules, "evaluate", side_effect=sqlite3.OperationalError("PRIVATE_SECRET")), \
                    mock.patch.object(console, "publish") as publish, \
                    mock.patch.object(monitor.signal, "signal"), \
                    self.assertRaisesRegex(ValueError, "Node monitor failed"):
                monitor.run(f.layout, node)
            self.assertEqual(publish.call_args.args[2]["monitor"]["state"], "failed")
            self.assertEqual(publish.call_args.args[2]["monitor"]["storage"], "unavailable")
            self.assertNotIn("PRIVATE_SECRET", json.dumps(publish.call_args.args[2]))
            self.assertIn("MONITOR_STARTED", f.codes())


if __name__ == "__main__":
    unittest.main()
