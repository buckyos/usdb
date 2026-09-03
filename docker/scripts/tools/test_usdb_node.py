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
        self.capacity_patcher = mock.patch.object(
            NODE,
            "_data_root_capacity",
            return_value=NODE.DataRootCapacity(
                filesystem_path=Path(self.temporary.name),
                total_bytes=3 * 1024**4,
                free_bytes=3 * 1024**4,
            ),
        )
        self.capacity_patcher.start()
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
            "schema_version": "usdb-release-manifest:v6",
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
        self.capacity_patcher.stop()
        self.temporary.cleanup()

    def write_manifest(self) -> None:
        content = json.dumps(self.manifest, indent=2, sort_keys=True) + "\n"
        self.manifest_path.write_text(content, encoding="utf-8")
        digest = hashlib.sha256(content.encode()).hexdigest()
        self.manifest_path.with_name(self.manifest_path.name + ".sha256").write_text(
            f"{digest}  {self.manifest_path.name}\n",
            encoding="utf-8",
        )

    def configure_full_node(self, layout: object, name: str) -> None:
        NODE.configure_node(
            layout,
            data_root=Path(self.temporary.name) / name,
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="",
            nat="",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
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
        self.assertEqual(env["USDB_FIREWALL_MODE"], "external")
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
                "n",
                "n",
                "y",
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
        self.assertEqual(env["USDB_FIREWALL_MODE"], "external")
        self.assertEqual(env["USDB_OPERATOR_SSH_PORT"], "22")
        self.assertEqual(
            env["BTC_RPC_USER"],
            NODE.default_bitcoin_rpc_user(layout),
        )
        self.assertIn("USDB P2P: public TCP/UDP 31303", output.getvalue())
        self.assertIn("Release-approved balance-history snapshot", output.getvalue())
        self.assertIn("Required/recommended: 1.5 TiB / 2.0 TiB", output.getvalue())

    def test_configure_rejects_small_or_insufficient_data_root_before_writes(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "insufficient-data-root"
        cases = (
            (
                NODE.DataRootCapacity(
                    filesystem_path=Path(self.temporary.name),
                    total_bytes=NODE.MIN_DATA_ROOT_BYTES - 1,
                    free_bytes=NODE.MIN_DATA_ROOT_BYTES - 1,
                ),
                "filesystem is too small",
            ),
            (
                NODE.DataRootCapacity(
                    filesystem_path=Path(self.temporary.name),
                    total_bytes=NODE.RECOMMENDED_DATA_ROOT_BYTES,
                    free_bytes=NODE.MIN_DATA_ROOT_BYTES - 1,
                ),
                "insufficient available space",
            ),
        )
        for capacity, message in cases:
            with self.subTest(message=message):
                with mock.patch.object(
                    NODE,
                    "_data_root_capacity",
                    return_value=capacity,
                ):
                    with self.assertRaisesRegex(ValueError, message):
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
                self.assertFalse(layout.node_env.exists())
                self.assertFalse(data_root.exists())

    def test_setup_rejects_insufficient_data_root_before_other_prompts(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "small-setup-data-root"
        answers = iter([str(data_root)])
        with mock.patch.object(
            NODE,
            "_data_root_capacity",
            return_value=NODE.DataRootCapacity(
                filesystem_path=Path(self.temporary.name),
                total_bytes=NODE.RECOMMENDED_DATA_ROOT_BYTES,
                free_bytes=NODE.MIN_DATA_ROOT_BYTES - 1,
            ),
        ):
            with self.assertRaisesRegex(ValueError, "insufficient available space"):
                NODE.setup_node(
                    layout,
                    input_fn=lambda _prompt: next(answers),
                    output=io.StringIO(),
                )
        self.assertFalse(layout.node_env.exists())
        self.assertFalse(data_root.exists())

    def test_setup_cancellation_writes_no_config_or_credentials(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "cancelled-data"
        answers = iter([str(data_root), "full", "", "n", "n", "n", "n"])
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
        answers = iter([str(data_root), "full", "", "n", "n", "y", "y"])
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
        env = NODE.read_env(result.node_env)
        self.assertEqual(env["SNAPSHOT_MODE"], "balance-history")
        self.assertTrue(env["BH_SNAPSHOT_FILE"].startswith("/snapshots/"))
        self.assertTrue(env["BH_SNAPSHOT_MANIFEST"].startswith("/snapshots/"))

    def test_setup_can_select_managed_ufw(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "managed-firewall-data"
        answers = iter([str(data_root), "full", "", "n", "y", "22", "n", "y"])

        result = NODE.setup_node(
            layout,
            input_fn=lambda _prompt: next(answers),
            output=io.StringIO(),
        )

        self.assertTrue(result.apply_firewall)
        self.assertEqual(
            NODE.read_env(result.node_env)["USDB_FIREWALL_MODE"],
            "managed",
        )

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

        output = io.StringIO()
        with (
            mock.patch.object(NODE, "doctor"),
            mock.patch.object(NODE, "run_helper", side_effect=record),
            mock.patch("sys.stdout", new=output),
        ):
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
        rendered = output.getvalue()
        for phase in (
            "preflight",
            "images",
            "bitcoin",
            "balance-history",
            "balance-history-readiness",
            "indexer-and-chain",
            "ready",
        ):
            self.assertIn(f"phase={phase}:", rendered)

    def test_dashboard_suppresses_only_duplicate_helper_heartbeats(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)

        environment = NODE._helper_environment(
            layout,
            sync_timeout_secs=123,
            quiet_progress=True,
        )

        self.assertEqual(environment["BTC_READY_WAIT_TIMEOUT_SECS"], "123")
        self.assertEqual(environment["BTC_READY_PROGRESS_INTERVAL_SECS"], "0")
        self.assertEqual(environment["USDB_READINESS_PROGRESS_INTERVAL_SECS"], "0")
        self.assertEqual(environment["USDB_TESTNET_BUNDLE_DIR"], str(layout.bundle_dir))

    def test_parallel_snapshot_progress_counts_completed_ranges_not_preallocation(self) -> None:
        staging = Path(self.temporary.name) / "snapshot-staging"
        staging.mkdir()
        snapshot = staging / "snapshot.db"
        with snapshot.open("wb") as output:
            output.truncate(100)
        part = staging / "snapshot.db.part"
        with part.open("wb") as output:
            output.truncate(100)
        (staging / "snapshot.db.part.ranges.json").write_text(
            json.dumps(
                {
                    "chunk_size_bytes": 25,
                    "completed_chunks": [0, 2],
                }
            ),
            encoding="utf-8",
        )
        snapshot.unlink()
        (staging / "manifest.json").write_bytes(b"0123456789")
        record = {
            "files": [
                {"path": "snapshot.db", "size": 100},
                {"path": "manifest.json", "size": 10},
            ]
        }

        self.assertEqual(NODE._snapshot_staging_bytes(staging, record), 60)

    def test_progress_renderer_has_stable_component_rows(self) -> None:
        components = [
            NODE._component_progress(component_id, "WAITING", "pending")
            for component_id, _label in NODE.PROGRESS_COMPONENTS
        ]
        report = {
            "release_id": "usdb-testnet-v0-r1",
            "observed_at": "2026-09-02T00:00:00+00:00",
            "overall_state": "WAITING",
            "components": components,
        }

        rendered = NODE.render_node_progress(report, phase="bitcoin", width=120)

        self.assertIn("phase=bitcoin", rendered)
        positions = [rendered.index(label) for _component_id, label in NODE.PROGRESS_COMPONENTS]
        self.assertEqual(positions, sorted(positions))
        self.assertEqual(rendered.count("[------------------------]"), 5)

    def test_terminal_progress_display_uses_alternate_screen(self) -> None:
        class TtyBuffer(io.StringIO):
            def isatty(self) -> bool:
                return True

        output = TtyBuffer()
        with mock.patch.dict(NODE.os.environ, {"TERM": "xterm-256color"}):
            display = NODE.TerminalProgressDisplay(output)
            display.start()
            display.render("first frame")
            display.render("second frame")
            display.close()

        rendered = output.getvalue()
        self.assertEqual(rendered.count(NODE.ALT_SCREEN_ENTER), 1)
        self.assertEqual(rendered.count(NODE.ALT_SCREEN_EXIT), 1)
        self.assertEqual(rendered.count(NODE.SCREEN_CLEAR), 2)
        self.assertIn(NODE.CURSOR_HIDE, rendered)
        self.assertIn(NODE.CURSOR_SHOW, rendered)

    def test_terminal_progress_display_falls_back_for_dumb_terminal(self) -> None:
        class TtyBuffer(io.StringIO):
            def isatty(self) -> bool:
                return True

        output = TtyBuffer()
        with mock.patch.dict(NODE.os.environ, {"TERM": "dumb"}):
            display = NODE.TerminalProgressDisplay(output)
            display.start()
            display.render("plain frame")
            display.close()

        self.assertFalse(display.live)
        self.assertEqual(output.getvalue(), "plain frame\n")

    def test_run_helper_uses_release_kit_as_working_directory(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        helper = layout.kit_root / "docker/scripts/tools/test-helper.sh"
        helper.parent.mkdir(parents=True, exist_ok=True)
        helper.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        completed = NODE.subprocess.CompletedProcess([str(helper)], 0, "", "")

        with mock.patch.object(NODE.subprocess, "run", return_value=completed) as run:
            NODE.run_helper(layout, helper.name, ["status"], capture_output=True)

        self.assertEqual(run.call_args.kwargs["cwd"], str(layout.kit_root))

    def test_snapshot_artifact_ready_waits_for_live_rocksdb_import(self) -> None:
        data_root = Path(self.temporary.name) / "snapshot-import-waiting"
        env = {
            "SNAPSHOT_MODE": "balance-history",
            "BH_DATA_HOST_DIR": str(data_root),
            "BH_SNAPSHOT_FILE": "/snapshots/release/snapshot.db",
            "BH_SNAPSHOT_MANIFEST": "/snapshots/release/snapshot.manifest.json",
        }
        artifact = {
            "state": "installed",
            "summary": "release-approved snapshot artifact is verified",
            "completed_bytes": 100,
            "expected_bytes": 100,
        }

        component = NODE._snapshot_component(artifact, env, None)

        self.assertEqual(component["state"], "WAITING")
        self.assertEqual(component["progress_percent"], 100.0)
        self.assertIn("after the Bitcoin readiness gate", component["detail"])

    def test_snapshot_import_reports_stage_and_aggregate_progress(self) -> None:
        data_root = Path(self.temporary.name) / "snapshot-import-running"
        progress_path = data_root / "bootstrap/snapshot-loader.progress.json"
        progress_path.parent.mkdir(parents=True)
        selected_file = "/snapshots/release/snapshot.db"
        progress_path.write_text(
            json.dumps(
                {
                    "schema_version": NODE.SNAPSHOT_IMPORT_PROGRESS_SCHEMA_VERSION,
                    "state": "running",
                    "stage": "script_registry",
                    "stage_index": 6,
                    "stage_count": 8,
                    "current": 75,
                    "total": 100,
                    "unit": "entries",
                    "stage_current": 25,
                    "stage_total": 50,
                    "block_height": 963800,
                    "snapshot_file": selected_file,
                    "message": "importing script registry entries",
                    "updated_at_unix": 1_788_000_000,
                }
            ),
            encoding="utf-8",
        )
        env = {
            "SNAPSHOT_MODE": "balance-history",
            "BH_DATA_HOST_DIR": str(data_root),
            "BH_SNAPSHOT_FILE": selected_file,
            "BH_SNAPSHOT_MANIFEST": "/snapshots/release/snapshot.manifest.json",
        }
        artifact = {"state": "installed", "summary": "artifact verified"}
        loader = {"state": "running", "health": "", "exit_code": None}

        component = NODE._snapshot_component(artifact, env, loader)

        self.assertEqual(component["state"], "IMPORTING")
        self.assertEqual(component["progress_percent"], 75.0)
        self.assertEqual(component["stage"], "script_registry")
        self.assertEqual(component["stage_current"], 25)
        self.assertIn("stage=script registry (6/8)", component["detail"])
        self.assertIn("stage_progress=25/50", component["detail"])

    def test_snapshot_import_ignores_stale_progress_for_selected_artifact(self) -> None:
        data_root = Path(self.temporary.name) / "snapshot-import-stale-progress"
        progress_path = data_root / "bootstrap/snapshot-loader.progress.json"
        progress_path.parent.mkdir(parents=True)
        progress_path.write_text(
            json.dumps(
                {
                    "schema_version": NODE.SNAPSHOT_IMPORT_PROGRESS_SCHEMA_VERSION,
                    "state": "complete",
                    "stage": "complete",
                    "stage_index": 8,
                    "stage_count": 8,
                    "current": 100,
                    "total": 100,
                    "unit": "entries",
                    "stage_current": 1,
                    "stage_total": 1,
                    "block_height": 963800,
                    "snapshot_file": "/snapshots/old/snapshot.db",
                    "message": "old import",
                    "updated_at_unix": 1_788_000_000,
                }
            ),
            encoding="utf-8",
        )
        env = {
            "SNAPSHOT_MODE": "balance-history",
            "BH_DATA_HOST_DIR": str(data_root),
            "BH_SNAPSHOT_FILE": "/snapshots/new/snapshot.db",
            "BH_SNAPSHOT_MANIFEST": "/snapshots/new/snapshot.manifest.json",
        }

        component = NODE._snapshot_component(
            {"state": "installed", "summary": "artifact verified"},
            env,
            {"state": "running", "health": "", "exit_code": None},
        )

        self.assertEqual(component["state"], "IMPORTING")
        self.assertIsNone(component["current"])
        self.assertIn("does not match the selected artifact", component["detail"])

    def test_snapshot_ready_requires_matching_marker_and_nonempty_live_db(self) -> None:
        data_root = Path(self.temporary.name) / "snapshot-import-ready"
        database = data_root / "db"
        database.mkdir(parents=True)
        (database / "CURRENT").write_text("MANIFEST-000001\n", encoding="utf-8")
        selected_file = "/snapshots/release/snapshot.db"
        selected_manifest = "/snapshots/release/snapshot.manifest.json"
        marker = data_root / "bootstrap/snapshot-loader.done.json"
        marker.parent.mkdir(parents=True)
        marker.write_text(
            json.dumps(
                {
                    "snapshot_mode": "balance-history",
                    "snapshot_file": selected_file,
                    "snapshot_manifest": selected_manifest,
                    "installed_at": "2026-09-02T00:00:00Z",
                }
            ),
            encoding="utf-8",
        )
        env = {
            "SNAPSHOT_MODE": "balance-history",
            "BH_DATA_HOST_DIR": str(data_root),
            "BH_SNAPSHOT_FILE": selected_file,
            "BH_SNAPSHOT_MANIFEST": selected_manifest,
        }
        artifact = {
            "state": "installed",
            "summary": "artifact verified",
            "completed_bytes": 100,
            "expected_bytes": 100,
        }

        component = NODE._snapshot_component(
            artifact,
            env,
            {"state": "exited", "health": "", "exit_code": 0},
        )

        self.assertEqual(component["state"], "READY")
        self.assertIn("live balance-history RocksDB", component["detail"])

    def test_snapshot_loader_failure_is_distinct_from_artifact_install(self) -> None:
        data_root = Path(self.temporary.name) / "snapshot-import-failed"
        env = {
            "SNAPSHOT_MODE": "balance-history",
            "BH_DATA_HOST_DIR": str(data_root),
            "BH_SNAPSHOT_FILE": "/snapshots/release/snapshot.db",
            "BH_SNAPSHOT_MANIFEST": "/snapshots/release/snapshot.manifest.json",
        }
        artifact = {"state": "installed", "summary": "artifact verified"}

        component = NODE._snapshot_component(
            artifact,
            env,
            {"state": "exited", "health": "", "exit_code": 1},
        )

        self.assertEqual(component["state"], "FAILED")
        self.assertIn("exit_code=1", component["detail"])

    def test_chain_progress_verifies_identity_and_reports_sync(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        running = {"state": "running", "health": "healthy", "exit_code": None}
        expected_hash = layout.network_identity["genesis_block_hash"]
        results = {
            "eth_chainId": hex(layout.network_identity["chain_id"]),
            "eth_getBlockByNumber": {"hash": expected_hash},
            "eth_syncing": {"currentBlock": "0x32", "highestBlock": "0x64"},
            "eth_blockNumber": "0x32",
            "net_peerCount": "0x3",
        }
        with mock.patch.object(NODE, "_json_rpc_batch", return_value=results):
            component = NODE._chain_component(layout, {}, running)

        self.assertEqual(component["state"], "SYNCING")
        self.assertEqual(component["current"], 50)
        self.assertEqual(component["total"], 100)
        self.assertEqual(component["progress_percent"], 50.0)

        tampered = dict(results)
        tampered["eth_chainId"] = hex(layout.network_identity["chain_id"] + 1)
        with mock.patch.object(NODE, "_json_rpc_batch", return_value=tampered):
            component = NODE._chain_component(layout, {}, running)
        self.assertEqual(component["state"], "BLOCKED")

    def test_progress_view_cross_checks_all_running_components(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "progress-view-data")
        running = {"state": "running", "health": "healthy", "exit_code": None}
        services = {
            "btc-node": running,
            "balance-history": running,
            "usdb-indexer": running,
            "usdb-chain": running,
        }
        bitcoin = {
            "schema_version": "usdb-bitcoin-readiness:v1",
            "ready": True,
            "status": {
                "blocks": 963_800,
                "headers": 963_800,
                "verification_progress": 1.0,
                "txindex_synced": True,
                "txindex_height": 963_800,
                "connections": 8,
            },
            "blockers": [],
        }
        readiness = [
            (
                {
                    "service": "balance-history",
                    "consensus_ready": False,
                    "current": 900_000,
                    "total": 963_800,
                },
                None,
            ),
            (
                {
                    "service": "usdb-indexer",
                    "consensus_ready": True,
                    "current": 963_800,
                    "total": 963_800,
                },
                None,
            ),
        ]
        chain_results = {
            "eth_chainId": hex(layout.network_identity["chain_id"]),
            "eth_getBlockByNumber": {
                "hash": layout.network_identity["genesis_block_hash"]
            },
            "eth_syncing": False,
            "eth_blockNumber": "0x1",
            "net_peerCount": "0x1",
        }
        with (
            mock.patch.object(NODE, "_collect_compose_services", return_value=services),
            mock.patch.object(NODE, "_bitcoin_startup_progress", return_value=bitcoin),
            mock.patch.object(NODE, "_read_service_readiness", side_effect=readiness),
            mock.patch.object(NODE, "_json_rpc_batch", return_value=chain_results),
        ):
            report = NODE.collect_node_progress(layout)

        self.assertEqual(report["schema_version"], "usdb-node-progress:v3")
        self.assertEqual(report["controller_state"], "uninstalled")
        self.assertEqual(report["overall_state"], "SYNCING")
        self.assertEqual(
            [component["state"] for component in report["components"]],
            ["SKIPPED", "READY", "SYNCING", "READY", "READY"],
        )

    def test_progress_view_explains_bitcoin_height_is_unavailable_before_rpc(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "progress-view-bitcoin-starting")
        services = {
            "btc-node": {"state": "running", "health": "starting", "exit_code": None}
        }
        with (
            mock.patch.object(NODE, "_collect_compose_services", return_value=services),
            mock.patch.object(NODE, "_bitcoin_startup_progress", return_value=None),
        ):
            report = NODE.collect_node_progress(layout)

        bitcoin = next(
            component for component in report["components"] if component["id"] == "bitcoin"
        )
        self.assertEqual(bitcoin["state"], "STARTING")
        self.assertIsNone(bitcoin["current"])
        self.assertIn("height is unavailable", bitcoin["detail"])
        self.assertIn("RPC responds", bitcoin["detail"])

    def test_status_parser_exposes_watch_and_progress_json(self) -> None:
        watch = NODE.build_parser().parse_args(["status", "--watch", "--refresh-secs", "2"])
        self.assertTrue(watch.watch)
        self.assertEqual(watch.refresh_secs, 2)
        progress = NODE.build_parser().parse_args(["status", "--progress-json"])
        self.assertTrue(progress.progress_json)

    def test_status_reports_unconfigured_without_runtime_queries(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        with mock.patch.object(NODE, "run_helper") as helper:
            report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "UNCONFIGURED")
        self.assertEqual(report["next_actions"], ["usdb-node setup"])
        helper.assert_not_called()

    def test_status_reports_activation_required_before_runtime_queries(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "activation-required-data")
        next_images = {key: value[:-64] + "9" * 64 for key, value in layout.images.items()}
        next_layout = replace(layout, release_id="usdb-testnet-v0-r2", images=next_images)

        with mock.patch.object(NODE, "run_helper") as helper:
            report = NODE.collect_node_status(next_layout)

        self.assertEqual(report["overall_state"], "ACTIVATION_REQUIRED")
        self.assertEqual(report["up"]["mode"], "explicit")
        self.assertEqual(
            report["next_actions"],
            [
                "usdb-node controller install",
                "usdb-node up --activate-release",
                "usdb-node activate-release",
            ],
        )
        helper.assert_not_called()

    def test_status_reports_resumable_snapshot_before_runtime_queries(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "incomplete-snapshot-data")
        record = NODE._approved_snapshot_record(layout)
        original = layout.node_env.read_text(encoding="utf-8")
        NODE._atomic_write_private(
            layout.node_env,
            NODE.render_env(original, NODE._snapshot_env_updates(record)),
        )
        staging = (
            Path(NODE.read_env(layout.node_env)["BH_SNAPSHOT_HOST_DIR"])
            / f".{record['snapshot_release_id']}.installing"
        )
        staging.mkdir(parents=True)

        with mock.patch.object(NODE, "run_helper") as helper:
            report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "SNAPSHOT_INCOMPLETE")
        self.assertEqual(report["checks"]["snapshot"]["state"], "incomplete")
        self.assertEqual(report["up"]["mode"], "automatic")
        self.assertEqual(
            report["next_actions"],
            [
                "usdb-node controller install",
                "usdb-node up",
                "usdb-node snapshot install",
            ],
        )
        helper.assert_not_called()

    def test_status_stopped_does_not_probe_readiness(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "stopped-status-data")
        calls: list[tuple[str, tuple[str, ...]]] = []

        def stopped(
            _layout: object,
            helper: str,
            arguments: list[str],
            **_kwargs: object,
        ) -> object:
            calls.append((helper, tuple(arguments)))
            return mock.Mock(returncode=0, stdout="[]\n", stderr="")

        with mock.patch.object(NODE, "run_helper", side_effect=stopped):
            report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "READY_TO_START")
        self.assertEqual(
            report["next_actions"],
            ["usdb-node controller install", "usdb-node up"],
        )
        self.assertEqual(
            calls,
            [
                ("run_testnet_bitcoin.sh", ("ps", "--all", "--format", "json")),
                ("run_testnet_runtime.sh", ("ps", "--all", "--format", "json")),
            ],
        )

    def test_status_starting_does_not_emit_derived_readiness_failures(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "starting-status-data")
        calls: list[tuple[str, tuple[str, ...]]] = []

        def starting(
            _layout: object,
            helper: str,
            arguments: list[str],
            **_kwargs: object,
        ) -> object:
            calls.append((helper, tuple(arguments)))
            if helper == "run_testnet_bitcoin.sh":
                output = json.dumps(
                    [{"Service": "btc-node", "State": "running", "Health": "starting"}]
                )
            else:
                output = "[]"
            return mock.Mock(returncode=0, stdout=output, stderr="")

        with mock.patch.object(NODE, "run_helper", side_effect=starting):
            output = io.StringIO()
            with mock.patch("sys.stdout", new=output):
                result = NODE.print_status(layout)

        self.assertEqual(result, 1)
        self.assertEqual(len(calls), 2)
        self.assertNotIn("connection refused", output.getvalue().lower())
        self.assertIn("STARTING", output.getvalue())

    def test_status_reports_bitcoin_ibd_as_starting_with_progress(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "bitcoin-ibd-status-data")
        calls: list[tuple[str, tuple[str, ...]]] = []

        def starting(
            _layout: object,
            helper: str,
            arguments: list[str],
            **_kwargs: object,
        ) -> object:
            calls.append((helper, tuple(arguments)))
            if arguments[0] == "progress":
                output = json.dumps(
                    {
                        "schema_version": "usdb-bitcoin-readiness:v1",
                        "ready": False,
                        "status": {
                            "blocks": 236_706,
                            "headers": 965_135,
                            "verification_progress": 0.0116,
                            "txindex_synced": False,
                            "txindex_height": 236_701,
                            "connections": 8,
                        },
                        "blockers": ["initialblockdownload=true"],
                    }
                )
            elif helper == "run_testnet_bitcoin.sh":
                output = json.dumps(
                    [{"Service": "btc-node", "State": "running", "Health": "unhealthy"}]
                )
            else:
                output = "[]"
            return mock.Mock(returncode=0, stdout=output, stderr="")

        with mock.patch.object(NODE, "run_helper", side_effect=starting):
            report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "STARTING")
        runtime = report["checks"]["runtime"]
        self.assertEqual(runtime["bitcoin_progress"]["status"]["blocks"], 236_706)
        self.assertEqual(
            calls[-1],
            ("run_testnet_bitcoin.sh", ("progress",)),
        )
        output = io.StringIO()
        with mock.patch("sys.stdout", new=output):
            NODE._print_node_status_report(report)
        self.assertIn("Bitcoin sync", output.getvalue())
        self.assertIn("verification=1.16%", output.getvalue())

    def test_status_ready_runs_readiness_after_all_core_services_are_running(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "ready-status-data")
        calls: list[tuple[str, tuple[str, ...]]] = []
        bitcoin = [{"Service": "btc-node", "State": "running", "Health": "healthy"}]
        runtime = [
            {"Service": service, "State": "running", "Health": "healthy"}
            for service in NODE.CORE_RUNTIME_SERVICES
            if service != "btc-node"
        ]

        def ready(
            _layout: object,
            helper: str,
            arguments: list[str],
            **_kwargs: object,
        ) -> object:
            calls.append((helper, tuple(arguments)))
            if arguments[0] == "ps":
                output = json.dumps(bitcoin if helper == "run_testnet_bitcoin.sh" else runtime)
            else:
                output = "ready\n"
            return mock.Mock(returncode=0, stdout=output, stderr="")

        with mock.patch.object(NODE, "run_helper", side_effect=ready):
            report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "READY")
        self.assertEqual(report["next_actions"], [])
        self.assertEqual(len(calls), 5)
        self.assertEqual(
            calls[2:],
            [
                ("run_testnet_bitcoin.sh", ("status",)),
                ("run_testnet_runtime.sh", ("data-status",)),
                ("run_testnet_runtime.sh", ("indexer-status",)),
            ],
        )

    def test_status_json_is_machine_readable(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        output = io.StringIO()
        with mock.patch("sys.stdout", new=output):
            result = NODE.print_status(layout, json_output=True)

        report = json.loads(output.getvalue())
        self.assertEqual(result, 1)
        self.assertEqual(report["schema_version"], "usdb-node-status:v2")
        self.assertEqual(report["overall_state"], "UNCONFIGURED")
        self.assertEqual(report["next_actions"], ["usdb-node setup"])
        self.assertEqual(report["up"]["mode"], "manual")
        self.assertTrue(report["operator_guidance"])

    def test_status_blocked_explains_manual_data_recovery_boundary(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        data_root = Path(self.temporary.name) / "blocked-status-data"
        self.configure_full_node(layout, "blocked-status-data")
        marker = (
            NODE._data_directories(layout, data_root)["USDB_INDEXER_DATA_HOST_DIR"]
            / NODE.DATASET_IDENTITY_FILE
        )
        marker.write_text("{}\n", encoding="utf-8")

        report = NODE.collect_node_status(layout)

        self.assertEqual(report["overall_state"], "BLOCKED")
        self.assertEqual(report["up"]["mode"], "manual")
        self.assertEqual(report["next_actions"], ["usdb-node doctor"])
        self.assertTrue(any("do not move" in item.lower() for item in report["operator_guidance"]))

    def test_operation_lock_rejects_a_concurrent_bundle_mutation(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        with NODE.node_operation_lock(layout, "first"):
            with self.assertRaisesRegex(ValueError, "another usdb-node operation"):
                with NODE.node_operation_lock(layout, "second"):
                    self.fail("concurrent operation lock unexpectedly succeeded")

        with NODE.node_operation_lock(layout, "after-release"):
            pass

    def test_controller_unit_is_non_activating_and_uses_stable_launcher(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        launcher = self.root / "bin/usdb-node"
        launcher.parent.mkdir()
        launcher.write_text("#!/bin/sh\n", encoding="utf-8")
        launcher.chmod(0o755)

        content = NODE.render_controller_unit(
            layout,
            launcher=launcher,
            service_user="node-user",
            home=self.root / "home",
            sync_timeout_secs=123,
            pull=False,
        )

        self.assertIn(f'ExecStart="{launcher}"', content)
        self.assertIn(
            'ExecStartPre="/usr/bin/docker" info --format {{.ServerVersion}}',
            content,
        )
        self.assertIn(f'--node-env "{layout.node_env}" controller run', content)
        self.assertIn("--sync-timeout-secs 123 --skip-pull", content)
        self.assertIn("Wants=network-online.target docker.service", content)
        self.assertIn("StartLimitIntervalSec=1800s", content)
        self.assertIn("StartLimitBurst=20", content)
        self.assertIn("Restart=on-failure", content)
        self.assertIn("RestartPreventExitStatus=2", content)
        self.assertNotIn("Requires=docker.service", content)
        self.assertNotIn("activate-release", content)

    def test_controller_install_writes_enables_but_does_not_start_unit(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "controller-install-data")
        launcher = self.root / "bin/usdb-node"
        launcher.parent.mkdir()
        launcher.write_text("#!/bin/sh\n", encoding="utf-8")
        launcher.chmod(0o755)
        unit_dir = self.root / "systemd"
        unit_dir.mkdir()
        commands: list[list[str]] = []

        def privileged(arguments: list[str], *, check: bool = True) -> mock.Mock:
            self.assertTrue(check)
            commands.append(arguments)
            if arguments[0] == "install":
                shutil.copyfile(arguments[-2], arguments[-1])
            return mock.Mock(returncode=0)

        account = mock.Mock(pw_name="node-user", pw_dir=str(self.root / "home"))
        with (
            mock.patch.dict("os.environ", {"USDB_SYSTEMD_UNIT_DIR": str(unit_dir)}),
            mock.patch.object(NODE, "_systemd_available", return_value=True),
            mock.patch.object(NODE.shutil, "which", return_value="/usr/bin/docker"),
            mock.patch.object(NODE.pwd, "getpwuid", return_value=account),
            mock.patch.object(NODE, "_privileged_command", side_effect=privileged),
        ):
            path = NODE.install_controller_unit(layout, launcher=launcher)

        self.assertTrue(path.is_file())
        self.assertEqual(
            commands,
            [
                ["install", "-m", "0644", mock.ANY, str(path)],
                ["systemctl", "daemon-reload"],
                ["systemctl", "enable", path.name],
            ],
        )
        self.assertFalse(any("start" in command for command in commands))

    def test_controller_install_refuses_to_change_service_user(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        self.configure_full_node(layout, "controller-owner-data")
        launcher = self.root / "bin/usdb-node"
        launcher.parent.mkdir()
        launcher.write_text("#!/bin/sh\n", encoding="utf-8")
        launcher.chmod(0o755)
        unit_dir = self.root / "systemd"
        unit_dir.mkdir()
        unit = unit_dir / NODE.controller_unit_name(layout)
        unit.write_text("[Service]\nUser=original-user\n", encoding="utf-8")
        account = mock.Mock(pw_name="replacement-user", pw_dir=str(self.root / "home"))

        with (
            mock.patch.dict("os.environ", {"USDB_SYSTEMD_UNIT_DIR": str(unit_dir)}),
            mock.patch.object(NODE, "_systemd_available", return_value=True),
            mock.patch.object(NODE.shutil, "which", return_value="/usr/bin/docker"),
            mock.patch.object(NODE.pwd, "getpwuid", return_value=account),
            mock.patch.object(NODE, "_privileged_command") as privileged,
            self.assertRaisesRegex(ValueError, "refusing to change"),
        ):
            NODE.install_controller_unit(layout, launcher=launcher)

        privileged.assert_not_called()

    def test_up_is_idempotent_when_node_is_already_ready(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        report = {"overall_state": "READY"}
        with (
            mock.patch.object(NODE, "collect_node_status", return_value=report),
            mock.patch.object(NODE, "start_node") as start,
            mock.patch.object(NODE, "activate_release") as activate,
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=False,
                allow_activation=False,
                sync_timeout_secs=60,
                pull=True,
                json_output=False,
            )

        self.assertEqual(return_code, 0)
        self.assertEqual(result["outcome"], "ready")
        self.assertEqual(result["schema_version"], "usdb-node-up:v1")
        start.assert_not_called()
        activate.assert_not_called()

    def test_controller_submission_is_a_noop_when_node_is_already_ready(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        report = {"overall_state": "READY"}
        with (
            mock.patch.object(NODE, "collect_node_status", return_value=report),
            mock.patch.object(NODE, "_require_controller_unit") as require_unit,
            mock.patch.object(NODE, "start_controller_unit") as start,
            mock.patch.object(NODE, "activate_release") as activate,
        ):
            result, return_code = NODE.submit_up_to_controller(
                layout,
                dry_run=False,
                allow_activation=False,
            )

        self.assertEqual(return_code, 0)
        self.assertEqual(result["outcome"], "ready")
        require_unit.assert_not_called()
        start.assert_not_called()
        activate.assert_not_called()

    def test_controller_maps_manual_state_to_restart_preventing_exit_code(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        result = {
            "outcome": "manual_action_required",
            "initial_state": "ACTIVATION_REQUIRED",
            "completed_actions": [],
            "status": {"overall_state": "ACTIVATION_REQUIRED"},
        }
        with (
            mock.patch.object(NODE, "up_node", return_value=(result, 1)) as up,
            mock.patch.object(NODE, "print_up_result"),
        ):
            return_code = NODE.run_bootstrap_controller(
                layout,
                sync_timeout_secs=60,
                pull=True,
            )

        self.assertEqual(return_code, NODE.CONTROLLER_MANUAL_EXIT_CODE)
        up.assert_called_once_with(
            layout,
            dry_run=False,
            allow_activation=False,
            sync_timeout_secs=60,
            pull=True,
            json_output=False,
            enable_progress=False,
        )

    def test_controller_submission_keeps_release_activation_explicit(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        reports = [
            {"overall_state": "ACTIVATION_REQUIRED"},
            {"overall_state": "ACTIVATION_REQUIRED"},
            {"overall_state": "READY_TO_START"},
        ]
        with (
            mock.patch.object(NODE, "collect_node_status", side_effect=reports),
            mock.patch.object(NODE, "_require_controller_unit"),
            mock.patch.object(NODE, "activate_release") as activate,
            mock.patch.object(
                NODE,
                "start_controller_unit",
                return_value="usdb-node-bootstrap-test.service",
            ) as start,
        ):
            result, return_code = NODE.submit_up_to_controller(
                layout,
                dry_run=False,
                allow_activation=True,
            )

        self.assertEqual(return_code, 0)
        self.assertEqual(result["outcome"], "controller_started")
        self.assertEqual(result["completed_actions"], ["activate-release"])
        activate.assert_called_once_with(layout)
        start.assert_called_once_with(layout)

    def test_explicit_controller_start_clears_a_bounded_retry_stop(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        unit = self.root / "usdb-node-bootstrap-test.service"
        with (
            mock.patch.object(NODE, "_require_controller_unit", return_value=unit),
            mock.patch.object(NODE, "_privileged_command") as privileged,
        ):
            name = NODE.start_controller_unit(layout)

        self.assertEqual(name, unit.name)
        self.assertEqual(
            privileged.call_args_list,
            [
                mock.call(["systemctl", "reset-failed", unit.name], check=False),
                mock.call(["systemctl", "start", "--no-block", unit.name]),
            ],
        )

    def test_background_up_does_not_hold_the_foreground_operation_lock(self) -> None:
        parser = NODE.build_parser()
        background = parser.parse_args(["up"])
        foreground = parser.parse_args(["up", "--foreground"])

        self.assertIsNone(NODE._operation_name(background))
        self.assertIsNone(NODE._operation_name(foreground))

    def test_background_up_submits_controller_without_running_foreground_start(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(["up"])
        result = {
            "outcome": "controller_started",
            "initial_state": "READY_TO_START",
            "completed_actions": [],
            "status": {"overall_state": "READY_TO_START"},
        }
        output = io.StringIO()
        with (
            mock.patch.object(
                NODE,
                "submit_up_to_controller",
                return_value=(result, 0),
            ) as submit,
            mock.patch.object(NODE, "start_node") as foreground_start,
            mock.patch.object(NODE, "print_up_result"),
            mock.patch("sys.stdout", new=output),
        ):
            return_code = NODE._execute_command(layout, args)

        self.assertEqual(return_code, 0)
        submit.assert_called_once_with(
            layout,
            dry_run=False,
            allow_activation=False,
        )
        foreground_start.assert_not_called()

    def test_foreground_up_uses_the_complete_up_state_machine(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(
            ["up", "--foreground", "--dry-run", "--json"]
        )
        result = {
            "outcome": "dry_run",
            "initial_state": "SNAPSHOT_INCOMPLETE",
            "completed_actions": [],
            "status": {"overall_state": "SNAPSHOT_INCOMPLETE"},
        }
        with (
            mock.patch.object(NODE, "up_node", return_value=(result, 0)) as up,
            mock.patch.object(NODE, "print_up_result") as print_result,
        ):
            return_code = NODE._execute_command(layout, args)

        self.assertEqual(return_code, 0)
        up.assert_called_once_with(
            layout,
            dry_run=True,
            allow_activation=False,
            sync_timeout_secs=NODE.DEFAULT_SYNC_TIMEOUT_SECS,
            pull=True,
            json_output=True,
            enable_progress=False,
        )
        print_result.assert_called_once_with(result, json_output=True)

    def test_resume_is_not_a_public_command(self) -> None:
        with (
            mock.patch("sys.stderr", new=io.StringIO()),
            self.assertRaises(SystemExit),
        ):
            NODE.build_parser().parse_args(["resume"])

    def test_setup_defers_selected_snapshot_work_to_controller(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(["setup"])
        output = io.StringIO()
        setup_result = NODE.SetupResult(
            node_env=self.node_env,
            apply_firewall=False,
            install_snapshot=True,
        )
        controller_path = self.root / "usdb-node-bootstrap-test.service"
        with (
            mock.patch.object(NODE, "_controller_install_context"),
            mock.patch.object(NODE, "setup_node", return_value=setup_result),
            mock.patch.object(NODE, "install_snapshot_release") as install_snapshot,
            mock.patch.object(
                NODE,
                "install_controller_unit",
                return_value=controller_path,
            ) as install_controller,
            mock.patch.object(sys.stdin, "isatty", return_value=True),
            mock.patch("sys.stdout", new=output),
            mock.patch.object(output, "isatty", return_value=True),
        ):
            return_code = NODE._execute_command(layout, args)

        self.assertEqual(return_code, 0)
        install_snapshot.assert_not_called()
        install_controller.assert_called_once_with(layout)
        self.assertIn("bootstrap controller will download", output.getvalue())
        self.assertIn("Installed and enabled", output.getvalue())
        self.assertIn("usdb-node doctor, then usdb-node up", output.getvalue())

    def test_setup_no_controller_writes_config_without_touching_systemd(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(["setup", "--no-controller"])
        output = io.StringIO()
        setup_result = NODE.SetupResult(
            node_env=self.node_env,
            apply_firewall=False,
            install_snapshot=False,
        )
        with (
            mock.patch.object(NODE, "setup_node", return_value=setup_result),
            mock.patch.object(NODE, "_controller_install_context") as controller_context,
            mock.patch.object(NODE, "install_controller_unit") as install_controller,
            mock.patch.object(sys.stdin, "isatty", return_value=True),
            mock.patch("sys.stdout", new=output),
            mock.patch.object(output, "isatty", return_value=True),
        ):
            return_code = NODE._execute_command(layout, args)

        self.assertEqual(return_code, 0)
        controller_context.assert_not_called()
        install_controller.assert_not_called()
        self.assertIn("Skipped the bootstrap controller", output.getvalue())
        self.assertIn("usdb-node up --foreground", output.getvalue())

    def test_setup_preserves_configuration_when_controller_install_fails(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(["setup"])
        setup_result = NODE.SetupResult(
            node_env=self.node_env,
            apply_firewall=False,
            install_snapshot=False,
        )
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.object(NODE, "_controller_install_context"),
            mock.patch.object(NODE, "setup_node", return_value=setup_result),
            mock.patch.object(
                NODE,
                "install_controller_unit",
                side_effect=ValueError("systemd unavailable"),
            ),
            mock.patch.object(sys.stdin, "isatty", return_value=True),
            mock.patch("sys.stdout", new=stdout),
            mock.patch.object(stdout, "isatty", return_value=True),
            mock.patch("sys.stderr", new=stderr),
            self.assertRaisesRegex(ValueError, "systemd unavailable"),
        ):
            NODE._execute_command(layout, args)

        self.assertIn("Node configuration was preserved", stderr.getvalue())
        self.assertIn("usdb-node controller install", stderr.getvalue())

    def test_setup_checks_controller_host_before_writing_configuration(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        args = NODE.build_parser().parse_args(["setup"])
        with (
            mock.patch.object(
                NODE,
                "_controller_install_context",
                side_effect=ValueError("systemd unavailable"),
            ),
            mock.patch.object(NODE, "setup_node") as setup,
            mock.patch.object(sys.stdin, "isatty", return_value=True),
            mock.patch.object(sys.stdout, "isatty", return_value=True),
            self.assertRaisesRegex(ValueError, "systemd unavailable"),
        ):
            NODE._execute_command(layout, args)

        setup.assert_not_called()

    def test_detaching_progress_does_not_stop_controller(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        progress = {
            "overall_state": "SYNCING",
            "components": [],
        }
        result = {
            "outcome": "controller_started",
            "initial_state": "STARTING",
            "status": {"overall_state": "STARTING"},
        }
        with (
            mock.patch.object(NODE, "collect_node_progress", return_value=progress),
            mock.patch.object(NODE, "render_node_progress", return_value="progress"),
            mock.patch.object(NODE, "controller_active_state", return_value="active"),
            mock.patch.object(
                NODE,
                "collect_node_status",
                return_value={"overall_state": "STARTING"},
            ),
            mock.patch.object(NODE.time, "sleep", side_effect=KeyboardInterrupt),
            mock.patch("sys.stderr", new=io.StringIO()),
            mock.patch.object(NODE, "stop_controller_unit") as stop,
        ):
            detached, return_code = NODE.follow_submitted_controller(layout, result)

        self.assertEqual(return_code, 0)
        self.assertEqual(detached["outcome"], "controller_detached")
        stop.assert_not_called()

    def test_up_requires_explicit_activation(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        report = {"overall_state": "ACTIVATION_REQUIRED"}
        with (
            mock.patch.object(NODE, "collect_node_status", return_value=report),
            mock.patch.object(NODE, "activate_release") as activate,
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=False,
                allow_activation=False,
                sync_timeout_secs=60,
                pull=True,
                json_output=False,
            )

        self.assertEqual(return_code, 1)
        self.assertEqual(result["outcome"], "manual_action_required")
        activate.assert_not_called()
        self.assertFalse((layout.node_env.parent / ".usdb-node-operation.lock").exists())

    def test_up_runs_activation_snapshot_and_startup_in_order(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        reports = [
            {"overall_state": "ACTIVATION_REQUIRED"},
            {"overall_state": "ACTIVATION_REQUIRED"},
            {"overall_state": "SNAPSHOT_INCOMPLETE"},
            {"overall_state": "READY_TO_START"},
            {"overall_state": "READY"},
        ]
        calls: list[str] = []

        with (
            mock.patch.object(NODE, "collect_node_status", side_effect=reports),
            mock.patch.object(
                NODE,
                "activate_release",
                side_effect=lambda _layout: calls.append("activate-release"),
            ),
            mock.patch.object(
                NODE,
                "install_snapshot_release",
                side_effect=lambda _layout: calls.append("snapshot-install"),
            ),
            mock.patch.object(
                NODE,
                "start_node",
                side_effect=lambda _layout, **_kwargs: calls.append("up"),
            ) as start,
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=False,
                allow_activation=True,
                sync_timeout_secs=120,
                pull=False,
                json_output=True,
            )

        self.assertEqual(return_code, 0)
        self.assertEqual(result["outcome"], "ready")
        self.assertEqual(calls, ["activate-release", "snapshot-install", "up"])
        self.assertEqual(result["completed_actions"], calls)
        start.assert_called_once_with(
            layout,
            sync_timeout_secs=120,
            pull=False,
            output_to_stderr=True,
        )

    def test_up_dry_run_does_not_change_snapshot_state(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        report = {"overall_state": "SNAPSHOT_INCOMPLETE"}
        with (
            mock.patch.object(NODE, "collect_node_status", return_value=report),
            mock.patch.object(NODE, "install_snapshot_release") as install,
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=True,
                allow_activation=False,
                sync_timeout_secs=60,
                pull=True,
                json_output=False,
            )

        self.assertEqual(return_code, 0)
        self.assertEqual(result["planned_actions"], ["snapshot-install"])
        install.assert_not_called()
        self.assertFalse((layout.node_env.parent / ".usdb-node-operation.lock").exists())

    def test_up_stops_when_action_makes_no_forward_progress(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        reports = [
            {"overall_state": "STARTING"},
            {"overall_state": "STARTING"},
            {"overall_state": "STARTING"},
        ]
        with (
            mock.patch.object(NODE, "collect_node_status", side_effect=reports),
            mock.patch.object(NODE, "start_node") as start,
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=False,
                allow_activation=False,
                sync_timeout_secs=60,
                pull=True,
                json_output=False,
            )

        self.assertEqual(return_code, 1)
        self.assertEqual(result["outcome"], "no_forward_progress")
        start.assert_called_once()

    def test_up_json_keeps_operation_logs_off_stdout(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        reports = [
            {"overall_state": "READY_TO_START"},
            {"overall_state": "READY_TO_START"},
            {"overall_state": "READY"},
        ]

        def noisy_start(_layout: object, **_kwargs: object) -> None:
            print("startup progress")

        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch.object(NODE, "collect_node_status", side_effect=reports),
            mock.patch.object(NODE, "start_node", side_effect=noisy_start),
            mock.patch("sys.stdout", new=stdout),
            mock.patch("sys.stderr", new=stderr),
        ):
            result, return_code = NODE.up_node(
                layout,
                dry_run=False,
                allow_activation=False,
                sync_timeout_secs=60,
                pull=True,
                json_output=True,
            )
            NODE.print_up_result(result, json_output=True)

        decoded = json.loads(stdout.getvalue())
        self.assertEqual(return_code, 0)
        self.assertEqual(decoded["outcome"], "ready")
        self.assertNotIn("startup progress", stdout.getvalue())
        self.assertIn("startup progress", stderr.getvalue())

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
            firewall_mode="managed",
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
            firewall_mode="managed",
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
            output_to_stderr=False,
        )
        helper.assert_called_once_with(
            layout,
            "run_testnet_runtime.sh",
            ["validate-node"],
            output_to_stderr=False,
        )
        firewall.assert_called_once_with(layout, "check", output_to_stderr=False)

    def test_doctor_skips_ufw_for_external_firewall_mode(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        NODE.configure_node(
            layout,
            data_root=Path(self.temporary.name) / "external-firewall-data",
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="",
            nat="",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
        )
        output = io.StringIO()
        with (
            mock.patch.object(NODE, "run_host_action") as host,
            mock.patch.object(NODE, "run_helper") as helper,
            mock.patch.object(NODE, "run_firewall_action") as firewall,
            mock.patch("sys.stdout", new=output),
        ):
            NODE.doctor(layout)
        host.assert_called_once()
        helper.assert_called_once_with(
            layout,
            "run_testnet_runtime.sh",
            ["validate-node"],
            output_to_stderr=False,
        )
        firewall.assert_not_called()
        self.assertIn("skipped UFW inspection", output.getvalue())

    def test_external_mode_rejects_explicit_ufw_action_until_changed(self) -> None:
        layout = NODE.load_release_layout(self.root, self.node_env)
        NODE.configure_node(
            layout,
            data_root=Path(self.temporary.name) / "firewall-mode-data",
            role="full",
            miner_address="",
            miner_threads=1,
            bootnodes="",
            nat="",
            bitcoin_rpc_user="node-a",
            bitcoin_p2p="private",
        )

        with self.assertRaisesRegex(ValueError, "firewall mode is external"):
            NODE.run_firewall_action(layout, "check")

        NODE.set_firewall_mode(layout, "managed")
        self.assertEqual(NODE.configured_firewall_mode(layout), "managed")

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
        legacy = "\n".join(
            line
            for line in layout.node_env.read_text(encoding="utf-8").splitlines()
            if not line.startswith("USDB_FIREWALL_MODE=")
        ) + "\n"
        NODE._atomic_write_private(layout.node_env, legacy)
        self.assertEqual(NODE.configured_firewall_mode(layout), "managed")
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
        self.assertEqual(updated["USDB_FIREWALL_MODE"], "managed")
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
