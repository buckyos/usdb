#!/usr/bin/env python3
"""Regression tests for independent canonical replay and its weekly completion gate."""

import copy
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

SCRIPTS = Path(__file__).resolve().parents[1] / "src/btc/usdb-indexer/scripts"
sys.path.insert(0, str(SCRIPTS))
from common.world_soak_fixture import add_replay_fixture, completed_soak_fixture
from compare_world_replay import (
    load_session, prepare_configs, ReplayRPC, ReplayRPCError, stop_services, wait_ready, wait_historical_refs,
)
from regtest_world_simulator import RegtestWorldSimulator
from world_replay_state import collect_pages, first_difference
from world_soak_coverage import check_world_replay_coverage


class PaginationTests(unittest.TestCase):
    def test_reads_all_offset_pages(self):
        calls = []

        def rpc(method, params):
            calls.append(params[0]["page"])
            return {"total": 3, "resolved_height": 20, "items": [{"id": calls[-1]}]}

        result = collect_pages(rpc, "passes", {"at_height": 20}, "id")
        self.assertEqual(calls, [0, 1, 2])
        self.assertEqual([row["id"] for row in result["items"]], [0, 1, 2])

    def test_cursor_tokens_belong_to_each_instance(self):
        def collect(token):
            pages = iter([
                {"total": 2, "items": [{"id": "a"}], "next_cursor": token},
                {"total": 2, "items": [{"id": "b"}], "next_cursor": None},
            ])
            return collect_pages(lambda *_: next(pages), "candidates", {}, "id", cursor=True)

        self.assertEqual(collect("rollback-cursor"), collect("fresh-cursor"))

    def test_rejects_truncation_duplicates_and_metadata_drift(self):
        for second in (
            {"total": 2, "items": []},
            {"total": 2, "items": [{"id": "a"}]},
            {"total": 3, "items": [{"id": "b"}]},
        ):
            pages = iter([{"total": 2, "items": [{"id": "a"}]}, second])
            with self.subTest(second=second), self.assertRaises(ValueError):
                collect_pages(lambda *_: next(pages), "passes", {}, "id")

    def test_incomplete_final_cursor_page_fails(self):
        with self.assertRaisesRegex(ValueError, "incomplete"):
            collect_pages(lambda *_: {"total": 2, "items": [{"id": "a"}], "next_cursor": None},
                          "candidates", {}, "id", cursor=True)


class SemanticComparisonTests(unittest.TestCase):
    def test_detects_materialized_state_and_ledger_corruption(self):
        expected = {
            "pass": {"owner": "alice", "state": "active", "satpoint": "tx:0:0", "prev": ["old"],
                     "raw_energy": "123", "leader_pass_id": "leader"},
            "history": [{"height": 7, "owner": "alice"}],
            "balance": [{"height": 7, "balance": 123}],
            "candidates": ["a", "b"], "breakdown": [{"collab": "c", "contribution": "50"}],
        }
        self.assertIsNone(first_difference(expected, copy.deepcopy(expected)))
        for field in expected["pass"]:
            actual = copy.deepcopy(expected)
            actual["pass"][field] = None
            with self.subTest(field=field):
                self.assertEqual(first_difference(expected, actual)["path"], f"$.pass.{field}")
        for field in ("history", "balance", "candidates", "breakdown"):
            actual = copy.deepcopy(expected)
            actual[field].pop()
            with self.subTest(field=field):
                self.assertEqual(first_difference(expected, actual)["path"], f"$.{field}")
        actual = copy.deepcopy(expected)
        actual["candidates"].reverse()
        self.assertIsNotNone(first_difference(expected, actual))


class ReplayGateTests(unittest.TestCase):
    def setUp(self):
        self.start, self.end = completed_soak_fixture()
        self.replay = add_replay_fixture(self.start, self.end)

    def test_all_reorgs_and_final_state_must_match(self):
        check_world_replay_coverage(self.start, self.end, self.replay)
        for field, value in (
            ("status", "failed"), ("session_start_ts_ms", 99), ("fresh_databases", False),
            ("final_hash", "orphaned"), ("comparisons", self.replay["comparisons"][:-1]),
        ):
            broken = {**self.replay, field: value}
            with self.subTest(field=field), self.assertRaises(ValueError):
                check_world_replay_coverage(self.start, self.end, broken)
        self.replay["comparisons"][0]["actual_sha256"] = "corrupt"
        with self.assertRaises(ValueError):
            check_world_replay_coverage(self.start, self.end, self.replay)

    def test_missing_reorg_manifest_fails_even_when_count_is_positive(self):
        self.end["replay_checkpoints"].pop(0)
        with self.assertRaises(ValueError):
            check_world_replay_coverage(self.start, self.end, self.replay)

    def test_actual_summary_rejects_missing_and_stale_replay(self):
        script = (SCRIPTS / "run_regtest_world_soak_matrix.sh").read_text()
        program = re.search(r'python3 - "\$seed".*?<<\'PY\'\n(.*?)\nPY', script, re.DOTALL).group(1)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for replay, passes in ((None, False), (self.replay, True),
                                   ({**self.replay, "session_start_ts_ms": 999}, False)):
                records = [self.start, self.end] + ([replay] if replay else [])
                report = root / "report.jsonl"
                report.write_text("".join(json.dumps(item) + "\n" for item in records))
                result = subprocess.run([
                    sys.executable, "-", "43", "2500", "1", "1", "0", str(report),
                    str(root / "summary.json"), str(SCRIPTS),
                ], input=program, text=True, capture_output=True)
                with self.subTest(replay=replay is not None):
                    self.assertEqual(result.returncode == 0, passes, result.stderr)

    def test_incomplete_latest_session_cannot_reuse_prior_success(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.jsonl"
            records = (self.start, self.end, {**self.start, "ts_ms": 999})
            report.write_text("".join(json.dumps(item) + "\n" for item in records))
            with self.assertRaisesRegex(ValueError, "no completed"):
                load_session(report)


class ReplayIsolationTests(unittest.TestCase):
    def test_copies_only_configuration_into_new_roots(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            balance, usdb, scratch = (root / name for name in ("source-balance", "source-usdb", "scratch"))
            for path in (balance, usdb, scratch):
                path.mkdir()
                (path / "state-marker").write_text("original state")
            (balance / "config.toml").write_text('root_dir = "old"\n[rpc_server]\nport = 1234\n')
            config = {"bitcoin": {"network": "regtest"}, "balance_history": {"rpc_url": "old"},
                      "usdb": {"genesis_block_height": 1, "inscription_source": "bitcoind"}}
            (usdb / "config.json").write_text(json.dumps(config))
            new_balance, new_usdb = prepare_configs(balance, usdb, scratch, 4001, 4002)
            self.assertEqual([p.name for p in new_balance.iterdir()], ["config.toml"])
            self.assertEqual([p.name for p in new_usdb.iterdir()], ["config.json"])
            updated = json.loads((new_usdb / "config.json").read_text())
            self.assertEqual(updated["balance_history"]["rpc_url"], "http://127.0.0.1:4001")
            self.assertEqual(updated["usdb"]["rpc_server_port"], 4002)
            self.assertEqual(json.loads((usdb / "config.json").read_text()), config)
            with self.assertRaises(FileExistsError):
                prepare_configs(balance, usdb, scratch, 4001, 4002)

    def test_stops_only_owned_process_groups(self):
        processes = [mock.Mock(pid=101), mock.Mock(pid=102)]
        with mock.patch("compare_world_replay.os.killpg") as kill:
            stop_services(processes)
        self.assertEqual([call.args[0] for call in kill.call_args_list], [102, 101])
        for process in processes:
            process.wait.assert_called_once()

    def test_dead_service_fails_without_waiting_for_the_full_budget(self):
        import time
        rpc = ReplayRPC("http://unused", time.monotonic() + 60, [mock.Mock(poll=lambda: 1, args=["service"])])
        with self.assertRaisesRegex(RuntimeError, "service exited"):
            wait_ready(rpc, rpc, 20, "hash", time.monotonic() + 60)

    def test_same_height_wrong_hash_cannot_pass_readiness(self):
        def rpc(method, params):
            if method == "get_readiness":
                return {"consensus_ready": True}
            return {"stable_height": 20, "stable_block_hash": "orphan"}

        with mock.patch("compare_world_replay.time.monotonic", side_effect=[0, 0, 2]), \
             mock.patch("compare_world_replay.time.sleep"), self.assertRaisesRegex(ValueError, "timeout"):
            wait_ready(rpc, rpc, 20, "canonical", 1)

    def test_head_readiness_waits_for_historical_backfill(self):
        calls = []

        def rpc(method, params):
            height = params[0]["block_height"]
            calls.append(height)
            if len(calls) == 1:
                raise ReplayRPCError(method, {"code": -32049, "message": "HISTORY_NOT_AVAILABLE"})
            return {"block_height": height, "snapshot_info": {"stable_block_hash": "hash"}}

        with mock.patch("compare_world_replay.time.sleep") as sleep:
            wait_historical_refs(rpc, [{"height": 10, "block_hash": "hash"}], 20, time.monotonic() + 10)
        self.assertEqual(calls, [10, 10, 19])
        sleep.assert_called_once()

    def test_historical_wait_does_not_retry_mismatch_or_hide_timeout(self):
        for code, message in ((-32049, "HISTORY_NOT_AVAILABLE"), (-32047, "SNAPSHOT_ID_MISMATCH")):
            rpc = mock.Mock(side_effect=ReplayRPCError("state", {"code": code, "message": message}))
            with self.subTest(message=message), mock.patch("compare_world_replay.time.sleep") as sleep, \
                 self.assertRaises(ValueError):
                wait_historical_refs(rpc, [{"height": 10, "block_hash": "hash"}], 20, 0)
            sleep.assert_not_called()

    def test_capture_uses_preexisting_state_only_and_recovery_preserves_manifest(self):
        sim = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        sim.active_agent_count, sim.reorg_events_applied = 0, 2
        sim.metrics = sim.pass_owner_by_id = sim.pass_identity_by_id = {}
        sim.agents = sim.validator_samples = []
        sim.replay_checkpoints = [{"kind": "reorg", "file": "saved.json"}]
        sim.action_seed = 43
        sim.args = SimpleNamespace(blocks=2500, stable_lag_blocks=10)
        saved = sim.build_between_ticks_snapshot(batch_seed=43, next_tick=1001, current_height=11000)
        self.assertEqual(saved["replay_checkpoints"], sim.replay_checkpoints)
        sim.replay_checkpoints = []
        sim.apply_recovery_snapshot(saved)
        self.assertEqual(sim.replay_checkpoints, saved["replay_checkpoints"])

    def test_completed_work_keeps_recovery_until_external_replay_succeeds(self):
        sim = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        sim.args = mock.Mock(blocks=80, replay_check_enabled=True)
        sim.action_seed = 43
        sim.total_agents = sim.active_agent_count = 1
        sim.report_path = sim.recovery_state_path = None
        sim.resume_state = {"status": "between_ticks", "next_tick": 81, "batch_seed": 43}
        sim.apply_recovery_snapshot = sim.log = mock.Mock()
        sim.metrics, sim.reorg_events_applied, sim.replay_checkpoints = {}, 4, []
        sim.finalize_validator_samples = mock.Mock(return_value={"final_height": 1100})
        sim.capture_replay_checkpoint = sim.write_recovery_state = mock.Mock()
        sim.build_between_ticks_snapshot = mock.Mock(return_value={"next_tick": 81})
        sim.validator_sample_summary = mock.Mock(return_value={"pending": 0})
        sim.clear_recovery_state = sim.execute_agent_action = mock.Mock()
        sim.emit_report = mock.Mock()
        sim.run()
        sim.execute_agent_action.assert_not_called()
        sim.clear_recovery_state.assert_not_called()
        sim.build_between_ticks_snapshot.assert_called_once_with(batch_seed=43, next_tick=81, current_height=1100)
        self.assertEqual(sim.emit_report.call_args.args[1]["completed_work_ticks"], 80)


if __name__ == "__main__":
    unittest.main()
