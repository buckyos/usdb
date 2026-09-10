"""Exercise Docker timing observations, stale RPCs and the terminal dashboard."""

import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE


class ProgressDisplayTests(unittest.TestCase):
    def test_optional_start_time_probe_is_bounded_and_cannot_block_readiness(self):
        container = {"ID": "a" * 64, "Service": "btc-node", "State": "running", "Health": "healthy"}
        outputs = [subprocess.CompletedProcess([], 0, json.dumps(container)),
                   subprocess.CompletedProcess([], 0, "")]
        with (
            mock.patch.object(NODE, "run_helper", side_effect=outputs),
            mock.patch.object(NODE.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 0, '"2026-09-08T04:00:00.000000000Z"\n')) as inspect,
        ):
            services = NODE._collect_compose_services(mock.Mock(), include_started_at=True, command_timeout_secs=3)
        self.assertEqual(services["btc-node"]["started_at"], "2026-09-08T04:00:00.000000000Z")
        self.assertEqual(inspect.call_args.kwargs["timeout"], 3)
        self.assertIn("{{json .State.StartedAt}}", inspect.call_args.args[0])
        for failure in (subprocess.TimeoutExpired("docker", 3), OSError("unavailable"),
                        subprocess.CalledProcessError(1, "docker")):
            with (
                self.subTest(failure=failure),
                mock.patch.object(NODE, "run_helper", side_effect=outputs),
                mock.patch.object(NODE.subprocess, "run", side_effect=failure),
            ):
                services = NODE._collect_compose_services(mock.Mock(), include_started_at=True)
                self.assertEqual(services["btc-node"]["state"], "running")
                self.assertNotIn("started_at", services["btc-node"])

    def test_rolling_eta_is_visible_on_narrow_terminal_and_stale_rpc_hides_it(self):
        history = NODE.NodeProgressHistory()
        for seconds, current in [(0, 70), (15, 71), (30, 72)]:
            component = NODE._component_progress("bitcoin", "SYNCING", "long detail " * 40,
                                                 current=current, total=100)
            component.update(service_started_at="2026-09-08T04:00:00Z", service_elapsed_secs=7200 + seconds)
            report = {"release_id": "test", "observed_at": "now", "overall_state": "SYNCING", "components": [component]}
            result = history.apply(report, observed_monotonic=seconds)
        rendered = NODE.render_node_progress(result, width=80)
        timing_line = next(line for line in rendered.splitlines() if "Process elapsed=" in line)
        self.assertIn("02:00:30", timing_line)
        self.assertIn("ETA=~00:07:00", timing_line)
        self.assertLessEqual(len(timing_line), 80)
        self.assertEqual(result["overall_state"], report["overall_state"])
        self.assertNotIn("timing", report["components"][0])
        stale = NODE._component_progress("bitcoin", "STARTING", "RPC timeout")
        result = history.apply({**report, "components": [stale]}, observed_monotonic=35)
        self.assertIn("STALE", result["components"][0]["detail"])
        self.assertIsNone(result["components"][0]["timing"]["eta_secs"])
        self.assertIn("ETA=-- (unavailable)", NODE.render_node_progress(result))


if __name__ == "__main__":
    unittest.main()
