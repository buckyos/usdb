#!/usr/bin/env python3
"""Check that explorer contracts export committed canonical data, not live worktree state."""
import hashlib
import json
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import export_explorer_contract as EXPORT

REVISION = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()


class ExplorerContractExportTests(unittest.TestCase):
    def test_export_binds_network_bytes_and_source_revision(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            value = EXPORT.export_contract(ROOT, REVISION, output)
            catalog = (output / "usdb-testnet-v1.json").read_bytes()
            identity = json.loads(catalog)
            self.assertEqual(value["source"]["revision"], REVISION)
            self.assertEqual(value["catalog"]["sha256"], hashlib.sha256(catalog).hexdigest())
            self.assertEqual(value["source"]["path"], "docker/networks/testnet-v1")
            self.assertEqual(identity["bundle_id"], "usdb-testnet-v1")
            self.assertEqual(identity["chain_id"], 202610030)
            self.assertEqual(identity["genesis_block_hash"], "0xb0b6ebc9a6c2e051855c2d61dff9a51635ea1db09c898a3c0126e27b9e779314")
            self.assertEqual(identity["btc_activation_registry_id"], "53b4bfed53b55a4accbd947d04d113a684f1af896d39d2d9bd984e07ad543f32")
            self.assertEqual(identity["btc_rules_scope"], "usdb-testnet-v1")
            with self.assertRaises(FileExistsError):
                EXPORT.export_contract(ROOT, REVISION, output)

    def test_mutable_revision_is_rejected_before_reading_source(self):
        with patch.object(EXPORT.subprocess, "check_output") as read:
            with self.assertRaisesRegex(ValueError, "exact 40-character"):
                EXPORT.export_contract(ROOT, "master", Path("unused"))
            read.assert_not_called()


if __name__ == "__main__":
    unittest.main()
