#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("check_json_rpc_readiness.py")
SPEC = importlib.util.spec_from_file_location("check_json_rpc_readiness", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
READINESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READINESS)


class JsonRpcReadinessTests(unittest.TestCase):
    def test_accepts_expected_ready_service(self) -> None:
        self.assertTrue(
            READINESS.validate_readiness(
                {"service": "balance-history", "consensus_ready": True},
                "balance-history",
            )
        )

    def test_accepts_expected_service_that_is_still_syncing(self) -> None:
        self.assertFalse(
            READINESS.validate_readiness(
                {"service": "usdb-indexer", "consensus_ready": False},
                "usdb-indexer",
            )
        )

    def test_rejects_service_mismatch(self) -> None:
        with self.assertRaisesRegex(ValueError, "service mismatch"):
            READINESS.validate_readiness(
                {"service": "usdb-indexer", "consensus_ready": True},
                "balance-history",
            )

    def test_rejects_non_boolean_readiness(self) -> None:
        with self.assertRaisesRegex(ValueError, "must be a boolean"):
            READINESS.validate_readiness(
                {"service": "balance-history", "consensus_ready": "true"},
                "balance-history",
            )


if __name__ == "__main__":
    unittest.main()
