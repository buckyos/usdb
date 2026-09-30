#!/usr/bin/env python3
"""Shutdown observations must distinguish old events from current write progress."""

from datetime import datetime, timezone
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import bitcoin_shutdown_progress as progress
import test_testnet_bitcoin_release as release_tests

START = datetime(2026, 9, 30, 12, 1, 3, tzinfo=timezone.utc).timestamp()
FLUSH = "[warning] Flushing large (37582620 entries) UTXO set to disk, it may take several minutes"
MEMPOOL = "Dumped mempool: 0.000s to copy, 0.329s to dump, 27 bytes dumped to file"


class BitcoinShutdownProgressTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-core-shutdown-log-")
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "debug.log"

    def append(self, message, seconds=1):
        stamp = datetime.fromtimestamp(START + seconds, timezone.utc).isoformat().replace("+00:00", "Z")
        with self.path.open("a") as output:
            output.write(f"{stamp} {message}\n")

    def read(self, elapsed=75, since=START):
        return progress.read_shutdown_progress(self.path, since, START + elapsed)

    def test_utxo_flush_replaces_completed_mempool_step_without_inventing_progress(self):
        self.append("Shutdown in progress...", 0)
        self.append(MEMPOOL)
        saved = progress.render_shutdown_progress(self.read())
        self.assertIn("Mempool saved (27 bytes; completed step)", saved)
        self.append(FLUSH)
        first = self.read(elapsed=15)
        second = self.read(elapsed=75)
        self.assertEqual(first["stage"], second["stage"])
        self.assertEqual(first["logged_at"], second["logged_at"])
        rendered = progress.render_shutdown_progress(second)
        self.assertIn("Flushing UTXO set to disk (37,582,620 entries", rendered)
        self.assertIn("completion percentage unavailable", rendered)
        self.assertIn("2026-09-30T12:01:04Z", rendered)
        self.assertIn("No newer stage log for 00:01:14", rendered)
        self.assertNotIn("27 bytes", rendered)

    def test_previous_shutdown_and_future_or_untimestamped_lines_are_not_current(self):
        self.append(FLUSH, -5)
        self.append("Shutdown done", 100)
        with self.path.open("a") as output:
            output.write(f"{FLUSH}\n2026-09-30T12:01:04 {FLUSH}\ninvalid {MEMPOOL}\n")
        self.assertEqual(self.read(), {})
        self.assertIn("no current shutdown-stage log", progress.render_shutdown_progress(self.read()))

    def test_modern_and_legacy_shutdown_logs_do_not_imply_container_exit(self):
        for spelling in ("Shutdown", "Shutdown:"):
            with self.subTest(spelling=spelling):
                self.path.write_text("")
                self.append(f"{spelling} In progress...", 0)
                self.assertEqual(self.read()["stage"], "Stopping Core services")
                self.append(f"{spelling} done", 1)
                self.assertIn("awaiting container exit", self.read()["stage"])

    def test_second_precision_and_fractional_log_timestamps(self):
        self.append(FLUSH, 0)
        self.assertIn("Flushing UTXO", self.read(since=START + 0.5)["stage"])
        self.append(MEMPOOL, 1.5)
        self.assertEqual(self.read(elapsed=2)["age_seconds"], 0)
        self.assertEqual(self.read(since=START + 3), {})

    def test_partial_line_then_rotation_or_truncation_does_not_retain_stale_phase(self):
        self.path.write_text(f"2026-09-30T12:01:04Z {FLUSH}")
        self.assertEqual(self.read(), {})
        with self.path.open("a") as output:
            output.write("\n")
        self.assertIn("Flushing UTXO", self.read()["stage"])
        self.path.rename(self.path.with_suffix(".old"))
        self.assertEqual(self.read(), {})
        self.append(MEMPOOL)
        self.assertIn("Mempool saved", self.read()["stage"])
        self.path.write_text("")
        self.assertEqual(self.read(), {})

    def test_bounded_tail_can_observe_flush_after_large_log(self):
        self.path.write_bytes(b"x" * 4096 + b"\n")
        self.append(FLUSH)
        with mock.patch.object(progress, "MAX_READ_BYTES", 512):
            self.assertIn("37,582,620 entries", self.read()["stage"])

    def test_unreadable_or_nonregular_log_is_unavailable_without_blocking(self):
        self.assertEqual(self.read(), {})
        self.path.mkdir()
        self.assertEqual(self.read(), {})
        self.path.rmdir()
        os.mkfifo(self.path)
        self.assertEqual(self.read(), {})
        self.path.unlink()
        target = self.path.with_suffix(".target")
        target.write_text(f"2026-09-30T12:01:04Z {FLUSH}\n")
        self.path.symlink_to(target)
        self.assertEqual(self.read(), {})
        self.path.unlink()
        self.append(FLUSH)
        with mock.patch.object(progress.os, "open", side_effect=PermissionError):
            self.assertEqual(self.read(), {})

    def test_shell_heartbeats_show_observation_and_age_until_safe_exit(self):
        runner = release_tests.TestnetBitcoinReleaseTests()
        result, calls = runner.run_fake_bitcoin_down(exit_code=0, slow_shutdown=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["restart-no", "rpc-stop", "down"])
        self.assertIn("elapsed=00:00:15; waiting for safe process exit", result.stderr)
        self.assertIn("elapsed=00:00:30; waiting for safe process exit", result.stderr)
        self.assertIn("Last observed stage: Flushing UTXO set to disk (37,582,620 entries", result.stderr)
        self.assertIn("No newer stage log for", result.stderr)
        self.assertIn("shutdown completed", result.stderr)
        self.assertNotIn("27 bytes", result.stderr)

    def test_observer_failure_never_aborts_waiting_or_forces_kill(self):
        runner = release_tests.TestnetBitcoinReleaseTests()
        result, calls = runner.run_fake_bitcoin_down(exit_code=0, slow_shutdown=True, observer_fails=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["restart-no", "rpc-stop", "down"])
        self.assertIn("shutdown log observation failed", result.stderr)
        self.assertIn("shutdown completed", result.stderr)


if __name__ == "__main__":
    unittest.main()
