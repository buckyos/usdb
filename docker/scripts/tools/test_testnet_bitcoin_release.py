#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import tempfile
import textwrap
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
    def run_fake_bitcoin_down(
        self, exit_code: int
    ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            node_env = bundle / "node.env"
            node_env.write_text("", encoding="utf-8")
            (bundle / "network.env").write_text("", encoding="utf-8")
            state = root / "state"
            state.write_text("running\n", encoding="utf-8")
            calls = root / "calls"
            fake_bin = root / "bin"
            fake_bin.mkdir()
            docker = fake_bin / "docker"
            docker.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env bash
                    set -euo pipefail
                    if [[ "${1}" == "compose" ]]; then
                      if [[ " $* " == *" ps --all --quiet btc-node "* ]]; then
                        printf 'fake-container\\n'
                      elif [[ " $* " == *" stop btc-node "* ]]; then
                        printf 'stop\\n' >>"${FAKE_DOCKER_CALLS}"
                        printf 'exited\\n' >"${FAKE_DOCKER_STATE}"
                      elif [[ " $* " == *" down --remove-orphans "* ]]; then
                        printf 'down\\n' >>"${FAKE_DOCKER_CALLS}"
                      else
                        printf 'unexpected compose invocation: %s\\n' "$*" >&2
                        exit 3
                      fi
                    elif [[ "${1}" == "inspect" ]]; then
                      case "${3}" in
                        *State.Status*) cat "${FAKE_DOCKER_STATE}" ;;
                        *State.ExitCode*) printf '%s\\n' "${FAKE_DOCKER_EXIT_CODE}" ;;
                        *State.OOMKilled*) printf 'false\\n' ;;
                        *) printf 'unexpected inspect format: %s\\n' "${3}" >&2; exit 4 ;;
                      esac
                    else
                      printf 'unexpected docker invocation: %s\\n' "$*" >&2
                      exit 5
                    fi
                    """
                ),
                encoding="utf-8",
            )
            docker.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "PATH": f"{fake_bin}:{environment['PATH']}",
                    "USDB_TESTNET_BUNDLE_DIR": str(bundle),
                    "USDB_TESTNET_NODE_ENV": str(node_env),
                    "FAKE_DOCKER_STATE": str(state),
                    "FAKE_DOCKER_CALLS": str(calls),
                    "FAKE_DOCKER_EXIT_CODE": str(exit_code),
                }
            )
            result = subprocess.run(
                [str(ROOT / "docker/scripts/tools/run_testnet_bitcoin.sh"), "down"],
                check=False,
                capture_output=True,
                text=True,
                env=environment,
                timeout=10,
            )
            return result, calls.read_text(encoding="utf-8").splitlines()

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
        self.assertIn(
            "memswap_limit: ${BTC_MEMORY_SWAP_LIMIT:-${BTC_MEMORY_LIMIT:-5g}}",
            content,
        )
        self.assertIn("stop_grace_period: ${BTC_STOP_GRACE_PERIOD:-30m}", content)
        self.assertIn("external: true", content)

    def test_testnet_node_contract_uses_private_rpc_and_co_located_memory_profile(self) -> None:
        env = read_env(ROOT / "docker/networks/testnet-v0/node.env.example")
        network_env = read_env(ROOT / "docker/networks/testnet-v0/network.env")
        self.assertEqual(env["BTC_RPC_URL"], "http://btc-node:8332")
        self.assertEqual(env["USDB_DATA_ROOT"], "/home/usdb/.usdb")
        self.assertEqual(env["USDB_DATA_LAYOUT"], "usdb-node-data-layout:v2")
        self.assertRegex(env["USDB_RUNTIME_COMPATIBILITY_ID"], r"^[0-9a-f]{64}$")
        self.assertEqual(
            env["BTC_NODE_DATA_HOST_DIR"],
            "/home/usdb/.usdb/datasets/bitcoin/btc-mainnet",
        )
        self.assertTrue(
            env["BH_DATA_HOST_DIR"].startswith(
                "/home/usdb/.usdb/datasets/balance-history/btc-mainnet/"
            )
        )
        self.assertRegex(Path(env["BH_DATA_HOST_DIR"]).name, r"^[0-9a-f]{64}$")
        self.assertTrue(
            env["USDB_INDEXER_DATA_HOST_DIR"].startswith(
                "/home/usdb/.usdb/datasets/usdb-indexer/"
            )
        )
        self.assertRegex(
            Path(env["USDB_INDEXER_DATA_HOST_DIR"]).name,
            r"^[0-9a-f]{64}$",
        )
        self.assertEqual(
            env["USDB_CHAIN_DATA_HOST_DIR"],
            "/home/usdb/.usdb/networks/usdb-testnet-v0/usdb-chain",
        )
        self.assertEqual(
            env["CONTROL_PLANE_DATA_HOST_DIR"],
            "/home/usdb/.usdb/networks/usdb-testnet-v0/control-plane",
        )
        self.assertEqual(network_env["BTC_MIN_READY_HEIGHT"], "963800")
        self.assertEqual(network_env["BTC_MAX_TIP_AGE_SECS"], "7200")
        self.assertEqual(network_env["BTC_MIN_CONNECTIONS"], "1")
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(env["BTC_P2P_BIND_PORT"], "8333")
        self.assertEqual(env["BTC_RESOURCE_PROFILE"], "balanced-32g")
        self.assertEqual(env["BTC_MEMORY_LIMIT"], "5g")
        self.assertEqual(env["BTC_MEMORY_SWAP_LIMIT"], "6g")
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
        for action in (
            "up-data",
            "data-status",
            "wait-data-origin",
            "wait-data",
            "up-indexer",
            "wait-indexer",
            "up-chain",
            "indexer-status",
        ):
            self.assertIn(action, content)
        self.assertIn("--minimum-stable-height", content)

    def test_bitcoin_runner_exposes_machine_readable_sync_progress(self) -> None:
        content = (ROOT / "docker/scripts/tools/run_testnet_bitcoin.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("progress  Print one machine-readable", content)
        self.assertIn("--status-json", content)
        self.assertIn("wait-data", content)
        self.assertIn("--data-start", content)
        self.assertIn("BTC_READY_PROGRESS_INTERVAL_SECS", content)

    def test_bitcoin_runner_stops_explicitly_and_reports_shutdown_outcome(self) -> None:
        content = (ROOT / "docker/scripts/tools/run_testnet_bitcoin.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("compose stop btc-node", content)
        self.assertIn("Bitcoin Core shutdown in progress", content)
        self.assertIn("Bitcoin Core shutdown completed", content)
        self.assertIn('"${exit_code}" == "137"', content)

        result, calls = self.run_fake_bitcoin_down(exit_code=0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["stop", "down"])
        self.assertIn("shutdown started", result.stderr)
        self.assertIn("shutdown completed", result.stderr)

    def test_bitcoin_runner_reports_forced_stop_after_compose_cleanup(self) -> None:
        result, calls = self.run_fake_bitcoin_down(exit_code=137)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, ["stop", "down"])
        self.assertIn("forcibly killed", result.stderr)

    def test_runtime_runner_can_suppress_readiness_heartbeat_for_dashboard(self) -> None:
        content = (ROOT / "docker/scripts/tools/run_testnet_runtime.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("USDB_READINESS_PROGRESS_INTERVAL_SECS", content)


if __name__ == "__main__":
    unittest.main()
