#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import hashlib
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

import runtime_compatibility as RUNTIME


class NetworkBundleValidatorTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="usdb-network-bundle-test-")
        self.root = Path(self.temp_dir.name)
        source = MODULE_PATH.parents[2] / "networks/testnet-v0"
        self.bundle = self.root / "testnet-v0"
        shutil.copytree(
            source,
            self.bundle,
            ignore=shutil.ignore_patterns("node.env", "runtime"),
        )
        self.network = VALIDATOR.validate_network_bundle(self.bundle)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def runtime_network_identity(self) -> dict:
        genesis_manifest = json.loads(
            (self.bundle / "artifacts/usdb-genesis.manifest.json").read_text(
                encoding="utf-8"
            )
        )
        btc_source = self.network["btc_source"]
        return {
            "bundle_id": self.network["network_bundle_id"],
            "chain_id": self.network["chain_id"],
            "genesis_block_hash": genesis_manifest["block_hash"],
            "btc_network_id": btc_source["network_id"],
            "btc_index_origin_height": btc_source["index_origin_height"],
            "btc_activation_registry_id": btc_source["activation_registry_id"],
        }

    def write_node_env(self, *, legacy_layout: bool = False, **overrides: str) -> Path:
        data_root = self.root / "node-data"
        compatibility = RUNTIME.build_runtime_compatibility(
            self.runtime_network_identity()
        )
        data_paths = (
            RUNTIME.build_legacy_data_paths(data_root)
            if legacy_layout
            else RUNTIME.build_persistent_data_paths(
                data_root,
                self.runtime_network_identity(),
                compatibility,
            )
        )
        values = {
            "USDB_SERVICES_IMAGE": "ghcr.io/buckyos/usdb-services@sha256:" + "a" * 64,
            "USDB_CHAIN_IMAGE": "ghcr.io/buckyos/usdb-chain@sha256:" + "b" * 64,
            "USDB_BITCOIN_IMAGE": "ghcr.io/buckyos/usdb-bitcoin-core@sha256:" + "c" * 64,
            "BTC_RPC_URL": "http://btc-node:8332",
            "BTC_AUTH_MODE": "userpass",
            "BTC_RPC_USER": "test-user",
            "BTC_RPC_PASSWORD": "test-password",
            "BTC_P2P_BIND_ADDRESS": "127.0.0.1",
            "BTC_P2P_BIND_PORT": "8333",
            "BTC_RESOURCE_PROFILE": "balanced-32g",
            "BTC_MEMORY_LIMIT": "5g",
            "BTC_MEMORY_SWAP_LIMIT": "6g",
            "BTC_DBCACHE_MB": "3072",
            "USDB_FIREWALL_MODE": "external",
            "USDB_OPERATOR_SSH_PORT": "22",
            "USDB_DATA_ROOT": str(data_root),
            **{key: str(value) for key, value in data_paths.items()},
            "BTC_RPCAUTH_HOST_FILE": str(self.root / "bitcoin-rpcauth"),
            "USDB_NODE_ROLE": "full",
            "USDB_P2P_BIND_ADDRESS": "0.0.0.0",
            "USDB_P2P_BIND_PORT": "31303",
            "USDB_HTTP_BIND_ADDRESS": "127.0.0.1",
            "USDB_WS_BIND_ADDRESS": "127.0.0.1",
            "BH_BIND_ADDRESS": "127.0.0.1",
            "USDB_INDEXER_BIND_ADDRESS": "127.0.0.1",
            "CONTROL_PLANE_BIND_ADDRESS": "127.0.0.1",
            "SNAPSHOT_MODE": "none",
            "BH_SNAPSHOT_HOST_DIR": str(self.root / "snapshot"),
            "BH_SNAPSHOT_FILE": "",
            "BH_SNAPSHOT_MANIFEST": "",
            "USDB_INDEXER_CHECKPOINT_MANIFEST": "",
        }
        if not legacy_layout:
            values["USDB_DATA_LAYOUT"] = RUNTIME.DATA_LAYOUT_VERSION
            values["USDB_RUNTIME_COMPATIBILITY_ID"] = compatibility[
                "compatibility_id"
            ]
        values.update(overrides)
        for key in RUNTIME.PERSISTENT_DATA_SERVICES:
            candidate = Path(values[key])
            if candidate.is_absolute():
                candidate.mkdir(parents=True, exist_ok=True)
        path = self.root / "node.env"
        path.write_text("".join(f"{key}={value}\n" for key, value in values.items()), encoding="utf-8")
        return path

    def write_rpcauth(self, mode: int = 0o600) -> None:
        salt = "00112233445566778899aabbccddeeff"
        digest = hmac.new(salt.encode(), b"test-password", "sha256").hexdigest()
        path = self.root / "bitcoin-rpcauth"
        path.write_text(f"test-user:{salt}${digest}\n", encoding="utf-8")
        path.chmod(mode)

    def validate_node_env(
        self,
        path: Path,
        require_runtime: bool,
        require_bitcoin_runtime: bool = False,
    ) -> None:
        VALIDATOR.validate_node_env(
            path,
            self.network,
            require_runtime,
            require_bitcoin_runtime,
        )

    def test_bitcoin_resource_profile_must_match_memory_swap_and_dbcache(self) -> None:
        path = self.write_node_env(BTC_RESOURCE_PROFILE="performance-64g")
        with self.assertRaisesRegex(ValueError, "BTC_MEMORY_LIMIT does not match"):
            self.validate_node_env(path, False)

        path = self.write_node_env(BTC_MEMORY_SWAP_LIMIT="7g")
        with self.assertRaisesRegex(ValueError, "BTC_MEMORY_SWAP_LIMIT does not match"):
            self.validate_node_env(path, False)

        path = self.write_node_env(BTC_DBCACHE_MB="4096")
        with self.assertRaisesRegex(ValueError, "BTC_DBCACHE_MB does not match"):
            self.validate_node_env(path, False)

        path = self.write_node_env(BTC_RESOURCE_PROFILE="custom")
        with self.assertRaisesRegex(ValueError, "BTC_RESOURCE_PROFILE"):
            self.validate_node_env(path, False)

        path = self.write_node_env(
            BTC_RESOURCE_PROFILE="ibd-64g",
            BTC_MEMORY_LIMIT="32g",
            BTC_MEMORY_SWAP_LIMIT="34g",
            BTC_DBCACHE_MB="20480",
        )
        self.validate_node_env(path, False)

    def write_snapshot_artifacts(
        self,
        height: int,
        stem: str,
        release_dir: str | None = None,
    ) -> tuple[str, str]:
        snapshot_dir = self.root / "snapshot"
        snapshot_dir.mkdir(exist_ok=True)
        artifact_dir = snapshot_dir / release_dir if release_dir else snapshot_dir
        artifact_dir.mkdir(exist_ok=True)
        file_name = f"{stem}.db"
        manifest_name = f"{stem}.manifest.json"
        (artifact_dir / file_name).touch()
        state_ref = {
            "block_height": height,
            "stable_block_hash": "1" * 64,
            "latest_block_commit": "2" * 64,
            "snapshot_id": "3" * 64,
            "consensus_identity": {"stable_lag": 10},
        }
        manifest = {
            "manifest_version": VALIDATOR.EXPECTED_SNAPSHOT_MANIFEST_VERSION,
            "file_name": file_name,
            "file_sha256": "0" * 64,
            "state_ref": state_ref,
            "db_identity": {"btc_network": "bitcoin"},
            "balance_query_floor": height,
            "history_query_floor": min(height + 1, 0xFFFFFFFF),
            "signature_scheme": VALIDATOR.EXPECTED_SNAPSHOT_SIGNATURE_SCHEME,
            "signing_key_id": "test-snapshot-signer",
        }
        (artifact_dir / manifest_name).write_text(json.dumps(manifest), encoding="utf-8")
        (artifact_dir / f"{stem}.manifest.sig").touch()
        prefix = f"/snapshots/{release_dir}" if release_dir else "/snapshots"
        return f"{prefix}/{file_name}", f"{prefix}/{manifest_name}"

    def write_indexer_checkpoint_artifacts(
        self,
        height: int,
        snapshot_manifest_name: str,
    ) -> str:
        snapshot_dir = self.root / "snapshot"
        snapshot_manifest_path = snapshot_dir / snapshot_manifest_name
        snapshot_manifest = json.loads(snapshot_manifest_path.read_text(encoding="utf-8"))
        operation_id = "4" * 64
        artifact_name = f"usdb-indexer-checkpoint-{height}-{operation_id[:16]}"
        artifact_dir = snapshot_dir / artifact_name
        data_dir = artifact_dir / "data"
        data_dir.mkdir(parents=True)
        data_file = data_dir / "miner_pass.db"
        data_file.write_bytes(b"checkpoint")
        identity = {
            "block_height": height,
            "stable_block_hash": snapshot_manifest["state_ref"]["stable_block_hash"],
            "latest_block_commit": snapshot_manifest["state_ref"]["latest_block_commit"],
            "snapshot_id": snapshot_manifest["state_ref"]["snapshot_id"],
            "activation_registry_id": "5" * 64,
            "active_version_set_id": "6" * 64,
            "local_state_commit": "7" * 64,
            "system_state_id": "8" * 64,
        }
        indexer_state_ref = {
            "block_height": height,
            "snapshot_info": {
                "stable_block_hash": identity["stable_block_hash"],
                "latest_block_commit": identity["latest_block_commit"],
                "snapshot_id": identity["snapshot_id"],
            },
            "local_state_commit_info": {
                "activation_registry_id": identity["activation_registry_id"],
                "active_version_set_id": identity["active_version_set_id"],
                "local_state_commit": identity["local_state_commit"],
            },
            "system_state_info": {"system_state_id": identity["system_state_id"]},
        }
        manifest = {
            "manifest_version": VALIDATOR.EXPECTED_INDEXER_CHECKPOINT_MANIFEST_VERSION,
            "tool_version": "0.1.0-test",
            "data_schema_version": VALIDATOR.EXPECTED_INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION,
            "operation_id": operation_id,
            "network_bundle_id": self.network["network_bundle_id"],
            "chain_id": self.network["chain_id"],
            "btc_network": "bitcoin",
            "index_origin_height": 963800,
            "checkpoint_height": height,
            "artifact_dir_name": artifact_name,
            "files": [
                {
                    "path": "miner_pass.db",
                    "size": data_file.stat().st_size,
                    "sha256": hashlib.sha256(data_file.read_bytes()).hexdigest(),
                }
            ],
            "balance_history": {
                "manifest_file_name": snapshot_manifest_name,
                "manifest_sha256": hashlib.sha256(snapshot_manifest_path.read_bytes()).hexdigest(),
                "snapshot_file_name": snapshot_manifest["file_name"],
                "snapshot_file_sha256": snapshot_manifest["file_sha256"],
                "state_ref": snapshot_manifest["state_ref"],
                "balance_query_floor": snapshot_manifest["balance_query_floor"],
                "history_query_floor": snapshot_manifest["history_query_floor"],
            },
            "indexer_state_ref": indexer_state_ref,
            "state_identity": identity,
            "signature_scheme": VALIDATOR.EXPECTED_SNAPSHOT_SIGNATURE_SCHEME,
            "signing_key_id": "test-snapshot-signer",
            "generated_at": 1,
        }
        (artifact_dir / "usdb-indexer-checkpoint.manifest.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )
        (artifact_dir / "usdb-indexer-checkpoint.manifest.sig").touch()
        return f"/snapshots/{artifact_name}/usdb-indexer-checkpoint.manifest.json"

    def test_checked_in_bundle_is_valid(self) -> None:
        network = VALIDATOR.validate_network_bundle(self.bundle)
        self.assertEqual(network["network_bundle_id"], "usdb-testnet-v0")

    def test_index_origin_is_read_from_each_network_bundle(self) -> None:
        self.assertEqual(
            VALIDATOR.network_index_origin_height(
                {"btc_source": {"index_origin_height": 123456}}
            ),
            123456,
        )

    def test_registry_stable_lag_is_release_frozen(self) -> None:
        registry = json.loads(
            (
                MODULE_PATH.parents[2]
                / "../src/btc/usdb-util/activation-registry/btc-mainnet.json"
            ).resolve().read_text(encoding="utf-8")
        )
        self.assertEqual(
            VALIDATOR.btc_registry_stable_lag_blocks(VALIDATOR.EXPECTED_BTC_REGISTRY),
            registry["scope"]["stable_lag_blocks"],
        )
        with self.assertRaisesRegex(ValueError, "unsupported BTC activation registry ID"):
            VALIDATOR.btc_registry_stable_lag_blocks("00" * 32)

    def test_testnet_rejects_known_development_bootstrap_admin(self) -> None:
        path = self.bundle / "artifacts/usdb-chain-bootstrap-config.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["bootstrapAdmin"]["address"] = "0xabCd35AfbB4561213fEAfF01B5F91e18F8Df7c37"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "known development address"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_mainnet_rejects_known_development_bootstrap_admin(self) -> None:
        with self.assertRaisesRegex(ValueError, "known development address"):
            VALIDATOR.validate_bootstrap_admin(
                "mainnet",
                "0xabCd35AfbB4561213fEAfF01B5F91e18F8Df7c37",
            )

    def test_development_fixture_accepts_known_bootstrap_admin(self) -> None:
        address = "0xabCd35AfbB4561213fEAfF01B5F91e18F8Df7c37"
        self.assertEqual(VALIDATOR.validate_bootstrap_admin("development", address), address)

    def test_unfrozen_testnet_bootstrap_admin_is_rejected(self) -> None:
        path = self.bundle / "artifacts/usdb-chain-bootstrap-config.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["bootstrapAdmin"]["address"] = "0x1111111111111111111111111111111111111111"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected testnet-v0 bootstrap admin"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_genesis_bootstrap_admin_allocation_is_required(self) -> None:
        path = self.bundle / "artifacts/usdb-genesis.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        del value["alloc"][VALIDATOR.EXPECTED_BOOTSTRAP_ADMIN[2:].lower()]
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "admin allocation is missing"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_genesis_bootstrap_admin_balance_must_match_config(self) -> None:
        path = self.bundle / "artifacts/usdb-genesis.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["alloc"][VALIDATOR.EXPECTED_BOOTSTRAP_ADMIN[2:].lower()]["balance"] = "0x1"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "admin balance mismatch"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_sourcedao_bootstrap_admin_must_match_chain_config(self) -> None:
        path = self.bundle / "artifacts/sourcedao-bootstrap-config.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["bootstrapAdminAddress"] = "0x1111111111111111111111111111111111111111"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "bootstrap admin mismatch"):
            VALIDATOR.validate_network_bundle(self.bundle)

    def test_genesis_must_not_fund_known_development_admin(self) -> None:
        path = self.bundle / "artifacts/usdb-genesis.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        value["alloc"]["abcd35afbb4561213feaff01b5f91e18f8df7c37"] = {
            "balance": "0x1"
        }
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "must not fund a known development"):
            VALIDATOR.validate_network_bundle(self.bundle)

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
            self.validate_node_env(path, False)

    def test_versioned_image_tag_is_rejected(self) -> None:
        path = self.write_node_env(USDB_CHAIN_IMAGE="ghcr.io/buckyos/usdb-chain:testnet-v0-r1")
        with self.assertRaisesRegex(ValueError, "canonical GHCR digest reference"):
            self.validate_node_env(path, False)

    def test_mutable_bitcoin_image_is_rejected(self) -> None:
        path = self.write_node_env(USDB_BITCOIN_IMAGE="ghcr.io/buckyos/usdb-bitcoin-core:28.1")
        with self.assertRaisesRegex(ValueError, "canonical GHCR digest reference"):
            self.validate_node_env(path, False)

    def test_external_bitcoin_rpc_is_rejected(self) -> None:
        path = self.write_node_env(BTC_RPC_URL="http://host.docker.internal:8332")
        with self.assertRaisesRegex(ValueError, "private btc-node endpoint"):
            self.validate_node_env(path, False)

    def test_public_bitcoin_p2p_bind_is_supported(self) -> None:
        path = self.write_node_env(BTC_P2P_BIND_ADDRESS="0.0.0.0")
        self.validate_node_env(path, False)

    def test_operator_ssh_port_must_be_valid(self) -> None:
        for value in ("", "0", "65536", "not-a-port"):
            with self.subTest(value=value):
                path = self.write_node_env(USDB_OPERATOR_SSH_PORT=value)
                with self.assertRaisesRegex(ValueError, "USDB_OPERATOR_SSH_PORT"):
                    self.validate_node_env(path, False)

    def test_firewall_mode_must_be_external_or_managed(self) -> None:
        for value in ("", "disabled", "ufw"):
            with self.subTest(value=value):
                path = self.write_node_env(USDB_FIREWALL_MODE=value)
                with self.assertRaisesRegex(ValueError, "USDB_FIREWALL_MODE"):
                    self.validate_node_env(path, False)

    def test_legacy_node_env_without_firewall_mode_keeps_managed_semantics(self) -> None:
        path = self.write_node_env()
        content = "\n".join(
            line
            for line in path.read_text(encoding="utf-8").splitlines()
            if not line.startswith("USDB_FIREWALL_MODE=")
        ) + "\n"
        path.write_text(content, encoding="utf-8")

        self.validate_node_env(path, False)

    def test_noncanonical_bitcoin_p2p_bind_is_rejected(self) -> None:
        path = self.write_node_env(BTC_P2P_BIND_ADDRESS="192.0.2.10")
        with self.assertRaisesRegex(ValueError, "must be explicit loopback-only or public"):
            self.validate_node_env(path, False)

    def test_empty_bitcoin_p2p_bind_is_rejected(self) -> None:
        path = self.write_node_env(BTC_P2P_BIND_ADDRESS="")
        with self.assertRaisesRegex(ValueError, "must be explicit loopback-only or public"):
            self.validate_node_env(path, False)

    def test_public_operator_rpc_bind_is_rejected(self) -> None:
        path = self.write_node_env(USDB_HTTP_BIND_ADDRESS="0.0.0.0")
        with self.assertRaisesRegex(ValueError, "USDB_HTTP_BIND_ADDRESS must be loopback-only"):
            self.validate_node_env(path, False)

    def test_miner_requires_address(self) -> None:
        path = self.write_node_env(USDB_NODE_ROLE="miner")
        with self.assertRaisesRegex(ValueError, "USDB_MINER_ADDRESS"):
            self.validate_node_env(path, False)

    def test_miner_accepts_address_without_fixed_pass(self) -> None:
        path = self.write_node_env(
            USDB_NODE_ROLE="miner",
            USDB_MINER_ADDRESS="0x1111111111111111111111111111111111111111",
        )
        self.validate_node_env(path, False)

    def test_miner_rejects_invalid_address(self) -> None:
        path = self.write_node_env(
            USDB_NODE_ROLE="miner",
            USDB_MINER_ADDRESS="0x1234",
        )
        with self.assertRaisesRegex(ValueError, "non-zero EVM address"):
            self.validate_node_env(path, False)

    def test_runtime_accepts_full_sync_without_snapshot_files(self) -> None:
        path = self.write_node_env()
        self.validate_node_env(path, True)

    def test_runtime_requires_signed_snapshot_set_when_enabled(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963800,
            "snapshot_963800",
        )
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
        )
        self.validate_node_env(path, True)

    def test_runtime_accepts_snapshot_below_network_origin_with_dynamic_name(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963700,
            "release-bootstrap-a",
        )
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
        )
        self.validate_node_env(path, True)

    def test_runtime_accepts_snapshot_in_immutable_release_directory(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963800,
            "release-bootstrap-b",
            "balance-history-bitcoin-h963800-0123456789abcdef",
        )
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
        )
        self.validate_node_env(path, True)

    def test_runtime_rejects_snapshot_above_network_origin(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(963801, "future-snapshot")
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
        )
        with self.assertRaisesRegex(ValueError, "exceeds network index origin 963800"):
            self.validate_node_env(path, True)

    def test_runtime_accepts_paired_checkpoint_above_network_origin(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963900,
            "paired-snapshot",
        )
        checkpoint_manifest = self.write_indexer_checkpoint_artifacts(
            963900,
            Path(snapshot_manifest).name,
        )
        path = self.write_node_env(
            SNAPSHOT_MODE="paired-checkpoint",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
            USDB_INDEXER_CHECKPOINT_MANIFEST=checkpoint_manifest,
        )
        self.validate_node_env(path, True)

    def test_runtime_rejects_paired_checkpoint_state_mismatch(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963900,
            "paired-mismatch",
        )
        checkpoint_manifest = self.write_indexer_checkpoint_artifacts(
            963900,
            Path(snapshot_manifest).name,
        )
        checkpoint_path = self.root / "snapshot" / Path(checkpoint_manifest).parent.name / Path(checkpoint_manifest).name
        checkpoint = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        checkpoint["state_identity"]["snapshot_id"] = "9" * 64
        checkpoint_path.write_text(json.dumps(checkpoint), encoding="utf-8")
        path = self.write_node_env(
            SNAPSHOT_MODE="paired-checkpoint",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
            USDB_INDEXER_CHECKPOINT_MANIFEST=checkpoint_manifest,
        )
        with self.assertRaisesRegex(ValueError, "normalized upstream identity mismatch"):
            self.validate_node_env(path, True)

    def test_runtime_rejects_paired_checkpoint_without_tool_version(self) -> None:
        snapshot_file, snapshot_manifest = self.write_snapshot_artifacts(
            963900,
            "paired-no-tool-version",
        )
        checkpoint_manifest = self.write_indexer_checkpoint_artifacts(
            963900,
            Path(snapshot_manifest).name,
        )
        checkpoint_path = (
            self.root
            / "snapshot"
            / Path(checkpoint_manifest).parent.name
            / Path(checkpoint_manifest).name
        )
        checkpoint = json.loads(checkpoint_path.read_text(encoding="utf-8"))
        del checkpoint["tool_version"]
        checkpoint_path.write_text(json.dumps(checkpoint), encoding="utf-8")
        path = self.write_node_env(
            SNAPSHOT_MODE="paired-checkpoint",
            BH_SNAPSHOT_FILE=snapshot_file,
            BH_SNAPSHOT_MANIFEST=snapshot_manifest,
            USDB_INDEXER_CHECKPOINT_MANIFEST=checkpoint_manifest,
        )
        with self.assertRaisesRegex(ValueError, "tool_version is required"):
            self.validate_node_env(path, True)

    def test_full_sync_rejects_stale_snapshot_inputs(self) -> None:
        path = self.write_node_env(BH_SNAPSHOT_FILE="/snapshots/snapshot_963800.db")
        with self.assertRaisesRegex(ValueError, "requires empty BH_SNAPSHOT_FILE"):
            self.validate_node_env(path, True)

    def test_snapshot_mode_requires_canonical_container_paths(self) -> None:
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE="/tmp/snapshot.db",
            BH_SNAPSHOT_MANIFEST="/snapshots/snapshot_963800.manifest.json",
        )
        with self.assertRaisesRegex(ValueError, "direct child of /snapshots"):
            self.validate_node_env(path, False)

    def test_snapshot_mode_requires_matching_dynamic_basenames(self) -> None:
        path = self.write_node_env(
            SNAPSHOT_MODE="balance-history",
            BH_SNAPSHOT_FILE="/snapshots/bootstrap-a.db",
            BH_SNAPSHOT_MANIFEST="/snapshots/bootstrap-b.manifest.json",
        )
        with self.assertRaisesRegex(ValueError, "basenames must match"):
            self.validate_node_env(path, False)

    def test_bitcoin_runtime_requires_private_full_node_paths(self) -> None:
        path = self.write_node_env()
        self.write_rpcauth()
        self.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_public_auth_file(self) -> None:
        path = self.write_node_env()
        self.write_rpcauth(0o644)
        with self.assertRaisesRegex(ValueError, "group/world accessible"):
            self.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_password_mismatch(self) -> None:
        path = self.write_node_env(BTC_RPC_PASSWORD="wrong-password")
        self.write_rpcauth()
        with self.assertRaisesRegex(ValueError, "does not match RPC password"):
            self.validate_node_env(path, False, True)

    def test_bitcoin_runtime_rejects_relative_data_path(self) -> None:
        path = self.write_node_env(BTC_NODE_DATA_HOST_DIR="relative/bitcoin")
        with self.assertRaisesRegex(ValueError, "must be an absolute path"):
            self.validate_node_env(path, False)

    def test_rejects_persistent_data_path_outside_root(self) -> None:
        path = self.write_node_env(BH_DATA_HOST_DIR=str(self.root / "other-balance-history"))
        with self.assertRaisesRegex(ValueError, "below USDB_DATA_ROOT"):
            self.validate_node_env(path, False)

    def test_legacy_layout_remains_recognizable_without_being_upgraded(self) -> None:
        path = self.write_node_env(legacy_layout=True)
        self.validate_node_env(path, False)
        env = VALIDATOR.read_env(path)
        self.assertNotIn("USDB_DATA_LAYOUT", env)
        self.assertEqual(
            env["USDB_CHAIN_DATA_HOST_DIR"],
            str(self.root / "node-data/usdb-chain"),
        )

    def test_v2_layout_rejects_runtime_compatibility_mismatch(self) -> None:
        path = self.write_node_env()
        with self.assertRaisesRegex(ValueError, "selected release"):
            VALIDATOR.validate_node_env(
                path,
                self.network,
                False,
                False,
                None,
                "f" * 64,
            )


if __name__ == "__main__":
    unittest.main()
