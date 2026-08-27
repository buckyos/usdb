#!/usr/bin/env python3

from __future__ import annotations

import argparse
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("regtest_balance_oracle.py")
SPEC = importlib.util.spec_from_file_location("regtest_balance_oracle", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
ORACLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ORACLE)


def block(height: int, transactions: list[dict]) -> dict:
    return {"height": height, "tx": transactions}


def output(number: int, value: str, address: str | None = None) -> dict:
    script = {} if address is None else {"address": address}
    return {"n": number, "value": value, "scriptPubKey": script}


class BalanceOracleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="usdb-bh-oracle-")
        self.state_file = Path(self.temp_dir.name) / "state.json"
        ORACLE.cmd_init(
            argparse.Namespace(
                state_file=str(self.state_file),
                start_height=0,
                addresses_json=json.dumps(["addr-a", "addr-b"]),
            )
        )

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def apply(self, value: dict) -> None:
        ORACLE.cmd_apply_block(
            argparse.Namespace(
                state_file=str(self.state_file),
                block_json=io.StringIO(json.dumps(value)),
            )
        )

    def state(self) -> dict:
        return json.loads(self.state_file.read_text())

    def test_tracks_exact_deltas_spends_and_zero_net_movements(self) -> None:
        self.apply(
            block(
                1,
                [
                    {
                        "txid": "tx-1",
                        "vin": [{"coinbase": "00"}],
                        "vout": [output(0, "1.25", "addr-a"), output(1, "0", None)],
                    }
                ],
            )
        )
        self.apply(
            block(
                2,
                [
                    {
                        "txid": "tx-2",
                        "vin": [{"txid": "tx-1", "vout": 0}],
                        "vout": [
                            output(0, "0.25", "addr-a"),
                            output(1, "1.00", "addr-b"),
                        ],
                    }
                ],
            )
        )
        self.apply(
            block(
                3,
                [
                    {
                        "txid": "tx-3",
                        "vin": [{"txid": "tx-2", "vout": 0}],
                        "vout": [output(0, "0.25", "addr-a")],
                    }
                ],
            )
        )
        self.apply(block(4, []))

        state = self.state()
        self.assertEqual(state["balances"], {"addr-a": 25_000_000, "addr-b": 100_000_000})
        self.assertEqual(state["deltas"]["addr-a"]["1"], 125_000_000)
        self.assertEqual(state["deltas"]["addr-a"]["2"], -100_000_000)
        self.assertEqual(state["deltas"]["addr-a"]["3"], 0)
        self.assertNotIn("4", state["deltas"]["addr-a"])
        self.assertEqual(state["deltas"]["addr-b"]["2"], 100_000_000)
        self.assertEqual(
            state["utxos"],
            {
                "tx-2:1": {"address": "addr-b", "value": 100_000_000},
                "tx-3:0": {"address": "addr-a", "value": 25_000_000},
            },
        )
        self.assertEqual(
            sorted(state["spent_utxos"]),
            ["tx-1:0", "tx-2:0"],
        )
        self.assertEqual(
            ORACLE.history_rows(state, "addr-a", 1, 5),
            [
                {"block_height": 1, "delta": 125_000_000, "balance": 125_000_000},
                {"block_height": 2, "delta": -100_000_000, "balance": 25_000_000},
                {"block_height": 3, "delta": 0, "balance": 25_000_000},
            ],
        )

    def test_rejects_non_contiguous_block_stream(self) -> None:
        with self.assertRaisesRegex(SystemExit, "expected 1, got 2"):
            self.apply(block(2, []))


if __name__ == "__main__":
    unittest.main()
