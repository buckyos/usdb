#!/usr/bin/env python3

import json
import random
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path
from types import SimpleNamespace

from regtest_world_simulator import (
    ActionExpectation,
    Agent,
    PassIdentity,
    PlannedAction,
    RegtestWorldSimulator,
    ValidatorSampleCandidate,
    WorldSimError,
)

REPO_ROOT = Path(__file__).resolve().parents[4]


class RegtestWorldSimulatorFormulaTests(unittest.TestCase):
    def test_collab_contribution_matches_floor_formula_at_boundaries(self) -> None:
        calc = RegtestWorldSimulator.calc_collab_contribution
        self.assertEqual(calc(0), 0)
        self.assertEqual(calc(1), 0)
        self.assertEqual(calc(2), 1)
        self.assertEqual(calc(101), 50)

        maximum = RegtestWorldSimulator.ENERGY_MAX
        expected = (
            maximum // RegtestWorldSimulator.BPS_DENOMINATOR
        ) * RegtestWorldSimulator.COLLAB_WEIGHT_BPS + (
            maximum % RegtestWorldSimulator.BPS_DENOMINATOR
        ) * RegtestWorldSimulator.COLLAB_WEIGHT_BPS // RegtestWorldSimulator.BPS_DENOMINATOR
        self.assertEqual(calc(maximum), expected)
        self.assertEqual(calc(maximum + 1), expected)

    def test_candidate_winner_uses_effective_energy_then_pass_id(self) -> None:
        def candidate(pass_id: str, effective_energy: int) -> ValidatorSampleCandidate:
            return ValidatorSampleCandidate(
                inscription_id=pass_id,
                owner="owner",
                state="active",
                pass_kind="standard",
                raw_energy=effective_energy,
                collab_contribution=0,
                effective_energy=effective_energy,
                level=0,
                difficulty_factor_bps=10_000,
            )

        winner = RegtestWorldSimulator.choose_candidate_set_winner(
            [candidate("b" * 64 + "i0", 10), candidate("a" * 64 + "i0", 10)]
        )
        self.assertEqual(winner.inscription_id, "a" * 64 + "i0")


class RegtestWorldSimulatorEnergyIntervalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.simulator = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.simulator.metrics = dict.fromkeys([
            "agent_energy_check_ok", "agent_energy_check_balance_events",
            "agent_energy_check_baseline", "agent_energy_check_skipped_no_active",
            "agent_energy_check_skipped_same_height",
        ], 0)
        self.agent = Agent(0, "alice", "btc-address", "usdb-address", "owner", "steady")
        self.agent.active_pass_id = "pass-a"
        self.snapshots = {}
        self.rows = []
        self.point_balance = 200_000
        self.simulator.get_pass_energy_snapshot = mock.Mock(
            side_effect=lambda pass_id, height, mode: self.snapshots[height]
        )
        self.simulator.get_balance_at_height = mock.Mock(
            side_effect=lambda owner, height: self.point_balance
        )
        self.simulator.rpc_balance_history = mock.Mock(side_effect=lambda *args: self.rows)
        self.check(100, 1000)

    def snapshot(self, height, energy, balance=200_000, age=80):
        return {
            "query_block_height": height, "record_block_height": height,
            "state": "active", "owner_address": "owner",
            "owner_balance": balance, "active_block_height": age,
            "raw_energy": str(energy),
        }

    def check(self, height, energy, balance=200_000, age=80):
        self.snapshots[height] = self.snapshot(height, energy, balance, age)
        self.point_balance = balance
        self.simulator.run_agent_self_check(self.agent, height)

    def test_weekly_cadence_and_confirmation_gaps_recompute_energy(self):
        # Each check uses a new baseline after succeeding at a nonconsecutive height.
        height, energy = 100, 1000
        for gap in (1, 11, 55, 110):
            previous = height
            height += gap
            energy += gap * 2
            self.check(height, energy)
            self.simulator.rpc_balance_history.assert_called_with(
                "get_address_balance", [{
                    "script_hash": "owner", "block_height": None,
                    "block_range": {"start": previous + 1, "end": height + 1},
                }],
            )
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 4)

    def test_wrong_energy_at_gap_55_fails_without_advancing_baseline(self):
        with self.assertRaisesRegex(WorldSimError, "energy mismatch"):
            self.check(155, 0)
        self.assertEqual(self.agent.oracle_last_checked_height, 100)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)

    def seed_spend_refill(self):
        self.rows = [
            {"block_height": 110, "balance": 100_000, "delta": -100_000},
            {"block_height": 130, "balance": 200_000, "delta": 100_000},
        ]
        # At 110: 1000 + 2*10 - floor(1*30*3/2) = 975.
        # At 130: 975 + 1*20 = 995. Refill preserves age 80.
        self.snapshots[110] = self.snapshot(110, 975, 100_000)
        self.snapshots[130] = self.snapshot(130, 995)

    def test_spend_refill_with_identical_endpoint_balances_keeps_penalty(self):
        self.seed_spend_refill()
        self.check(155, 1045)
        self.assertEqual(self.simulator.metrics["agent_energy_check_balance_events"], 2)

    def test_endpoint_only_growth_cannot_hide_intermediate_spend(self):
        self.seed_spend_refill()
        with self.assertRaisesRegex(WorldSimError, "energy mismatch"):
            self.check(155, 1110)

    def test_correct_endpoint_cannot_hide_corrupt_intermediate_settlement(self):
        self.seed_spend_refill()
        self.snapshots[110]["raw_energy"] = "976"
        with self.assertRaisesRegex(WorldSimError, "energy mismatch"):
            self.check(155, 1045)

    def test_unit_clear_and_refill_reset_age_at_each_boundary(self):
        self.agent.oracle_last_owner_balance = 100_001
        self.rows = [
            {"block_height": 110, "balance": 99_999, "delta": -2},
            {"block_height": 130, "balance": 100_000, "delta": 1},
        ]
        self.snapshots[110] = self.snapshot(110, 965, 99_999, 110)
        self.snapshots[130] = self.snapshot(130, 965, 100_000, 130)
        self.check(155, 990, 100_000, 130)

    def test_subunit_withdrawal_preserves_age_and_has_no_penalty(self):
        self.agent.oracle_last_owner_balance = 199_999
        self.rows = [{"block_height": 110, "balance": 100_000, "delta": -99_999}]
        self.snapshots[110] = self.snapshot(110, 1010, 100_000)
        self.check(155, 1055, 100_000)

    def test_positive_funding_keeps_age_while_changing_growth_slope(self):
        self.rows = [{"block_height": 110, "balance": 300_000, "delta": 100_000}]
        self.snapshots[110] = self.snapshot(110, 1020, 300_000)
        self.check(155, 1155, 300_000)

    def test_wrong_age_is_rejected_even_with_correct_energy(self):
        with self.assertRaisesRegex(WorldSimError, "energy mismatch"):
            self.check(155, 1110, age=100)

    def test_growth_saturates_before_penalty_and_penalty_floors_at_zero(self):
        maximum = RegtestWorldSimulator.ENERGY_MAX
        self.agent.oracle_last_energy = maximum - 1
        self.rows = [{"block_height": 110, "balance": 100_000, "delta": -100_000}]
        self.check(110, maximum - 45, 100_000)
        self.agent.oracle_last_energy = 0
        self.rows = [{"block_height": 111, "balance": 0, "delta": -100_000}]
        self.check(111, 0, 0, 111)

    def test_malformed_or_incomplete_balance_timelines_fail_closed(self):
        cases = [
            None,
            [{}],
            [{"block_height": 100, "balance": 200_000, "delta": 0}],
            [{"block_height": 156, "balance": 200_000, "delta": 0}],
            [{"block_height": 110, "balance": 100_000, "delta": -1}],
            [{"block_height": 110, "balance": -1, "delta": -200_001}],
            [{"block_height": 110, "balance": 200_000, "delta": 0}] * 2,
        ]
        self.snapshots[110] = self.snapshot(110, 1020)
        for rows in cases:
            with self.subTest(rows=rows):
                self.rows = rows
                with self.assertRaises(WorldSimError):
                    self.check(155, 1110)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)

    def test_current_balance_must_match_balance_history(self):
        self.snapshots[155] = self.snapshot(155, 1110, 100_000)
        with self.assertRaisesRegex(WorldSimError, "balance mismatch"):
            self.simulator.run_agent_self_check(self.agent, 155)

    def test_pass_switch_establishes_baseline_without_counting_numeric_success(self):
        self.agent.active_pass_id = "reminted-pass"
        self.check(155, 123, age=155)
        self.assertEqual(self.simulator.metrics["agent_energy_check_baseline"], 2)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)
        self.check(210, 233, age=155)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 1)

    def test_no_active_pass_resets_baseline_and_records_skip(self):
        self.agent.active_pass_id = None
        self.simulator.run_agent_self_check(self.agent, 155)
        self.assertIsNone(self.agent.oracle_last_energy)
        self.assertEqual(self.simulator.metrics["agent_energy_check_skipped_no_active"], 1)
        self.agent.active_pass_id = "new-pass"
        self.check(210, 0, age=210)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)

    def test_same_height_does_not_count_as_replay_but_still_rejects_mutation(self):
        self.check(100, 1000)
        self.assertEqual(self.simulator.metrics["agent_energy_check_skipped_same_height"], 1)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)
        with self.assertRaisesRegex(WorldSimError, "energy mismatch"):
            self.check(100, 1001)

    def test_reorg_resets_baseline_before_lower_height_is_accepted(self):
        with self.assertRaisesRegex(WorldSimError, "height regressed"):
            self.check(99, 998)
        self.simulator.agents = [self.agent]
        self.simulator.pass_owner_by_id = {}
        self.simulator.pass_identity_by_id = {}
        self.simulator.reset_local_chain_view()
        self.agent.active_pass_id = "pass-a"
        self.check(99, 50)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 0)
        self.check(154, 160)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 1)

    def test_serialized_baseline_resumes_the_same_numeric_interval(self):
        saved = self.simulator.serialize_agent(self.agent)
        self.agent = Agent(0, "alice", "btc-address", "usdb-address", "owner", "steady")
        self.simulator.apply_agent_state(self.agent, json.loads(json.dumps(saved)))
        self.seed_spend_refill()
        self.check(155, 1045)
        self.assertEqual(self.simulator.metrics["agent_energy_check_ok"], 1)


class RegtestWorldSoakEnergyCoverageTests(unittest.TestCase):
    def test_soak_summary_requires_actual_numeric_checks(self):
        script = (Path(__file__).with_name("run_regtest_world_soak_matrix.sh")).read_text()
        # Exercise the actual summary entrypoint, including its failure gate.
        match = re.search(r'python3 - "\$seed".*?<<\'PY\'\n(.*?)\nPY', script, re.DOTALL)
        self.assertIsNotNone(match)
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.jsonl"
            summary = Path(directory) / "summary.json"
            for metrics, passes in [
                ({"agent_self_check_ok": 100}, False),
                ({"agent_energy_check_ok": 0, "agent_energy_check_baseline": 100}, False),
                ({"agent_energy_check_ok": 1}, True),
                ({"agent_energy_check_ok": 1, "agent_self_check_fail": 1}, False),
            ]:
                with self.subTest(metrics=metrics):
                    report.write_text(json.dumps({"event": "session_end", "final_metrics": metrics}) + "\n")
                    result = subprocess.run(
                        [sys.executable, "-", "43", "2500", "1", "1", "0", str(report), str(summary)],
                        input=match.group(1), text=True, capture_output=True,
                    )
                    self.assertEqual(result.returncode == 0, passes, result.stderr)
                    if not passes:
                        self.assertIn(
                            "non-zero failure metrics" if metrics.get("agent_self_check_fail")
                            else "no strict numeric energy intervals",
                            result.stderr,
                        )


class RegtestWorldSimulatorPayloadTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.simulator = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.simulator.temp_dir = Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def load_payload(self, identity: PassIdentity, prev: list[str]) -> dict[str, object]:
        path = self.simulator.write_mint_content(
            usdb_main="0x" + "11" * 20,
            identity=identity,
            prev=prev,
        )
        return json.loads(path.read_text(encoding="utf-8"))

    def test_writes_strict_standard_payload(self) -> None:
        payload = self.load_payload(PassIdentity(pass_kind="standard"), [])
        self.assertEqual(
            set(payload), {"p", "op", "v", "prev", "usdb_main"}
        )
        self.assertEqual(payload["v"], 1)

    def test_writes_strict_fixed_and_address_collab_payloads(self) -> None:
        fixed = self.load_payload(
            PassIdentity(
                pass_kind="collab",
                leader_ref_kind="leader_pass_id",
                leader_ref_value="a" * 64 + "i0",
            ),
            ["b" * 64 + "i0"],
        )
        self.assertEqual(
            set(fixed), {"p", "op", "v", "prev", "leader_pass_id"}
        )
        self.assertEqual(fixed["prev"], ["b" * 64 + "i0"])

        address = self.load_payload(
            PassIdentity(
                pass_kind="collab",
                leader_ref_kind="leader_btc_addr",
                leader_ref_value="bcrt1qleader",
            ),
            [],
        )
        self.assertEqual(
            set(address), {"p", "op", "v", "prev", "leader_btc_addr"}
        )


class RegtestWorldSimulatorEconomicViewTests(unittest.TestCase):
    def setUp(self) -> None:
        self.simulator = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.simulator.args = SimpleNamespace(economic_page_limit=2)

    def test_resolves_fixed_and_address_collab_leaders_independently(self) -> None:
        leader = {"inscription_id": "a" * 64 + "i0", "owner": "owner-a"}
        standards_by_id = {leader["inscription_id"]: leader}
        standards_by_owner = {leader["owner"]: leader}
        self.simulator.address_to_script_hash = lambda _: "owner-a"

        fixed = self.simulator.resolve_expected_collab_leader(
            {"inscription_id": "fixed", "leader_pass_id": leader["inscription_id"]},
            standards_by_id,
            standards_by_owner,
        )
        address = self.simulator.resolve_expected_collab_leader(
            {"inscription_id": "address", "leader_btc_addr": "bcrt1qleader"},
            standards_by_id,
            standards_by_owner,
        )
        missing = self.simulator.resolve_expected_collab_leader(
            {"inscription_id": "fixed-old", "leader_pass_id": "f" * 64 + "i0"},
            standards_by_id,
            standards_by_owner,
        )

        self.assertEqual(fixed, leader["inscription_id"])
        self.assertEqual(address, leader["inscription_id"])
        self.assertIsNone(missing)

    def test_breakdown_cursor_pages_recompute_aggregate(self) -> None:
        leader_id = "a" * 64 + "i0"
        external_state = {"btc_height": 120, "snapshot_id": "snapshot-120"}
        rows = [
            {
                "collab_pass_id": "b" * 64 + "i0",
                "collab_owner_script_hash": "owner-b",
                "collab_raw_energy": "8",
                "collab_weight_bps": 5_000,
                "collab_contribution": "4",
                "leader_ref_kind": "leader_pass_id",
                "leader_ref_value": leader_id,
            },
            {
                "collab_pass_id": "c" * 64 + "i0",
                "collab_owner_script_hash": "owner-c",
                "collab_raw_energy": "6",
                "collab_weight_bps": 5_000,
                "collab_contribution": "3",
                "leader_ref_kind": "leader_btc_addr",
                "leader_ref_value": "bcrt1qleader",
            },
            {
                "collab_pass_id": "d" * 64 + "i0",
                "collab_owner_script_hash": "owner-d",
                "collab_raw_energy": "1",
                "collab_weight_bps": 5_000,
                "collab_contribution": "0",
                "leader_ref_kind": "leader_pass_id",
                "leader_ref_value": leader_id,
            },
        ]
        calls: list[dict[str, object]] = []

        def rpc_usdb(method: str, params: list[dict[str, object]]) -> dict[str, object]:
            self.assertEqual(method, "get_collab_breakdown")
            request = params[0]
            calls.append(request)
            cursor = request.get("cursor")
            page_rows = rows[:2] if cursor is None else rows[2:]
            return {
                "view_version": RegtestWorldSimulator.ECONOMIC_VIEW_VERSION,
                "external_state": external_state,
                "leader_pass_id": leader_id,
                "leader_state": "active",
                "leader_pass_kind": "standard",
                "sort": "collab_pass_id_asc",
                "total": 3,
                "aggregate_collab_contribution": "7",
                "limit": 2,
                "max_limit": 500,
                "next_cursor": "cursor-2" if cursor is None else None,
                "items": page_rows,
            }

        self.simulator.rpc_usdb = rpc_usdb
        result = self.simulator.load_collab_breakdown_at_height(
            leader_id,
            120,
            {"requested_height": 120},
            sort="collab_pass_id_asc",
            expected_external_state=external_state,
        )

        self.assertEqual(len(result["items"]), 3)
        self.assertIn("block_height", calls[0])
        self.assertIn("context", calls[0])
        self.assertNotIn("block_height", calls[1])
        self.assertNotIn("context", calls[1])
        self.assertEqual(calls[1]["cursor"], "cursor-2")


class RegtestWorldSimulatorResilienceTests(unittest.TestCase):
    def test_ord_transient_error_classifier_is_strict_and_case_insensitive(
        self,
    ) -> None:
        self.assertTrue(
            RegtestWorldSimulator.is_ord_transient_error(
                'error code: -4\nerror message:\nWallet is currently rescanning. '
                "Abort existing rescan or wait."
            )
        )
        self.assertTrue(
            RegtestWorldSimulator.is_ord_transient_error(
                "output in wallet but not in ord server"
            )
        )
        self.assertFalse(
            RegtestWorldSimulator.is_ord_transient_error(
                "mandatory-script-verify-flag-failed"
            )
        )

    @staticmethod
    def make_agent(agent_id: int, owner: str) -> Agent:
        return Agent(
            agent_id=agent_id,
            wallet_name=f"agent-{agent_id}",
            receive_address=f"address-{agent_id}",
            usdb_main_address="0x" + f"{agent_id + 1:02x}" * 20,
            owner_script_hash=owner,
            persona="holder",
        )

    def setUp(self) -> None:
        self.simulator = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.simulator.agents = [
            self.make_agent(0, "owner-a"),
            self.make_agent(1, "owner-b"),
        ]
        self.simulator.pass_owner_by_id = {}
        self.simulator.pass_identity_by_id = {}

    def test_rebuild_tracks_agent_rows_and_audits_external_non_active_rows(self) -> None:
        rows = [
            {
                "inscription_id": "active-a",
                "owner": "owner-a",
                "state": "active",
            },
            {
                "inscription_id": "consumed-b",
                "owner": "owner-b",
                "state": "consumed",
            },
            {
                "inscription_id": "dormant-external",
                "owner": "external-change",
                "state": "dormant",
            },
            {
                "inscription_id": "burned-external",
                "owner": "external-recipient",
                "state": "burned",
            },
        ]
        snapshots = {
            "active-a": {
                "inscription_id": "active-a",
                "pass_kind": "standard",
            },
            "consumed-b": {
                "inscription_id": "consumed-b",
                "pass_kind": "collab",
                "leader_pass_id": "leader",
            },
        }
        self.simulator.load_pass_energy_leaderboard_at_height = lambda *_: rows
        self.simulator.get_pass_snapshot = (
            lambda inscription_id, _height: snapshots.get(inscription_id)
        )

        result = self.simulator.rebuild_local_chain_view_from_height(120)

        self.assertEqual(result["loaded_pass_rows"], 4)
        self.assertEqual(result["tracked_pass_rows"], 2)
        self.assertEqual(result["external_owner_rows"], 2)
        self.assertEqual(result["external_dormant_owner_rows"], 1)
        self.assertEqual(result["external_terminal_owner_rows"], 1)
        self.assertEqual(result["unknown_active_owner_rows"], 0)
        self.assertEqual(result["active_owner_rows"], 1)
        self.assertEqual(self.simulator.agents[0].active_pass_id, "active-a")
        self.assertEqual(self.simulator.agents[0].owned_passes, {"active-a"})
        self.assertEqual(self.simulator.agents[1].owned_passes, {"consumed-b"})
        self.assertNotIn("dormant-external", self.simulator.pass_owner_by_id)

    def test_rebuild_rejects_external_active_owner(self) -> None:
        self.simulator.load_pass_energy_leaderboard_at_height = lambda *_: [
            {
                "inscription_id": "active-external",
                "owner": "external-owner",
                "state": "active",
            }
        ]
        self.simulator.get_pass_snapshot = lambda *_: None

        with self.assertRaisesRegex(
            WorldSimError, "active rows owned by unknown script hashes"
        ):
            self.simulator.rebuild_local_chain_view_from_height(120)

    def test_snapshot_not_ready_is_retryable_only_for_exact_consensus_error(self) -> None:
        retryable = {
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": RegtestWorldSimulator.SNAPSHOT_NOT_READY_RPC_CODE,
                "message": RegtestWorldSimulator.SNAPSHOT_NOT_READY_RPC_MESSAGE,
            },
        }
        self.assertIsNone(
            RegtestWorldSimulator.snapshot_rpc_result_if_ready(
                retryable, "get_snapshot_info"
            )
        )

        wrong_message = {
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": RegtestWorldSimulator.SNAPSHOT_NOT_READY_RPC_CODE,
                "message": "DIFFERENT_ERROR",
            },
        }
        with self.assertRaisesRegex(WorldSimError, "returned error"):
            RegtestWorldSimulator.snapshot_rpc_result_if_ready(
                wrong_message, "get_snapshot_info"
            )

    def test_snapshot_ready_requires_an_object_result(self) -> None:
        snapshot = {"stable_height": 120, "stable_block_hash": "block-120"}
        self.assertEqual(
            RegtestWorldSimulator.snapshot_rpc_result_if_ready(
                {"result": snapshot}, "get_snapshot_info"
            ),
            snapshot,
        )
        with self.assertRaisesRegex(WorldSimError, "non-object snapshot result"):
            RegtestWorldSimulator.snapshot_rpc_result_if_ready(
                {"result": 120}, "get_snapshot_info"
            )

    def test_replacement_blocks_restore_transactions_missing_from_mempool(self) -> None:
        self.simulator.args = SimpleNamespace(stable_lag_blocks=10, miner_wallet="miner")
        blocks = [
            {"height": height, "hash": f"old-{height}", "transactions": []}
            for height in range(101, 114)
        ]
        # The action block is the eleventh disconnected block. Core does not
        # resurrect these dependent transactions after invalidateblock.
        blocks[2]["transactions"] = [
            {"txid": "commit", "hex": "raw-commit"},
            {"txid": "reveal", "hex": "raw-reveal"},
        ]
        self.simulator.get_bitcoin_block_height = lambda: 100
        self.simulator.get_mempool_txids = lambda: []
        generated = []

        def btc_cli(_wallet, args):
            if args[0] == "getnewaddress":
                return "replacement-address"
            if args[0] == "generateblock":
                generated.append(json.loads(args[2]))
                return json.dumps({"hash": f"new-{100 + len(generated)}"})
            height = int(args[1].split("-")[1])
            return json.dumps({"height": height, "tx": ["coinbase"] + [
                tx["txid"] for tx in blocks[height - 101]["transactions"]
            ]})

        self.simulator.run_btc_cli = btc_cli
        result = self.simulator.mine_replacement_blocks(3, blocks)
        self.assertEqual(generated[2], ["raw-commit", "raw-reveal"])
        self.assertEqual(sum(bool(txs) for txs in generated), 1)
        self.assertEqual(len(generated), 13)
        self.assertEqual(result["replacement_replayed_tx_count"], 2)
        self.assertEqual(result["replacement_remaining_mempool_tx_count"], 0)

    def test_replacement_blocks_reject_noncontiguous_height_plan(self) -> None:
        self.simulator.args = SimpleNamespace(stable_lag_blocks=1)
        self.simulator.get_bitcoin_block_height = lambda: 100
        with self.assertRaisesRegex(WorldSimError, "replacement height range"):
            self.simulator.mine_replacement_blocks(2, [
                {"height": 101}, {"height": 103}, {"height": 104}
            ])

    def test_replacement_blocks_reject_missing_transactions_and_residual_mempool(self) -> None:
        self.simulator.args = SimpleNamespace(stable_lag_blocks=0, miner_wallet="miner")
        self.simulator.get_bitcoin_block_height = lambda: 100
        block = {
            "height": 101, "hash": "old",
            "transactions": [{"txid": "spend", "hex": "raw-spend"}],
        }
        for included, remaining, error in (
            (["coinbase"], [], "replay mismatch"),
            (["coinbase", "spend"], ["unrelated"], "left disconnected transactions"),
        ):
            with self.subTest(included=included, remaining=remaining):
                self.simulator.get_mempool_txids = lambda: remaining
                self.simulator.run_btc_cli = lambda _wallet, args: {
                    "getnewaddress": "address",
                    "generateblock": json.dumps({"hash": "new"}),
                    "getblock": json.dumps({"height": 101, "tx": included}),
                }[args[0]]
                with self.assertRaisesRegex(WorldSimError, error):
                    self.simulator.mine_replacement_blocks(1, [block])

    def test_reorg_capture_uses_old_blocks_and_excludes_coinbase(self) -> None:
        self.simulator.get_block_hash = lambda height: f"old-{height}"
        self.simulator.run_btc_cli = lambda _wallet, args: json.dumps({
            "height": int(args[1].split("-")[1]),
            "tx": [{"txid": "coinbase", "hex": "cb"}, {"txid": "spend", "hex": "raw"}],
        })
        blocks = self.simulator.capture_reorg_blocks(101, 113)
        self.assertEqual(len(blocks), 13)
        self.assertEqual(blocks[0]["transactions"], [{"txid": "spend", "hex": "raw"}])
        self.assertEqual(blocks[-1]["height"], 113)

    def test_ord_reorg_trigger_advances_one_stable_height(self) -> None:
        self.simulator.args = SimpleNamespace(stable_lag_blocks=10)
        self.simulator.mine_one_empty_block = lambda: 274

        self.assertEqual(self.simulator.mine_ord_reorg_trigger_block(263), 264)

    def test_ord_reorg_trigger_rejects_unexpected_raw_tip(self) -> None:
        self.simulator.args = SimpleNamespace(stable_lag_blocks=10)
        self.simulator.mine_one_empty_block = lambda: 275

        with self.assertRaisesRegex(WorldSimError, "unexpected raw tip"):
            self.simulator.mine_ord_reorg_trigger_block(263)

    def test_stable_height_saturates_and_subtracts_configured_lag(self) -> None:
        stable_height = RegtestWorldSimulator.stable_height_for_tip
        self.assertEqual(stable_height(142, 10), 132)
        self.assertEqual(stable_height(9, 10), 0)

    def test_ord_tip_identity_requires_block_count_and_canonical_hash(self) -> None:
        matches = RegtestWorldSimulator.ord_tip_matches_bitcoin

        self.assertTrue(matches(100, "canonical", 101, "canonical"))
        self.assertFalse(matches(100, "canonical", 100, "canonical"))
        self.assertFalse(matches(100, "canonical", 101, "orphan"))

    def test_wait_ord_server_synced_rejects_same_height_orphan(self) -> None:
        self.simulator.args = SimpleNamespace(sync_timeout_sec=1)
        ord_hashes = iter(["orphan", "canonical"])
        self.simulator.get_bitcoin_block_height = lambda: 100
        self.simulator.get_block_hash = lambda _height: "canonical"
        self.simulator.get_ord_server_block_count = lambda: 101
        self.simulator.get_ord_server_block_hash = lambda _height: next(ord_hashes)

        with mock.patch("regtest_world_simulator.time.sleep") as sleep:
            self.simulator.wait_ord_server_synced()

        sleep.assert_called_once_with(0.8)

    def test_mine_one_block_advances_action_to_stable_frontier(self) -> None:
        self.simulator.args = SimpleNamespace(
            miner_wallet="miner-wallet",
            mining_address="miner-address",
            stable_lag_blocks=10,
        )
        calls: list[tuple[str | None, list[str]]] = []

        def run_btc_cli(wallet: str | None, args: list[str]) -> str:
            calls.append((wallet, args))
            if args == ["getblockcount"]:
                return "153"
            return "[]"

        self.simulator.run_btc_cli = run_btc_cli
        self.simulator.get_mempool_txids = lambda: []

        self.assertEqual(self.simulator.mine_one_block(), 153)
        self.assertEqual(
            calls,
            [
                ("miner-wallet", ["generatetoaddress", "1", "miner-address"]),
                (None, ["getblockcount"]),
                (None, ["generatetoaddress", "10", "miner-address"]),
            ],
        )

    def test_between_tick_recovery_records_stable_lag_and_current_schema(self) -> None:
        self.simulator.args = SimpleNamespace(blocks=2500, stable_lag_blocks=10)
        self.simulator.action_seed = 41
        self.simulator.active_agent_count = 0
        self.simulator.metrics = {}
        self.simulator.reorg_events_applied = 0
        self.simulator.pass_owner_by_id = {}
        self.simulator.pass_identity_by_id = {}
        self.simulator.agents = []
        self.simulator.validator_samples = []

        snapshot = self.simulator.build_between_ticks_snapshot(
            batch_seed=41, next_tick=501, current_height=5642
        )

        self.assertEqual(snapshot["version"], RegtestWorldSimulator.RECOVERY_STATE_VERSION)
        self.assertEqual(snapshot["stable_lag_blocks"], 10)

    def test_recovery_ignores_a_different_stable_lag(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            recovery_path = Path(temp_dir) / "recovery.json"
            recovery_path.write_text(
                json.dumps(
                    {
                        "version": RegtestWorldSimulator.RECOVERY_STATE_VERSION,
                        "seed": 41,
                        "batch_blocks": 2500,
                        "stable_lag_blocks": 5,
                    }
                ),
                encoding="utf-8",
            )
            self.simulator.recovery_state_path = recovery_path
            self.simulator.action_seed = 41
            self.simulator.args = SimpleNamespace(blocks=2500, stable_lag_blocks=10)
            messages: list[str] = []
            self.simulator.log = messages.append

            self.assertIsNone(self.simulator.load_recovery_state())
            self.assertIn("mismatched stable lag", messages[0])

    def test_runtime_drivers_supply_the_embedded_stable_lag(self) -> None:
        local_driver = (
            REPO_ROOT
            / "src/btc/usdb-indexer/scripts/regtest_world_sim.sh"
        ).read_text(encoding="utf-8")
        docker_driver = (
            REPO_ROOT / "docker/scripts/entrypoints/start_world_sim.sh"
        ).read_text(encoding="utf-8")
        ord_entrypoint = (
            REPO_ROOT / "docker/scripts/entrypoints/start_ord_server.sh"
        ).read_text(encoding="utf-8")
        world_sim_compose = (
            REPO_ROOT / "docker/compose.world-sim.yml"
        ).read_text(encoding="utf-8")
        shared_reorg_driver = (
            REPO_ROOT / "src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh"
        ).read_text(encoding="utf-8")
        live_ord_driver = (
            REPO_ROOT / "src/btc/usdb-indexer/scripts/regtest_live_ord_e2e.sh"
        ).read_text(encoding="utf-8")
        dockerfile = (REPO_ROOT / "docker/Dockerfile.world-sim-tools").read_text(
            encoding="utf-8"
        )

        self.assertIn("resolve_btc_stable_lag_blocks", local_driver)
        self.assertIn(
            '--stable-lag-blocks "$BTC_STABLE_LAG_BLOCKS"', local_driver
        )
        self.assertIn("resolve_btc_stable_lag_blocks", docker_driver)
        self.assertIn(
            '--stable-lag-blocks "${btc_stable_lag_blocks}"', docker_driver
        )
        self.assertIn(
            "btc-regtest.json /opt/usdb/world-sim/btc-regtest-activation-registry.json",
            dockerfile,
        )
        self.assertIn("resolve_ord_reorg_capacity", local_driver)
        self.assertIn('--savepoint-interval "$ORD_SAVEPOINT_INTERVAL"', local_driver)
        self.assertIn('--max-savepoints "$ORD_MAX_SAVEPOINTS"', local_driver)
        self.assertIn('--savepoint-interval "${ord_savepoint_interval}"', ord_entrypoint)
        self.assertIn("WORLD_SIM_ORD_MAX_SAVEPOINTS", world_sim_compose)
        for driver in (
            local_driver,
            docker_driver,
            shared_reorg_driver,
            live_ord_driver,
        ):
            self.assertIn("/blockhash/", driver)
            self.assertIn("expected_ord_block_count", driver)

    def test_recovery_json_write_is_atomic(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "recovery.json"

            RegtestWorldSimulator.write_json_atomic(path, {"version": 1})
            RegtestWorldSimulator.write_json_atomic(path, {"version": 2})

            self.assertEqual(
                json.loads(path.read_text(encoding="utf-8")),
                {"version": 2},
            )
            self.assertEqual(list(path.parent.glob(f".{path.name}.*.tmp")), [])

    def test_recovery_rejects_changed_agent_identity(self) -> None:
        self.simulator.active_agent_count = 2
        self.simulator.metrics = {}
        self.simulator.reorg_events_applied = 0
        self.simulator.validator_samples = []
        agent_payloads = [
            self.simulator.serialize_agent(agent) for agent in self.simulator.agents
        ]
        agent_payloads[1]["receive_address"] = "different-address"

        with self.assertRaisesRegex(
            WorldSimError, "recovery snapshot agent identity mismatch"
        ):
            self.simulator.apply_recovery_snapshot(
                {
                    "active_agent_count": 2,
                    "metrics": {},
                    "reorg_events_applied": 0,
                    "pass_owner_by_id": {},
                    "pass_identity_by_id": {},
                    "agents": agent_payloads,
                    "validator_samples": [],
                }
            )

    def test_select_spendable_owner_output_uses_largest_plain_owner_utxo(self) -> None:
        txid_a = "a" * 64
        txid_b = "b" * 64
        selected = RegtestWorldSimulator.select_spendable_owner_output(
            [
                {
                    "output": f"{txid_a}:0",
                    "address": "owner",
                    "amount": 900_000,
                    "inscriptions": ["pass"],
                },
                {
                    "output": f"{txid_a}:1",
                    "address": "other",
                    "amount": 2_000_000,
                    "inscriptions": [],
                },
                {
                    "output": f"{txid_a}:2",
                    "address": "owner",
                    "amount": 600_000,
                    "inscriptions": [],
                },
                {
                    "output": f"{txid_b}:3",
                    "address": "owner",
                    "amount": 800_000,
                    "inscriptions": [],
                },
            ],
            "owner",
        )

        self.assertEqual(selected, (txid_b, 3, 800_000))

    def test_spend_balance_uses_explicit_plain_owner_input(self) -> None:
        actor = self.make_agent(0, "owner-a")
        actor.receive_address = "owner-address"
        self.simulator.args = SimpleNamespace(
            fee_rate=1,
            mining_address="miner-address",
            miner_wallet="miner-wallet",
        )
        self.simulator.metrics = {"skip": 0, "spend_ok": 0}
        self.simulator.get_balance_at_height = lambda *_: 1_000_000
        self.simulator.load_spendable_owner_output = lambda *_: (
            "c" * 64,
            2,
            800_000,
        )
        calls: list[tuple[str | None, list[str]]] = []

        def run_btc_cli(wallet: str | None, args: list[str]) -> str:
            calls.append((wallet, args))
            return json.dumps({"complete": True, "txid": "d" * 64})

        self.simulator.run_btc_cli = run_btc_cli
        self.simulator.write_external_action_result = lambda **_: None

        detail, expectation = self.simulator.op_spend_balance(
            actor, "action-1", 120, random.Random(1)
        )

        self.assertIn("spend_balance:", detail)
        self.assertEqual(expectation.actor_pre_balance, 1_000_000)
        self.assertGreaterEqual(expectation.amount_sat or 0, 100_000)
        self.assertEqual(calls[0][0], actor.wallet_name)
        self.assertEqual(calls[0][1][0], "send")
        options = json.loads(calls[0][1][5])
        self.assertFalse(options["add_inputs"])
        self.assertEqual(options["inputs"], [{"txid": "c" * 64, "vout": 2}])
        self.assertEqual(options["change_address"], actor.receive_address)

    def test_spend_balance_requires_tracked_owner_delta(self) -> None:
        actor = self.make_agent(0, "owner-a")
        self.simulator.agents = [actor]
        expectation = ActionExpectation(
            action="spend_balance",
            actor_id=0,
            actor_pre_balance=1_000_000,
            amount_sat=100_000,
        )
        self.simulator.get_balance_at_height = lambda *_: 900_000
        self.simulator.verify_expectation(expectation, 121)

        self.simulator.get_balance_at_height = lambda *_: 900_001
        with self.assertRaisesRegex(
            WorldSimError, "spend_balance verification failed"
        ):
            self.simulator.verify_expectation(expectation, 121)

    def test_spend_recovery_probes_new_explicit_input_transaction(self) -> None:
        self.simulator.find_wallet_transaction_by_comment = lambda *_: None
        self.simulator.find_new_wallet_transaction = lambda *_: {"txid": "e" * 64}
        result = self.simulator.probe_inflight_external_action_result(
            plan=PlannedAction(
                slot_index=0,
                actor_id=0,
                action="spend_balance",
                action_id="action-1",
                probe_state={"wallet_name": "agent-0", "wallet_txids": ["old"]},
            ),
            pre_height=120,
            available_ids={0},
            tick=1,
            slot_index=0,
        )

        self.assertEqual(result["source"], "bitcoin-wallet-explicit-input")
        self.assertEqual(result["raw_output"], "e" * 64)


if __name__ == "__main__":
    unittest.main()
