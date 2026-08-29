#!/usr/bin/env python3

import os
import shlex
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
REGTEST_LIB = SCRIPT_DIR / "regtest_reorg_lib.sh"


class RegtestCleanupTests(unittest.TestCase):
    def run_cleanup(self, body: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            env = os.environ.copy()
            env.update(
                {
                    "WORK_DIR": str(root),
                    "BITCOIN_DIR": str(root / "bitcoin"),
                    "BALANCE_HISTORY_ROOT": str(root / "balance-history-root"),
                    "USDB_INDEXER_ROOT": str(root / "usdb-indexer-root"),
                    "BALANCE_HISTORY_LOG_FILE": str(root / "balance-history.log"),
                    "USDB_INDEXER_LOG_FILE": str(root / "usdb-indexer.log"),
                    "ORD_SERVER_LOG_FILE": str(root / "ord-server.log"),
                    "CURL_CONNECT_TIMEOUT_SEC": "1",
                    "CURL_MAX_TIME_SEC": "1",
                }
            )
            script = textwrap.dedent(
                f"""
                set -euo pipefail
                source {shlex.quote(str(REGTEST_LIB))}
                trap regtest_cleanup EXIT
                {body}
                """
            )
            return subprocess.run(
                ["bash", "-c", script],
                env=env,
                text=True,
                capture_output=True,
                timeout=10,
                check=False,
            )

    def test_cleanup_accepts_healthy_process_and_benign_logs(self) -> None:
        result = self.run_cleanup(
            """
            sleep 30 &
            BALANCE_HISTORY_PID=$!
            printf 'service started normally\\n' >"$BALANCE_HISTORY_LOG_FILE"
            """
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_cleanup_accepts_process_stopped_by_lifecycle_helper(self) -> None:
        result = self.run_cleanup(
            """
            sleep 30 &
            BALANCE_HISTORY_PID=$!
            regtest_stop_balance_history
            """
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_lifecycle_stop_rejects_process_that_already_exited(self) -> None:
        result = self.run_cleanup(
            """
            (exit 17) &
            BALANCE_HISTORY_PID=$!
            sleep 0.1
            regtest_stop_balance_history
            """
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Managed process exited unexpectedly", result.stdout)

    def test_cleanup_rejects_unexpected_process_exit(self) -> None:
        result = self.run_cleanup(
            """
            (exit 17) &
            BALANCE_HISTORY_PID=$!
            sleep 0.1
            """
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Managed process exited unexpectedly", result.stdout)

    def test_cleanup_rejects_panic_written_during_shutdown(self) -> None:
        result = self.run_cleanup(
            """
            sleep 30 &
            USDB_INDEXER_PID=$!
            printf \"thread 'worker' panicked at runtime shutdown\\n\" >"$USDB_INDEXER_LOG_FILE"
            """
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Fatal process log detected", result.stdout)

    def test_cleanup_preserves_original_test_failure(self) -> None:
        result = self.run_cleanup("false")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Failure diagnostics: exit_code=1", result.stdout)


if __name__ == "__main__":
    unittest.main()
