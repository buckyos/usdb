#!/usr/bin/env python3
"""Check direct range writes, durable resume, and transport failures against real HTTPS/curl."""

import collections
import errno
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
from common.snapshot_range_server import SnapshotRangeServer  # noqa: E402
import snapshot_distribution as D  # noqa: E402

MIB = 1024 * 1024


class SnapshotRangeDownloadTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="snapshot-direct-ranges-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = b"".join(bytes([n + 1]) * MIB for n in range(4)) + b"last short range"
        self.digest = hashlib.sha256(self.payload).hexdigest()
        self.destination = self.root / "snapshot.db.part"
        self.state_path, self.work_dir = D._range_download_paths(self.destination)
        self.server = SnapshotRangeServer(self.root, self.payload)
        self.addCleanup(self.server.close)

    def download(self, **overrides):
        arguments = dict(
            expected_size=len(self.payload), expected_sha256=self.digest,
            concurrency=3, chunk_size=MIB, curl_executable=str(self.server.curl),
        )
        arguments.update(overrides)
        D._download_parallel_ranges(self.server.url, self.destination, **arguments)

    def seed_resume(self, completed):
        with self.destination.open("wb") as output:
            output.truncate(len(self.payload))
            for index in completed:
                output.seek(index * MIB)
                output.write(self.payload[index * MIB:(index + 1) * MIB])
        self.state_path.write_text(json.dumps({
            "version": 1, "url_sha256": hashlib.sha256(self.server.url.encode()).hexdigest(),
            "expected_size": len(self.payload), "expected_sha256": self.digest,
            "chunk_size_bytes": MIB, "completed_chunks": completed,
        }))

    def test_parallel_direct_writes_reserve_space_and_write_payload_once(self):
        writes = []
        lock = threading.Lock()
        pwrite = D._pwrite_all

        def record(fd, data, offset):
            # All blocks must already be reserved before any network payload is written.
            self.assertGreaterEqual(os.fstat(fd).st_blocks * 512, len(self.payload))
            self.assertFalse(list(self.work_dir.glob("*.part")))
            with lock:
                writes.append((offset, len(data)))
            pwrite(fd, data, offset)

        with mock.patch.object(D, "_pwrite_all", side_effect=record):
            self.download()
        self.assertEqual(self.destination.read_bytes(), self.payload)
        self.assertEqual(sum(length for _, length in writes), len(self.payload))
        self.assertLessEqual(max(length for _, length in writes), D.RANGE_READ_BUFFER_SIZE)
        self.assertGreaterEqual(self.server.max_active, 2)
        self.assertEqual(json.loads(self.state_path.read_text())["completed_chunks"], list(range(5)))
        self.assertEqual(list(self.work_dir.iterdir()), [])
        D._verify_download(self.destination, len(self.payload), self.digest, progress_label="test")

    def test_resume_preserves_old_chunks_and_reserves_sparse_holes_even_with_empty_checkpoint(self):
        for completed in ([0, 2], []):
            with self.subTest(completed=completed):
                self.seed_resume(completed)
                self.server.requests.clear()
                reserve = D._reserve_download_space
                reservations = []

                def record(fd, size):
                    self.assertEqual(self.server.requests, [])
                    reservations.append(size)
                    reserve(fd, size)

                with mock.patch.object(D, "_reserve_download_space", side_effect=record):
                    self.download(concurrency=2, chunk_size=2 * MIB)
                self.assertEqual(reservations, [len(self.payload)])
                self.assertEqual(sorted(start // MIB for start, _ in self.server.requests),
                                 [n for n in range(5) if n not in completed])
                self.assertEqual(self.destination.read_bytes(), self.payload)

    def test_interrupted_stream_retries_from_range_start(self):
        self.server.plans[MIB] = collections.deque(["cut", "ok"])
        with mock.patch.object(D.time, "sleep"):
            self.download()
        self.assertEqual(self.server.requests.count((MIB, 2 * MIB - 1)), 2)
        self.assertEqual(self.destination.read_bytes(), self.payload)

    def test_https_redirect_uses_final_response_headers(self):
        with self.destination.open("wb") as output:
            output.truncate(len(self.payload))
        self.work_dir.mkdir()
        fd = os.open(self.destination, os.O_RDWR)
        self.addCleanup(os.close, fd)
        result = D._download_range_chunk(
            url=self.server.url.replace("/artifact", "/redirect"), index=1, chunk_size=MIB,
            expected_size=len(self.payload), work_dir=self.work_dir,
            destination_descriptor=fd, curl_executable=str(self.server.curl),
        )
        self.assertEqual(result, (1, MIB))
        self.assertEqual(os.pread(fd, MIB, MIB), self.payload[MIB:2 * MIB])

    def test_invalid_responses_never_overwrite_neighboring_ranges(self):
        self.work_dir.mkdir()
        for mode in ("status-200", "wrong-range", "wrong-total", "duplicate-range", "encoded", "oversize"):
            with self.subTest(mode=mode):
                original = b"Z" * len(self.payload)
                self.destination.write_bytes(original)
                self.server.plans[MIB] = collections.deque([mode])
                fd = os.open(self.destination, os.O_RDWR)
                try:
                    with mock.patch.object(D, "RANGE_DOWNLOAD_ATTEMPTS", 1), self.assertRaises(ValueError):
                        D._download_range_chunk(
                            url=self.server.url, index=1, chunk_size=MIB,
                            expected_size=len(self.payload), work_dir=self.work_dir,
                            destination_descriptor=fd, curl_executable=str(self.server.curl),
                        )
                finally:
                    os.close(fd)
                actual = self.destination.read_bytes()
                self.assertEqual(actual[:MIB], original[:MIB])
                self.assertEqual(actual[2 * MIB:], original[2 * MIB:])
                self.assertFalse(self.state_path.exists())

    def test_no_space_fails_before_network_and_preserves_resume(self):
        for resuming in (False, True):
            with self.subTest(resuming=resuming):
                if resuming:
                    self.seed_resume([0])
                    checkpoint = self.state_path.read_bytes()
                with mock.patch.object(D.os, "posix_fallocate", side_effect=OSError(errno.ENOSPC, "full")), \
                     self.assertRaisesRegex(ValueError, "failed to reserve"):
                    self.download()
                self.assertEqual(self.server.requests, [])
                if resuming:
                    self.assertEqual(self.state_path.read_bytes(), checkpoint)
                    self.assertEqual(self.destination.read_bytes()[:MIB], self.payload[:MIB])
                else:
                    self.assertFalse(self.state_path.exists())

    def test_failed_data_fsync_never_advances_checkpoint_and_restart_redownloads(self):
        fsync = D.os.fsync
        target_syncs = 0

        def fail_checkpoint(fd):
            nonlocal target_syncs
            if os.fstat(fd).st_ino == self.destination.stat().st_ino:
                target_syncs += 1
                if target_syncs > 1:
                    raise OSError(errno.EIO, "simulated data flush failure")
            return fsync(fd)

        with mock.patch.object(D.os, "fsync", side_effect=fail_checkpoint), self.assertRaises(OSError):
            self.download()
        self.assertEqual(json.loads(self.state_path.read_text())["completed_chunks"], [])
        self.server.requests.clear()
        self.download()
        self.assertEqual(len(self.server.requests), 5)
        self.assertEqual(self.destination.read_bytes(), self.payload)

    def test_final_sha256_remains_required_after_complete_ranges(self):
        # Correct ranges and lengths cannot prove that the server returned the signed bytes.
        self.server.payload = b"X" * len(self.payload)
        self.download()
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            D._verify_download(self.destination, len(self.payload), self.digest, progress_label="test")


if __name__ == "__main__":
    unittest.main()
