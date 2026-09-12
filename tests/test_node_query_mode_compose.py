#!/usr/bin/env python3
"""Verify node policy reaches Compose without publishing tracing to the host network."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
# Reuse the existing node-kit fixture until its legacy suite moves to tests/.
import test_usdb_node as node_tests

NODE = node_tests.NODE


class QueryModeComposeTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required")
    def test_archive_tracing_environment_and_loopback_publication(self) -> None:
        fixture = node_tests.UsdbNodeTests()
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        layout = NODE.load_release_layout(fixture.root, fixture.node_env)
        fixture.configure_full_node(layout, "query-compose")
        with mock.patch.object(NODE, "_collect_compose_services", return_value={}):
            NODE.set_query_mode(layout, state_mode="archive", tracing="on")
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("USDB_", "BTC_", "BH_", "COMPOSE_"))}
        environment.update(USDB_NETWORK_ARTIFACTS_DIR=str(fixture.bundle / "artifacts"),
                           BH_SNAPSHOT_TRUST_HOST_DIR=str(fixture.bundle / "trust"))
        result = subprocess.run(
            ["docker", "compose", "--project-name", "query-mode-test",
             "--env-file", str(fixture.bundle / "network.env"), "--env-file", str(fixture.node_env),
             "-f", str(ROOT / "docker/compose.runtime.yml"),
             "-f", str(fixture.bundle / "compose.network.yml"), "config", "--format", "json"],
            env=environment, text=True, capture_output=True, timeout=30, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        chain = json.loads(result.stdout)["services"]["usdb-chain"]
        self.assertEqual(chain["environment"]["USDB_NODE_ROLE"], "full")
        self.assertEqual(chain["environment"]["USDB_CHAIN_GCMODE"], "archive")
        self.assertEqual(chain["environment"]["USDB_CHAIN_TRACING"], "1")
        rpc_ports = [port for port in chain["ports"] if port["target"] in (8545, 8546)]
        self.assertEqual(len(rpc_ports), 2)
        self.assertTrue(all(port["host_ip"] == "127.0.0.1" for port in rpc_ports))


if __name__ == "__main__":
    unittest.main()
