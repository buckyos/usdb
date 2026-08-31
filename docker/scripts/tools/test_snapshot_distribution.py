#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stderr
from pathlib import Path
from urllib.parse import urlparse
from unittest import mock


MODULE_PATH = Path(__file__).with_name("snapshot_distribution.py")
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
        self.temporary = tempfile.TemporaryDirectory(prefix="snapshot-distribution-test-")
        self.root = Path(self.temporary.name)
        self.artifact = self.root / "artifact"
        self.artifact.mkdir()
        self.height = 42
        self.block_hash = "1" * 64
        self.snapshot_id = "2" * 64
        self.signing_key_id = "snapshot-signer-1"
        self.snapshot = self.artifact / "snapshot_42.db"
        self.snapshot_bytes = b"tiny immutable snapshot database"
        self.snapshot.write_bytes(self.snapshot_bytes)
        self.manifest = self.artifact / "snapshot_42.manifest.json"
        manifest_value = {
            "manifest_version": DISTRIBUTION.SNAPSHOT_MANIFEST_VERSION,
            "file_name": self.snapshot.name,
            "file_sha256": sha256(self.snapshot),
            "state_ref": {
                "block_height": self.height,
                "stable_block_hash": self.block_hash,
                "snapshot_id": self.snapshot_id,
            },
            "db_identity": {"btc_network": "bitcoin"},
            "balance_query_floor": self.height,
            "history_query_floor": self.height + 1,
            "signature_scheme": "ed25519",
            "signing_key_id": self.signing_key_id,
            "generated_at": 100,
        }
        self.manifest.write_text(json.dumps(manifest_value), encoding="utf-8")
        self.signature = self.artifact / "snapshot_42.manifest.sig"
        self.signature.write_text("detached-signature", encoding="utf-8")
        complete = {
            "version": 1,
            "height": self.height,
            "network": "bitcoin",
            "btc_block_hash": self.block_hash,
            "snapshot_id": self.snapshot_id,
            "snapshot_file": self.snapshot.name,
            "manifest_file": self.manifest.name,
            "signature_file": self.signature.name,
            "file_sha256": sha256(self.snapshot),
            "balance_history_count": 1,
            "utxo_count": 2,
            "block_commit_count": 3,
            "script_registry_count": 4,
            "completed_at": 101,
        }
        (self.artifact / "complete.json").write_text(json.dumps(complete), encoding="utf-8")
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
        self.finalization.write_text(
            json.dumps(
                {
                    "version": 1,
                    "height": self.height,
                    "network": "bitcoin",
                    "btc_block_hash": self.block_hash,
                    "snapshot_id": self.snapshot_id,
                    "snapshot_file": self.snapshot.name,
                    "manifest_file": self.manifest.name,
                    "signature_file": self.signature.name,
                    "file_sha256": sha256(self.snapshot),
                    "signing_key_id": self.signing_key_id,
                    "trusted_keys_sha256": sha256(self.trusted_keys),
                    "producer_revision": "a" * 40,
                    "finalizer_revision": "b" * 40,
                    "finalized_at_utc": "2026-08-30T00:00:00+00:00",
                }
            ),
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def prepare(self) -> tuple[Path, dict[str, object], str]:
        return DISTRIBUTION.prepare_release_record(
            artifact_dir=self.artifact,
            trusted_keys=self.trusted_keys,
            finalization_marker=self.finalization,
            public_base_url="https://snapshots.example.test",
            producer_revision="a" * 40,
            output_dir=self.root / "records",
        )

    def test_prepare_upload_and_resumable_install_are_content_addressed(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        self.assertEqual(sha256(record_path), record_sha256)
        self.assertEqual(record["height"], self.height)
        self.assertEqual(record["trusted_keys"]["sha256"], sha256(self.trusted_keys))
        self.assertEqual(record["producer"]["artifact_producer_revision"], "a" * 40)
        self.assertEqual(record["producer"]["artifact_finalizer_revision"], "b" * 40)

        object_root = self.root / "objects"
        client = FakeAwsClient(object_root)
        published = DISTRIBUTION.upload_release(record_path, self.artifact, client)
        record_key = f"snapshot-records/v2/{record_sha256}.json"
        self.assertEqual(published["record_object_key"], record_key)
        upload_events = [event for event in client.events if event[0] == "upload"]
        self.assertEqual(upload_events[-1], ("upload", record_key))
        first_upload_count = len(upload_events)
        replayed_publish = DISTRIBUTION.upload_release(record_path, self.artifact, client)
        self.assertEqual(replayed_publish["record_url"], published["record_url"])
        self.assertEqual(
            len([event for event in client.events if event[0] == "upload"]),
            first_upload_count,
        )

        download_calls: list[str] = []

        def download(url: str, destination: Path, _curl: str) -> None:
            download_calls.append(url)
            source = object_root / urlparse(url).path.lstrip("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)

        destination_root = self.root / "installed"
        staging = destination_root / f".{record['snapshot_release_id']}.installing"
        staging.mkdir(parents=True)
        signature_entry = next(
            item for item in record["files"] if item["role"] == "snapshot_signature"
        )
        shutil.copyfile(
            self.artifact / signature_entry["path"],
            staging / f"{signature_entry['path']}.part",
        )
        with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=download):
            installed = DISTRIBUTION.install_release(
                record_url=published["record_url"],
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
                expected_network="bitcoin",
                max_height=self.height,
            )
            replay = DISTRIBUTION.install_release(
                record_url=published["record_url"],
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
                expected_network="bitcoin",
                max_height=self.height,
            )
        self.assertEqual(installed, replay)
        self.assertEqual(installed.snapshot_file.read_bytes(), self.snapshot.read_bytes())
        self.assertFalse(any(destination_root.glob(".*.installing")))
        self.assertFalse(
            any(url.endswith(signature_entry["object_key"]) for url in download_calls),
            "a complete staged .part file should be reused without another download",
        )

    def test_aws_cli_uses_r2_endpoint_region_and_profile_without_credentials(self) -> None:
        client = DISTRIBUTION.AwsCliClient(
            endpoint_url=DISTRIBUTION.DEFAULT_ENDPOINT_URL,
            bucket=DISTRIBUTION.DEFAULT_BUCKET,
            region="auto",
            profile="publisher",
        )
        response = mock.Mock(
            returncode=0,
            stdout=json.dumps({"ContentLength": 1, "Metadata": {}}),
            stderr="",
        )
        with mock.patch.object(DISTRIBUTION.subprocess, "run", return_value=response) as run:
            client.head("snapshot-records/v2/" + "1" * 64 + ".json")
        command = run.call_args.args[0]
        self.assertEqual(command[:7], [
            "aws",
            "--profile",
            "publisher",
            "--region",
            "auto",
            "--endpoint-url",
            DISTRIBUTION.DEFAULT_ENDPOINT_URL,
        ])
        self.assertNotIn("access-key", " ".join(command).lower())

    def test_forced_local_hash_progress_reports_bytes_and_completion(self) -> None:
        output = io.StringIO()
        with mock.patch.dict(DISTRIBUTION.os.environ, {"USDB_SNAPSHOT_FORCE_PROGRESS": "1"}):
            with redirect_stderr(output):
                digest = DISTRIBUTION._sha256(
                    self.snapshot,
                    progress_label=f"Local verify {self.snapshot.name}",
                )
        progress = output.getvalue()
        self.assertEqual(digest, sha256(self.snapshot))
        self.assertIn("Local verify snapshot_42.db", progress)
        self.assertIn("100.00%", progress)
        self.assertIn("complete", progress)

    def test_aws_cli_upload_progress_is_redirected_away_from_json_stdout(self) -> None:
        client = DISTRIBUTION.AwsCliClient(
            endpoint_url=DISTRIBUTION.DEFAULT_ENDPOINT_URL,
            bucket=DISTRIBUTION.DEFAULT_BUCKET,
            region="auto",
            profile="publisher",
        )
        output = io.StringIO()
        with redirect_stderr(output):
            with mock.patch.object(DISTRIBUTION, "_progress_enabled", return_value=True):
                with mock.patch.object(DISTRIBUTION.subprocess, "run") as run:
                    client.upload(
                        self.snapshot,
                        "snapshots/v2/test/snapshot_42.db",
                        sha256(self.snapshot),
                        self.snapshot.stat().st_size,
                        "application/octet-stream",
                    )
        command = run.call_args.args[0]
        self.assertNotIn("--only-show-errors", command)
        self.assertIs(run.call_args.kwargs["stdout"], output)
        self.assertIn("Upload snapshot_42.db: complete", output.getvalue())

    def test_upload_rejects_snapshot_db_tamper_before_remote_write(self) -> None:
        record_path, _record, _digest = self.prepare()
        self.snapshot.write_bytes(b"x" * len(self.snapshot_bytes))
        client = FakeAwsClient(self.root / "objects")
        with self.assertRaisesRegex(ValueError, "snapshot release source file SHA-256 mismatch"):
            DISTRIBUTION.upload_release(record_path, self.artifact, client)
        self.assertEqual(client.events, [])

    def test_install_rejects_wrong_catalog_and_height_before_artifact_download(self) -> None:
        record_path, _record, _digest = self.prepare()
        object_root = self.root / "objects"
        client = FakeAwsClient(object_root)
        published = DISTRIBUTION.upload_release(record_path, self.artifact, client)

        calls: list[str] = []

        def download(url: str, destination: Path, _curl: str) -> None:
            calls.append(url)
            source = object_root / urlparse(url).path.lstrip("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)

        with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=download):
            with self.assertRaisesRegex(ValueError, "exceeds allowed maximum"):
                DISTRIBUTION.install_release(
                    record_url=published["record_url"],
                    destination_root=self.root / "height-rejected",
                    trusted_keys=self.trusted_keys,
                    max_height=self.height - 1,
                )
        self.assertEqual(len(calls), 1, "only the small release record should be downloaded")

        wrong_catalog = self.root / self.trusted_keys.name
        wrong_catalog.write_text('{"keys": []}', encoding="utf-8")
        with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=download):
            with self.assertRaisesRegex(ValueError, "trusted-key catalog"):
                DISTRIBUTION.install_release(
                    record_url=published["record_url"],
                    destination_root=self.root / "catalog-rejected",
                    trusted_keys=wrong_catalog,
                )


if __name__ == "__main__":
    unittest.main()
