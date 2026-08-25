#!/usr/bin/env python3

import pathlib
import unittest


ROOT_DIR = pathlib.Path(__file__).resolve().parents[3]
COMPOSE_FILE = ROOT_DIR / "docker/compose.base.yml"
ENV_EXAMPLES = (
    "bootstrap.env.example",
    "dev-full-sim.env.example",
    "dev-full.env.example",
    "dev-sim.env.example",
    "joiner.env.example",
    "world-sim.env.example",
)


class P2PDefaultsTests(unittest.TestCase):
    def test_ethw_node_publishes_canonical_usdb_p2p_port(self) -> None:
        content = COMPOSE_FILE.read_text(encoding="utf-8")

        self.assertIn(
            '"${ETHW_P2P_BIND_PORT:-31303}:${ETHW_P2P_PORT:-31303}/tcp"',
            content,
        )
        self.assertIn(
            '"${ETHW_P2P_BIND_PORT:-31303}:${ETHW_P2P_PORT:-31303}/udp"',
            content,
        )
        self.assertNotIn(
            '"${ETHW_P2P_BIND_PORT:-30303}:${ETHW_P2P_PORT:-30303}"',
            content,
        )

    def test_example_geth_commands_listen_on_canonical_port(self) -> None:
        for name in ENV_EXAMPLES:
            with self.subTest(name=name):
                content = (ROOT_DIR / "docker/env" / name).read_text(encoding="utf-8")
                command = next(
                    line for line in content.splitlines() if line.startswith("ETHW_COMMAND=")
                )
                self.assertIn(" --port 31303 ", command)


if __name__ == "__main__":
    unittest.main()
