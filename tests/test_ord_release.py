#!/usr/bin/env python3
"""Ord source pins and stopped-node dataset transitions, without real services."""

import io
import json
from pathlib import Path
import sys
import tempfile
import tomllib
import unittest
from contextlib import redirect_stdout
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import ord_release as release
import resource_policy as policy
import usdb_minting as minting
import usdb_node as node
from common.native_node import native_kit


class OrdReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        for name, value in (("effective_memory_bytes", 64 * policy.GIB), ("_collect_compose_services", {})):
            patch = mock.patch.object(node, name, return_value=value)
            patch.start()
            self.addCleanup(patch.stop)
        with mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(self.layout, data_root=self.root / "data", role="full", miner_address="",
                miner_threads=1, bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private",
                resource_management="auto", minting=True)
        self.old_path = minting.data_path(self.root / "data", release.LEGACY_VERSION)
        self.old_path.mkdir()
        (self.old_path / "identity.json").write_text(json.dumps(release.LEGACY_IDENTITY))
        (self.old_path / "index.redb").write_bytes(b"valuable older index")
        node._atomic_write_private(self.layout.node_env, node.upsert_env(self.layout.node_env.read_text(),
                                  {"ORD_DATA_HOST_DIR": str(self.old_path)}))
        self.original = self.layout.node_env.read_bytes()

    def activate(self):
        with redirect_stdout(io.StringIO()) as output:
            node.activate_release(self.layout)
        return output.getvalue()

    def test_build_and_dataset_pins_agree(self):
        for name in ("usdb-services", "world-sim-tools"):
            text = (ROOT / f"docker/Dockerfile.{name}").read_text()
            self.assertIn(f"ARG ORD_VERSION={release.VERSION}", text)
            self.assertIn(f"ARG ORD_REVISION={release.REVISION}", text)
            self.assertIn(f"docker/locks/ord-{release.VERSION}.Cargo.lock", text)
            self.assertIn('--locked --bin ord --release', text)
        lock = tomllib.loads((ROOT / f"docker/locks/ord-{release.VERSION}.Cargo.lock").read_text())
        self.assertEqual(next(package["version"] for package in lock["package"] if package["name"] == "ord"), release.VERSION)
        self.assertEqual(release.IDENTITY["index_schema"], 34)
        self.assertTrue((self.layout.kit_root / "docker/scripts/tools/ord_release.py").is_file())

    def test_activation_preserves_old_index_credentials_and_backs_up_configuration(self):
        env = node.read_env(self.layout.node_env)
        minting.validate(env)
        with self.assertRaisesRegex(ValueError, "activate-release"):
            minting.prepare(env)
        self.assertIn("retained", self.activate())
        current = node.read_env(self.layout.node_env)
        self.assertEqual(Path(current["ORD_DATA_HOST_DIR"]), minting.data_path(self.root / "data"))
        self.assertEqual((self.old_path / "index.redb").read_bytes(), b"valuable older index")
        self.assertEqual(json.loads((self.old_path / "identity.json").read_text()), release.LEGACY_IDENTITY)
        self.assertEqual(json.loads((Path(current["ORD_DATA_HOST_DIR"]) / "identity.json").read_text()), release.IDENTITY)
        backup = self.root / "node.env.ord-upgrade-backup"
        self.assertEqual(backup.read_bytes(), self.original)
        self.assertEqual(backup.stat().st_mode & 0o777, 0o600)
        for key in env.keys() - {"ORD_DATA_HOST_DIR"}:
            self.assertEqual(current[key], env[key], key)
        self.activate()
        self.assertEqual(backup.read_bytes(), self.original)

    def test_old_ready_report_cannot_enable_new_backend(self):
        (self.old_path / "progress.json").write_text(json.dumps(dict(schema_version="usdb-ord-progress:v1",
            state="READY", observed_at_ms=1000, canonical=True, history_validated=True, txindex_synced=True)))
        report = minting.progress(node.read_env(self.layout.node_env), now_ms=1001)
        self.assertEqual(report["state"], "BLOCKED_CONFIG")
        self.assertFalse(report["backend_ready"])
        self.assertIn("activate-release", report["guidance"])

    def test_running_or_unknown_services_prevent_dataset_switch(self):
        for state in ("running", "paused", "restarting", "unknown"):
            with mock.patch.object(node, "_collect_compose_services", return_value={"ord-server": {"state": state}}):
                with self.assertRaisesRegex(ValueError, "usdb-node down"):
                    self.activate()
            self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_unknown_old_marker_or_new_unmarked_index_is_not_adopted(self):
        marker = self.old_path / "identity.json"
        marker.write_text('{}')
        with self.assertRaisesRegex(ValueError, "identity differs"):
            self.activate()
        marker.write_text(json.dumps(release.LEGACY_IDENTITY))
        current = minting.data_path(self.root / "data")
        (current / "identity.json").unlink()
        (current / "index.redb").write_bytes(b"unknown database")
        with self.assertRaisesRegex(ValueError, "nonempty unmarked"):
            self.activate()
        self.assertEqual((current / "index.redb").read_bytes(), b"unknown database")
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_failed_image_validation_rolls_back_configuration(self):
        with mock.patch.object(node, "_validate_node_release_images", side_effect=ValueError("image mismatch")):
            with self.assertRaisesRegex(ValueError, "image mismatch"):
                self.activate()
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)
        self.assertEqual((self.old_path / "index.redb").read_bytes(), b"valuable older index")

    def test_disabled_backend_never_creates_or_starts_an_index(self):
        current = minting.data_path(self.root / "data")
        (current / "identity.json").unlink()
        current.rmdir()
        env = {**node.read_env(self.layout.node_env), "USDB_MINTING_ENABLED": "0", "BTC_TXINDEX": "0"}
        env.update(policy.build_resource_plan(64 * policy.GIB, "bitcoin", env).environment())
        node._atomic_write_private(self.layout.node_env, node.upsert_env(self.layout.node_env.read_text(), env))
        self.activate()
        self.assertFalse(current.exists())
        self.assertEqual(minting.progress(node.read_env(self.layout.node_env))["state"], "DISABLED")


if __name__ == "__main__":
    unittest.main()
