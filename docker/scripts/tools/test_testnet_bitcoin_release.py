#!/usr/bin/env python3

from __future__ import annotations

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def read_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line and not line.startswith("#"):
            key, value = line.split("=", 1)
            values[key] = value
    return values


class TestnetBitcoinReleaseTests(unittest.TestCase):
    def test_dockerfile_freezes_release_and_three_signers(self) -> None:
        content = (ROOT / "docker/Dockerfile.bitcoin-core").read_text(encoding="utf-8")
        self.assertIn("ARG BITCOIN_VERSION=28.1", content)
        self.assertIn("07f77afd326639145b9ba9562912b2ad2ccec47b8a305bd075b4f4cb127b7ed7", content)
        for fingerprint in (
            "152812300785C96444D3334D17565732E08E5E41",
            "9DEAE0DC7063249FB05474681E4AED62986CD25D",
            "0CCBAAFD76A2ECE2CCD3141DE2FFD5B1D88CA97D",
        ):
            self.assertIn(fingerprint, content)

    def test_release_compose_keeps_rpc_private_and_data_external(self) -> None:
        content = (ROOT / "docker/compose.bitcoin.yml").read_text(encoding="utf-8")
        self.assertIn("${BTC_NODE_DATA_HOST_DIR:?BTC_NODE_DATA_HOST_DIR is required}:/data/bitcoin", content)
        self.assertIn("${BTC_P2P_BIND_ADDRESS:-127.0.0.1}:${BTC_P2P_BIND_PORT:-8333}:8333/tcp", content)
        self.assertNotIn(":8332:8332", content)
        self.assertIn("external: true", content)

    def test_testnet_node_contract_uses_private_rpc_and_co_located_memory_profile(self) -> None:
        env = read_env(ROOT / "docker/networks/testnet-v0/node.env.example")
        network_env = read_env(ROOT / "docker/networks/testnet-v0/network.env")
        self.assertEqual(env["BTC_RPC_URL"], "http://btc-node:8332")
        self.assertEqual(env["USDB_DATA_ROOT"], "/home/usdb/.usdb")
        self.assertEqual(env["BTC_NODE_DATA_HOST_DIR"], "/home/usdb/.usdb/bitcoin/mainnet")
        self.assertEqual(env["BH_DATA_HOST_DIR"], "/home/usdb/.usdb/balance-history")
        self.assertEqual(env["USDB_INDEXER_DATA_HOST_DIR"], "/home/usdb/.usdb/usdb-indexer")
        self.assertEqual(env["USDB_CHAIN_DATA_HOST_DIR"], "/home/usdb/.usdb/usdb-chain")
        self.assertEqual(env["CONTROL_PLANE_DATA_HOST_DIR"], "/home/usdb/.usdb/control-plane")
        self.assertEqual(network_env["BTC_MIN_READY_HEIGHT"], "963800")
        self.assertEqual(network_env["BTC_MAX_TIP_AGE_SECS"], "7200")
        self.assertEqual(network_env["BTC_MIN_CONNECTIONS"], "1")
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(env["BTC_P2P_BIND_PORT"], "8333")
        self.assertEqual(env["BTC_DBCACHE_MB"], "3072")
        self.assertEqual(env["SNAPSHOT_MODE"], "none")
        self.assertEqual(env["BH_SNAPSHOT_FILE"], "")
        self.assertEqual(env["BH_SYNC_LOCAL_LOADER_THRESHOLD"], "500")
        limits_gib = sum(
            int(env[name].removesuffix("g"))
            for name in (
                "BTC_MEMORY_LIMIT",
                "BH_MEMORY_LIMIT",
                "USDB_INDEXER_MEMORY_LIMIT",
                "USDB_CHAIN_MEMORY_LIMIT",
                "CONTROL_PLANE_MEMORY_LIMIT",
            )
        )
        self.assertLessEqual(limits_gib, 27)

    def test_runtime_and_bitcoin_share_an_external_network(self) -> None:
        for relative in ("docker/compose.bitcoin.yml", "docker/compose.runtime.yml"):
            with self.subTest(relative=relative):
                content = (ROOT / relative).read_text(encoding="utf-8")
                network = content.rsplit("networks:", 1)[1]
                self.assertIn("external: true", network)
                self.assertIn("USDB_DOCKER_NETWORK", network)

    def test_full_sync_runtime_mounts_bitcoin_data_read_only(self) -> None:
        content = (ROOT / "docker/compose.runtime.yml").read_text(encoding="utf-8")
        self.assertIn(
            "${BTC_NODE_DATA_HOST_DIR:?BTC_NODE_DATA_HOST_DIR is required}:/data/bitcoin:ro",
            content,
        )
        self.assertIn("SNAPSHOT_MODE: ${SNAPSHOT_MODE:-none}", content)
        for host_key, container_path in (
            ("BH_DATA_HOST_DIR", "/data/balance-history"),
            ("USDB_INDEXER_DATA_HOST_DIR", "/data/usdb-indexer"),
            ("USDB_CHAIN_DATA_HOST_DIR", "/data/usdb-chain"),
            ("CONTROL_PLANE_DATA_HOST_DIR", "/data/usdb-control-plane"),
        ):
            self.assertIn(f"${{{host_key}:?{host_key} is required}}:{container_path}", content)
        self.assertNotIn("balance-history-data:", content)
        self.assertNotIn("usdb-indexer-data:", content)
        self.assertNotIn("usdb-chain-data:", content)
        self.assertNotIn("control-plane-data:", content)

    def test_runtime_runner_exposes_phased_data_start(self) -> None:
        content = (ROOT / "docker/scripts/tools/run_testnet_runtime.sh").read_text(encoding="utf-8")
        for action in ("up-data", "data-status", "wait-data", "indexer-status"):
            self.assertIn(action, content)


if __name__ == "__main__":
    unittest.main()
