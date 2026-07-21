#!/usr/bin/env python3

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from regtest_world_simulator import (
    PassIdentity,
    RegtestWorldSimulator,
    ValidatorSampleCandidate,
)


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


if __name__ == "__main__":
    unittest.main()
