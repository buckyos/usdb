#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("generate_bitcoin_rpcauth.py")
SPEC = importlib.util.spec_from_file_location("generate_bitcoin_rpcauth", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class BitcoinRpcAuthGeneratorTests(unittest.TestCase):
    @staticmethod
    def write_node_env(path: Path, rpcauth: Path, mode: int = 0o600) -> None:
        path.write_text(
            "\n".join(
                (
                    f"BTC_RPCAUTH_HOST_FILE={rpcauth}",
                    "BTC_RPC_USER=replace-me",
                    "BTC_RPC_PASSWORD=replace-me",
                    "UNRELATED=value",
                    "",
                )
            ),
            encoding="utf-8",
        )
        path.chmod(mode)

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

    def test_cli_updates_private_node_env_without_printing_password(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            root = Path(temp)
            output = root / "secure/rpcauth"
            output.parent.mkdir(mode=0o700)
            node_env = root / "node.env"
            self.write_node_env(node_env, output)

            completed = subprocess.run(
                (
                    sys.executable,
                    str(MODULE_PATH),
                    "--username",
                    "usdb-testnet",
                    "--output",
                    str(output),
                    "--node-env",
                    str(node_env),
                ),
                check=True,
                capture_output=True,
                text=True,
            )

            summary = json.loads(completed.stdout)
            node_env_values = dict(
                line.split("=", 1)
                for line in node_env.read_text(encoding="utf-8").splitlines()
            )
            password = node_env_values["BTC_RPC_PASSWORD"]
            self.assertEqual(summary["username"], "usdb-testnet")
            self.assertNotIn("password", summary)
            self.assertNotIn(password, completed.stdout)
            self.assertNotIn(password, completed.stderr)
            self.assertNotEqual(password, "replace-me")
            self.assertEqual(node_env_values["BTC_RPC_USER"], "usdb-testnet")
            self.assertEqual(node_env_values["UNRELATED"], "value")
            self.assertEqual(stat.S_IMODE(node_env.stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)

    def test_install_refuses_insecure_node_env_before_creating_rpcauth(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            root = Path(temp)
            output = root / "rpcauth"
            node_env = root / "node.env"
            self.write_node_env(node_env, output, mode=0o644)

            with self.assertRaisesRegex(ValueError, "group/world accessible"):
                GENERATOR.install_into_node_env("usdb-testnet", output, node_env)

            self.assertFalse(output.exists())

    def test_install_refuses_rpcauth_path_mismatch_before_writing(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            root = Path(temp)
            output = root / "rpcauth"
            node_env = root / "node.env"
            self.write_node_env(node_env, root / "other-rpcauth")

            with self.assertRaisesRegex(ValueError, "does not match"):
                GENERATOR.install_into_node_env("usdb-testnet", output, node_env)

            self.assertFalse(output.exists())

    def test_install_refuses_node_env_symlink_before_writing(self) -> None:
        with tempfile.TemporaryDirectory(prefix="usdb-rpcauth-") as temp:
            root = Path(temp)
            output = root / "rpcauth"
            real_node_env = root / "real-node.env"
            self.write_node_env(real_node_env, output)
            node_env = root / "node.env"
            node_env.symlink_to(real_node_env)

            with self.assertRaisesRegex(ValueError, "regular file"):
                GENERATOR.install_into_node_env("usdb-testnet", output, node_env)

            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
