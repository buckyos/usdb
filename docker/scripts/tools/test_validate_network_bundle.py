#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import hmac
import json
import shutil
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("validate_network_bundle.py")
SPEC = importlib.util.spec_from_file_location("validate_network_bundle", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


class NetworkBundleValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="usdb-network-bundle-test-")
        self.root = Path(self.temp_dir.name)
        source = MODULE_PATH.parents[2] / "networks/testnet-v0"
        self.bundle = self.root / "testnet-v0"
        shutil.copytree(source, self.bundle, ignore=shutil.ignore_patterns("node.env", "runtime"))

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def write_node_env(self, **overrides: str) -> Path:
        values = {
            "USDB_SERVICES_IMAGE": "ghcr.io/buckyos/usdb-services@sha256:" + "a" * 64,
            "USDB_CHAIN_IMAGE": "ghcr.io/buckyos/usdb-chain@sha256:" + "b" * 64,
            "USDB_BITCOIN_IMAGE": "ghcr.io/buckyos/usdb-bitcoin-core@sha256:" + "c" * 64,
            "BTC_RPC_URL": "http://btc-node:8332",
            "BTC_AUTH_MODE": "userpass",
            "BTC_RPC_USER": "test-user",
            "BTC_RPC_PASSWORD": "test-password",
            "BTC_P2P_BIND_PORT": "8333",
            "BTC_NODE_DATA_HOST_DIR": str(self.root / "bitcoin"),
            "BTC_RPCAUTH_HOST_FILE": str(self.root / "bitcoin-rpcauth"),
            "USDB_NODE_ROLE": "full",
            "USDB_P2P_BIND_PORT": "31303",
            "SNAPSHOT_MODE": "none",
            "BH_SNAPSHOT_HOST_DIR": str(self.root / "snapshot"),
            "BH_SNAPSHOT_FILE": "",
            "BH_SNAPSHOT_MANIFEST": "",
        }
        values.update(overrides)
        path = self.root / "node.env"
        path.write_text("".join(f"{key}={value}\n" for key, value in values.items()), encoding="utf-8")
        return path

    def write_rpcauth(self, mode: int = 0o600) -> None:
        salt = "00112233445566778899aabbccddeeff"
        digest = hmac.new(salt.encode(), b"test-password", "sha256").hexdigest()
        path = self.root / "bitcoin-rpcauth"
        path.write_text(f"test-user:{salt}${digest}\n", encoding="utf-8")
        path.chmod(mode)

    def test_checked_in_bundle_is_valid(self) -> None:
        network = VALIDATOR.validate_network_bundle(self.bundle)
        self.assertEqual(network["network_bundle_id"], "usdb-testnet-v0")

    def test_chain_identity_mismatch_is_rejected(self) -> None:
        path = self.bundle / "artifacts/usdb-chain-bootstrap-config.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["chainId"] += 1
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "chain ID mismatch"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_artifact_tamper_is_rejected(self) -> None:
        path = self.bundle / "trust/usdb-mainnet-snapshot-v1.trusted-keys.json"
        path.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_invalid_genesis_block_hash_is_rejected(self) -> None:
        manifest_path = self.bundle / "artifacts/usdb-genesis.manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["block_hash"] = "not-a-block-hash"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        network_path = self.bundle / "network.json"
        network = json.loads(network_path.read_text(encoding="utf-8"))
        network["artifacts"]["genesis_manifest"]["sha256"] = VALIDATOR.sha256(
            manifest_path
        )
        network_path.write_text(json.dumps(network), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "invalid genesis manifest block hash"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_artifact_path_escape_is_rejected(self) -> None:
        path = self.bundle / "network.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["artifacts"]["genesis"]["path"] = "../outside.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "path escapes bundle"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_sourcedao_operational_path_is_rejected(self) -> None:
        path = self.bundle / "artifacts/sourcedao-bootstrap-config.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["rpcUrl"] = "http://usdb-chain:8545"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "must not freeze an RPC URL"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_mutable_image_tag_is_rejected(self) -> None:
        path = self.write_node_env(USDB_CHAIN_IMAGE="ghcr.io/buckyos/usdb-chain:latest")
        with self.assertRaisesRegex(ValueError, "must not use latest"):
            VALIDATOR.validate_node_env(path, False)

    def test_versioned_image_tag_is_rejected(self) -> None:
        path = self.write_node_env(USDB_CHAIN_IMAGE="ghcr.io/buckyos/usdb-chain:testnet-v0-r1")
        with self.assertRaisesRegex(ValueError, "canonical GHCR digest reference"):
            VALIDATOR.validate_node_env(path, False)

    def test_mutable_bitcoin_image_is_rejected(self) -> None:
        path = self.write_node_env(USDB_BITCOIN_IMAGE="ghcr.io/buckyos/usdb-bitcoin-core:28.1")
        with self.assertRaisesRegex(ValueError, "canonical GHCR digest reference"):
            VALIDATOR.validate_node_env(path, False)

    def test_external_bitcoin_rpc_is_rejected(self) -> None:
        path = self.write_node_env(BTC_RPC_URL="http://host.docker.internal:8332")
        with self.assertRaisesRegex(ValueError, "private btc-node endpoint"):
            VALIDATOR.validate_node_env(path, False)

    def test_miner_requires_address_and_pass(self) -> None:
        path = self.write_node_env(USDB_NODE_ROLE="miner")
        with self.assertRaisesRegex(ValueError, "USDB_MINER_ADDRESS"):
            VALIDATOR.validate_node_env(path, False)

    def test_runtime_accepts_full_sync_without_snapshot_files(self) -> None:
        path = self.write_node_env()
        VALIDATOR.validate_node_env(path, True)

    def test_runtime_requires_signed_snapshot_set_when_enabled(self) -> None:
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE="/snapshots/snapshot_963800.db",
            BH_SNAPSHOT_MANIFEST="/snapshots/snapshot_963800.manifest.json",
        )
        snapshot_dir = self.root / "snapshot"
        snapshot_dir.mkdir()
        for name in (
            "snapshot_963800.db",
            "snapshot_963800.manifest.json",
            "snapshot_963800.manifest.sig",
        ):
            (snapshot_dir / name).touch()
        VALIDATOR.validate_node_env(path, True)

    def test_full_sync_rejects_stale_snapshot_inputs(self) -> None:
        path = self.write_node_env(BH_SNAPSHOT_FILE="/snapshots/snapshot_963800.db")
        with self.assertRaisesRegex(ValueError, "requires empty BH_SNAPSHOT_FILE"):
            VALIDATOR.validate_node_env(path, True)

    def test_snapshot_mode_requires_canonical_container_paths(self) -> None:
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE="/tmp/snapshot.db",
            BH_SNAPSHOT_MANIFEST="/snapshots/snapshot_963800.manifest.json",
        )
        with self.assertRaisesRegex(ValueError, "canonical snapshot file path"):
            VALIDATOR.validate_node_env(path, False)

    def test_bitcoin_runtime_requires_private_full_node_paths(self) -> None:
        path = self.write_node_env()
        bitcoin_dir = self.root / "bitcoin"
        bitcoin_dir.mkdir()
        self.write_rpcauth()
        VALIDATOR.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_public_auth_file(self) -> None:
        path = self.write_node_env()
        (self.root / "bitcoin").mkdir()
        self.write_rpcauth(0o644)
        with self.assertRaisesRegex(ValueError, "group/world accessible"):
            VALIDATOR.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_password_mismatch(self) -> None:
        path = self.write_node_env(BTC_RPC_PASSWORD="wrong-password")
        (self.root / "bitcoin").mkdir()
        self.write_rpcauth()
        with self.assertRaisesRegex(ValueError, "does not match RPC password"):
            VALIDATOR.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_relative_data_path(self) -> None:
        path = self.write_node_env(BTC_NODE_DATA_HOST_DIR="relative/bitcoin")
        self.write_rpcauth()
        with self.assertRaisesRegex(ValueError, "must be an absolute path"):
            VALIDATOR.validate_node_env(path, False, True)


if __name__ == "__main__":
    unittest.main()
