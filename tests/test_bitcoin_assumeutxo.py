#!/usr/bin/env python3
"""Exercise resumable HTTPS download and Core RPC bootstrap recovery boundaries."""

from collections import deque
from contextlib import redirect_stderr
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import bitcoin_assumeutxo as BOOT
from common.bitcoin_bootstrap import BootstrapCore
from common.snapshot_range_server import SnapshotRangeServer
from common.download_recovery import DownloadClock


class BitcoinBootstrapTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-bitcoin-bootstrap-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.payload = bytes(range(256)) * 32
        self.snapshot = BOOT.Snapshot(935000, "a" * 64, hashlib.sha256(self.payload).hexdigest(), len(self.payload))
        self.source = self.root / "snapshot.dat"
        self.state = self.root / "state"
        self.log = io.StringIO()
        context = redirect_stderr(self.log)
        context.__enter__()
        self.addCleanup(context.__exit__, None, None, None)

    def core(self):
        core = BootstrapCore(self.snapshot)
        self.addCleanup(core.close)
        return core, BOOT.Rpc(core.url, user="test", password="pass")

    def activate(self, rpc, **kwargs):
        return BOOT.activate(self.snapshot, self.source, rpc, self.state, poll_seconds=0.01, wait_seconds=2, reserve_bytes=0, **kwargs)

    def origin(self):
        origin = SnapshotRangeServer(self.root, self.payload)
        self.addCleanup(origin.close)
        patch = mock.patch.dict(os.environ, {"SSL_CERT_FILE": str(self.root / "server.crt"), "no_proxy": "127.0.0.1"})
        patch.start()
        self.addCleanup(patch.stop)
        return origin

    def test_download_resumes_interrupted_response_then_verifies_entire_file(self):
        origin = self.origin()
        origin.required_user_agent = "usdb-snapshot-verifier/1"
        url = origin.url.rsplit("/", 1)[0] + "/redirect"
        origin.plans[0] = deque(["cut-valid"])
        with mock.patch.object(BOOT.time, "sleep", side_effect=KeyboardInterrupt), self.assertRaises(KeyboardInterrupt):
            BOOT.download_snapshot(self.snapshot, self.source, url, reserve_bytes=0)
        part = self.source.with_name("snapshot.dat.download") / "snapshot.part"
        offset = part.stat().st_size
        self.assertGreater(offset, 0)
        self.assertLess(offset, len(self.payload))
        self.assertFalse(self.source.exists())
        BOOT.download_snapshot(self.snapshot, self.source, url, reserve_bytes=0)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertIn((offset, len(self.payload) - 1), origin.requests)
        self.assertFalse(part.exists())
        # A restart verifies the local file and does not require a download URL.
        BOOT.download_snapshot(self.snapshot, self.source, "", reserve_bytes=0)
        self.assertEqual(len(origin.requests), 2)

    def test_download_automatically_resumes_transport_failure(self):
        origin = self.origin()
        origin.plans[0] = deque(["cut-valid"])
        with mock.patch.object(BOOT.time, "sleep") as sleep:
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(origin.requests, [(0, len(self.payload) - 1), (len(self.payload) // 2, len(self.payload) - 1)])
        sleep.assert_called_once_with(15)
        self.assertIn('"phase": "download_retry"', self.log.getvalue())

    def test_transport_retries_pause_only_after_six_hours_without_progress(self):
        origin = self.origin()
        origin.delay.set()
        origin.plans[0] = deque(["cut-valid"])
        origin.plans[4096] = deque(["http-503"] * 100)
        clock = DownloadClock()
        with mock.patch.object(BOOT, "time", clock), self.assertRaises(BOOT.DownloadRetryExhausted):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        work = self.source.with_name("snapshot.dat.download")
        record = json.loads((work / "progress.json").read_text())
        self.assertGreater(len(origin.requests), 6)
        self.assertEqual(clock.elapsed, 6 * 60 * 60)
        self.assertEqual(clock.sleeps[:6], [15, 30, 60, 120, 240, 300])
        self.assertLessEqual(max(clock.sleeps), 300)
        self.assertEqual(record["phase"], "download_failed")
        self.assertTrue(record["details"]["retryable"])
        self.assertTrue(record["details"]["retry_exhausted"])
        self.assertIn("status 503", record["details"]["last_error"])
        self.assertEqual(record["details"]["bytes"], 4096)
        self.assertEqual((work / "snapshot.part").read_bytes(), self.payload[:4096])
        self.assertFalse(self.source.exists())
        # An explicit new invocation reopens the recovery window and resumes.
        origin.plans.clear()
        BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(origin.requests[-1][0], 4096)

    def test_advancing_download_survives_more_than_six_interruptions(self):
        origin = self.origin()
        clock = DownloadClock()
        offset = 0
        for _ in range(8):
            origin.plans[offset] = deque(["cut-valid"])
            offset += (len(self.payload) - offset) // 2
        with mock.patch.object(BOOT, "time", clock), mock.patch.object(BOOT, "DOWNLOAD_NO_PROGRESS_SECS", 40):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(len(origin.requests), 9)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertGreater(clock.elapsed, 40)
        self.assertEqual(clock.sleeps, [15] * 8)

    def test_socket_timeout_retains_received_prefix_smaller_than_read_chunk(self):
        origin = self.origin()
        origin.plans[0] = deque(["stall-valid"])
        with mock.patch.object(BOOT, "time", DownloadClock()), \
             mock.patch.object(BOOT, "DOWNLOAD_TIMEOUT_SECS", 0.5):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(origin.requests, [(0, 8191), (4096, 8191)])
        self.assertIn("Snapshot transport interrupted (timeout)", self.log.getvalue())

    def test_backoff_and_last_error_remain_observable_during_recovery(self):
        origin = self.origin()
        origin.plans[0] = deque(["http-503", "http-503", "http-503", "http-503"])
        clock, records = DownloadClock(), []
        progress = self.source.with_name("snapshot.dat.download") / "progress.json"
        clock.on_sleep = lambda: records.append(json.loads(progress.read_text()))
        with mock.patch.object(BOOT, "time", clock):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(clock.sleeps, [15, 30, 60, 120])
        for attempt, record in enumerate(records, 1):
            details = record["details"]
            self.assertEqual(record["phase"], "download_retry")
            self.assertEqual(details["attempt"], attempt)
            self.assertEqual(details["retry_count"], attempt - 1)
            self.assertEqual(details["next_retry_at"], record["updated_at"] + details["retry_delay_seconds"])
            self.assertIn("status 503", details["last_error"])
        events = [json.loads(line.split(": ", 1)[1]) for line in self.log.getvalue().splitlines()
                  if line.startswith("Bitcoin bootstrap progress: ")]
        resumed = [event for event in events if event["phase"] == "downloading" and event["attempt"] == 5]
        self.assertEqual(resumed[0]["retry_count"], 4)
        self.assertIn("status 503", resumed[0]["last_error"])
        self.assertNotIn("next_retry_at", resumed[0])
        self.assertEqual(self.source.read_bytes(), self.payload)

    def test_transport_error_categories_preserve_security_failures(self):
        cases = [(TimeoutError("secret"), True, "timeout"),
                 (ConnectionResetError("secret"), True, "connection reset"),
                 (BOOT.socket.gaierror(-3, "secret"), True, "DNS lookup failed"),
                 (BOOT.ssl.SSLEOFError("secret"), True, "connection closed"),
                 (BOOT.ssl.SSLCertVerificationError("secret"), False, "TLS"),
                 (BOOT.ssl.SSLError("secret"), False, "TLS")]
        for error, retryable, text in cases:
            with self.subTest(kind=type(error).__name__):
                failure = BOOT.download_transport_error(BOOT.urllib.error.URLError(error))
                self.assertEqual(isinstance(failure, BOOT.DownloadInterrupted), retryable)
                self.assertIn(text, str(failure))
                self.assertNotIn("secret", str(failure))

    def test_writeback_is_bounded_by_bytes_even_without_elapsed_time(self):
        origin = self.origin()
        windows = []
        flush = BOOT.flush_download

        def observe(output, start, end):
            windows.append(end - start)
            flush(output, start, end)

        with mock.patch.object(BOOT, "DOWNLOAD_FLUSH_BYTES", 2048), \
             mock.patch.object(BOOT, "CHUNK_BYTES", 1024), \
             mock.patch.object(BOOT, "flush_download", side_effect=observe), \
             mock.patch.object(BOOT.time, "monotonic", return_value=1):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(sum(windows), len(self.payload))
        self.assertLessEqual(max(windows), 2048)

    def test_disk_errors_do_not_trigger_transport_retries(self):
        origin = self.origin()
        with mock.patch.object(BOOT, "flush_download", side_effect=OSError(28, "disk full")), \
             mock.patch.object(BOOT.time, "sleep") as sleep, self.assertRaises(OSError):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=0)
        sleep.assert_not_called()
        record = json.loads((self.source.with_name("snapshot.dat.download") / "progress.json").read_text())
        self.assertFalse(record["details"]["retryable"])
        self.assertIn("errno 28", record["details"]["error"])

    def test_permanent_http_and_tls_failures_are_not_retryable(self):
        for error in (BOOT.urllib.error.HTTPError("https://secret", 403, "secret", {}, None),
                      BOOT.urllib.error.URLError(BOOT.ssl.SSLCertVerificationError("secret"))):
            with self.subTest(error=type(error).__name__):
                failure = BOOT.download_transport_error(error)
                self.assertNotIsInstance(failure, BOOT.DownloadInterrupted)
                self.assertNotIn("secret", str(failure))

    def test_wrong_ranges_encodings_and_download_hash_never_publish(self):
        origin = self.origin()
        for mode in ("wrong-range", "wrong-total", "duplicate-range", "encoded", "cut"):
            with self.subTest(mode=mode):
                target = self.root / f"{mode}.dat"
                origin.plans[0] = deque([mode])
                with mock.patch.object(BOOT, "time", DownloadClock()), self.assertRaises(ValueError):
                    BOOT.download_snapshot(self.snapshot, target, origin.url, reserve_bytes=0)
                self.assertFalse(target.exists())
        bad = self.root / "bad.dat"
        bad.write_bytes(b"x" * len(self.payload))
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            BOOT.download_snapshot(self.snapshot, bad, "", reserve_bytes=0)
        self.assertEqual(bad.read_bytes(), b"x" * len(self.payload))

    def test_disk_reserve_and_identity_mismatch_block_before_download(self):
        origin = self.origin()
        with self.assertRaisesRegex(ValueError, "Insufficient"):
            BOOT.download_snapshot(self.snapshot, self.source, origin.url, reserve_bytes=2**63)
        self.assertEqual(origin.requests, [])
        altered = BOOT.Snapshot(935000, "b" * 64, self.snapshot.file_sha256, len(self.payload))
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            BOOT.download_snapshot(altered, self.source, origin.url, reserve_bytes=0)
        self.assertEqual(origin.requests, [])

    def test_initial_load_waits_for_headers_and_recovers_without_local_marker(self):
        core, rpc = self.core()
        self.source.write_bytes(self.payload)
        core.header_delay = 2
        report = self.activate(rpc)
        self.assertTrue(report["bootstrap_ready"])
        self.assertTrue(report["snapshot_active"])
        self.assertFalse(report["history_validated"])
        self.assertEqual(report["background_height"], 12)
        self.assertEqual(core.calls["loadtxoutset"], 1)
        self.assertGreaterEqual(core.calls["getblockheader"], 3)
        (self.state / "activation.json").unlink()
        self.source.unlink()
        self.assertTrue(self.activate(rpc)["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 1)

    def test_fully_validated_single_chainstate_is_reused_without_snapshot(self):
        core, rpc = self.core()
        core.active, core.validated, core.snapshot_hash = True, True, None
        report = self.activate(rpc)
        self.assertEqual(report["phase"], "fully_validated_chain")
        self.assertTrue(report["history_validated"])
        self.assertFalse(report["snapshot_active"])
        self.assertEqual(core.calls["loadtxoutset"], 0)
        self.assertFalse(self.source.exists())

    def test_foreground_readiness_is_independent_of_background_and_indexes(self):
        core, rpc = self.core()
        core.active = True
        core.height = core.headers = core.origin_height + 2
        env = dict(BTC_MIN_READY_HEIGHT=str(core.origin_height), BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH=core.origin_hash)
        report = BOOT.tip_status(rpc, self.snapshot, env)
        self.assertTrue(report["tip_ready"])
        self.assertFalse(report["history_validated"])
        self.assertEqual(report["background_height"], 12)
        self.assertEqual(core.calls["getindexinfo"], 0)
        for field, value in (("connections", 0), ("tip_time", 0), ("headers", core.height + 1), ("height", core.origin_height - 1)):
            previous = getattr(core, field)
            setattr(core, field, value)
            with self.subTest(field=field):
                self.assertFalse(BOOT.tip_status(rpc, self.snapshot, env)["tip_ready"])
            setattr(core, field, previous)
        core.origin_hash = "d" * 64
        with self.assertRaisesRegex(ValueError, "origin hash mismatch"):
            BOOT.tip_status(rpc, self.snapshot, env)

    def test_node_preparation_still_verifies_file_when_core_is_already_validated(self):
        core, rpc = self.core()
        core.active, core.validated, core.snapshot_hash = True, True, None
        self.source.write_bytes(b"x" * len(self.payload))
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            self.activate(rpc, ensure_snapshot_file=True)
        self.source.write_bytes(self.payload)
        self.assertTrue(self.activate(rpc, ensure_snapshot_file=True)["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_restart_reuses_active_baseline_without_reading_or_loading_source(self):
        core, rpc = self.core()
        self.source.write_bytes(self.payload)
        with mock.patch.object(BOOT, "verify_snapshot", wraps=BOOT.verify_snapshot) as verify:
            self.assertTrue(self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)["bootstrap_ready"])
            self.assertEqual(verify.call_count, 1)
        initial_loads = core.calls["loadtxoutset"]
        for validated in (False, True):
            core.validated = validated
            with self.subTest(history_validated=validated), \
                    mock.patch.object(BOOT, "download_snapshot", side_effect=AssertionError("Unexpected source scan")):
                report = self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
            self.assertTrue(report["snapshot_file_reused"])
            self.assertEqual(report["history_validated"], validated)
            self.assertEqual(core.calls["loadtxoutset"], initial_loads)
            saved = json.loads((self.state / "activation.json").read_text())
            self.assertTrue(saved["details"]["report"]["snapshot_file_reused"])

    def test_fresh_bootstrap_journal_reuses_retained_core_and_snapshot(self):
        core, rpc = self.core()
        core.active = True
        self.source.write_bytes(self.payload)
        # Uninstall may remove the network-local activation journal while
        # retaining both Core chainstates and the raw file needed by fresh BH.
        for validated in (False, True):
            core.validated = validated
            self.state = self.root / ("fresh-validated" if validated else "fresh-background")
            self.assertFalse(self.state.exists())
            with self.subTest(history_validated=validated), \
                    mock.patch.object(BOOT, "download_snapshot", side_effect=AssertionError("Unexpected download or file scan")):
                report = self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
            self.assertTrue(report["snapshot_file_reused"])
            self.assertTrue(report["bootstrap_ready"])
            self.assertEqual(report["history_validated"], validated)
            self.assertTrue((self.state / "activation.json").exists())
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_reuse_policy_never_skips_verification_before_a_new_core_import(self):
        core, rpc = self.core()
        self.source.write_bytes(b"x" * len(self.payload))
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_reuse_requires_a_live_matching_core_baseline_not_old_journals(self):
        core, rpc = self.core()
        core.active = True
        self.source.write_bytes(self.payload)
        self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        core.canonical_hash = "d" * 64
        with mock.patch.object(BOOT, "download_snapshot", side_effect=AssertionError("Source preparation before identity check")), \
                self.assertRaisesRegex(ValueError, "canonical baseline"):
            self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        core.canonical_hash = self.snapshot.base_hash
        core.active = False
        self.source.write_bytes(b"x" * len(self.payload))
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_missing_source_is_downloaded_and_verified_for_downstream_import(self):
        core, rpc = self.core()
        core.active = True
        origin = self.origin()
        with mock.patch.object(BOOT, "verify_snapshot", wraps=BOOT.verify_snapshot) as verify:
            report = self.activate(rpc, url=origin.url, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
            self.assertEqual(verify.call_count, 1)
        self.assertTrue(report["bootstrap_ready"])
        self.assertNotIn("snapshot_file_reused", report)
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_reused_file_has_only_metadata_checks_consumers_own_content_validation(self):
        core, rpc = self.core()
        core.active = True
        # A running Core baseline authenticates its database, not these bytes.
        # Native BH's scan_snapshot verifies both hashes if it still imports them.
        self.source.write_bytes(b"x" * len(self.payload))
        with mock.patch.object(BOOT, "verify_snapshot", side_effect=AssertionError("Unexpected hash scan")):
            report = self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        self.assertTrue(report["snapshot_file_reused"])
        self.source.write_bytes(b"short")
        with self.assertRaisesRegex(ValueError, "file size mismatch"):
            self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        self.source.unlink()
        self.source.symlink_to(self.root / "missing-source")
        with self.assertRaisesRegex(ValueError, "regular file"):
            self.activate(rpc, ensure_snapshot_file=True, reuse_active_snapshot_file=True)
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_reuse_mode_requires_explicit_downstream_file_preparation(self):
        core, rpc = self.core()
        with self.assertRaisesRegex(ValueError, "requires downstream"):
            self.activate(rpc, reuse_active_snapshot_file=True)
        self.assertEqual(core.calls, {})

    def test_wrong_network_pruning_or_baseline_never_loads(self):
        core, rpc = self.core()
        for attribute, value in (("chain", "test"), ("pruned", True), ("version", 280100)):
            previous = getattr(core, attribute)
            setattr(core, attribute, value)
            with self.subTest(attribute=attribute), self.assertRaises(ValueError):
                self.activate(rpc)
            setattr(core, attribute, previous)
        core.active = True
        core.snapshot_hash = "b" * 64
        with self.assertRaisesRegex(ValueError, "different AssumeUTXO"):
            self.activate(rpc)
        core.snapshot_hash = self.snapshot.base_hash
        core.canonical_hash = "c" * 64
        with self.assertRaisesRegex(ValueError, "canonical baseline"):
            self.activate(rpc)
        self.assertEqual(core.calls["loadtxoutset"], 0)

    def test_lost_load_response_is_reconciled_from_core(self):
        core, rpc = self.core()
        core.drop_reply = True
        self.source.write_bytes(self.payload)
        self.assertTrue(self.activate(rpc)["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 1)

    def test_unknown_request_outcome_requires_explicit_retry(self):
        core, rpc = self.core()
        core.drop_before_activation = True
        self.source.write_bytes(self.payload)
        with self.assertRaisesRegex(ValueError, "connection"):
            self.activate(rpc)
        core.warmup_remaining = 2
        with self.assertRaisesRegex(ValueError, "retry-interrupted-load"):
            self.activate(rpc)
        self.assertEqual(core.calls["loadtxoutset"], 1)
        core.drop_before_activation = False
        self.assertTrue(self.activate(rpc, retry_interrupted=True)["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 2)

    def test_observer_timeout_before_load_can_retry_normally(self):
        core, rpc = self.core()
        self.source.write_bytes(self.payload)
        clock = mock.Mock(wraps=time)
        clock.monotonic.return_value = 0
        download = BOOT.download_snapshot

        def prepare(*args, **kwargs):
            download(*args, **kwargs)
            clock.monotonic.return_value = 2

        # Exhaust the deadline during preparation, before any load can be sent.
        with mock.patch.object(BOOT, "time", clock), mock.patch.object(BOOT, "download_snapshot", side_effect=prepare):
            with self.assertRaisesRegex(ValueError, "deadline"):
                BOOT.activate(self.snapshot, self.source, rpc, self.state, poll_seconds=0.01, wait_seconds=1, reserve_bytes=0)
        self.assertFalse(core.started_load.is_set())
        self.assertEqual(core.calls["loadtxoutset"], 0)
        self.assertTrue(self.activate(rpc)["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 1)

    def test_observer_timeout_does_not_duplicate_an_active_load(self):
        core, rpc = self.core()
        core.release_load.clear()
        self.source.write_bytes(self.payload)
        wait_seconds = 30
        clock = mock.Mock(wraps=time)
        # Trigger the deadline only after Core acknowledges the load. Real time
        # remains a watchdog if startup breaks; it does not order the scenario.
        clock.monotonic.side_effect = lambda: time.monotonic() + (wait_seconds if core.started_load.is_set() else 0)
        with mock.patch.object(BOOT, "time", clock), self.assertRaisesRegex(ValueError, "deadline"):
            BOOT.activate(self.snapshot, self.source, rpc, self.state, poll_seconds=0.01, wait_seconds=wait_seconds, reserve_bytes=0)
        self.assertTrue(core.started_load.is_set())
        self.assertTrue(core.loading)
        self.assertEqual(core.calls["loadtxoutset"], 1)
        call = rpc.call
        active_observations = []

        def observe_then_release(method, *args, **kwargs):
            result = call(method, *args, **kwargs)
            if method == "getrpcinfo" and any(command["method"] == "loadtxoutset" for command in result["active_commands"]):
                # Keep the import pending until the resumed observer sees it.
                active_observations.append(result)
                core.release_load.set()
            return result

        with mock.patch.object(rpc, "call", side_effect=observe_then_release):
            report = BOOT.activate(self.snapshot, self.source, rpc, self.state, poll_seconds=0.01, wait_seconds=wait_seconds, reserve_bytes=0)
        self.assertTrue(active_observations)
        self.assertTrue(report["bootstrap_ready"])
        self.assertEqual(core.calls["loadtxoutset"], 1)

    def test_explicit_core_error_retains_code_without_sensitive_message(self):
        core, rpc = self.core()
        core.reject_code = -32603
        self.source.write_bytes(self.payload)
        with self.assertRaisesRegex(ValueError, "code -32603") as raised:
            self.activate(rpc)
        self.assertNotIn("Sensitive", str(raised.exception) + self.log.getvalue())
        self.assertEqual(json.loads((self.state / "activation.json").read_text())["phase"], "load_failed")

    def test_concurrent_operator_is_rejected_without_touching_core(self):
        core, rpc = self.core()
        with BOOT.exclusive_directory(self.state):
            with self.assertRaisesRegex(ValueError, "Another bootstrap"):
                self.activate(rpc)
        self.assertEqual(core.calls, {})

    def test_invalid_authentication_or_wait_policy_fails_before_rpc(self):
        with self.assertRaisesRegex(ValueError, "authentication"):
            BOOT.Rpc("http://127.0.0.1:1")
        core, rpc = self.core()
        for interval in (0, float("nan"), float("inf")):
            with self.subTest(interval=interval), self.assertRaises(ValueError):
                BOOT.activate(self.snapshot, self.source, rpc, self.state, poll_seconds=interval)
        self.assertEqual(core.calls, {})

    def test_rpc_redirect_cannot_forward_authentication(self):
        core, rpc = self.core()
        target, _ = self.core()
        core.redirect = target.url
        with self.assertRaises(BOOT.RpcFailure):
            rpc.call("getnetworkinfo")
        self.assertEqual(target.calls, {})

    def test_cli_reports_pinned_baseline_readiness_and_rejects_other_networks(self):
        self.snapshot = BOOT.pinned_snapshot({})
        core, _ = self.core()
        core.active = True
        env = {"PATH": os.environ["PATH"], "BTC_RPC_URL": core.url, "BTC_RPC_USER": "test", "BTC_RPC_PASSWORD": "pass",
               "BTC_NETWORK": "bitcoin", "PYTHONDONTWRITEBYTECODE": "1"}
        command = [sys.executable, str(ROOT / "docker/scripts/tools/bitcoin_assumeutxo.py"), "status"]
        core.warmup_remaining = 1
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["bootstrap_ready"])
        self.assertEqual(json.loads(result.stdout)["rpc_probe"]["retries"], 1)
        self.assertEqual(core.calls["loadtxoutset"], 0)
        # A transport recovery cannot override a mismatched baseline identity.
        core.canonical_hash = "d" * 64
        core.warmup_remaining = 1
        rejected = subprocess.run(command, env=env, capture_output=True, text=True, timeout=5)
        self.assertEqual(rejected.returncode, 1)
        self.assertEqual(json.loads(rejected.stdout)["error_kind"], "identity_or_configuration")
        self.assertFalse(json.loads(rejected.stdout)["bootstrap_ready"])
        result = subprocess.run(command, env={**env, "BTC_NETWORK": "regtest"}, capture_output=True, text=True, timeout=5)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(json.loads(result.stdout)["bootstrap_ready"])


if __name__ == "__main__":
    unittest.main()
