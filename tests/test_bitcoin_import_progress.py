#!/usr/bin/env python3
"""Check Core log progress across flushes, restarts and bounded/partial log reads."""

from datetime import datetime, timezone
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import bitcoin_import_progress as progress

BASE = "a" * 64
START = datetime(2026, 9, 14, 8, 30, tzinfo=timezone.utc).timestamp()


class BitcoinImportProgressTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-core-import-log-")
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "debug.log"
        progress._CACHE.clear()

    def append(self, text, second=1):
        stamp = datetime.fromtimestamp(START + second, timezone.utc).isoformat().replace("+00:00", "Z")
        with self.path.open("a") as output:
            output.write(f"{stamp} {text}\n")

    def read(self, since=START):
        return progress.read_import_progress(self.path, BASE, since)

    def test_import_flush_and_hash_phases_preserve_counts_without_declaring_ready(self):
        self.append(f"[snapshot] loading 2000000 coins from snapshot {BASE}")
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)", 2)
        self.assertEqual(self.read()["progress_percent"], 50)
        self.append("FlushSnapshotToDisk: flushing coins cache (120 MB) started", 3)
        self.assertEqual(self.read()["phase"], "flushing_cache")
        self.append("FlushSnapshotToDisk: flushing coins cache (120 MB) completed (50ms)", 4)
        self.assertEqual(self.read()["phase"], "reading")
        self.append(f"[snapshot] loaded 2000000 (120 MB) coins from snapshot {BASE}", 5)
        report = self.read()
        self.assertEqual(report["imported_coins"], 2000000)
        self.assertEqual(report["phase"], "flushing")
        self.assertIsNone(report["progress_percent"])
        self.append("FlushSnapshotToDisk: saving snapshot chainstate (120 MB) completed (100ms)", 6)
        self.assertEqual(self.read()["phase"], "verifying")
        self.append("[snapshot] validated snapshot (0 MB)", 7)
        self.append(f"[snapshot] successfully activated snapshot {BASE}", 8)
        self.assertEqual(self.read()["phase"], "activating")
        self.assertNotIn("bootstrap_ready", self.read())

    def test_watch_retains_progress_while_unrelated_block_logs_advance(self):
        with mock.patch.object(progress, "MAX_READ_BYTES", 512):
            self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)")
            self.assertEqual(self.read()["imported_coins"], 1000000)
            for _ in range(10):
                self.append("UpdateTip: " + "x" * 100)
                self.assertEqual(self.read()["progress_percent"], 50)
            # A fresh observer with no recent snapshot log must not invent a percentage.
            progress._CACHE.clear()
            self.assertEqual(self.read(), {})

    def test_partial_log_line_is_only_applied_once_complete(self):
        self.path.write_text("2026-09-14T08:30:01Z [snapshot] 1000000 coins")
        self.assertEqual(self.read(), {})
        with self.path.open("a") as output:
            output.write(" loaded (50.00%, 120 MB)\n")
        self.assertEqual(self.read()["progress_percent"], 50)

    def test_process_restart_and_new_attempt_cannot_reuse_old_progress(self):
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)", 1)
        self.assertEqual(self.read()["progress_percent"], 50)
        self.assertEqual(self.read(since=START + 5), {})
        self.append(f"[snapshot] loading 2000000 coins from snapshot {BASE}", 6)
        self.assertEqual(self.read(since=START + 5)["imported_coins"], 0)

    def test_rotation_truncation_and_unreadable_logs_fall_back(self):
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)")
        self.assertTrue(self.read())
        self.path.rename(self.path.with_suffix(".old"))
        self.path.touch()
        self.assertEqual(self.read(), {})
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)")
        self.assertTrue(self.read())
        self.path.write_text("")
        self.assertEqual(self.read(), {})
        self.path.unlink()
        self.assertEqual(self.read(), {})
        self.path.symlink_to(self.path.with_suffix(".old"))
        self.assertEqual(self.read(), {})

    def test_other_baseline_and_malformed_records_do_not_supply_progress(self):
        self.append(f"[snapshot] loading 2000000 coins from snapshot {'b' * 64}")
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)")
        self.assertEqual(self.read(), {})
        self.append(f"[snapshot] loading 2000000 coins from snapshot {BASE}")
        self.append("[snapshot] 3000000 coins loaded (150.00%, 120 MB)")
        self.assertEqual(self.read()["imported_coins"], 0)
        self.assertEqual(self.read(since=None), {})
        self.assertEqual(self.read(since=float("nan")), {})

    def test_cold_read_is_bounded_and_accepts_recent_progress_without_start_line(self):
        with self.path.open("wb") as output:
            output.truncate(8192)
            output.seek(8192)
            output.write(b"\n")
        self.append("[snapshot] 1000000 coins loaded (50.00%, 120 MB)")
        with mock.patch.object(progress, "MAX_READ_BYTES", 512):
            report = self.read()
        self.assertEqual(report["progress_percent"], 50)
        self.assertNotIn("total_coins", report)


if __name__ == "__main__":
    unittest.main()
