#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from urllib.parse import urlparse
from unittest import mock


MODULE_PATH = Path(__file__).with_name("snapshot_distribution.py")
STRICT_JSON_CORPUS = (
    MODULE_PATH.parents[3]
    / "src/btc/usdb-util/testdata/strict-json-duplicate-key-corpus.json"
)
SPEC = importlib.util.spec_from_file_location("snapshot_distribution", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
DISTRIBUTION = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DISTRIBUTION
SPEC.loader.exec_module(DISTRIBUTION)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class FakeAwsClient:
    def __init__(self, object_root: Path) -> None:
        self.object_root = object_root
        self.metadata: dict[str, dict[str, object]] = {}
        self.events: list[tuple[str, str]] = []

    def head(self, object_key: str) -> dict[str, object] | None:
        self.events.append(("head", object_key))
        return self.metadata.get(object_key)

    def upload(
        self,
        source: Path,
        object_key: str,
        digest: str,
        size: int,
        _content_type: str,
    ) -> None:
        self.events.append(("upload", object_key))
        destination = self.object_root / object_key
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
        self.metadata[object_key] = {
            "ContentLength": size,
            "Metadata": {"usdb-sha256": digest, "usdb-size": str(size)},
        }


class SnapshotDistributionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="snapshot-distribution-v3-")
        self.root = Path(self.temporary.name)
        self.artifact = self.root / "artifact"
        self.core = self.artifact / "core"
        self.registry = self.artifact / "script-registry"
        self.core.mkdir(parents=True)
        self.registry.mkdir()
        self.height = 42
        self.block_hash = "11" * 32
        self.core_snapshot_id = "22" * 32
        self.core_artifact_id = "33" * 32
        self.registry_artifact_id = "44" * 32
        self.signing_key_id = "snapshot-signer-1"

        self._write_core()
        self._write_registry()
        self.trusted_keys = self.root / "snapshot.trusted-keys.json"
        self.trusted_keys.write_text(
            json.dumps(
                {
                    "keys": [
                        {
                            "key_id": self.signing_key_id,
                            "public_key_base64": "cHVibGljLWtleQ==",
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        self.finalization = self.root / "artifact-finalized.json"
        self._write_finalization(include_registry=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _write_core(self) -> None:
        database = self.core / "balance_history_core_42.db"
        database.write_bytes(b"core-snapshot-database")
        manifest = self.core / "balance_history_core_42.manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "manifest_version": DISTRIBUTION.CORE_MANIFEST_VERSION,
                    "artifact_type": "balance_history_core",
                    "snapshot_schema_version": "balance-history-core-snapshot:v1",
                    "registry_included": False,
                    "core_snapshot_id": self.core_snapshot_id,
                    "core_artifact_id": self.core_artifact_id,
                    "file_name": database.name,
                    "file_sha256": sha256(database),
                    "state_ref": {
                        "block_height": self.height,
                        "stable_block_hash": self.block_hash,
                        "snapshot_id": self.core_snapshot_id,
                    },
                    "db_identity": {"btc_network": "bitcoin"},
                    "balance_query_floor": self.height,
                    "history_query_floor": self.height + 1,
                    "signature_scheme": "ed25519",
                    "signing_key_id": self.signing_key_id,
                    "generated_at": 100,
                }
            ),
            encoding="utf-8",
        )
        manifest.with_suffix(".sig").write_text("core-signature", encoding="utf-8")
        (self.core / "complete.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "height": self.height,
                    "network": "bitcoin",
                    "btc_block_hash": self.block_hash,
                    "snapshot_id": self.core_snapshot_id,
                    "core_artifact_id": self.core_artifact_id,
                    "snapshot_file": database.name,
                    "manifest_file": manifest.name,
                    "signature_file": manifest.with_suffix(".sig").name,
                    "file_sha256": sha256(database),
                    "balance_history_count": 1,
                    "utxo_count": 2,
                    "block_commit_count": 3,
                    "completed_at": 101,
                }
            ),
            encoding="utf-8",
        )

    def _write_registry(self) -> None:
        database = self.registry / "script_registry_42.db"
        database.write_bytes(b"registry-sidecar-database")
        manifest = self.registry / "script_registry_42.manifest.json"
        manifest.write_text(
            json.dumps(
                {
                    "manifest_version": DISTRIBUTION.REGISTRY_MANIFEST_VERSION,
                    "artifact_type": "balance_history_script_registry",
                    "registry_schema_version": "balance-history-script-registry-sqlite:v1",
                    "policy": "auxiliary_seen_scripts_non_consensus_v1",
                    "registry_artifact_id": self.registry_artifact_id,
                    "file_name": database.name,
                    "file_sha256": sha256(database),
                    "base": {
                        "btc_network": "bitcoin",
                        "btc_genesis_hash": "55" * 32,
                        "base_height": self.height,
                        "base_block_hash": self.block_hash,
                        "core_snapshot_id": self.core_snapshot_id,
                    },
                    "entry_count": 4,
                    "signature_scheme": "ed25519",
                    "signing_key_id": self.signing_key_id,
                    "generated_at": 100,
                }
            ),
            encoding="utf-8",
        )
        manifest.with_suffix(".sig").write_text("registry-signature", encoding="utf-8")
        (self.registry / "complete.json").write_text(
            json.dumps(
                {
                    "version": 2,
                    "height": self.height,
                    "network": "bitcoin",
                    "btc_block_hash": self.block_hash,
                    "core_snapshot_id": self.core_snapshot_id,
                    "registry_artifact_id": self.registry_artifact_id,
                    "registry_file": database.name,
                    "manifest_file": manifest.name,
                    "signature_file": manifest.with_suffix(".sig").name,
                    "file_sha256": sha256(database),
                    "entry_count": 4,
                    "completed_at": 102,
                }
            ),
            encoding="utf-8",
        )

    def _finalized_component(self, component: str) -> dict[str, object]:
        is_core = component == "core"
        directory = self.core if is_core else self.registry
        database = directory / (
            "balance_history_core_42.db" if is_core else "script_registry_42.db"
        )
        manifest = database.with_suffix(".manifest.json")
        return {
            "component": component,
            "height": self.height,
            "network": "bitcoin",
            "btc_block_hash": self.block_hash,
            "core_snapshot_id": self.core_snapshot_id,
            "artifact_id": self.core_artifact_id if is_core else self.registry_artifact_id,
            "artifact_dir": f"snapshots/000000000042/hash/{'core' if is_core else 'script-registry'}",
            "file": database.name,
            "manifest_file": manifest.name,
            "signature_file": manifest.with_suffix(".sig").name,
            "file_sha256": sha256(database),
            "signing_key_id": self.signing_key_id,
            "trusted_keys_sha256": sha256(self.trusted_keys),
        }

    def _write_finalization(self, *, include_registry: bool) -> None:
        self.finalization.write_text(
            json.dumps(
                {
                    "schema_version": "usdb-snapshot-artifact-finalization:v2",
                    "artifacts": {
                        "core": self._finalized_component("core"),
                        "script_registry": (
                            self._finalized_component("script_registry")
                            if include_registry
                            else None
                        ),
                    },
                    "producer_revision": "aa" * 20,
                    "finalizer_revision": "bb" * 20,
                    "finalized_at_utc": "2026-09-05T00:00:00Z",
                    "trusted_keys_sha256": sha256(self.trusted_keys),
                }
            ),
            encoding="utf-8",
        )

    def prepare(self) -> tuple[Path, dict[str, object], str]:
        return DISTRIBUTION.prepare_release_record(
            artifact_dir=self.artifact,
            trusted_keys=self.trusted_keys,
            finalization_marker=self.finalization,
            public_base_url="https://snapshots.example.test",
            producer_revision="aa" * 20,
            output_dir=self.root / "records",
        )

    def test_shared_strict_json_corpus_matches_python_parser(self) -> None:
        corpus = json.loads(STRICT_JSON_CORPUS.read_text(encoding="utf-8"))
        for case in corpus["cases"]:
            with self.subTest(case=case["name"]):
                try:
                    json.loads(case["json"], object_pairs_hook=DISTRIBUTION._strict_object)
                    valid = True
                except ValueError:
                    valid = False
                self.assertEqual(valid, case["valid"])

    def test_prepare_builds_required_core_and_optional_registry(self) -> None:
        record_path, record, digest = self.prepare()
        self.assertEqual(record["schema_version"], DISTRIBUTION.RECORD_SCHEMA_VERSION)
        self.assertEqual(sha256(record_path), digest)
        self.assertEqual(record["components"]["core"]["artifact_id"], self.core_artifact_id)
        self.assertEqual(
            record["components"]["script_registry"]["artifact_id"],
            self.registry_artifact_id,
        )
        self.assertTrue(
            all(
                item["object_key"].startswith("snapshots/v3/")
                for item in DISTRIBUTION._all_files(record)
            )
        )

    def test_completion_version_matches_the_rust_artifact_writer(self) -> None:
        source = MODULE_PATH.parents[3] / "src/btc/balance-history-snapshot-tool/src/state.rs"
        version = re.search(r"const COMPLETE_MARKER_VERSION:\s*u32\s*=\s*([0-9]+);", source.read_text())
        self.assertIsNotNone(version)
        self.assertEqual(DISTRIBUTION.COMPLETE_MARKER_VERSION, int(version[1]))

    def test_prepare_rejects_legacy_unknown_and_non_integer_completion_versions(self) -> None:
        for directory in (self.core, self.registry):
            complete = directory / "complete.json"
            original = complete.read_bytes()
            marker = json.loads(original)
            for version in (1, 3, 2.0, True):
                with self.subTest(component=directory.name, version=version):
                    marker["version"] = version
                    complete.write_text(json.dumps(marker))
                    with self.assertRaisesRegex(ValueError, "completion marker version: expected 2"):
                        self.prepare()
            complete.write_bytes(original)

    def test_prepare_allows_core_only_release(self) -> None:
        self._write_finalization(include_registry=False)
        _path, record, _digest = self.prepare()
        self.assertIsNone(record["components"]["script_registry"])
        self.assertEqual(len(DISTRIBUTION._all_files(record)), 4)

    def test_upload_is_idempotent_and_record_is_content_addressed(self) -> None:
        record_path, record, digest = self.prepare()
        client = FakeAwsClient(self.root / "objects")
        first = DISTRIBUTION.upload_release(record_path, self.artifact, client)
        first_uploads = len([event for event in client.events if event[0] == "upload"])
        second = DISTRIBUTION.upload_release(record_path, self.artifact, client)
        self.assertEqual(first["record_url"], second["record_url"])
        self.assertEqual(first["record_object_key"], f"snapshot-records/v3/{digest}.json")
        self.assertEqual(
            first_uploads,
            len([event for event in client.events if event[0] == "upload"]),
        )
        self.assertEqual(first_uploads, len(DISTRIBUTION._all_files(record)) + 1)

    def test_core_and_registry_install_independently(self) -> None:
        record_path, record, digest = self.prepare()
        object_root = self.root / "objects"
        client = FakeAwsClient(object_root)
        published = DISTRIBUTION.upload_release(record_path, self.artifact, client)
        self.assertTrue(published["record_url"].endswith(f"/{digest}.json"))
        calls: list[str] = []

        def download(url: str, destination: Path, _curl: str) -> None:
            calls.append(url)
            source = object_root / urlparse(url).path.lstrip("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)

        with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=download):
            core = DISTRIBUTION.install_release(
                record_url=published["record_url"],
                approved_record_path=record_path,
                destination_root=self.root / "installed-core",
                trusted_keys=self.trusted_keys,
                expected_network="bitcoin",
                max_height=self.height,
                component="core",
            )
            registry = DISTRIBUTION.install_release(
                record_url=published["record_url"],
                approved_record_path=record_path,
                destination_root=self.root / "installed-registry",
                trusted_keys=self.trusted_keys,
                expected_network="bitcoin",
                max_height=self.height,
                component="script_registry",
            )
        self.assertEqual(core.component, "core")
        self.assertEqual(core.release_dir.name, record["snapshot_release_id"])
        self.assertEqual(registry.component, "script_registry")
        self.assertEqual(registry.release_dir.name, self.registry_artifact_id)
        self.assertEqual(core.snapshot_file.read_bytes(), b"core-snapshot-database")
        self.assertEqual(registry.snapshot_file.read_bytes(), b"registry-sidecar-database")
        self.assertTrue(any("/core/" in url for url in calls))
        self.assertTrue(any("/script-registry/" in url for url in calls))

    def test_install_rejects_missing_optional_component(self) -> None:
        self._write_finalization(include_registry=False)
        record_path, record, digest = self.prepare()
        with self.assertRaisesRegex(ValueError, "no script_registry component"):
            DISTRIBUTION.install_release(
                record_url=f"{record['public_base_url']}/snapshot-records/v3/{digest}.json",
                approved_record_path=record_path,
                destination_root=self.root / "installed",
                trusted_keys=self.trusted_keys,
                component="script_registry",
            )

    def test_record_validation_rejects_cross_component_identity_change(self) -> None:
        _path, record, _digest = self.prepare()
        record["components"]["script_registry"]["artifact_id"] = "99" * 32
        with self.assertRaisesRegex(ValueError, "object prefix mismatch"):
            DISTRIBUTION.validate_release_record(record)

    def test_record_validation_rejects_btc_block_hash_change(self) -> None:
        _path, record, _digest = self.prepare()
        record["btc_block_hash"] = "99" * 32

        with self.assertRaisesRegex(ValueError, "artifact set ID"):
            DISTRIBUTION.validate_release_record(record)

    def test_prepare_rejects_completion_marker_identity_tampering(self) -> None:
        complete = self.registry / "complete.json"
        marker = json.loads(complete.read_text(encoding="utf-8"))
        marker["core_snapshot_id"] = "99" * 32
        complete.write_text(json.dumps(marker), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "completion marker identity mismatch"):
            self.prepare()

    def test_upload_rejects_local_database_tamper(self) -> None:
        record_path, _record, _digest = self.prepare()
        (self.core / "balance_history_core_42.db").write_bytes(b"tampered")
        client = FakeAwsClient(self.root / "objects")
        with self.assertRaisesRegex(ValueError, "size mismatch|SHA-256 mismatch"):
            DISTRIBUTION.upload_release(record_path, self.artifact, client)
        self.assertFalse(any(event[0] == "upload" for event in client.events))

    def test_install_requires_digest_pinned_https_record(self) -> None:
        record_path, _record, _digest = self.prepare()
        for url in (
            "http://snapshots.example.test/snapshot-records/v3/" + "aa" * 32 + ".json",
            "https://snapshots.example.test/snapshot-records/v3/latest.json",
        ):
            with self.subTest(url=url):
                with self.assertRaisesRegex(ValueError, "HTTPS|SHA-256"):
                    DISTRIBUTION.install_release(
                        record_url=url,
                        approved_record_path=record_path,
                        destination_root=self.root / "installed",
                        trusted_keys=self.trusted_keys,
                    )

    def test_write_new_or_identical_never_replaces_different_content(self) -> None:
        path = self.root / "immutable.json"
        DISTRIBUTION._write_new_or_identical(path, b"first")
        DISTRIBUTION._write_new_or_identical(path, b"first")
        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            DISTRIBUTION._write_new_or_identical(path, b"second")
        self.assertEqual(path.read_bytes(), b"first")


if __name__ == "__main__":
    unittest.main()
