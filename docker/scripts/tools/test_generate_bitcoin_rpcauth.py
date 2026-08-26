#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import stat
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("generate_bitcoin_rpcauth.py")
SPEC = importlib.util.spec_from_file_location("generate_bitcoin_rpcauth", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class BitcoinRpcAuthGeneratorTests(unittest.TestCase):
    def test_generates_private_file_and_client_credentials(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            output = Path(temp) / "secure/rpcauth"
            result = GENERATOR.generate("usdb-testnet", output)
            self.assertEqual(result["username"], "usdb-testnet")
            self.assertTrue(result["password"])
            self.assertTrue(output.read_text(encoding="utf-8").startswith("usdb-testnet:"))
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)

    def test_refuses_to_replace_existing_secret(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            output = Path(temp) / "rpcauth"
            output.touch()
            with self.assertRaisesRegex(ValueError, "refusing to replace"):
                GENERATOR.generate("usdb-testnet", output)

    def test_rejects_invalid_username(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            with self.assertRaisesRegex(ValueError, "unsupported characters"):
                GENERATOR.generate("bad:user", Path(temp) / "rpcauth")


if __name__ == "__main__":
    unittest.main()
