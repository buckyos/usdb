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


class RegtestStableLagScriptTests(unittest.TestCase):
    def script_text(self, name: str) -> str:
        return (SCRIPT_DIR / name).read_text(encoding="utf-8")

    def run_stable_height_helper(
        self, tip_height: int, target_height: int
    ) -> subprocess.CompletedProcess[str]:
        script = textwrap.dedent(
            f"""
            set -euo pipefail
            source {shlex.quote(str(REGTEST_LIB))}
            BTC_STABLE_LAG_BLOCKS=10
            ORD_SERVER_PID=""
            regtest_get_bitcoin_tip_height() {{ echo {tip_height}; }}
            regtest_get_new_address() {{ echo bcrt1test; }}
            regtest_mine_blocks() {{ echo "mined=$1 address=$2"; }}
            regtest_ensure_stable_height_reachable {target_height}
            """
        )
        return subprocess.run(
            ["bash", "-c", script],
            text=True,
            capture_output=True,
            timeout=10,
            check=False,
        )

    def test_stable_height_helper_mines_only_missing_lag_window(self) -> None:
        result = self.run_stable_height_helper(tip_height=40, target_height=40)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Mining 10 stabilization block(s)", result.stdout)
        self.assertIn("mined=10 address=bcrt1test", result.stdout)

    def test_stable_height_helper_is_noop_when_target_is_stable(self) -> None:
        result = self.run_stable_height_helper(tip_height=50, target_height=40)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "")

    def test_protocol_smoke_mines_through_stability_window(self) -> None:
        script = self.script_text("regtest_e2e_smoke.sh")
        self.assertIn('BTC_STABLE_LAG_BLOCKS="${BTC_STABLE_LAG_BLOCKS:-10}"', script)
        self.assertIn(
            "target_tip_height=$((effective_target_height + BTC_STABLE_LAG_BLOCKS))",
            script,
        )
        self.assertIn('generatetoaddress "$target_tip_height"', script)
        self.assertIn(
            '--btc-stable-lag-blocks "$BTC_STABLE_LAG_BLOCKS"', script
        )

        runner = self.script_text("regtest_scenario_runner.py")
        self.assertIn("def stabilize_latest_btc_events", runner)
        self.assertIn(
            "expected_height = self.stabilize_latest_btc_events(mining_address)",
            runner,
        )

    def test_same_height_reorg_rebuilds_stability_window(self) -> None:
        script = self.script_text("regtest_same_height_reorg_smoke.sh")
        self.assertIn(
            "target_tip_height=$((TARGET_HEIGHT + BTC_STABLE_LAG_BLOCKS))", script
        )
        self.assertIn(
            'regtest_mine_blocks "$((BTC_STABLE_LAG_BLOCKS + 1))"', script
        )

    def test_validator_payload_uses_stable_context_height(self) -> None:
        script = self.script_text("regtest_live_ord_validator_block_body_e2e.sh")
        self.assertIn(
            'current_context_height="$((current_tip_height - BTC_STABLE_LAG_BLOCKS))"',
            script,
        )
        self.assertIn(
            'regtest_wait_until_balance_history_synced_eq "$current_context_height"',
            script,
        )

    def test_manual_validator_payloads_include_stable_lag(self) -> None:
        script = REGTEST_LIB.read_text(encoding="utf-8")
        stable_lag_assignment = (
            '"stable_lag": '
            'state_ref["snapshot_info"]["consensus_identity"]["stable_lag"]'
        )
        self.assertEqual(script.count(stable_lag_assignment), 2)


if __name__ == "__main__":
    unittest.main()
