#!/usr/bin/env python3
"""Check visible waiting, HTTP retries and failed-download isolation with real curl."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest

from common.installer_http import InstallerHTTPServer


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "docker/scripts/tools/install_usdb_node.sh"
RELEASE_ID = "usdb-testnet-v0-r1"
MANIFEST = "usdb-release-manifest.json"


class InstallerProgressTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-installer-progress-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        manifest = json.dumps({"release_id": RELEASE_ID}).encode()
        (self.assets / MANIFEST).write_bytes(manifest)
        (self.assets / (MANIFEST + ".sha256")).write_text(
            f"{hashlib.sha256(manifest).hexdigest()}  {MANIFEST}\n")
        # Respect neither real proxies nor user curl configuration in network fixtures.
        self.env = {**os.environ, "CURL_HOME": str(self.root), "NO_PROXY": "127.0.0.1",
                    "no_proxy": "127.0.0.1", "LC_ALL": "C"}

    def command(self, url):
        return ["bash", str(INSTALLER), "--release-id", RELEASE_ID, "--release-base-url", url,
                "--install-root", str(self.root / "releases"), "--bin-dir", str(self.root / "bin")]

    def assert_no_install(self):
        self.assertFalse((self.root / "bin/usdb-node").exists())
        self.assertEqual(list((self.root / "releases").iterdir()), [])

    def test_progress_is_visible_before_server_sends_any_bytes(self):
        entered = threading.Event()
        release = threading.Event()

        def hold_response(path, count):
            entered.set()
            release.wait(timeout=10)
            return 404

        with InstallerHTTPServer(self.assets, hold_response) as server, \
             (self.root / "stderr.log").open("w+") as log:
            process = subprocess.Popen(self.command(server.url), env=self.env, stdout=subprocess.DEVNULL, stderr=log)
            try:
                self.assertTrue(entered.wait(timeout=5), "installer did not request its manifest")
                deadline = time.monotonic() + 5
                while True:
                    output = (self.root / "stderr.log").read_text()
                    if "Average Speed" in output or time.monotonic() >= deadline:
                        break
                    time.sleep(0.02)
                self.assertIsNone(process.poll())
                self.assertIn("[1/5]", output)
                self.assertIn("Downloading " + MANIFEST, output)
                self.assertIn("Average Speed", output)
                self.assertNotIn("Downloaded " + MANIFEST, output)
                release.set()
                self.assertNotEqual(process.wait(timeout=10), 0)
            finally:
                release.set()
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
            log.seek(0)
            self.assertIn("Download failed: " + MANIFEST, log.read())
        self.assert_no_install()

    def test_transient_http_failure_retries_and_then_verifies_manifest(self):
        def fail_once(path, count):
            return 503 if path == "/" + MANIFEST and count == 1 else None

        with InstallerHTTPServer(self.assets, fail_once) as server:
            result = subprocess.run(self.command(server.url), env=self.env, capture_output=True, text=True, timeout=20)
            self.assertEqual(server.requests["/" + MANIFEST], 2)
            self.assertEqual(server.requests[f"/{RELEASE_ID}-node-kit.tar.gz"], 1)
        # The fixture intentionally has no archive: a retry must not turn a later 404 into success.
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Will retry", result.stderr)
        self.assertIn("[2/5] Verifying release manifest", result.stderr)
        self.assertIn(f"Download failed: {RELEASE_ID}-node-kit.tar.gz", result.stderr)
        self.assert_no_install()

    def test_invalid_manifest_stops_before_downloading_archive(self):
        (self.assets / MANIFEST).write_text("tampered")
        with InstallerHTTPServer(self.assets) as server:
            result = subprocess.run(self.command(server.url), env=self.env, capture_output=True, text=True, timeout=10)
            self.assertEqual(server.requests[f"/{RELEASE_ID}-node-kit.tar.gz"], 0)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Checksum mismatch", result.stderr)
        self.assertNotIn("[3/5]", result.stderr)
        self.assert_no_install()


if __name__ == "__main__":
    unittest.main()
