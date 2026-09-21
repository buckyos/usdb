"""Exercise Docker timing observations, stale RPCs and the terminal dashboard."""

import copy
import json
import io
from contextlib import redirect_stdout
from types import SimpleNamespace
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE
from node_progress_render import render_node_progress
from common.node_progress import ready_miner_progress


class ProgressDisplayTests(unittest.TestCase):
    def test_ready_miner_with_optional_indexing_has_separate_action_and_work_groups(self):
        report = ready_miner_progress()
        original = copy.deepcopy(report)
        rendered = render_node_progress(report, unicode=True)
        self.assertIn("Node READY | Mining ACTIVE | Resources steady", rendered)
        attention, work, services, preparation = (rendered.index("◆ " + title) for title in
                                                  ("Attention", "Work in progress", "Node services", "Preparation"))
        self.assertTrue(attention < work < services < preparation)
        self.assertIn("Action: usdb-node controller install", rendered[attention:work])
        self.assertIn("Bitcoin txindex", rendered[work:services])
        self.assertIn("19.05%", rendered[work:services])
        self.assertIn("184,401 / 967,943", rendered[work:services])
        self.assertIn("WAITING_TXINDEX", rendered[work:services])
        self.assertNotIn("100.00%", rendered[services:])
        self.assertIn("uptime 00:03:04", rendered[services:preparation])
        self.assertIn("FIRST_NODE: acknowledged first node", rendered[services:preparation])
        self.assertIn("File: download complete", rendered[preparation:])
        self.assertIn("SHA-256: verified", rendered[preparation:])
        self.assertIn("Bitcoin history", rendered[preparation:])
        self.assertIn("Wallet transactions: unavailable", rendered)
        self.assertNotIn("Script registry", rendered)
        self.assertNotIn("Genesis:", rendered)
        self.assertNotIn("Latest block", rendered)
        expanded = render_node_progress(report, details=True)
        self.assertIn("Script registry", expanded)
        self.assertIn("Genesis: " + report["network"]["genesis_hash"], expanded)
        self.assertIn(report["components"][-1]["head"]["hash"], expanded)
        self.assertIn("Existing snapshot baseline reused; no file rescan needed", expanded)
        self.assertEqual(report, original)

    def test_rendering_imports_without_node_runtime_and_performs_no_io(self):
        result = subprocess.run([sys.executable, "-c",
            "import sys; from node_progress_render import render_node_progress; "
            "assert 'usdb_node' not in sys.modules; "
            "sys.addaudithook(lambda event, args: (_ for _ in ()).throw(AssertionError(event)) "
            "if event.startswith(('open', 'socket.', 'subprocess.')) else None); "
            "print(render_node_progress(dict(components=[], overall_state='READY')))"],
            cwd=Path(NODE.__file__).parent, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Node READY", result.stdout)

    def test_narrow_display_keeps_actions_and_never_claims_index_readiness_from_height(self):
        report = ready_miner_progress()
        report["minting"].update(txindex_height=967943, txindex_synced=False)
        action = "usdb-node controller install --sync-timeout-secs 604800 --skip-pull"
        report["controller"]["actions"] = [action]
        for width in (40, 80, 120):
            for unicode in (False, True):
                with self.subTest(width=width, unicode=unicode):
                    rendered = render_node_progress(report, width=width, unicode=unicode)
                    self.assertTrue(all(len(line) <= width for line in rendered.splitlines()))
                    compact = " ".join(rendered.split())
                    self.assertIn(action, compact)
                    self.assertIn("Bitcoin txindex INDEXING", compact)
                    self.assertIn("Waiting for Core to report txindex synced=true", compact)
                    self.assertIn("ETA=-- (unavailable)", compact)
                    if not unicode:
                        self.assertTrue(rendered.isascii())
                        self.assertNotIn("\x1b", rendered)
        report["minting"].update(state="UNAVAILABLE", txindex_height=None, core_height=None)
        rendered = render_node_progress(report)
        self.assertNotIn("100.00%", rendered)
        self.assertIn("[!] Bitcoin txindex", " ".join(rendered.split()))

    def test_optional_failure_and_resource_errors_are_visible_without_changing_core_state(self):
        report = ready_miner_progress()
        report["resources"]["error"] = "Runtime memory exceeds the configured budget"
        report["minting"].update(state="BLOCKED_DISK", disk_free_bytes=10 * 1024**3,
                                disk_required_bytes=50 * 1024**3, guidance="Add capacity; retain existing indexes")
        rendered = render_node_progress(report)
        attention = rendered.split("== Attention")[1].split("== Work in progress")[0]
        self.assertIn("Runtime memory exceeds the configured budget", attention)
        self.assertIn("Add capacity; retain existing indexes", attention)
        self.assertIn("free 10.0GiB | reserve 50.0GiB", attention)
        self.assertIn("Node READY", rendered)
        self.assertEqual(report["overall_state"], "READY")

    def test_terminal_symbols_require_both_tty_and_unicode_encoding(self):
        with mock.patch.dict(NODE.os.environ, {"TERM": "xterm"}):
            for tty, encoding, supported in ((True, "utf-8", True), (True, "ascii", False),
                                              (False, "utf-8", False), (True, None, False)):
                stream = mock.Mock(encoding=encoding, isatty=mock.Mock(return_value=tty))
                self.assertEqual(NODE._progress_unicode_supported(stream), supported)

    def test_details_cli_and_json_keep_observations_unchanged(self):
        report = ready_miner_progress()
        for flags in (("--details",), ("--watch", "--details")):
            args = NODE.build_parser().parse_args(["status", *flags])
            with mock.patch.object(NODE, "print_progress_status", return_value=0) as progress:
                NODE._execute_command(object(), args)
            self.assertTrue(progress.call_args.kwargs["details"])
        for flag in ("--json", "--progress-json"):
            args = NODE.build_parser().parse_args(["status", flag, "--details"])
            with self.assertRaisesRegex(ValueError, "text display option"):
                NODE._execute_command(object(), args)
        output = io.StringIO()
        with mock.patch.object(NODE, "collect_node_progress", return_value=report), redirect_stdout(output):
            self.assertEqual(NODE.print_progress_status(object(), json_output=True, watch=False, refresh_secs=5), 0)
        self.assertEqual(json.loads(output.getvalue()), report)
        self.assertNotIn("\x1b", output.getvalue())

    def test_watch_closes_terminal_on_unexpected_observation_failure(self):
        with mock.patch.object(NODE, "collect_node_progress", side_effect=ValueError("test failure")), \
             mock.patch.object(NODE, "TerminalProgressDisplay") as display:
            with self.assertRaisesRegex(ValueError, "test failure"):
                NODE.print_progress_status(object(), json_output=False, watch=True, refresh_secs=5)
        display.return_value.close.assert_called_once()

    def test_network_identity_is_visible_without_chain_rpc(self):
        layout = SimpleNamespace(bundle_id="usdb-testnet-v0", network_identity={
            "chain_id": 202608250, "network_id": 202608250,
            "genesis_block_hash": "0x" + "ab" * 32, "btc_network_id": "btc-mainnet"})
        identity = NODE._status_network_identity(layout)
        report = dict(release_id="test", observed_at="now", overall_state="STARTING", components=[],
                      network=identity, node_role="full", checks={}, next_actions=[], operator_guidance=[],
                      up={"mode": "observe", "summary": "waiting"})
        output = io.StringIO()
        with redirect_stdout(output):
            NODE._print_node_status_report(report)
        for rendered in (output.getvalue(), NODE.render_node_progress(report, width=80, details=True)):
            self.assertIn("Network: usdb-testnet-v0 | Chain ID: 202608250 | Role: full", rendered)
            self.assertIn("Genesis: 0x" + "ab" * 32, rendered)
            self.assertIn("P2P network ID: 202608250 | Bitcoin source: btc-mainnet", rendered)
        self.assertEqual(identity["source"], "release_bundle")

    def test_up_watch_continues_through_ready_and_failure_until_detached(self):
        observations = [dict(release_id="test", observed_at="now", components=[], overall_state=state)
                        for state in ("SYNCING", "READY", "READY", "FAILED")]
        with mock.patch.object(NODE, "collect_node_progress", side_effect=[*observations, KeyboardInterrupt]), \
             mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "DEGRADED"}), \
             mock.patch.object(NODE, "TerminalProgressDisplay") as display, \
             mock.patch.object(NODE.time, "sleep"), \
             mock.patch.object(NODE, "stop_controller_unit") as stop, \
             mock.patch.object(NODE, "start_controller_unit") as start:
            result, code = NODE.follow_submitted_controller(object(), {})
        self.assertEqual(code, 0)
        self.assertEqual(result["outcome"], "controller_detached")
        self.assertEqual(display.return_value.render.call_count, 4)
        display.return_value.close.assert_called_once()
        start.assert_not_called()
        stop.assert_not_called()

    def test_genesis_milestone_renders_without_optional_range_metadata(self):
        component = NODE._component_progress("balance_history", "STARTING", "RPC unavailable")
        component["genesis_milestone"] = dict(height=963800, state="last observed available; RPC unavailable")
        report = dict(release_id="test", observed_at="now", overall_state="STARTING", components=[component])
        for fields in ({}, {"sync_start_height": 935000}, {"stable_lag_blocks": 10}):
            with self.subTest(fields=fields):
                rendered = NODE.render_node_progress({**report, "components": [{**component, **fields}]}, width=80)
                self.assertIn("Genesis 963800: last observed available; RPC unavailable", " ".join(rendered.split()))
                self.assertEqual("Blocks from" in rendered, "sync_start_height" in fields)
                self.assertEqual("confirmation blocks" in rendered, "stable_lag_blocks" in fields)

    def test_unknown_service_phase_keeps_indeterminate_progress_renderable(self):
        component = NODE._component_progress("balance_history", "IMPORTING", "waiting for progress")
        component["progress_phase"] = None
        report = dict(release_id="test", observed_at="now", overall_state="IMPORTING", components=[component])
        rendered = NODE.render_node_progress(report, width=80)
        self.assertIn("[RUN] Balance history IMPORTING", " ".join(rendered.split()))
        self.assertNotIn("[---", rendered)
        self.assertNotIn("0.00%", rendered)
        self.assertNotIn("Stage elapsed=", rendered)

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
