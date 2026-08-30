#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("prepare_release_node_kit.py")
SPEC = importlib.util.spec_from_file_location("prepare_release_node_kit", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

from release_manifest import build_network_identity

REPOSITORY_ROOT = MODULE_PATH.resolve().parents[3]
SOURCE_BUNDLE = REPOSITORY_ROOT / "docker/networks/testnet-v0"


class PrepareReleaseNodeKitTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="prepare-node-kit-test-")
        self.root = Path(self.temporary.name)
        self.bundle = self.root / "testnet-v0"
        shutil.copytree(SOURCE_BUNDLE, self.bundle)
        digest = "1" * 64
        self.manifest = self.root / "usdb-release-manifest.json"
        content = json.dumps(
            {
                "schema_version": "usdb-release-manifest:v3",
                "release_id": "usdb-testnet-v0-r1",
                "network_bundle": build_network_identity(self.bundle),
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
            },
            indent=2,
            sort_keys=True,
        ) + "\n"
        self.manifest.write_text(content, encoding="utf-8")
        checksum = hashlib.sha256(content.encode()).hexdigest()
        self.manifest_checksum = self.root / "usdb-release-manifest.json.sha256"
        self.manifest_checksum.write_text(
            f"{checksum}  usdb-release-manifest.json\n",
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_builds_a_loadable_self_contained_node_kit(self) -> None:
        (self.bundle / "node.env").write_text("BTC_RPC_PASSWORD=secret\n", encoding="utf-8")
        output = self.root / "output/usdb-node-kit"
        result = BUILDER.build_node_kit(
            repository_root=REPOSITORY_ROOT,
            bundle_dir=self.bundle,
            manifest_path=self.manifest,
            manifest_checksum_path=self.manifest_checksum,
            output_dir=output,
        )
        self.assertEqual(result, output.resolve())
        layout = BUILDER.load_release_layout(output)
        self.assertEqual(layout.release_id, "usdb-testnet-v0-r1")
        self.assertTrue((output / "docker/compose.runtime.yml").is_file())
        self.assertTrue((output / "docker/scripts/tools/usdb_node.py").is_file())
        self.assertFalse((layout.bundle_dir / "node.env").exists())

        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            BUILDER.build_node_kit(
                repository_root=REPOSITORY_ROOT,
                bundle_dir=self.bundle,
                manifest_path=self.manifest,
                manifest_checksum_path=self.manifest_checksum,
                output_dir=output,
            )


if __name__ == "__main__":
    unittest.main()
