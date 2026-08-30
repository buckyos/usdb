#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import shutil
import stat
import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("usdb_node.py")
SPEC = importlib.util.spec_from_file_location("usdb_node", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
NODE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = NODE
SPEC.loader.exec_module(NODE)

REPOSITORY_ROOT = MODULE_PATH.parents[3]
SOURCE_BUNDLE = REPOSITORY_ROOT / "docker/networks/testnet-v0"


class UsdbNodeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="usdb-node-test-")
        self.root = Path(self.temporary.name) / "usdb-node-kit"
        self.bundle = self.root / "docker/networks/usdb-testnet-v0"
        self.bundle.parent.mkdir(parents=True)
        shutil.copytree(SOURCE_BUNDLE, self.bundle)
        self.release_dir = self.root / "release"
        self.release_dir.mkdir()
        self.manifest_path = self.release_dir / "usdb-release-manifest.json"
        digest = "1" * 64
        self.manifest = {
            "schema_version": "usdb-release-manifest:v3",
            "release_id": "usdb-testnet-v0-r1",
            "network_bundle": NODE.build_network_identity(self.bundle),
            "images": {
                "usdb_services": {
                    "reference": f"ghcr.io/buckyos/usdb-services@sha256:{digest}"
                },
                "usdb_chain": {
                    "reference": f"ghcr.io/buckyos/usdb-chain@sha256:{'2' * 64}"
                },
                "bitcoin_core": {
                    "reference": f"ghcr.io/buckyos/usdb-bitcoin-core@sha256:{'3' * 64}"
                },
            },
        }
        self.write_manifest()
        self.node_env = self.root / "private/node.env"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_manifest(self) -> None:
        content = json.dumps(self.manifest, indent=2, sort_keys=True) + "\n"
        self.manifest_path.write_text(content, encoding="utf-8")
        digest = hashlib.sha256(content.encode()).hexdigest()
        self.manifest_path.with_name(self.manifest_path.name + ".sha256").write_text(
            f"{digest}  {self.manifest_path.name}\n",
            encoding="utf-8",
        )

    def test_configure_derives_release_images_paths_and_private_credentials(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "node-data"
        node_env = NODE.configure_node(
            layout,
            data_root=data_root,
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="",
            nat="extip:203.0.113.10",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
        )
        env = NODE.read_env(node_env)
        self.assertEqual(env["USDB_SERVICES_IMAGE"], self.manifest["images"]["usdb_services"]["reference"])
        self.assertEqual(env["BTC_NODE_DATA_HOST_DIR"], str(data_root / "bitcoin/mainnet"))
        self.assertEqual(env["USDB_DATA_ROOT"], str(data_root))
        self.assertEqual(env["BH_DATA_HOST_DIR"], str(data_root / "balance-history"))
        self.assertEqual(env["USDB_INDEXER_DATA_HOST_DIR"], str(data_root / "usdb-indexer"))
        self.assertEqual(env["USDB_CHAIN_DATA_HOST_DIR"], str(data_root / "usdb-chain"))
        self.assertEqual(env["CONTROL_PLANE_DATA_HOST_DIR"], str(data_root / "control-plane"))
        self.assertEqual(env["BTC_RPC_USER"], "node-a")
        self.assertTrue(env["BTC_RPC_PASSWORD"])
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(env["USDB_NODE_ROLE"], "full")
        self.assertEqual(env["USDB_NAT"], "extip:203.0.113.10")
        self.assertEqual(stat.S_IMODE(node_env.stat().st_mode), 0o600)
        rpcauth = data_root / "secure/bitcoin-mainnet-rpcauth"
        self.assertTrue(rpcauth.is_file())
        self.assertEqual(stat.S_IMODE(rpcauth.stat().st_mode), 0o600)
        for relative in NODE.PERSISTENT_DATA_PATHS.values():
            self.assertTrue((data_root / relative).is_dir())

        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            NODE.configure_node(
                layout,
                data_root=data_root,
                role="full",
                miner_address="",
                miner_threads=1,
                bootnodes="",
                nat="",
                bitcoin_rpc_user="node-a",
                bitcoin_p2p="private",
            )

    def test_setup_collects_only_operator_owned_choices(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "setup-data"
        address = "0x1111111111111111111111111111111111111111"
        answers = iter(
            [
                str(data_root),
                "miner",
                address,
                "2",
                "enode://example",
                "n",
                "y",
            ]
        )
        output = io.StringIO()
        NODE.setup_node(layout, input_fn=lambda _prompt: next(answers), output=output)
        env = NODE.read_env(layout.node_env)
        self.assertEqual(env["USDB_NODE_ROLE"], "miner")
        self.assertEqual(env["USDB_MINER_ADDRESS"], address)
        self.assertEqual(env["USDB_MINER_THREADS"], "2")
        self.assertEqual(env["USDB_BOOTNODES"], "enode://example")
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(
            env["BTC_RPC_USER"],
            NODE.default_bitcoin_rpc_user(layout),
        )
        self.assertIn("USDB P2P: public TCP/UDP 31303", output.getvalue())

    def test_setup_cancellation_writes_no_config_or_credentials(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "cancelled-data"
        answers = iter([str(data_root), "full", "", "n", "n"])
        with self.assertRaisesRegex(ValueError, "setup cancelled"):
            NODE.setup_node(
                layout,
                input_fn=lambda _prompt: next(answers),
                output=io.StringIO(),
            )
        self.assertFalse(layout.node_env.exists())
        self.assertFalse((data_root / "secure/bitcoin-mainnet-rpcauth").exists())

    def test_default_rpc_user_is_bundle_and_host_scoped(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.assertEqual(
            NODE.default_bitcoin_rpc_user(layout, "Node A.example"),
            "usdb-testnet-v0-Node-A.example",
        )

    def test_set_role_requires_and_records_miner_identity(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        NODE.configure_node(
            layout,
            data_root=Path(self.temporary.name) / "node-data",
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="",
            nat="",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
        )
        address = "0x1111111111111111111111111111111111111111"
        NODE.set_role(layout, role="miner", miner_address=address, miner_threads=0)
        env = NODE.read_env(layout.node_env)
        self.assertEqual(env["USDB_NODE_ROLE"], "miner")
        self.assertEqual(env["USDB_MINER_ADDRESS"], address)
        self.assertEqual(env["USDB_MINER_THREADS"], "0")

        with self.assertRaisesRegex(ValueError, "requires a non-zero EVM"):
            NODE.set_role(layout, role="miner", miner_address="", miner_threads=1)

    def test_release_identity_mismatch_is_rejected(self) -> None:
        self.manifest["network_bundle"]["chain_id"] += 1
        self.write_manifest()
        with self.assertRaisesRegex(ValueError, "network identity"):
            NODE.load_release_layout(self.root, self.node_env)

    def test_startup_order_is_internal_and_resumable(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        calls: list[tuple[str, tuple[str, ...]]] = []

        def record(_layout: object, helper: str, arguments: list[str], **_kwargs: object) -> object:
            calls.append((helper, tuple(arguments)))
            return mock.Mock(returncode=0)

        with mock.patch.object(NODE, "doctor"), mock.patch.object(NODE, "run_helper", side_effect=record):
            NODE.start_node(layout, sync_timeout_secs=123, pull=True)
        self.assertEqual(
            calls,
            [
                ("run_testnet_bitcoin.sh", ("pull",)),
                ("run_testnet_runtime.sh", ("pull",)),
                ("run_testnet_bitcoin.sh", ("up",)),
                ("run_testnet_runtime.sh", ("up-data",)),
                ("run_testnet_runtime.sh", ("wait-data", "123")),
                ("run_testnet_runtime.sh", ("up",)),
            ],
        )

    def test_render_env_rejects_missing_keys_and_newlines(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing keys"):
            NODE.render_env("A=1\n", {"B": "2"})
        with self.assertRaisesRegex(ValueError, "newline"):
            NODE.render_env("A=1\n", {"A": "bad\nvalue"})

    def test_activate_release_changes_only_manifest_owned_images(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        NODE.configure_node(
            layout,
            data_root=Path(self.temporary.name) / "node-data",
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="enode://example",
            nat="",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
        )
        original = NODE.read_env(layout.node_env)
        replacement_images = {
            key: value[:-64] + "9" * 64 for key, value in layout.images.items()
        }
        next_layout = replace(layout, release_id="usdb-testnet-v0-r2", images=replacement_images)
        with self.assertRaisesRegex(ValueError, "activate-release"):
            NODE._validate_node_release_images(next_layout)
        NODE.activate_release(next_layout)
        updated = NODE.read_env(layout.node_env)
        for key, value in replacement_images.items():
            self.assertEqual(updated[key], value)
        self.assertEqual(updated["BTC_RPC_PASSWORD"], original["BTC_RPC_PASSWORD"])
        self.assertEqual(updated["USDB_BOOTNODES"], "enode://example")


if __name__ == "__main__":
    unittest.main()
