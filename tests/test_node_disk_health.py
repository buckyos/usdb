"""Disk faults stay separate from slow I/O, readiness and missing log permissions."""
import json
from pathlib import Path
import subprocess
import sys
import threading
import contextlib
import io
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_disk_health as disk
import node_resource_monitor as resources
import node_monitor
import usdb_node as node
from common.node_monitor import BASE, MonitorFixture


def records(messages, at=BASE):
    return [json.dumps(dict(MESSAGE=message, __REALTIME_TIMESTAMP=str(at * 1000))) for message in messages]


class DiskHealthTests(unittest.TestCase):
    def test_classifies_kernel_errors_without_exporting_raw_messages(self):
        value = disk.summarize(records([
            "ata1.00: SError: { BadCRC } private-path",
            "ata1: hard resetting link",
            "I/O error, dev sda, sector 42 secret",
            "EXT4-fs error (device sda1): PRIVATE",
            "EXT4-fs (sda1): Remounting filesystem read-only",
            "nvme0n1: I/O error, dev nvme0n1",
            "ord: commit elapsed=7200, iowait=100%, password=SECRET",
        ]), BASE)
        self.assertEqual({v["code"]: v["count"] for v in value["errors"]},
                         dict(ata_link_error=2, io_error=2, filesystem_error=1, filesystem_read_only=1))
        self.assertNotIn("secret", json.dumps(value).lower())
        self.assertNotIn("sda", json.dumps(value))
        self.assertEqual(disk.summarize(records(["I/O error"], BASE - disk.WINDOW_MS - 1), BASE)["errors"], [])
        self.assertTrue(disk.summarize(records(["I/O error"] * disk.MAX_RECORDS), BASE)["truncated"])

    def test_cache_and_unavailable_sources_are_not_healthy(self):
        reader = disk.Collector()
        with mock.patch.object(disk, "read_kernel_records", return_value=[]) as read, \
             mock.patch.object(disk.time, "monotonic", side_effect=[0, 10, 61]):
            self.assertEqual(reader.sample()["status"], "available")
            reader.sample()
            self.assertEqual(read.call_count, 1)
            read.side_effect = PermissionError()
            self.assertEqual(reader.sample()["status"], "unavailable")
            self.assertEqual(read.call_count, 2)
        for error in (FileNotFoundError(), subprocess.TimeoutExpired("journalctl", 3), ValueError()):
            with mock.patch.object(disk, "read_kernel_records", side_effect=error):
                self.assertEqual(disk.Collector().sample()["status"], "unavailable")

    def test_reader_bounds_execution_and_handles_empty_match_permission_and_large_output(self):
        def run(args, **kwargs):
            self.assertEqual(args[0], "journalctl")
            self.assertIn("--boot", args)
            self.assertEqual(kwargs["timeout"], 3)
            return subprocess.CompletedProcess(args, 1)
        with mock.patch.object(disk.subprocess, "run", side_effect=run):
            self.assertEqual(disk.read_kernel_records(BASE), [])
        def denied(args, **kwargs):
            kwargs["stderr"].write(b"Hint: You are currently not seeing messages from the system.")
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(disk.subprocess, "run", side_effect=denied), self.assertRaises(ValueError):
            disk.read_kernel_records(BASE)
        def large(args, **kwargs):
            kwargs["stdout"].write(b"x" * (disk.MAX_OUTPUT + 1))
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(disk.subprocess, "run", side_effect=large), self.assertRaises(ValueError):
            disk.read_kernel_records(BASE)

    def test_errors_fire_immediately_deduplicate_and_unknown_does_not_resolve(self):
        with MonitorFixture() as f:
            value = disk.summarize(records(["ata1: hard resetting link"]), BASE)
            resources.evaluate(f.store, {"disk_health": value}, BASE, f.settings)
            alert = next(a for a in f.store.alerts() if a["code"] == "DISK_KERNEL_ERRORS")
            self.assertEqual((alert["state"], alert["severity"]), ("firing", "critical"))
            disk.evaluate(f.store, value, BASE + 1000, f.settings)
            self.assertEqual(sum(v["code"] == "ALERT_FIRING" for v in f.store.events()), 1)
            disk.evaluate(f.store, dict(status="unavailable", at_ms=BASE + 60000), BASE + 60000, f.settings)
            active = next(a for a in f.store.alerts() if a["code"] == "DISK_KERNEL_ERRORS")
            self.assertEqual((active["state"], active["condition"]), ("firing", "unknown"))
            disk.evaluate(f.store, dict(status="unavailable", at_ms=BASE + 120000), BASE + 120000, f.settings)
            self.assertIn("DISK_HEALTH_UNAVAILABLE", [a["code"] for a in f.store.alerts() if a["state"] == "firing"])
            # Only fresh successful empty windows resolve; this is not a hardware repair claim.
            for offset in (180000, 240000):
                disk.evaluate(f.store, disk.summarize([], BASE + offset), BASE + offset, f.settings)
            self.assertFalse(f.store.alerts())

    def test_disk_evidence_survives_a_failed_resource_probe(self):
        import control_plane_resources
        with MonitorFixture() as f:
            stopped = threading.Event()
            sample = disk.summarize(records(["I/O error"]), BASE)
            def failed_probe(*_):
                stopped.set()
                raise OSError("Docker observation failed")
            with mock.patch.object(disk.Collector, "sample", return_value=sample), \
                 mock.patch.object(control_plane_resources.ResourceCollector, "sample", side_effect=failed_probe), \
                 contextlib.redirect_stderr(io.StringIO()):
                observer = resources.ResourceObserver(f.layout, node, stopped, .01, f.settings)
                observer.worker.join(2)
            self.assertFalse(observer.worker.is_alive())
            self.assertEqual(observer.snapshot()["disk_health"], sample)
            self.assertEqual(observer.snapshot()["history"]["state"], "unavailable")

    def test_monitor_summary_marks_retained_disk_observation_stopped_or_stale(self):
        with MonitorFixture() as f:
            f.store.put("disk_health", disk.summarize([], BASE))
            self.assertEqual(node_monitor.summary(f.store, "stopped", at=BASE)["disk_health"]["status"], "stopped")
            self.assertEqual(node_monitor.summary(f.store, "running", at=BASE + 240000)["disk_health"]["status"], "stale")

    def test_stale_sample_cannot_continue_good_or_bad_evidence(self):
        with MonitorFixture() as f:
            sample = disk.summarize(records(["I/O error"]), BASE)
            disk.evaluate(f.store, sample, BASE, f.settings)
            disk.evaluate(f.store, sample, BASE + disk.INTERVAL_MS * 4, f.settings)
            self.assertEqual(f.store.get("disk_health")["status"], "stale")
            active = next(a for a in f.store.alerts() if a["code"] == "DISK_KERNEL_ERRORS")
            self.assertEqual(active["condition"], "unknown")


if __name__ == "__main__":
    unittest.main()
