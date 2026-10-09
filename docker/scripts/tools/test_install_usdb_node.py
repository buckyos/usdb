#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path


INSTALLER = Path(__file__).with_name("install_usdb_node.sh")
RELEASE_ID = "usdb-testnet-v0-r1"
sys.path.insert(0, str(INSTALLER.resolve().parents[3] / "tests"))
from common.install_guidance import copy_tools, minimal_manifest


class InstallUsdbNodeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="install-usdb-node-test-")
        self.root = Path(self.temporary.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.install_root = self.root / "releases"
        self.bin_dir = self.root / "bin"
        self.home = self.root / "home"
        self.home.mkdir()
        self.create_release_assets()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def write_checksum(path: Path) -> None:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        path.with_name(path.name + ".sha256").write_text(
            f"{digest}  {path.name}\n",
            encoding="utf-8",
        )

    def create_release_assets(self) -> None:
        manifest = self.assets / "usdb-release-manifest.json"
        manifest.write_text(
            json.dumps(minimal_manifest(RELEASE_ID), sort_keys=True) + "\n",
            encoding="utf-8",
        )
        self.write_checksum(manifest)

        staging = self.root / "staging/usdb-node-kit"
        tools = staging / "docker/scripts/tools"
        release = staging / "release"
        tools.mkdir(parents=True)
        release.mkdir(parents=True)
        controller = tools / "usdb_node.py"
        copy_tools(staging)
        controller.chmod(0o755)
        (release / manifest.name).write_bytes(manifest.read_bytes())
        (release / f"{manifest.name}.sha256").write_bytes(
            manifest.with_name(manifest.name + ".sha256").read_bytes()
        )
        archive = self.assets / f"{RELEASE_ID}-node-kit.tar.gz"
        with tarfile.open(archive, "w:gz") as target:
            target.add(staging, arcname="usdb-node-kit")
        self.write_checksum(archive)

    def install(self, *extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                str(INSTALLER),
                "--release-id",
                RELEASE_ID,
                "--release-base-url",
                self.assets.as_uri(),
                "--install-root",
                str(self.install_root),
                "--bin-dir",
                str(self.bin_dir),
                *extra,
            ],
            check=False,
            capture_output=True,
            text=True,
            env={**os.environ, "HOME": str(self.home)},
        )

    def test_installs_and_reuses_an_identical_immutable_release(self) -> None:
        first = self.install()
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertIn("Existing node configuration and running service images were not activated or changed", first.stdout)
        launcher = self.bin_dir / "usdb-node"
        self.assertTrue(launcher.is_symlink())
        self.assertTrue(os.access(launcher.resolve(), os.X_OK))
        self.assertEqual(
            launcher.resolve(),
            self.install_root / RELEASE_ID / "docker/scripts/tools/usdb_node.py",
        )
        self.assertIn("usdb-node prepare-host", first.stdout)
        self.assertIn("usdb-node setup", first.stdout)
        self.assertIn("setup --no-controller", first.stdout)
        self.assertIn("usdb-node doctor", first.stdout)
        self.assertIn("usdb-node up", first.stdout)
        self.assertIn("usdb-node status", first.stdout)
        self.assertIn("does not run these commands automatically", first.stdout)
        self.assertIn("no standard node configuration found", first.stdout)
        self.assertNotIn("usdb-node activate-release", first.stdout)
        self.assertNotIn("upgrade-release --resume", first.stdout)
        for stage in range(1, 6):
            self.assertIn(f"[{stage}/5]", first.stderr)
        self.assertIn("Downloading " + f"{RELEASE_ID}-node-kit.tar.gz", first.stderr)
        self.assertNotIn("[usdb-install]", first.stdout)

        second = self.install()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertIn("already installed", second.stdout)

    def test_installer_shows_only_cross_network_advice(self):
        config = self.home / ".config/usdb/usdb-testnet-v2/node.env"
        config.parent.mkdir(parents=True)
        config.write_text(f"USDB_DATA_ROOT={self.root / 'data'}\n")
        before = config.read_bytes()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("target network setup required", result.stdout)
        self.assertIn("Default: usdb-testnet-v2", result.stdout)
        self.assertIn("usdb-node upgrade-plan", result.stdout)
        self.assertNotIn("usdb-node prepare-host", result.stdout)
        self.assertNotIn("data_rebuild", result.stdout)
        self.assertEqual(config.read_bytes(), before)

    def test_advice_failure_does_not_fail_installation_or_print_private_values(self):
        config = self.home / ".config/usdb/usdb-testnet-v0/node.env"
        config.parent.mkdir(parents=True)
        config.write_text("SECRET-bad-key=private-value\n")
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.bin_dir / "usdb-node").is_symlink())
        self.assertIn("Tool installation succeeded; deployment advice could not be completed", result.stdout)
        self.assertNotIn("SECRET", result.stdout + result.stderr)
        self.assertNotIn("private-value", result.stdout + result.stderr)
        self.assertNotIn("usdb-node setup", result.stdout)

    def test_rejects_archive_checksum_mismatch(self) -> None:
        archive = self.assets / f"{RELEASE_ID}-node-kit.tar.gz"
        archive.write_bytes(archive.read_bytes() + b"tamper")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Checksum mismatch", result.stderr)

    def test_rejects_archive_path_traversal(self) -> None:
        archive = self.assets / f"{RELEASE_ID}-node-kit.tar.gz"
        with tarfile.open(archive, "w:gz") as target:
            entry = tarfile.TarInfo("../escape")
            entry.size = 1
            target.addfile(entry, io.BytesIO(b"x"))
        self.write_checksum(archive)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe node kit archive entry", result.stderr)

    def test_release_bound_digests_reject_another_valid_asset(self) -> None:
        result = self.install("--expected-manifest-sha256", "0" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Release-bound checksum mismatch", result.stderr)

    def test_rejects_links_before_extracting_any_member(self) -> None:
        for link_type in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            with self.subTest(link_type=link_type):
                archive = self.assets / f"{RELEASE_ID}-node-kit.tar.gz"
                with tarfile.open(archive, "w:gz") as target:
                    entry = tarfile.TarInfo("usdb-node-kit/link")
                    entry.type = link_type
                    entry.linkname = "../../escape"
                    target.addfile(entry)
                self.write_checksum(archive)
                result = self.install()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("unsafe node kit archive entry", result.stderr)
                self.assertFalse((self.root / "escape").exists())


if __name__ == "__main__":
    unittest.main()
