#!/usr/bin/env python3
"""Check bounded world-sim waits across service readiness transitions."""

from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src/btc/usdb-indexer/scripts"))
from regtest_world_simulator import RegtestWorldSimulator, WorldSimError  # noqa: E402


class WorldReadinessTests(unittest.TestCase):
    def setUp(self):
        self.simulator = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.simulator.args = SimpleNamespace(
            balance_history_rpc_url="balance-history",
            usdb_indexer_rpc_url="usdb-indexer",
            sync_timeout_sec=2,
        )
        self.not_ready = {
            "error": {"code": -32041, "message": "SNAPSHOT_NOT_READY"}
        }

    def test_waits_retry_gated_heights_including_target_zero(self):
        for wait_name in ("wait_balance_history_height_exact", "wait_service_height_exact"):
            for gated_service in ("balance-history", "usdb-indexer"):
                if wait_name == "wait_balance_history_height_exact" and gated_service == "usdb-indexer":
                    continue
                with self.subTest(wait=wait_name, service=gated_service):
                    gated_calls = 0

                    def rpc_call(url, method, params):
                        nonlocal gated_calls
                        if method == "get_readiness":
                            return {"result": {"consensus_ready": True}}
                        if url == gated_service:
                            gated_calls += 1
                            if gated_calls == 1:
                                return self.not_ready
                        return {"result": 0}

                    self.simulator.rpc_call = rpc_call
                    with mock.patch("regtest_world_simulator.time.sleep") as sleep:
                        getattr(self.simulator, wait_name)(0)
                    self.assertEqual(gated_calls, 2)
                    sleep.assert_called_once()

    def test_waits_require_exact_height_and_consensus_readiness(self):
        for wait_name in ("wait_balance_history_height_exact", "wait_service_height_exact"):
            with self.subTest(wait=wait_name):
                heights = iter([11, 10, 10])
                ready = iter([True, False, True])

                def rpc_call(url, method, params):
                    if method == "get_readiness":
                        return {"result": {"consensus_ready": next(ready) if url == "balance-history" else True}}
                    return {"result": next(heights) if url == "balance-history" else 10}

                self.simulator.rpc_call = rpc_call
                with mock.patch("regtest_world_simulator.time.sleep") as sleep:
                    getattr(self.simulator, wait_name)(10)
                self.assertEqual(sleep.call_count, 2)

    def test_persistent_not_ready_times_out(self):
        for wait_name in ("wait_balance_history_height_exact", "wait_service_height_exact"):
            with self.subTest(wait=wait_name):
                self.simulator.rpc_call = lambda url, method, params: (
                    {"result": {"consensus_ready": False}}
                    if method == "get_readiness" else self.not_ready
                )
                with mock.patch("regtest_world_simulator.time.time", side_effect=[0, 3]):
                    with self.assertRaisesRegex(WorldSimError, "exact sync timeout"):
                        getattr(self.simulator, wait_name)(10)

    def test_other_rpc_errors_and_ordinary_assertions_remain_fatal(self):
        for error in (
            {"code": -32042, "message": "SNAPSHOT_ID_MISMATCH"},
            {"code": -32041, "message": "unexpected error"},
        ):
            self.simulator.rpc_call = lambda *args: {"error": error}
            for wait_name in ("wait_balance_history_height_exact", "wait_service_height_exact"):
                with self.subTest(wait=wait_name, error=error):
                    with self.assertRaises(WorldSimError):
                        getattr(self.simulator, wait_name)(10)
        self.simulator.rpc_call = lambda *args: self.not_ready
        with self.assertRaises(WorldSimError):
            self.simulator.rpc_balance_history("get_block_height", [])


if __name__ == "__main__":
    unittest.main()
