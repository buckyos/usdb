#!/usr/bin/env python3
"""P1 process recovery: actual locks/journals, controlled service and observation inputs."""
from collections import deque
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import signal
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_monitor as monitor
import resource_policy as policy
import usdb_node as node
from common.process_recovery import Processes, write_json
from common.process_recovery_worker import container, layout
from common.snapshot_range_server import SnapshotRangeServer
from common.bitcoin_bootstrap import BootstrapCore
import bitcoin_assumeutxo as boot


class NodeProcessRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.processes = Processes(self.id())
        self.addCleanup(self.processes.close)
        self.root = self.processes.root
        self.layout = layout(self.root)

    def configure(self, phase="bitcoin"):
        env = {**policy.build_resource_plan(64 * policy.GIB, phase, {}).environment(),
               "USDB_BITCOIN_IMAGE": "bitcoin@sha256:1", "USDB_SERVICES_IMAGE": "services@sha256:2",
               "USDB_MONITOR_ENABLED": "1", "USDB_NODE_ROLE": "full"}
        self.layout.node_env.write_text("".join(f"{key}={value}\n" for key, value in env.items()))
        self.layout.node_env.chmod(0o600)
        services = ["btc-node"] if phase == "bitcoin" else ["btc-node", "balance-history", "usdb-indexer"]
        write_json(self.root / "containers.json", {s: container(env, s) for s in services})

    def actions(self):
        path = self.root / "actions.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def controller_restart(self, stop_signal):
        self.configure()
        child = self.processes.start("controller", "overlap", "waiting-dependency")
        self.processes.boundary(child, "waiting-dependency")
        before = self.actions()
        duplicate = self.processes.start("controller", "overlap", "")
        self.assertNotEqual(duplicate.wait(timeout=8), 0)
        self.assertIn("another usdb-node operation", duplicate.log_path.read_text())
        self.assertEqual(self.actions(), before)
        self.assertEqual(self.processes.stop(child, stop_signal), -stop_signal)
        self.processes.success(self.processes.start("controller", "overlap", ""))
        self.assertEqual(self.actions(), before, "already adopted Core allocation was applied again")
        self.assertEqual((self.root / ".usdb-node-operation.lock").read_text(), "")
        self.processes.success(self.processes.start("controller", "overlap", ""))
        self.assertEqual(self.actions(), before, "READY controller restarted services")

    def test_controller_kill_restarts_waiting_and_rejects_concurrent_operation(self):
        self.controller_restart(signal.SIGKILL)

    def test_controller_term_restarts_waiting_and_rejects_concurrent_operation(self):
        self.controller_restart(signal.SIGTERM)

    def resource_crash(self, old, target, boundary):
        self.configure(old)
        original_services = set(json.loads((self.root / "containers.json").read_text()))
        child = self.processes.start("controller", target, boundary)
        self.processes.boundary(child, boundary)
        self.assertTrue(node._read_resource_state(self.layout)["pending"])
        self.assertEqual(self.processes.stop(child, signal.SIGKILL), -signal.SIGKILL)
        self.processes.success(self.processes.start("controller", target, ""))
        state = node._read_resource_state(self.layout)
        self.assertFalse(state["pending"])
        self.assertEqual(state["recover_services"], [])
        self.assertEqual(state["phase"], target)
        actions = [item["action"] for item in self.actions()]
        self.assertEqual(actions.count("start"), 1, "resumption duplicated Core startup")
        self.assertLess(actions.index("down"), actions.index("start"))
        env = node.read_env(self.layout.node_env)
        observed = json.loads((self.root / "containers.json").read_text())
        for service in original_services:
            self.assertIn(service, observed)
            self.assertEqual(observed[service]["state"], "running", f"{service} was not restored")
            self.assertTrue(node._resource_container_matches(observed[service], env, service))
        node._check_running_resource_budget(env, observed)

    def test_bitcoin_overlap_after_stop(self):
        self.resource_crash("bitcoin", "overlap", "after-down")

    def test_bitcoin_overlap_after_start(self):
        self.resource_crash("bitcoin", "overlap", "after-start")

    def test_overlap_steady_after_stop(self):
        self.resource_crash("overlap", "steady", "after-down")

    def test_overlap_steady_after_start(self):
        self.resource_crash("overlap", "steady", "after-start")

    def test_monitor_term_and_kill_keep_incident_and_publish_fresh_samples(self):
        self.configure("steady")
        original_config = self.layout.node_env.read_bytes()
        original_containers = (self.root / "containers.json").read_bytes()
        last_session, last_sample, alert_id = None, 0, None
        for stop_signal in (signal.SIGTERM, signal.SIGKILL, signal.SIGTERM):
            child = self.processes.start("monitor")
            def sampled():
                try:
                    with monitor.database(self.layout) as store:
                        at = store.get("last_sample_ms") or 0
                        if store.get("process_id") == child.pid and at > last_sample:
                            return store.get("session"), at, store.alerts(), store.events()
                except (FileNotFoundError, ValueError):
                    return None
            session, last_sample, alerts, events = self.processes.wait(sampled, child)
            # Require another successful sample from this process, not only its initialization.
            session, last_sample, alerts, events = self.processes.wait(sampled, child)
            def published():
                path = self.root / "console/node-progress.json"
                if not path.exists():
                    return False
                value = json.loads(path.read_text())
                return (value.get("monitor", {}).get("session") == session
                        and value.get("observed_at_ms", 0) >= last_sample)
            self.processes.wait(published, child)
            self.assertNotEqual(session, last_session)
            self.assertEqual(len(alerts), 1)
            alert_id = alert_id or alerts[0]["alert_id"]
            self.assertEqual(alerts[0]["alert_id"], alert_id)
            self.assertEqual(sum(e["code"] == "ALERT_FIRING" for e in events), 1)
            duplicate = self.processes.start("monitor")
            self.assertNotEqual(duplicate.wait(timeout=8), 0)
            self.assertIn("already running", duplicate.log_path.read_text())
            self.assertTrue(monitor.initialized_process(self.layout, child.pid))
            code = self.processes.stop(child, stop_signal)
            self.assertEqual(code, 0 if stop_signal == signal.SIGTERM else -signal.SIGKILL)
            self.assertFalse(monitor.is_running(self.layout))
            last_session = session
        self.assertEqual(self.layout.node_env.read_bytes(), original_config)
        self.assertEqual((self.root / "containers.json").read_bytes(), original_containers)

    def test_download_process_resumes_saved_range_and_rejects_corruption(self):
        payload = bytes(range(256)) * 32
        server = SnapshotRangeServer(self.root, payload)
        self.addCleanup(server.close)
        server.plans[0] = deque(["cut-valid"])
        write_json(self.root / "download.json", dict(url=server.url, snapshot=dict(
            base_height=935000, base_hash="a" * 64, file_sha256=hashlib.sha256(payload).hexdigest(), size_bytes=len(payload))))
        env = {"SSL_CERT_FILE": str(self.root / "server.crt"), "no_proxy": "127.0.0.1"}
        child = self.processes.start("download", "download-retry", env=env)
        self.processes.boundary(child, "download-retry")
        partial = self.root / "snapshot.dat.download/snapshot.part"
        offset = partial.stat().st_size
        self.assertTrue(0 < offset < len(payload))
        self.assertFalse((self.root / "snapshot.dat").exists())
        self.assertEqual(self.processes.stop(child, signal.SIGKILL), -signal.SIGKILL)
        self.processes.success(self.processes.start("download", "", env=env))
        self.assertIn((offset, len(payload) - 1), server.requests)
        source = self.root / "snapshot.dat"
        self.assertEqual(source.read_bytes(), payload)
        source.write_bytes(b"X" + payload[1:])
        child = self.processes.start("download", "", env=env)
        self.assertNotEqual(child.wait(timeout=8), 0)
        self.assertIn("SHA-256", child.log_path.read_text())

    def activation_fixture(self):
        payload = bytes(range(256)) * 32
        snapshot = boot.Snapshot(935000, "a" * 64, hashlib.sha256(payload).hexdigest(), len(payload))
        core = BootstrapCore(snapshot)
        self.addCleanup(core.close)
        core.release_load.clear()
        (self.root / "snapshot.dat").write_bytes(payload)
        write_json(self.root / "activate.json", dict(snapshot=asdict(snapshot), rpc=core.url))
        return core

    def test_core_import_observer_restart_does_not_repeat_an_active_load(self):
        core = self.activation_fixture()
        first = self.processes.start("activate")
        self.processes.wait(core.started_load.is_set, first)
        self.assertEqual(self.processes.stop(first, signal.SIGKILL), -signal.SIGKILL)
        calls = core.calls["getrpcinfo"]
        resumed = self.processes.start("activate")
        self.processes.wait(lambda: core.calls["getrpcinfo"] > calls, resumed)
        self.assertEqual(core.calls["loadtxoutset"], 1)
        core.release_load.set()
        self.processes.success(resumed)
        self.assertTrue(json.loads((self.root / "activation-result.json").read_text())["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 1)

    def test_core_import_unknown_outcome_requires_explicit_recovery(self):
        core = self.activation_fixture()
        core.drop_before_activation = True
        first = self.processes.start("activate")
        self.processes.wait(core.started_load.is_set, first)
        self.assertEqual(self.processes.stop(first, signal.SIGKILL), -signal.SIGKILL)
        core.release_load.set()
        self.processes.wait(lambda: not core.loading)
        resumed = self.processes.start("activate")
        self.assertNotEqual(resumed.wait(timeout=8), 0)
        self.assertIn("Previous load outcome is uncertain", resumed.log_path.read_text())
        self.assertEqual(core.calls["loadtxoutset"], 1)


if __name__ == "__main__":
    unittest.main()
