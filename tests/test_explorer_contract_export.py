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
            catalog = (output / "usdb-testnet-v0.json").read_bytes()
            identity = json.loads(catalog)
            self.assertEqual(value["source"]["revision"], REVISION)
            self.assertEqual(value["catalog"]["sha256"], hashlib.sha256(catalog).hexdigest())
            self.assertEqual(identity["chain_id"], 202608250)
            self.assertEqual(identity["genesis_block_hash"], "0x12a1baed070d1521d791b73956a8b5cf1613fc9504636f215390c1f839992a23")
            with self.assertRaises(FileExistsError):
                EXPORT.export_contract(ROOT, REVISION, output)

    def test_mutable_revision_is_rejected_before_reading_source(self):
        with patch.object(EXPORT.subprocess, "check_output") as read:
            with self.assertRaisesRegex(ValueError, "exact 40-character"):
                EXPORT.export_contract(ROOT, "master", Path("unused"))
            read.assert_not_called()


if __name__ == "__main__":
    unittest.main()
