#!/usr/bin/env python3
"""Exercise displayed node progress without changing bootstrap readiness decisions."""

import copy
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE  # noqa: E402
import assumeutxo_node as NATIVE  # noqa: E402


class NodeProgressObservationTests(unittest.TestCase):
    def test_native_startup_gap_retains_renderable_range_without_claiming_readiness(self):
        ready = NATIVE._balance_history_progress(
            NODE._component_progress("balance_history", "READY", "ready"),
            dict(phase="Synced", stable_height=966992, query_ready=True), {},
            dict(headers=967002), {}, base=935000, origin=963800, stable_lag=10,
            max_height=966992,
        )
        # A recreated container has no native range metadata until it is running.
        starting = NODE._indexed_service_component(
            "balance_history", dict(state="created"), None, "RPC unavailable", "waiting",
        )
        report = dict(release_id="r25", observed_at="now", overall_state="READY", components=[ready])
        pending = {**report, "overall_state": "STARTING", "components": [starting]}
        original = copy.deepcopy(pending)
        history = NODE.NodeProgressHistory(max_stale_age_secs=60)
        history.apply(report, observed_monotonic=0)
        observed = history.apply(pending, observed_monotonic=5)
        rendered = NODE.render_node_progress(observed, width=80)
        self.assertIn("Blocks from 935000 | Genesis 963800: last observed available; RPC unavailable", " ".join(rendered.split()))
        self.assertIn("Target (last observed): Bitcoin headers minus 10 confirmation blocks", " ".join(rendered.split()))
        self.assertIn("Configured maximum target: 966992", rendered)
        self.assertEqual(observed["components"][0]["state"], "STARTING")
        self.assertEqual(observed["overall_state"], "STARTING")
        self.assertEqual(pending, original)
        expired = history.apply(pending, observed_monotonic=61)
        self.assertNotIn("Genesis", NODE.render_node_progress(expired))
        self.assertIsNone(expired["components"][0]["current"])
        recovered = history.apply(report, observed_monotonic=62)
        self.assertIn("Genesis 963800: available", NODE.render_node_progress(recovered, details=True))
        self.assertNotIn("STALE", NODE.render_node_progress(recovered))

    def test_indexer_target_survives_only_as_labeled_stale_evidence_after_rpc_loss(self):
        ready = NODE._indexed_service_component("usdb_indexer", dict(state="running"),
            dict(consensus_ready=True, synced_block_height=967943, balance_history_stable_height=967943), None, "waiting")
        report = dict(release_id="test", observed_at="now", components=[ready])
        history = NODE.NodeProgressHistory(max_stale_age_secs=60)
        history.apply(report, observed_monotonic=0)
        missing = NODE._indexed_service_component("usdb_indexer", dict(state="running"), None, "RPC unavailable", "waiting")
        pending = {**report, "components": [missing]}
        observed = history.apply(pending, observed_monotonic=5)
        rendered = " ".join(NODE.render_node_progress(observed).split())
        self.assertIn("Target (last observed): balance-history available stable height = 967943", rendered)
        self.assertEqual(observed["components"][0]["state"], "STARTING")
        expired = history.apply(pending, observed_monotonic=61)
        self.assertNotIn("available stable height", NODE.render_node_progress(expired))

    def test_balance_history_fallback_target_is_not_labeled_live_bitcoin_headers(self):
        for core, activation, fields, expected in (
            ({}, {}, dict(total=967943), "Target: balance-history reported sync target = 967943"),
            ({}, dict(details=dict(report=dict(headers=967961))), {},
             "Target: last observed Bitcoin headers minus 10 confirmation blocks = 967951"),
            ({}, {}, {}, "Target: unavailable"),
        ):
            with self.subTest(expected=expected):
                item = NATIVE._balance_history_progress(
                    NODE._component_progress("balance_history", "READY", "ready"),
                    dict(phase="Indexing", stable_height=967943, query_ready=True, **fields), {}, core, activation,
                    base=935000, origin=963800, stable_lag=10)
                rendered = " ".join(NODE.render_node_progress(dict(components=[item])).split())
                self.assertIn(expected, rendered)

    def test_indexer_waits_for_upstream_without_claiming_zero_block_sync(self):
        readiness = dict(consensus_ready=False, current=0, total=0, synced_block_height=None,
                         blockers=["SyncedHeightMissing", "UpstreamReadinessUnknown", "UpstreamSnapshotMissing"])
        component = NODE._indexed_service_component("usdb_indexer", {"state": "running"}, readiness, None, "waiting")
        self.assertEqual(component["state"], "WAITING")
        self.assertIsNone(component["current"])
        self.assertIsNone(component["total"])
        self.assertIn("indexing has not started", component["detail"])
        report = dict(release_id="test", observed_at="now", overall_state="WAITING", components=[component])
        rendered = NODE.render_node_progress(report)
        self.assertNotIn("0/0", rendered)
        self.assertIn("[WAIT] USDB indexer", rendered)
        self.assertNotIn("[RUN]", rendered)
        # Real block processing or durable progress must not be hidden by this classification.
        for fields in (dict(block_processing_pending_height=963800), dict(current=963810, total=963900, synced_block_height=963809)):
            with self.subTest(fields=fields):
                active = NODE._indexed_service_component("usdb_indexer", {"state": "running"}, {**readiness, **fields}, None, "waiting")
                self.assertEqual(active["state"], "SYNCING")

    def test_snapshot_rpc_outage_retains_only_recent_display_evidence(self):
        fresh = NODE._component_progress("snapshot", "READY", "Snapshot baseline and raw file ready", progress_percent=100)
        fresh["observation_identity"] = ["r25", "core-start", 1000]
        unavailable = NODE._component_progress("snapshot", "STARTING", "Core RPC unavailable")
        unavailable.update(observation_unavailable=True, display_state="UNAVAILABLE", observation_identity=fresh["observation_identity"])
        report = dict(release_id="r25", observed_at="now", overall_state="STARTING", components=[fresh])
        history = NODE.NodeProgressHistory(max_stale_age_secs=60)
        history.apply(report, observed_monotonic=0)
        observed = history.apply({**report, "components": [unavailable]}, observed_monotonic=5)
        component = observed["components"][0]
        self.assertEqual(component["progress_percent"], 100)
        self.assertEqual(component["state"], "STARTING")
        self.assertEqual(observed["overall_state"], "STARTING")
        self.assertEqual(component["display_state"], "STALE")
        self.assertEqual(component["last_observed_at"], "now")
        self.assertIn("STALE", NODE.render_node_progress(observed))
        self.assertNotIn("UNKNOWN", NODE.render_node_progress(observed))
        self.assertIsNone(unavailable["progress_percent"])
        expired = history.apply({**report, "components": [unavailable]}, observed_monotonic=61)
        self.assertIsNone(expired["components"][0]["progress_percent"])
        self.assertIn("unavailable", NODE.render_node_progress(expired))

    def test_native_bitcoin_outage_retains_foreground_and_background_from_one_probe(self):
        fresh = NODE._component_progress("bitcoin", "READY", "foreground=967114; background=844060",
                                         current=967114, total=967114)
        fresh.update(observation_identity=["r27", "/bitcoin", "core-start"],
                     background_validation=dict(height=844060, target=935000, validated=False, available=True))
        unavailable = NODE._component_progress("bitcoin", "STARTING", "Bitcoin getchainstates RPC timeout")
        unavailable.update(observation_identity=fresh["observation_identity"], observation_unavailable=True,
                           display_state="UNAVAILABLE",
                           background_validation=dict(height=None, target=935000, validated=False, available=False))
        report = dict(release_id="r27", observed_at="2026-09-15T11:06:13Z", overall_state="READY", components=[fresh])
        pending = {**report, "observed_at": "2026-09-15T11:06:47Z", "overall_state": "STARTING",
                   "controller_state": "failed", "components": [unavailable]}
        original = copy.deepcopy(pending)
        history = NODE.NodeProgressHistory(max_stale_age_secs=60)
        history.apply(report, observed_monotonic=0)
        for elapsed in (34, 50):
            observed = history.apply(pending, observed_monotonic=elapsed)
            component = observed["components"][0]
            self.assertEqual(component["display_state"], "STALE")
            self.assertEqual(component["state"], "STARTING")
            self.assertEqual(component["current"], 967114)
            self.assertEqual(component["last_observed_state"], "READY")
            self.assertEqual(component["stale_age_secs"], elapsed)
            self.assertEqual(component["background_validation"]["height"], 844060)
            self.assertFalse(component["background_validation"]["available"])
            self.assertEqual(observed["overall_state"], "STARTING")
            self.assertIsNone(component["timing"]["eta_secs"])
            rendered = NODE.render_node_progress(observed, width=80)
            self.assertIn("Core background history: STALE 844060/935000", rendered)
            self.assertIn("2026-09-15T11:06:13Z", rendered)
            self.assertIn("Latest probe: Bitcoin getchainstates RPC timeout", rendered)
            self.assertIn("controller=failed", rendered)
            self.assertNotIn("Core background history: UNAVAILABLE", rendered)
        self.assertEqual(pending, original)
        expired = history.apply(pending, observed_monotonic=61)
        self.assertIsNone(expired["components"][0]["current"])
        self.assertIsNone(expired["components"][0]["background_validation"]["height"])
        self.assertIn("Core background history: UNAVAILABLE", NODE.render_node_progress(expired))
        recovered = history.apply(report, observed_monotonic=62)
        self.assertNotIn("STALE", NODE.render_node_progress(recovered))
        self.assertIn("Core background history: SYNCING 844060/935000", NODE.render_node_progress(recovered))

    def test_bitcoin_restart_or_failure_discards_previous_chainstate_progress(self):
        fresh = NODE._component_progress("bitcoin", "SYNCING", "syncing", current=940000, total=950000)
        fresh.update(observation_identity=["r27", "core-start"],
                     background_validation=dict(height=None, target=935000, validated=True, available=True))
        unavailable = NODE._component_progress("bitcoin", "STARTING", "Core RPC unavailable")
        unavailable.update(observation_unavailable=True, display_state="UNAVAILABLE",
                           observation_identity=fresh["observation_identity"],
                           background_validation=dict(height=None, target=935000, validated=False, available=False))
        report = dict(release_id="r27", observed_at="now", overall_state="STARTING", components=[fresh])
        for change in (dict(state="FAILED"), dict(state="BLOCKED"),
                       dict(observation_identity=["r27", "restarted"]), dict(observation_identity=["r28", "core-start"])):
            with self.subTest(change=change):
                history = NODE.NodeProgressHistory()
                history.apply(report, observed_monotonic=0)
                stale = history.apply({**report, "components": [unavailable]}, observed_monotonic=1)
                self.assertIn("STALE: last validated through baseline 935000", NODE.render_node_progress(stale))
                history.apply({**report, "components": [{**unavailable, **change}]}, observed_monotonic=2)
                after = history.apply({**report, "components": [unavailable]}, observed_monotonic=3)
                self.assertIsNone(after["components"][0]["current"])
                self.assertNotIn("STALE", NODE.render_node_progress(after))

    def test_snapshot_restart_failure_or_new_import_clears_cached_completion(self):
        fresh = NODE._component_progress("snapshot", "READY", "ready", progress_percent=100)
        fresh["observation_identity"] = ["r25", "core-start", 1000]
        unavailable = NODE._component_progress("snapshot", "STARTING", "Core RPC unavailable")
        unavailable.update(observation_unavailable=True, observation_identity=fresh["observation_identity"])
        report = dict(release_id="r25", observed_at="now", overall_state="STARTING", components=[fresh])
        for change in (dict(state="FAILED"), dict(state="IMPORTING"), dict(observation_identity=["r25", "restarted", 1000])):
            with self.subTest(change=change):
                history = NODE.NodeProgressHistory()
                history.apply(report, observed_monotonic=0)
                history.apply({**report, "components": [{**unavailable, **change, "observation_unavailable": change.get("state") is None}]}, observed_monotonic=1)
                observed = history.apply({**report, "components": [unavailable]}, observed_monotonic=2)
                self.assertIsNone(observed["components"][0]["progress_percent"])

    def test_indexer_uses_durable_height_and_current_upstream_target(self):
        readiness = {
            "consensus_ready": False,
            "current": 965751, "total": 965751,
            "synced_block_height": 965751, "balance_history_stable_height": 965754,
            "message": "Syncing block 965751", "blockers": ["CatchingUp"],
        }
        original = copy.deepcopy(readiness)
        component = NODE._indexed_service_component(
            "usdb_indexer", {"state": "running"}, readiness, None, "waiting",
        )
        self.assertEqual((component["current"], component["total"]), (965751, 965754))
        self.assertLess(component["progress_percent"], 100)
        self.assertIn("remaining_blocks=3", component["detail"])
        self.assertEqual(component["state"], "SYNCING")
        self.assertEqual(readiness, original)

    def test_indexer_target_falls_back_when_durable_fields_are_invalid(self):
        for invalid in (None, True, -1, "965754"):
            with self.subTest(invalid=invalid):
                component = NODE._indexed_service_component(
                    "usdb_indexer", {"state": "running"}, {
                        "consensus_ready": False, "current": 10, "total": 20,
                        "synced_block_height": 10, "balance_history_stable_height": invalid,
                    }, None, "waiting",
                )
                self.assertEqual((component["current"], component["total"]), (10, 20))

    def test_controller_follow_retains_display_values_without_reusing_readiness(self):
        ready_component = NODE._component_progress(
            "usdb_indexer", "READY", "ready", current=965751, total=965751,
        )
        timeout_component = NODE._component_progress(
            "usdb_indexer", "STARTING", "readiness check failed: timed out",
        )
        fresh = {"observed_at": "2026-09-06T12:00:00Z", "overall_state": "SYNCING",
                 "components": [ready_component]}
        timeout = {"observed_at": "2026-09-06T12:00:05Z", "overall_state": "STARTING",
                   "components": [timeout_component]}
        layout = object()
        with (
            mock.patch.object(NODE, "collect_node_progress", side_effect=[fresh, timeout, KeyboardInterrupt]),
            mock.patch.object(NODE, "TerminalProgressDisplay"),
            mock.patch.object(NODE, "render_node_progress", return_value="progress") as render,
            mock.patch.object(NODE, "controller_active_state", return_value="active") as controller,
            mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "STARTING"}) as status,
            mock.patch.object(NODE.time, "sleep"),
        ):
            result, code = NODE.follow_submitted_controller(layout, {})
        displayed = render.call_args_list[1].args[0]["components"][0]
        self.assertEqual(displayed["current"], 965751)
        self.assertEqual(displayed["state"], "STARTING")
        self.assertIn("STALE", displayed["detail"])
        self.assertIn("timed out", displayed["detail"])
        self.assertIsNone(timeout_component["current"])
        controller.assert_not_called()
        status.assert_called_once_with(layout)
        self.assertEqual(result["outcome"], "controller_detached")
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
