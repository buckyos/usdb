#!/usr/bin/env python3

import os
import pathlib
import re
import subprocess
import tempfile
import tomllib
import unittest


ROOT_DIR = pathlib.Path(__file__).resolve().parents[3]
RENDERER = ROOT_DIR / "docker/scripts/helpers/render_balance_history_config.sh"
BASE_COMPOSE = ROOT_DIR / "docker/compose.base.yml"
PROFILE_COMPOSE = ROOT_DIR / "docker/compose.test-32gb.yml"
GIB = 1024**3


class BalanceHistoryMemoryProfileTests(unittest.TestCase):
    def compose_default(self, content: str, variable: str) -> str:
        match = re.search(rf"\$\{{{re.escape(variable)}:-([^}}]+)\}}", content)
        self.assertIsNotNone(match, f"missing Compose default for {variable}")
        return match.group(1)

    def test_renderer_writes_explicit_cache_limits(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir) / "balance-history"
            config_path = root / "config.toml"
            env = os.environ.copy()
            env.update(
                {
                    "BH_ROOT_DIR": str(root),
                    "BTC_AUTH_MODE": "none",
                    "BH_SYNC_UTXO_MAX_CACHE_BYTES": str(4 * GIB),
                    "BH_SYNC_BALANCE_MAX_CACHE_BYTES": str(12 * GIB),
                    "BH_SYNC_MAX_MEMORY_PERCENT": "85",
                }
            )
            subprocess.run([str(RENDERER), str(config_path)], env=env, check=True)

            with config_path.open("rb") as config_file:
                config = tomllib.load(config_file)
            self.assertEqual(config["sync"]["utxo_max_cache_bytes"], 4 * GIB)
            self.assertEqual(config["sync"]["balance_max_cache_bytes"], 12 * GIB)
            self.assertEqual(config["sync"]["max_memory_percent"], 85)

    def test_base_compose_uses_conservative_explicit_cache_defaults(self) -> None:
        content = BASE_COMPOSE.read_text(encoding="utf-8")
        self.assertGreaterEqual(
            content.count(
                "BH_SYNC_UTXO_MAX_CACHE_BYTES: "
                "${BH_SYNC_UTXO_MAX_CACHE_BYTES:-2147483648}"
            ),
            2,
        )
        self.assertGreaterEqual(
            content.count(
                "BH_SYNC_BALANCE_MAX_CACHE_BYTES: "
                "${BH_SYNC_BALANCE_MAX_CACHE_BYTES:-6442450944}"
            ),
            2,
        )

    def test_32gb_profile_leaves_runtime_headroom(self) -> None:
        content = PROFILE_COMPOSE.read_text(encoding="utf-8")
        self.assertIn("mem_limit: ${BH_MEMORY_LIMIT:-24g}", content)
        self.assertIn("memswap_limit: ${BH_MEMORY_SWAP_LIMIT:-24g}", content)
        self.assertIn(
            "BH_SYNC_UTXO_MAX_CACHE_BYTES: "
            "${BH_SYNC_UTXO_MAX_CACHE_BYTES:-4294967296}",
            content,
        )
        self.assertIn(
            "BH_SYNC_BALANCE_MAX_CACHE_BYTES: "
            "${BH_SYNC_BALANCE_MAX_CACHE_BYTES:-12884901888}",
            content,
        )
        memory_limit = int(self.compose_default(content, "BH_MEMORY_LIMIT").removesuffix("g")) * GIB
        utxo_cache = int(self.compose_default(content, "BH_SYNC_UTXO_MAX_CACHE_BYTES"))
        balance_cache = int(self.compose_default(content, "BH_SYNC_BALANCE_MAX_CACHE_BYTES"))
        self.assertLess(utxo_cache + balance_cache, memory_limit)


if __name__ == "__main__":
    unittest.main()
