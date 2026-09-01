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
        network_identity = NODE.build_network_identity(self.bundle)
        self.manifest = {
            "schema_version": "usdb-release-manifest:v5",
            "release_id": "usdb-testnet-v0-r1",
            "network_bundle": network_identity,
            "snapshot": NODE.build_snapshot_state(self.bundle),
            "runtime_compatibility": NODE.build_runtime_compatibility(network_identity),
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
        expected_paths = NODE._data_directories(layout, data_root)
        self.assertEqual(env["USDB_SERVICES_IMAGE"], self.manifest["images"]["usdb_services"]["reference"])
        self.assertEqual(env["USDB_DATA_ROOT"], str(data_root))
        self.assertEqual(env["USDB_DATA_LAYOUT"], NODE.DATA_LAYOUT_VERSION)
        self.assertEqual(
            env["USDB_RUNTIME_COMPATIBILITY_ID"],
            layout.runtime_compatibility["compatibility_id"],
        )
        for key, expected in expected_paths.items():
            self.assertEqual(env[key], str(expected))
        self.assertEqual(env["BTC_RPC_USER"], "node-a")
        self.assertTrue(env["BTC_RPC_PASSWORD"])
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(env["USDB_OPERATOR_SSH_PORT"], "22")
        self.assertEqual(env["USDB_NODE_ROLE"], "full")
        self.assertEqual(env["USDB_NAT"], "extip:203.0.113.10")
        self.assertEqual(stat.S_IMODE(node_env.stat().st_mode), 0o600)
        rpcauth = NODE.network_secure_dir(data_root, layout.bundle_id) / "bitcoin-mainnet-rpcauth"
        self.assertTrue(rpcauth.is_file())
        self.assertEqual(stat.S_IMODE(rpcauth.stat().st_mode), 0o600)
        for key, path in expected_paths.items():
            self.assertTrue(path.is_dir())
            marker = path / NODE.DATASET_IDENTITY_FILE
            self.assertTrue(marker.is_file())
            self.assertEqual(
                json.loads(marker.read_text(encoding="utf-8"))["service"],
                NODE.PERSISTENT_DATA_SERVICES[key],
            )

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
                "2222",
                "n",
                "y",
                "n",
            ]
        )
        output = io.StringIO()
        result = NODE.setup_node(
            layout,
            input_fn=lambda _prompt: next(answers),
            output=output,
        )
        env = NODE.read_env(layout.node_env)
        self.assertEqual(result.node_env, layout.node_env)
        self.assertFalse(result.apply_firewall)
        self.assertFalse(result.install_snapshot)
        self.assertEqual(env["USDB_NODE_ROLE"], "miner")
        self.assertEqual(env["USDB_MINER_ADDRESS"], address)
        self.assertEqual(env["USDB_MINER_THREADS"], "2")
        self.assertEqual(env["USDB_BOOTNODES"], "enode://example")
        self.assertEqual(env["BTC_P2P_BIND_ADDRESS"], "127.0.0.1")
        self.assertEqual(env["USDB_OPERATOR_SSH_PORT"], "2222")
        self.assertEqual(
            env["BTC_RPC_USER"],
            NODE.default_bitcoin_rpc_user(layout),
        )
        self.assertIn("USDB P2P: public TCP/UDP 31303", output.getvalue())
        self.assertIn("Release-approved balance-history snapshot", output.getvalue())

    def test_setup_cancellation_writes_no_config_or_credentials(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "cancelled-data"
        answers = iter([str(data_root), "full", "", "n", "22", "n", "n"])
        with self.assertRaisesRegex(ValueError, "setup cancelled"):
            NODE.setup_node(
                layout,
                input_fn=lambda _prompt: next(answers),
                output=io.StringIO(),
            )
        self.assertFalse(layout.node_env.exists())
        self.assertFalse(
            (NODE.network_secure_dir(data_root, layout.bundle_id) / "bitcoin-mainnet-rpcauth").exists()
        )

    def test_configure_rejects_non_empty_unmarked_dataset(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "unmarked-data"
        bitcoin = NODE._data_directories(layout, data_root)["BTC_NODE_DATA_HOST_DIR"]
        bitcoin.mkdir(parents=True)
        (bitcoin / "blocks").mkdir()
        with self.assertRaisesRegex(ValueError, "non-empty unmarked bitcoin_core"):
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

    def test_setup_can_select_the_release_approved_snapshot(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "snapshot-setup-data"
        answers = iter([str(data_root), "full", "", "n", "22", "y", "y", "n"])
        with mock.patch.object(
            NODE,
            "_disk_free_bytes",
            return_value=NODE._snapshot_required_free_bytes(layout.snapshot),
        ):
            result = NODE.setup_node(
                layout,
                input_fn=lambda _prompt: next(answers),
                output=io.StringIO(),
            )
        self.assertTrue(result.install_snapshot)
        self.assertFalse(result.apply_firewall)
        self.assertEqual(NODE.read_env(result.node_env)["SNAPSHOT_MODE"], "none")

    def test_detect_ssh_server_port_uses_server_side_port(self) -> None:
        self.assertEqual(
            NODE.detect_ssh_server_port(
                {"SSH_CONNECTION": "198.51.100.8 50000 203.0.113.5 2222"}
            ),
            2222,
        )
        self.assertEqual(NODE.detect_ssh_server_port({}), 22)
        self.assertEqual(NODE.detect_ssh_server_port({"SSH_CONNECTION": "invalid"}), 22)

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

    def test_host_and_firewall_actions_delegate_with_frozen_node_configuration(self) -> None:
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
            bitcoin_p2p="public",
            ssh_port=2222,
        )
        calls: list[tuple[str, tuple[str, ...], bool]] = []

        def record(
            _layout: object,
            helper: str,
            arguments: list[str],
            *,
            check: bool = True,
            **_kwargs: object,
        ) -> object:
            calls.append((helper, tuple(arguments), check))
            return mock.Mock(returncode=0)

        with mock.patch.object(NODE, "run_helper", side_effect=record):
            NODE.run_host_action(layout, "check", docker_user="usdb")
            NODE.run_firewall_action(layout, "apply", confirm=True)

        self.assertEqual(
            calls,
            [
                ("prepare_usdb_host.sh", ("check", "--docker-user", "usdb"), True),
                (
                    "prepare_usdb_firewall.sh",
                    (
                        "apply",
                        "--node-env",
                        str(layout.node_env),
                        "--ssh-port",
                        "2222",
                        "--bitcoin-p2p",
                        "public",
                        "--confirm",
                    ),
                    True,
                ),
            ],
        )

    def test_prepare_host_only_installs_after_interactive_confirmation(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        calls: list[tuple[str, bool]] = []

        def record(
            _layout: object,
            action: str,
            *,
            check: bool = True,
            **_kwargs: object,
        ) -> object:
            calls.append((action, check))
            return mock.Mock(returncode=1 if action == "check" else 0)

        with (
            mock.patch.object(NODE, "run_host_action", side_effect=record),
            mock.patch.object(NODE.sys, "stdin", mock.Mock(isatty=lambda: True)),
            mock.patch.object(NODE.sys, "stdout", mock.Mock(isatty=lambda: True)),
        ):
            NODE.prepare_host(
                layout,
                docker_user="usdb",
                input_fn=lambda _prompt: "yes",
                output=io.StringIO(),
            )
        self.assertEqual(calls, [("check", False), ("install", True)])

    def test_doctor_checks_host_runtime_identity_and_firewall(self) -> None:
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
        with (
            mock.patch.object(NODE, "run_host_action") as host,
            mock.patch.object(NODE, "run_helper") as helper,
            mock.patch.object(NODE, "run_firewall_action") as firewall,
        ):
            NODE.doctor(layout)
        host.assert_called_once_with(
            layout,
            "check",
            docker_user=NODE.default_docker_user(),
        )
        helper.assert_called_once_with(layout, "run_testnet_runtime.sh", ["validate-node"])
        firewall.assert_called_once_with(layout, "check")

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

        incompatible = dict(layout.runtime_compatibility)
        incompatible["compatibility_id"] = "8" * 64
        blocked_layout = replace(
            next_layout,
            release_id="usdb-testnet-v0-r3",
            runtime_compatibility=incompatible,
        )
        before = layout.node_env.read_text(encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "runtime compatibility ID"):
            NODE.activate_release(blocked_layout)
        self.assertEqual(layout.node_env.read_text(encoding="utf-8"), before)

    def test_activate_release_rejects_tampered_dataset_marker_atomically(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "tampered-marker-data"
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
        marker = (
            NODE._data_directories(layout, data_root)["USDB_INDEXER_DATA_HOST_DIR"]
            / NODE.DATASET_IDENTITY_FILE
        )
        marker.write_text("{}\n", encoding="utf-8")
        replacement_images = {
            key: value[:-64] + "7" * 64 for key, value in layout.images.items()
        }
        next_layout = replace(layout, release_id="usdb-testnet-v0-r2", images=replacement_images)
        before = layout.node_env.read_text(encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "dataset identity mismatch"):
            NODE.activate_release(next_layout)
        self.assertEqual(layout.node_env.read_text(encoding="utf-8"), before)

    def test_activate_release_rejects_legacy_node_config(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "legacy-data"
        legacy_paths = NODE.build_legacy_data_paths(data_root)
        for path in legacy_paths.values():
            path.mkdir(parents=True)
        template = (layout.bundle_dir / "node.env.example").read_text(encoding="utf-8")
        updates = {
            **layout.images,
            "USDB_DATA_ROOT": str(data_root),
            **{key: str(value) for key, value in legacy_paths.items()},
            "BTC_RPCAUTH_HOST_FILE": str(data_root / "secure/bitcoin-mainnet-rpcauth"),
            "BTC_RPC_USER": "node-a",
            "BTC_RPC_PASSWORD": "secret",
            "BH_SNAPSHOT_HOST_DIR": str(data_root / "releases/balance-history"),
        }
        legacy = NODE.render_env(template, updates)
        legacy = "\n".join(
            line
            for line in legacy.splitlines()
            if not line.startswith("USDB_DATA_LAYOUT=")
            and not line.startswith("USDB_RUNTIME_COMPATIBILITY_ID=")
        ) + "\n"
        NODE._atomic_write_private(layout.node_env, legacy)
        with self.assertRaisesRegex(ValueError, "legacy node data layout"):
            NODE.activate_release(layout)

    def _installed_snapshot_fixture(self, data_root: Path) -> mock.Mock:
        record = NODE._approved_snapshot_record(NODE.load_release_layout(self.root, self.node_env))
        release_id = record["snapshot_release_id"]
        by_role = {item["role"]: item for item in record["files"]}
        release_dir = NODE.snapshot_artifact_dir(data_root) / release_id
        release_dir.mkdir(parents=True)
        snapshot = release_dir / by_role["snapshot_db"]["path"]
        snapshot.write_bytes(b"snapshot")
        manifest = release_dir / by_role["snapshot_manifest"]["path"]
        manifest.write_text(
            json.dumps(
                {
                    "manifest_version": "balance-history-snapshot-manifest:v3",
                    "file_name": snapshot.name,
                    "file_sha256": hashlib.sha256(snapshot.read_bytes()).hexdigest(),
                    "state_ref": {
                        "block_height": 963800,
                        "stable_block_hash": record["btc_block_hash"],
                        "snapshot_id": record["snapshot_id"],
                    },
                    "db_identity": {"btc_network": "bitcoin"},
                    "balance_query_floor": 963800,
                    "history_query_floor": 963801,
                    "signature_scheme": "ed25519",
                    "signing_key_id": record["trusted_keys"]["signing_key_id"],
                }
            ),
            encoding="utf-8",
        )
        signature = release_dir / by_role["snapshot_signature"]["path"]
        signature.write_text("signature", encoding="utf-8")
        return mock.Mock(
            release_id=release_id,
            release_dir=release_dir,
            snapshot_file=snapshot,
            manifest_file=manifest,
            signature_file=signature,
            height=963800,
            network="bitcoin",
        )

    def test_snapshot_install_selects_nested_release_before_first_start(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "snapshot-node-data"
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
        installed = self._installed_snapshot_fixture(data_root)
        with mock.patch.object(NODE, "install_snapshot_artifact", return_value=installed) as install:
            result = NODE.install_snapshot_release(layout)
        self.assertEqual(result, installed.release_dir)
        env = NODE.read_env(layout.node_env)
        self.assertEqual(env["SNAPSHOT_MODE"], "balance-history")
        self.assertEqual(
            env["BH_SNAPSHOT_FILE"],
            f"/snapshots/{installed.release_id}/{installed.snapshot_file.name}",
        )
        self.assertEqual(
            env["BH_SNAPSHOT_MANIFEST"],
            f"/snapshots/{installed.release_id}/{installed.manifest_file.name}",
        )
        install.assert_called_once()
        self.assertEqual(install.call_args.kwargs["expected_network"], "bitcoin")
        self.assertEqual(install.call_args.kwargs["max_height"], 963800)
        self.assertEqual(
            install.call_args.kwargs["download_concurrency"],
            NODE.DEFAULT_DOWNLOAD_CONCURRENCY,
        )
        self.assertEqual(
            install.call_args.kwargs["download_chunk_size_mib"],
            NODE.DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
        )
        self.assertEqual(
            install.call_args.kwargs["record_url"],
            layout.snapshot["record"]["url"],
        )
        self.assertEqual(
            install.call_args.kwargs["approved_record_path"],
            layout.bundle_dir / layout.snapshot["record"]["path"],
        )

    def test_snapshot_install_rejects_initialized_database_before_download(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "initialized-node-data"
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
        database = (
            NODE._data_directories(layout, data_root)["BH_DATA_HOST_DIR"] / "db"
        )
        database.mkdir()
        (database / "CURRENT").write_text("initialized", encoding="utf-8")
        with mock.patch.object(NODE, "install_snapshot_artifact") as install:
            with self.assertRaisesRegex(ValueError, "after balance-history DB initialization"):
                NODE.install_snapshot_release(layout)
        install.assert_not_called()

    def test_snapshot_selection_survives_interrupted_download_and_blocks_runtime(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "interrupted-snapshot-data"
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
        with mock.patch.object(
            NODE,
            "install_snapshot_artifact",
            side_effect=ValueError("download interrupted"),
        ):
            with self.assertRaisesRegex(ValueError, "download interrupted"):
                NODE.install_snapshot_release(layout)
        env = NODE.read_env(layout.node_env)
        self.assertEqual(env["SNAPSHOT_MODE"], "balance-history")
        network = NODE.validate_network_bundle(layout.bundle_dir)
        with self.assertRaisesRegex(ValueError, "required snapshot artifact is missing"):
            NODE.validate_node_env(layout.node_env, network, True, False)

    def test_snapshot_cli_rejects_arbitrary_record_url(self) -> None:
        parser = NODE.build_parser()
        with mock.patch("sys.stderr", new=io.StringIO()):
            with self.assertRaises(SystemExit):
                parser.parse_args(
                    [
                        "snapshot",
                        "install",
                        "--record-url",
                        "https://unapproved.example/snapshot-record.json",
                    ]
                )

    def test_snapshot_cli_accepts_advanced_parallel_download_overrides(self) -> None:
        args = NODE.build_parser().parse_args(
            [
                "snapshot",
                "install",
                "--download-concurrency",
                "12",
                "--download-chunk-size-mib",
                "128",
            ]
        )
        self.assertEqual(args.download_concurrency, 12)
        self.assertEqual(args.download_chunk_size_mib, 128)

    def test_snapshot_install_rejects_invalid_parallel_download_settings_before_node_env_read(self) -> None:
        layout = mock.Mock()
        layout.node_env = mock.Mock()
        layout.node_env.is_file.side_effect = AssertionError("node env should not be inspected")

        with self.assertRaisesRegex(ValueError, "download concurrency"):
            NODE.install_snapshot_release(layout, download_concurrency=0)
        with self.assertRaisesRegex(ValueError, "download chunk size"):
            NODE.install_snapshot_release(layout, download_chunk_size_mib=0)


if __name__ == "__main__":
    unittest.main()
