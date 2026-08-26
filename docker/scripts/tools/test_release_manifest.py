#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import json
import shutil
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("release_manifest.py")
SPEC = importlib.util.spec_from_file_location("release_manifest", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class ReleaseManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="usdb-release-manifest-test-")
        self.root = Path(self.temp_dir.name)
        source = MODULE_PATH.parents[2] / "networks/testnet-v0"
        self.bundle = self.root / "testnet-v0"
        shutil.copytree(source, self.bundle, ignore=shutil.ignore_patterns("node.env", "runtime"))
        self.compatibility_lock = self.root / "ci-revisions.json"
        self.compatibility_lock.write_text(
            json.dumps(
                {
                    "schema_version": "usdb-ci-revisions:v1",
                    "coordinator": "go_ethereum",
                    "repositories": {
                        "go_ethereum": {
                            "repository": "buckyos/go-ethereum",
                            "directory": "go-ethereum",
                            "revision": "0" * 40,
                        },
                        "usdb": {
                            "repository": "buckyos/usdb",
                            "directory": "usdb",
                            "revision": "b" * 40,
                        },
                        "source_dao": {
                            "repository": "buckyos/SourceDAO",
                            "directory": "SourceDAO",
                            "revision": "c" * 40,
                        },
                    },
                    "toolchains": {},
                }
            ),
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def valid_manifest(self) -> dict:
        return RELEASE.create_manifest(
            bundle_dir=self.bundle,
            release_id="usdb-testnet-v0-r1",
            created_at_utc="2026-08-26T12:00:00Z",
            genesis_block_hash="0x" + "1" * 64,
            compatibility_lock_path=self.compatibility_lock,
            revisions={
                "go_ethereum": "a" * 40,
                "usdb": "b" * 40,
                "source_dao": "c" * 40,
            },
            image_references={
                "usdb_services": "ghcr.io/buckyos/usdb-services@sha256:" + "d" * 64,
                "usdb_chain": "ghcr.io/buckyos/usdb-chain@sha256:" + "e" * 64,
                "bitcoin_core": "ghcr.io/buckyos/usdb-bitcoin-core@sha256:" + "f" * 64,
            },
        )

    def test_candidate_round_trip_is_stable(self) -> None:
        manifest = self.valid_manifest()
        path = self.root / "release-manifest.json"
        RELEASE.write_manifest(path, manifest)
        self.assertEqual(RELEASE.load_manifest(path, self.bundle, self.compatibility_lock), manifest)
        self.assertTrue(RELEASE.HASH_RE.fullmatch(RELEASE.sha256(path)))

    def test_mutable_or_wrong_image_reference_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "canonical GHCR digest reference"):
            RELEASE.create_manifest(
                bundle_dir=self.bundle,
                release_id="usdb-testnet-v0-r1",
                created_at_utc="2026-08-26T12:00:00Z",
                genesis_block_hash="0x" + "1" * 64,
                compatibility_lock_path=self.compatibility_lock,
                revisions={
                    "go_ethereum": "a" * 40,
                    "usdb": "b" * 40,
                    "source_dao": "c" * 40,
                },
                image_references={
                    "usdb_services": "ghcr.io/buckyos/usdb-services:latest",
                    "usdb_chain": "ghcr.io/buckyos/usdb-chain@sha256:" + "e" * 64,
                    "bitcoin_core": "ghcr.io/buckyos/usdb-bitcoin-core@sha256:" + "f" * 64,
                },
            )

    def test_image_source_revision_mismatch_is_rejected(self) -> None:
        manifest = self.valid_manifest()
        manifest["images"]["usdb_chain"]["source_revision"] = "f" * 40
        with self.assertRaisesRegex(ValueError, "source_revision mismatch"):
            RELEASE.validate_manifest(manifest, self.bundle, self.compatibility_lock)

    def test_bundle_or_manifest_tamper_is_rejected(self) -> None:
        manifest = self.valid_manifest()
        tampered = copy.deepcopy(manifest)
        tampered["network_bundle"]["chain_id"] += 1
        with self.assertRaisesRegex(ValueError, "network identity does not match"):
            RELEASE.validate_manifest(tampered, self.bundle, self.compatibility_lock)

        network_path = self.bundle / "network.json"
        network = json.loads(network_path.read_text(encoding="utf-8"))
        network["chain_id"] += 1
        network_path.write_text(json.dumps(network), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected testnet-v0 chain ID"):
            RELEASE.validate_manifest(manifest, self.bundle, self.compatibility_lock)

    def test_selected_revision_must_match_go_compatibility_lock(self) -> None:
        lock = json.loads(self.compatibility_lock.read_text(encoding="utf-8"))
        lock["repositories"]["usdb"]["revision"] = "d" * 40
        self.compatibility_lock.write_text(json.dumps(lock), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "selected usdb revision does not match"):
            self.valid_manifest()

    def test_duplicate_keys_and_existing_output_are_rejected(self) -> None:
        path = self.root / "duplicate.json"
        path.write_text('{"schema_version":"a","schema_version":"b"}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            RELEASE.load_manifest(path, self.bundle, self.compatibility_lock)

        output = self.root / "existing.json"
        output.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "already exists"):
            RELEASE.write_manifest(output, self.valid_manifest())


if __name__ == "__main__":
    unittest.main()
