#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import io
import json
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

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

    def test_balance_history_origin_accepts_query_ready_catching_up_state(self) -> None:
        self.assertTrue(
            READINESS.validate_balance_history_origin(
                {
                    "service": "balance-history",
                    "query_ready": True,
                    "consensus_ready": False,
                    "stable_height": 963_800,
                    "stable_block_hash": "11" * 32,
                    "latest_block_commit": "22" * 32,
                    "blockers": ["CatchingUp"],
                },
                "balance-history",
                963_800,
            )
        )

    def test_balance_history_origin_rejects_incomplete_or_low_state(self) -> None:
        base = {
            "service": "balance-history",
            "query_ready": True,
            "consensus_ready": False,
            "stable_height": 963_799,
            "stable_block_hash": "11" * 32,
            "latest_block_commit": "22" * 32,
            "blockers": ["CatchingUp"],
        }
        self.assertFalse(
            READINESS.validate_balance_history_origin(
                base, "balance-history", 963_800
            )
        )
        self.assertFalse(
            READINESS.validate_balance_history_origin(
                {**base, "stable_height": 963_800, "latest_block_commit": None},
                "balance-history",
                963_800,
            )
        )
        self.assertFalse(
            READINESS.validate_balance_history_origin(
                {
                    **base,
                    "stable_height": 963_800,
                    "blockers": ["CatchingUp", "SnapshotInstallUnverified"],
                },
                "balance-history",
                963_800,
            )
        )

    def test_formats_balance_history_wait_progress(self) -> None:
        progress = READINESS.format_wait_progress(
            "balance-history",
            3_661,
            {
                "service": "balance-history",
                "consensus_ready": False,
                "phase": "Indexing",
                "current": 800_000,
                "total": 963_800,
                "stable_height": 800_000,
                "blockers": ["CatchingUp"],
            },
            ValueError("consensus_ready is false"),
        )
        self.assertIn("elapsed=01:01:01", progress)
        self.assertIn("progress=800000/963800 (83.00%)", progress)
        self.assertIn("phase=Indexing", progress)
        self.assertIn("stable_height=800000", progress)
        self.assertIn("blockers=CatchingUp", progress)

    def test_formats_indexer_wait_progress_and_rpc_failure(self) -> None:
        progress = READINESS.format_wait_progress(
            "usdb-indexer",
            10,
            {
                "service": "usdb-indexer",
                "consensus_ready": False,
                "current": 900_000,
                "total": 963_800,
                "synced_block_height": 900_000,
                "balance_history_stable_height": 963_800,
            },
            None,
        )
        self.assertIn("progress=900000/963800", progress)
        self.assertIn("synced_block_height=900000", progress)
        failure = READINESS.format_wait_progress(
            "balance-history",
            0,
            None,
            OSError("connection refused"),
        )
        self.assertIn("detail=connection refused", failure)

    def test_wait_main_keeps_heartbeat_off_result_stdout(self) -> None:
        not_ready = {
            "service": "balance-history",
            "consensus_ready": False,
            "phase": "Indexing",
            "current": 800_000,
            "total": 963_800,
        }
        ready = {**not_ready, "consensus_ready": True, "current": 963_800}
        arguments = SimpleNamespace(
            url="http://127.0.0.1:28010",
            expected_service="balance-history",
            require_consensus_ready=True,
            wait_timeout_secs=120,
            poll_interval_secs=1.0,
            request_timeout_secs=5.0,
            progress_interval_secs=30.0,
        )
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "fetch_readiness", side_effect=[not_ready, ready]),
            mock.patch.object(READINESS.time, "sleep"),
            mock.patch.object(
                READINESS.time,
                "monotonic",
                side_effect=[0.0, 0.0, 0.0, 0.0],
            ),
            mock.patch("sys.stdout", new=stdout),
            mock.patch("sys.stderr", new=stderr),
        ):
            result = READINESS.main()

        self.assertEqual(result, 0)
        self.assertTrue(json.loads(stdout.getvalue())["consensus_ready"])
        self.assertIn("balance-history readiness waiting", stderr.getvalue())
        self.assertNotIn("readiness waiting", stdout.getvalue())

    def test_zero_progress_interval_disables_wait_heartbeat(self) -> None:
        arguments = SimpleNamespace(
            url="http://127.0.0.1:28010",
            expected_service="balance-history",
            require_consensus_ready=True,
            wait_timeout_secs=1,
            poll_interval_secs=1.0,
            request_timeout_secs=5.0,
            progress_interval_secs=0.0,
        )
        not_ready = {
            "service": "balance-history",
            "consensus_ready": False,
            "current": 1,
            "total": 2,
        }
        stderr = io.StringIO()
        with (
            mock.patch.object(READINESS, "parse_args", return_value=arguments),
            mock.patch.object(READINESS, "fetch_readiness", return_value=not_ready),
            mock.patch.object(READINESS.time, "sleep"),
            mock.patch.object(READINESS.time, "monotonic", side_effect=[0.0, 0.0, 1.0]),
            mock.patch("sys.stderr", new=stderr),
        ):
            self.assertEqual(READINESS.main(), 1)

        self.assertNotIn("readiness waiting", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
