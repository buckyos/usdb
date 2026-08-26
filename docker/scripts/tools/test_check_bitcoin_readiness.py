#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_bitcoin_readiness.py")
SPEC = importlib.util.spec_from_file_location("check_bitcoin_readiness", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
READINESS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = READINESS
SPEC.loader.exec_module(READINESS)


class BitcoinReadinessTests(unittest.TestCase):
    def setUp(self) -> None:
        self.blockchain = {
            "chain": "main",
            "blocks": 963900,
            "headers": 963900,
            "initialblockdownload": False,
            "pruned": False,
            "time": 1_799_999_400,
        }
        self.indexes = {"txindex": {"synced": True, "best_block_height": 963900}}
        self.network = {"networkactive": True, "connections": 8}

    def evaluate(self):
        return READINESS.evaluate_readiness(
            self.blockchain,
            self.indexes,
            self.network,
            "main",
            963800,
            7200,
            1,
            1_800_000_000,
        )

    def test_ready_mainnet_is_accepted(self) -> None:
        status = self.evaluate()
        self.assertEqual(status.blocks, 963900)
        self.assertTrue(status.txindex_synced)

    def test_wrong_chain_is_rejected(self) -> None:
        self.blockchain["chain"] = "regtest"
        with self.assertRaisesRegex(ValueError, "expected main"):
            self.evaluate()

    def test_pruned_node_is_rejected(self) -> None:
        self.blockchain["pruned"] = True
        with self.assertRaisesRegex(ValueError, "non-pruned"):
            self.evaluate()

    def test_ibd_or_header_lag_is_rejected(self) -> None:
        self.blockchain["initialblockdownload"] = True
        self.blockchain["headers"] += 1
        with self.assertRaisesRegex(ValueError, "initialblockdownload=true.*does not match headers"):
            self.evaluate()

    def test_height_below_snapshot_origin_is_rejected(self) -> None:
        self.blockchain["blocks"] = 963799
        self.blockchain["headers"] = 963799
        self.indexes["txindex"]["best_block_height"] = 963799
        with self.assertRaisesRegex(ValueError, "minimum 963800"):
            self.evaluate()

    def test_missing_or_stale_txindex_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "txindex is missing"):
            READINESS.evaluate_readiness(
                self.blockchain, {}, self.network, "main", 963800, 7200, 1, 1_800_000_000
            )
        self.indexes["txindex"]["synced"] = False
        self.indexes["txindex"]["best_block_height"] -= 1
        with self.assertRaisesRegex(ValueError, "txindex.synced=false.*does not match blocks"):
            self.evaluate()

    def test_boolean_height_is_rejected(self) -> None:
        self.blockchain["blocks"] = True
        with self.assertRaisesRegex(ValueError, "must be int"):
            self.evaluate()

    def test_stale_or_disconnected_node_is_rejected(self) -> None:
        self.blockchain["time"] = 1_799_990_000
        self.network["networkactive"] = False
        self.network["connections"] = 0
        with self.assertRaisesRegex(ValueError, "tip age=.*networkactive=false.*connections=0"):
            self.evaluate()


if __name__ == "__main__":
    unittest.main()
