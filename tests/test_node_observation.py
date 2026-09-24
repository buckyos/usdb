"""Accept lossless readiness evidence and durable, private incident observations."""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import chain_file_inspection as files
import control_plane_monitor as console
import node_observation as observation
import usdb_node as node
import usdb_mining as mining
from common.mining import MiningFixture


class ObservationTests(unittest.TestCase):
    def write_incident(self, fixture, *, legacy=False):
        path = Path(fixture.env["USDB_CHAIN_DATA_HOST_DIR"]) / "recovery/deep-btc-reorg/halted.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        value = dict(schema_version=files.INCIDENT_SCHEMA, baseline_epoch=2, observed_epoch=3,
                     reason="upstream_reorg_epoch_advanced", detected_at="2026-09-24T01:02:03Z",
                     indexer_rpc_url="http://operator:PRIVATE@rpc", indexer_readiness={"message": "PRIVATE"},
                     code="UNTRUSTED_CODE", severity="info", recovery="automatic")
        if not legacy:
            value["incident_id"] = "ab" * 16
        path.write_text(json.dumps(value))
        return path

    def test_caught_up_indexer_retains_missing_commit_reason_through_console(self):
        with MiningFixture() as f:
            ready = {**f.ready, "rpc_alive": True, "query_ready": True, "consensus_ready": False,
                     "current": 100, "total": 100, "balance_history_stable_height": 100,
                     "block_processing_pending_height": 101, "blockers": ["SystemStateMissing"],
                     "message": "PRIVATE http://user:PRIVATE@rpc"}
            component = node._indexed_service_component("usdb_indexer", {"state": "running"}, ready, None, "")
            report = dict(overall_state="SYNCING", components=[component])
            observation.attach(report, f.layout, node)
            exported = console.project(report, 1000)
            result = exported["observations"]["services"]["usdb_indexer"]["readiness"]
            self.assertEqual(result["blockers"], ["SystemStateMissing"])
            self.assertEqual(result["block_processing_pending_height"], 101)
            self.assertEqual(result["upstream_reorg_epoch"], 0)
            self.assertTrue(result["query_ready"])
            self.assertFalse(result["consensus_ready"])
            self.assertNotIn("PRIVATE", json.dumps(exported))
            self.assertEqual(exported["observations"]["incidents"]["events"], [])

    def test_timeout_is_unknown_and_success_does_not_retain_previous_failure(self):
        failed = node._indexed_service_component("balance_history", {"state": "running"}, None, "PRIVATE timeout", "")
        self.assertEqual(failed["readiness"]["status"], "unavailable")
        self.assertIsNone(failed["readiness"]["consensus_ready"])
        self.assertEqual(failed["readiness"]["failure"]["recovery"], "unknown")
        recovered = observation.readiness({"consensus_ready": True, "blockers": []})
        self.assertIsNone(recovered["rpc_alive"])
        self.assertIsNone(recovered["failure"])

    def test_new_blocker_is_unknown_not_silently_dropped_or_exported(self):
        result = observation.readiness({"consensus_ready": False, "blockers": ["CatchingUp", "PRIVATE", {}]})
        self.assertEqual(result["blockers"], ["CatchingUp", "UnknownBlocker"])
        self.assertNotIn("PRIVATE", json.dumps(result))

    def test_latched_incident_is_stable_read_only_and_sanitized_for_new_and_old_images(self):
        for legacy in (False, True):
            with self.subTest(legacy=legacy), MiningFixture() as f:
                path = self.write_incident(f, legacy=legacy)
                before = path.read_bytes()
                reports = [observation.observe_incidents(f.layout, node) for _ in range(2)]
                self.assertEqual(reports[0], reports[1])
                event = reports[0]["events"][0]
                self.assertEqual(event["code"], "DEEP_REORG_HALTED")
                self.assertEqual(event["severity"], "critical")
                self.assertEqual(event["recovery"], "manual_intervention")
                self.assertEqual(event["observed_epoch"], 3)
                self.assertEqual(event["evidence_status"], "available")
                self.assertTrue(event["latched"])
                if legacy:
                    self.assertRegex(event["event_id"], r"^legacy-sha256:[0-9a-f]{64}$")
                else:
                    self.assertEqual(event["event_id"], "ab" * 16)
                self.assertEqual(path.read_bytes(), before)
                self.assertNotIn("PRIVATE", json.dumps(reports))

    def test_invalid_marker_still_reports_halt_without_inventing_metadata(self):
        values = ["{", "{}", '{"schema_version":1,"schema_version":2}',
                  json.dumps({"schema_version": files.INCIDENT_SCHEMA, "reason": []}),
                  "x" * (files.MAX_INCIDENT_BYTES + 1)]
        for content in values:
            with self.subTest(content=content[:40]), MiningFixture() as f:
                path = self.write_incident(f)
                path.write_text(content)
                event = observation.observe_incidents(f.layout, node)["events"][0]
                self.assertTrue(event["latched"])
                self.assertEqual(event["evidence_status"], "invalid")
                self.assertIsNone(event["detected_at"])
                self.assertEqual(path.read_text(), content)

    def test_nonregular_markers_are_not_opened(self):
        for kind in ("directory", "symlink", "fifo"):
            with self.subTest(kind=kind), MiningFixture() as f:
                path = self.write_incident(f)
                path.unlink()
                if kind == "directory":
                    path.mkdir()
                elif kind == "symlink":
                    target = f.root / "PRIVATE"
                    target.write_text("PRIVATE")
                    path.symlink_to(target)
                else:
                    os.mkfifo(path)
                event = observation.observe_incidents(f.layout, node)["events"][0]
                self.assertEqual(event["evidence_status"], "invalid")
                self.assertIsNone(event["event_id"])

    def test_permission_denied_uses_read_only_probe_and_preserves_event(self):
        with MiningFixture() as f:
            self.write_incident(f)
            expected = observation.observe_incidents(f.layout, node)
            with mock.patch.object(files, "inspect_files", side_effect=PermissionError()), \
                 mock.patch.object(mining.subprocess, "run", return_value=subprocess.CompletedProcess([], 0,
                     json.dumps({"schema_version": files.SCHEMA, "action": "incidents", "result": expected}))) as probe:
                self.assertEqual(observation.observe_incidents(f.layout, node), expected)
            args = probe.call_args.args[0]
            self.assertIn("--read-only", args)
            self.assertIn("--network=none", args)
            self.assertIn("--pull=never", args)
            self.assertEqual(probe.call_args.kwargs["timeout"], 30)

    def test_unreadable_incident_is_unknown_not_resolved(self):
        with MiningFixture() as f, mock.patch.object(mining, "_read_chain_files", side_effect=ValueError("PRIVATE")):
            result = observation.observe_incidents(f.layout, node)
            self.assertEqual(result, {"status": "unavailable", "events": []})

    def test_known_marker_remains_critical_when_its_contents_cannot_be_read(self):
        with MiningFixture() as f:
            self.write_incident(f)
            with mock.patch.object(files.os, "open", side_effect=PermissionError()):
                result = observation.observe_incidents(f.layout, node)
            self.assertEqual(result["status"], "available")
            event = result["events"][0]
            self.assertEqual(event["severity"], "critical")
            self.assertEqual(event["evidence_status"], "unavailable")
            self.assertTrue(event["latched"])
            self.assertIsNone(event["event_id"])

    def test_lifecycle_and_progress_see_marker_even_without_chain_process(self):
        with MiningFixture() as f:
            self.write_incident(f)
            lifecycle = dict(overall_state="READY", checks={}, components=[])
            progress = dict(overall_state="READY", components=[])
            with mock.patch.object(node, "_collect_node_status", return_value=lifecycle), \
                 mock.patch.object(node, "_collect_node_progress", return_value=progress), \
                 mock.patch("node_controller_status.inspect_controller", return_value={"state": "idle", "runtime_state": "inactive"}):
                status = node.collect_node_status(f.layout)
                watched = node.collect_node_progress(f.layout)
            for report in (status, watched):
                self.assertEqual(report["overall_state"], "BLOCKED")
                self.assertEqual(report["observations"]["incidents"]["events"][0]["event_id"], "ab" * 16)
            self.assertIn("DEEP_REORG_HALTED", node.render_node_progress(watched))
            projected = console.project(watched, 1000)
            self.assertEqual(projected["observations"]["incidents"], watched["observations"]["incidents"])

    def test_console_allowlist_drops_injected_fields_and_keeps_chain_head(self):
        raw = {"schema_version": observation.SCHEMA, "observed_at": "2026-09-24T01:02:03Z",
               "password": "PRIVATE", "incidents": {"status": "available", "events": []},
               "services": {"usdb_chain": {"peer_count": 2, "probe_status": "available",
                   "head": {"number": 10, "hash": "0x" + "aa" * 32, "timestamp": 100, "error": "PRIVATE"},
                   "runtime": {"state": "running", "container_id": "bb" * 32, "details_available": True,
                               "restart_count": 3, "oom_killed": True, "environment": "PRIVATE"}}}}
        result = console.project({"observations": raw}, 1000)["observations"]
        self.assertNotIn("PRIVATE", json.dumps(result))
        chain = result["services"]["usdb_chain"]
        self.assertEqual(chain["head"]["number"], 10)
        self.assertEqual(chain["peer_count"], 2)
        self.assertEqual(chain["runtime"]["restart_count"], 3)
        self.assertTrue(chain["runtime"]["oom_killed"])

    def test_unknown_versions_and_absent_runtime_are_not_healthy_evidence(self):
        self.assertEqual(console.project({}, 1000)["observations"]["status"], "not_observed")
        self.assertEqual(observation.project({"schema_version": "future"})["status"], "invalid")
        result = observation.runtime(observation.runtime(None))
        self.assertEqual(result["status"], "not_observed")
        self.assertIsNone(result["oom_killed"])
        self.assertIsNone(result["restart_count"])

    def test_export_does_not_refresh_a_service_probe_timestamp(self):
        ready = observation.readiness({"consensus_ready": False, "blockers": ["CatchingUp"]},
                                      observed_at="2026-09-24T01:02:03Z")
        raw = {"schema_version": observation.SCHEMA, "observed_at": "2026-09-24T02:00:00Z",
               "services": {"usdb_indexer": {"readiness": ready}},
               "incidents": {"status": "available", "events": []}}
        once = observation.project(raw)
        twice = observation.project(once)
        self.assertEqual(once, twice)
        self.assertEqual(twice["services"]["usdb_indexer"]["readiness"]["observed_at"], "2026-09-24T01:02:03+00:00")
        ready["observed_at"] = None
        self.assertIsNone(observation.project(raw)["services"]["usdb_indexer"]["readiness"]["observed_at"])

    def test_malformed_evidence_is_bounded_unknown_instead_of_an_exception(self):
        raw = {"schema_version": observation.SCHEMA, "services": {"usdb_chain": {
            "probe_status": [], "head": {"number": True, "hash": []},
            "runtime": {"state": {}, "health": [], "restart_count": True, "oom_killed": "false"}},
            "usdb_indexer": {"readiness": {"consensus_ready": "true"}}},
            "incidents": {"status": []}}
        result = observation.project(raw)
        self.assertEqual(result["incidents"]["status"], "unavailable")
        chain = result["services"]["usdb_chain"]
        self.assertNotIn("head", chain)
        self.assertEqual(chain["runtime"]["state"], "unknown")
        self.assertIsNone(chain["runtime"]["restart_count"])
        self.assertIsNone(chain["runtime"]["oom_killed"])
        self.assertEqual(result["services"]["usdb_indexer"]["readiness"]["status"], "invalid")

    def test_incident_epoch_and_timestamp_corruption_never_unlatches(self):
        for changes in ({"baseline_epoch": True}, {"observed_epoch": 2}, {"detected_at": "PRIVATE"},
                        {"detected_at": "2026-09-24T01:02:03"}):
            with self.subTest(changes=changes), MiningFixture() as f:
                path = self.write_incident(f)
                value = json.loads(path.read_text())
                value.update(changes)
                path.write_text(json.dumps(value))
                event = observation.observe_incidents(f.layout, node)["events"][0]
                self.assertTrue(event["latched"])
                self.assertEqual(event["evidence_status"], "invalid")
                self.assertNotIn("PRIVATE", json.dumps(event))


if __name__ == "__main__":
    unittest.main()
