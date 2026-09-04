#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

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
            "verificationprogress": 0.9999,
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

    def test_assessment_and_status_report_preserve_sync_progress(self) -> None:
        self.blockchain["blocks"] = 236_706
        self.blockchain["headers"] = 965_135
        self.blockchain["initialblockdownload"] = True
        self.blockchain["verificationprogress"] = 0.0116
        self.indexes["txindex"]["synced"] = False
        self.indexes["txindex"]["best_block_height"] = 236_701
        status, blockers = READINESS.assess_readiness(
            self.blockchain,
            self.indexes,
            self.network,
            "main",
            963_800,
            7200,
            1,
            1_800_000_000,
        )

        report = READINESS.readiness_report(status, blockers)
        self.assertFalse(report["ready"])
        self.assertEqual(report["status"]["blocks"], 236_706)
        self.assertIn("initialblockdownload=true", report["blockers"])
        progress = READINESS.format_wait_progress(status, blockers, 125)
        self.assertIn("elapsed=00:02:05", progress)
        self.assertIn("blocks=236706/965135", progress)
        self.assertIn("verification=1.16%", progress)
        self.assertIn("txindex=indexing@236701", progress)

    def test_verification_progress_must_be_bounded_number(self) -> None:
        self.blockchain["verificationprogress"] = True
        with self.assertRaisesRegex(ValueError, "must be a number"):
            self.evaluate()
        self.blockchain["verificationprogress"] = 1.1
        with self.assertRaisesRegex(ValueError, "between 0 and 1"):
            self.evaluate()

    def test_data_start_accepts_ibd_without_txindex_after_anchor(self) -> None:
        self.blockchain["initialblockdownload"] = True
        self.blockchain["headers"] = 965_000
        status, blockers = READINESS.assess_data_start_readiness(
            self.blockchain,
            self.network,
            "main",
            963_810,
            963_800,
            "11" * 32,
            "11" * 32,
        )

        self.assertEqual(blockers, [])
        self.assertEqual(status.minimum_height, 963_810)
        self.assertEqual(status.anchor_height, 963_800)
        self.assertTrue(status.initial_block_download)

    def test_data_start_waits_for_height_and_rejects_anchor_mismatch(self) -> None:
        self.blockchain["blocks"] = 963_799
        _, blockers = READINESS.assess_data_start_readiness(
            self.blockchain,
            self.network,
            "main",
            963_810,
            963_800,
            None,
            "11" * 32,
        )
        self.assertIn("minimum data-start height 963810", blockers[0])

        self.blockchain["blocks"] = 963_810
        _, blockers = READINESS.assess_data_start_readiness(
            self.blockchain,
            self.network,
            "main",
            963_810,
            963_800,
            "22" * 32,
            "11" * 32,
        )
        self.assertIn("expected", blockers[0])

    def test_data_start_main_does_not_query_txindex(self) -> None:
        calls: list[str] = []

        class FakeRpc:
            def call(inner_self, method: str):
                calls.append(method)
                if method == "getblockchaininfo":
                    value = dict(self.blockchain)
                    value["initialblockdownload"] = True
                    return value
                if method == "getnetworkinfo":
                    return self.network
                raise AssertionError(f"unexpected RPC method {method}")

            def get_block_hash(inner_self, height: int) -> str:
                self.assertEqual(height, 963_800)
                return "11" * 32

        arguments = SimpleNamespace(
            user="node",
            minimum_height=963_810,
            maximum_tip_age_secs=7_200,
            minimum_connections=1,
            rpc_timeout_secs=5.0,
            poll_interval_secs=1.0,
            progress_interval_secs=60.0,
            password_file="",
            url="http://127.0.0.1:8332",
            expected_chain="main",
            wait_timeout_secs=120.0,
            status_json=False,
            data_start=True,
            anchor_height=963_800,
            expected_block_hash="11" * 32,
        )
        stdout = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "read_password", return_value="secret"),
            mock.patch.object(READINESS, "BitcoinRpc", return_value=FakeRpc()),
            mock.patch("sys.stdout", new=stdout),
        ):
            result = READINESS.main()

        self.assertEqual(result, 0)
        self.assertNotIn("getindexinfo", calls)
        self.assertEqual(json.loads(stdout.getvalue())["minimum_height"], 963_810)
        self.assertEqual(json.loads(stdout.getvalue())["anchor_height"], 963_800)

    def test_data_start_requires_anchor_height_and_hash_as_a_pair(self) -> None:
        arguments = SimpleNamespace(
            user="node",
            minimum_height=963_810,
            maximum_tip_age_secs=7_200,
            minimum_connections=1,
            rpc_timeout_secs=5.0,
            poll_interval_secs=1.0,
            progress_interval_secs=60.0,
            password_file="",
            url="http://127.0.0.1:8332",
            expected_chain="main",
            wait_timeout_secs=120.0,
            status_json=False,
            data_start=True,
            anchor_height=963_800,
            expected_block_hash="",
        )
        with mock.patch.object(READINESS, "parse_args", return_value=arguments):
            with self.assertRaisesRegex(ValueError, "must be supplied together"):
                READINESS.main()

    def test_wait_main_keeps_heartbeat_off_json_stdout(self) -> None:
        blockchain_calls = 0

        class FakeRpc:
            def call(inner_self, method: str):
                nonlocal blockchain_calls
                if method == "getblockchaininfo":
                    blockchain_calls += 1
                    value = dict(self.blockchain)
                    if blockchain_calls == 1:
                        value["initialblockdownload"] = True
                        value["blocks"] = 900_000
                        value["headers"] = 963_900
                        value["verificationprogress"] = 0.9
                    return value
                if method == "getindexinfo":
                    return self.indexes
                return self.network

        arguments = SimpleNamespace(
            user="node",
            minimum_height=0,
            maximum_tip_age_secs=7_200,
            minimum_connections=1,
            rpc_timeout_secs=5.0,
            poll_interval_secs=1.0,
            progress_interval_secs=60.0,
            password_file="",
            url="http://127.0.0.1:8332",
            expected_chain="main",
            wait_timeout_secs=120.0,
            status_json=False,
        )
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "read_password", return_value="secret"),
            mock.patch.object(READINESS, "BitcoinRpc", return_value=FakeRpc()),
            mock.patch.object(READINESS.time, "sleep"),
            mock.patch.object(READINESS.time, "monotonic", side_effect=[0.0, 0.0, 0.0]),
            mock.patch("sys.stdout", new=stdout),
            mock.patch("sys.stderr", new=stderr),
        ):
            result = READINESS.main()

        self.assertEqual(result, 0)
        self.assertEqual(json.loads(stdout.getvalue())["blocks"], 963_900)
        self.assertIn("Bitcoin readiness waiting", stderr.getvalue())
        self.assertNotIn("Bitcoin readiness waiting", stdout.getvalue())

    def test_status_json_reports_not_ready_without_failure(self) -> None:
        class FakeRpc:
            def call(inner_self, method: str):
                if method == "getblockchaininfo":
                    value = dict(self.blockchain)
                    value["initialblockdownload"] = True
                    return value
                if method == "getindexinfo":
                    return self.indexes
                return self.network

        arguments = SimpleNamespace(
            user="node",
            minimum_height=0,
            maximum_tip_age_secs=7_200,
            minimum_connections=1,
            rpc_timeout_secs=5.0,
            poll_interval_secs=1.0,
            progress_interval_secs=60.0,
            password_file="",
            url="http://127.0.0.1:8332",
            expected_chain="main",
            wait_timeout_secs=0.0,
            status_json=True,
        )
        stdout = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "read_password", return_value="secret"),
            mock.patch.object(READINESS, "BitcoinRpc", return_value=FakeRpc()),
            mock.patch.object(READINESS.time, "monotonic", side_effect=[0.0, 0.0]),
            mock.patch("sys.stdout", new=stdout),
        ):
            result = READINESS.main()

        report = json.loads(stdout.getvalue())
        self.assertEqual(result, 0)
        self.assertFalse(report["ready"])
        self.assertEqual(report["schema_version"], "usdb-bitcoin-readiness:v1")

    def test_zero_progress_interval_disables_wait_heartbeat(self) -> None:
        class FakeRpc:
            def call(inner_self, method: str):
                if method == "getblockchaininfo":
                    value = dict(self.blockchain)
                    value["initialblockdownload"] = True
                    return value
                if method == "getindexinfo":
                    return self.indexes
                return self.network

        arguments = SimpleNamespace(
            user="node",
            minimum_height=0,
            maximum_tip_age_secs=7_200,
            minimum_connections=1,
            rpc_timeout_secs=5.0,
            poll_interval_secs=1.0,
            progress_interval_secs=0.0,
            password_file="",
            url="http://127.0.0.1:8332",
            expected_chain="main",
            wait_timeout_secs=1.0,
            status_json=False,
        )
        stderr = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "read_password", return_value="secret"),
            mock.patch.object(READINESS, "BitcoinRpc", return_value=FakeRpc()),
            mock.patch.object(READINESS.time, "sleep"),
            mock.patch.object(READINESS.time, "monotonic", side_effect=[0.0, 0.0, 1.0]),
            mock.patch("sys.stderr", new=stderr),
        ):
            with self.assertRaisesRegex(ValueError, "initialblockdownload"):
                READINESS.main()

        self.assertNotIn("readiness waiting", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
