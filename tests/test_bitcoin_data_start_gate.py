#!/usr/bin/env python3
"""Exercise the real validator/helper/controller boundary with a fake Bitcoin RPC process."""

import io
import json
import os
from pathlib import Path
import sys
from contextlib import redirect_stderr
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))

import usdb_node as NODE  # noqa: E402
import test_validate_network_bundle as BUNDLE_TESTS  # noqa: E402


class BitcoinDataStartGateTests(unittest.TestCase):
    def setUp(self):
        # Reuse the validator's isolated bundle and authenticated node fixtures.
        fixture = BUNDLE_TESTS.NetworkBundleValidatorTests()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        fixture.write_rpcauth()
        self.layout = SimpleNamespace(
            kit_root=ROOT,
            bundle_dir=fixture.bundle,
            bundle_id="usdb-testnet-v0",
            node_env=fixture.write_node_env(),
        )
        fake_bin = fixture.root / "bin"
        fake_bin.mkdir()
        self.calls = fixture.root / "docker-calls.json"
        docker = fake_bin / "docker"
        docker.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "from pathlib import Path\n"
            "assert 'exec' in sys.argv and '--data-start' in sys.argv\n"
            "assert '--status-json' in sys.argv\n"
            "Path(os.environ['FAKE_DOCKER_CALLS']).write_text(json.dumps(sys.argv[1:]))\n"
            "print(os.environ['FAKE_READINESS_REPORT'])\n",
            encoding="utf-8",
        )
        docker.chmod(0o755)
        self.anchor = NODE.BitcoinDataStartAnchor(963800, 963810, 10, "a" * 64)
        patcher = mock.patch.dict(os.environ, {
            "PATH": f"{fake_bin}:{os.environ['PATH']}",
            "FAKE_DOCKER_CALLS": str(self.calls),
        })
        patcher.start()
        self.addCleanup(patcher.stop)

    def test_data_progress_is_one_document_and_controller_observes_readiness(self):
        for ready in (True, False):
            with self.subTest(ready=ready):
                report = {"schema_version": "usdb-bitcoin-data-start-readiness:v1",
                          "ready": ready, "blockers": [] if ready else ["anchor mismatch"]}
                with mock.patch.dict(os.environ, {"FAKE_READINESS_REPORT": json.dumps(report)}):
                    result = NODE.run_helper(
                        self.layout, "run_testnet_bitcoin.sh",
                        ["data-progress", "963810", "963800", self.anchor.block_hash],
                        capture_output=True, command_timeout_secs=10,
                    )
                    self.assertEqual(json.loads(result.stdout), report)
                    self.assertTrue(json.loads(result.stderr)["bitcoin_runtime_checked"])
                    self.assertEqual(NODE._managed_data_ready(self.layout, self.anchor), ready)
                arguments = json.loads(self.calls.read_text())
                for name, value in (("--minimum-height", "963810"),
                                    ("--anchor-height", "963800"),
                                    ("--expected-block-hash", self.anchor.block_hash)):
                    self.assertEqual(arguments[arguments.index(name) + 1], value)

    def test_invalid_runtime_never_reaches_bitcoin_and_is_reported(self):
        content = self.layout.node_env.read_text().replace("BTC_DBCACHE_MB=3072", "BTC_DBCACHE_MB=1")
        self.layout.node_env.write_text(content)
        errors = io.StringIO()
        with redirect_stderr(errors):
            self.assertFalse(NODE._managed_data_ready(self.layout, self.anchor))
        self.assertFalse(self.calls.exists())
        self.assertIn("data-progress helper exited with code 1", errors.getvalue())

    def test_malformed_or_wrong_schema_reports_are_closed_and_logged(self):
        for payload in ('{}\n{"ready": true}', '{"ready": true}', '[]'):
            with self.subTest(payload=payload), \
                 mock.patch.dict(os.environ, {"FAKE_READINESS_REPORT": payload}):
                errors = io.StringIO()
                with redirect_stderr(errors):
                    self.assertFalse(NODE._managed_data_ready(self.layout, self.anchor))
                self.assertIn("readiness observation failed", errors.getvalue())
                self.assertIn("minimum_tip_height=963810", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
