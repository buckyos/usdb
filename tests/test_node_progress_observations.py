#!/usr/bin/env python3
"""Exercise displayed node progress without changing bootstrap readiness decisions."""

import copy
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE  # noqa: E402


class NodeProgressObservationTests(unittest.TestCase):
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
        self.assertEqual(controller.call_count, 2)
        status.assert_called_once_with(layout)
        self.assertEqual(result["outcome"], "controller_detached")
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
