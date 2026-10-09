"""Current-process Core warmup evidence, with bounded read-only log access."""

from datetime import datetime, timezone
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import bitcoin_import_progress as reader
import bitcoin_startup_progress as progress


class StartupProgressTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / "debug.log"
        self.start = reader.timestamp("2026-10-09T07:32:57Z")
        self.runtime = dict(state="running", details_available=True, container_id="a" * 64,
                            started_at="2026-10-09T07:32:57Z", oom_killed=False, exit_code=0)
        self.core = dict(error_kind="rpc_unavailable", rpc_available=False,
                         rpc_failure=dict(kind="warmup", code=-28))
        reader._CACHE.clear()

    def append(self, seconds, message):
        stamp = datetime.fromtimestamp(self.start + seconds, timezone.utc).isoformat()
        with self.path.open("a") as output:
            output.write(f"{stamp} {message}\n")

    def read(self, seconds=240):
        return progress.observe(self.core, self.runtime, self.path, self.start + seconds)

    def test_node1_snapshot_finalization_and_verification_timeline(self):
        self.append(1, "init message: Loading block index…")
        self.append(6, "[snapshot] computing UTXO stats for background chainstate to validate snapshot - this could take a few minutes")
        first = self.read(7)
        self.assertEqual(first["phase"], "validating_snapshot")
        self.assertNotIn("progress_percent", first)
        self.append(120, "UpdateTip: private irrelevant content")
        self.assertEqual(self.read(190), first)
        self.append(198, "[snapshot] snapshot beginning at deadbeef has been fully validated")
        self.append(201, "[snapshot] moving snapshot chainstate (/private/path) to default chainstate directory")
        moved = self.read(202)
        self.assertEqual(moved["phase"], "reinitializing_chainstate")
        self.assertEqual(moved["stage_started_at_ms"], int((self.start + 198) * 1000))
        self.assertNotIn("private", str(moved))
        self.append(218, "init message: Verifying blocks…")
        self.append(219, "Verification progress: 16%")
        self.assertEqual(self.read(219)["progress_percent"], 16)
        self.append(220, "Verification progress: 33%")
        advanced = self.read(220)
        self.append(225, "Verification progress: 33%")
        self.append(226, "Verification progress: 16%")
        self.append(227, "init message: Verifying blocks…")
        self.assertEqual(self.read(230), advanced)
        self.assertEqual(advanced["stage_started_at_ms"], int((self.start + 218) * 1000))
        self.assertEqual(advanced["last_progress_at_ms"], int((self.start + 220) * 1000))
        self.append(231, "init message: Done loading")
        self.assertEqual(self.read(), {})

    def test_missing_old_rotated_and_truncated_logs_do_not_extend_warmup(self):
        fallback = self.read()
        self.assertEqual(fallback["phase"], "starting")
        self.assertEqual(fallback["last_progress_at_ms"], int(self.start * 1000))
        self.append(-20, "init message: Loading block index…")
        self.append(500, "init message: Loading wallet…")
        self.assertEqual(self.read(), fallback)
        self.append(1, "init message: Loading block index…")
        self.assertEqual(self.read()["phase"], "loading_block_index")
        self.path.rename(self.path.with_suffix(".old"))
        self.path.touch()
        self.assertEqual(self.read(), fallback)
        self.append(2, "init message: Verifying blocks…")
        self.assertEqual(self.read()["phase"], "verifying_blocks")
        self.path.write_text("")
        self.assertEqual(self.read(), fallback)
        self.path.unlink()
        self.path.symlink_to(self.path.with_suffix(".old"))
        self.assertEqual(self.read(), fallback)

    def test_cold_tail_is_bounded_and_restart_discards_prior_run(self):
        self.append(1, "init message: Loading block index…")
        for i in range(20):
            self.append(2 + i, "Unrelated " + "x" * 100)
        with mock.patch.object(reader, "MAX_READ_BYTES", 512):
            self.assertEqual(self.read()["phase"], "starting")
        self.append(25, "init message: Loading wallet…")
        self.assertEqual(self.read()["phase"], "loading_wallet")
        self.runtime["started_at"] = datetime.fromtimestamp(self.start + 30, timezone.utc).isoformat()
        self.assertEqual(self.read()["phase"], "starting")

    def test_warmup_cannot_mask_transport_identity_or_container_failures(self):
        for kind in ("timeout", "connection", "service_unavailable", "authentication"):
            self.core["rpc_failure"]["kind"] = kind
            self.assertEqual(self.read(), {})
        self.core["rpc_failure"]["kind"] = "warmup"
        for fields in (dict(state="exited"), dict(oom_killed=True), dict(exit_code=1),
                       dict(details_available=False), dict(started_at=None)):
            with self.subTest(fields=fields), mock.patch.dict(self.runtime, fields):
                self.assertEqual(self.read(), {})
        self.core["error_kind"] = "identity_or_configuration"
        self.assertEqual(self.read(), {})

    def test_projection_strips_private_data_and_rejects_impossible_timing(self):
        value = self.read()
        self.assertEqual(progress.project({**value, "error": "secret", "path": "/private"}), value)
        self.assertEqual(progress.project({**value, "last_progress_at_ms": 1}), {})
        self.assertEqual(progress.project({**value, "phase": "arbitrary text"}), {})


if __name__ == "__main__":
    unittest.main()
