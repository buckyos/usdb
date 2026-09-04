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
        status_output = io.StringIO()
        with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=download):
            with mock.patch.object(
                DISTRIBUTION,
                "_sha256",
                wraps=DISTRIBUTION._sha256,
            ) as calculate_hash:
                with mock.patch.dict(
                    DISTRIBUTION.os.environ,
                    {"USDB_SNAPSHOT_FORCE_PROGRESS": "1"},
                ):
                    with redirect_stderr(status_output):
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
        snapshot_hashes = [
            Path(call.args[0]).name
            for call in calculate_hash.call_args_list
            if Path(call.args[0]).name
            in {self.snapshot.name, f"{self.snapshot.name}.part"}
        ]
        self.assertEqual(
            snapshot_hashes,
            [f"{self.snapshot.name}.part", self.snapshot.name],
            "fresh install and explicit cache replay must each hash the DB exactly once",
        )
        status = status_output.getvalue()
        self.assertIn("Verify downloaded snapshot_42.db", status)
        self.assertIn("Verify cached snapshot_42.db", status)
        self.assertIn("100.00%", status)
        self.assertIn("Snapshot files verified; publishing atomically", status)
        self.assertIn("Snapshot artifact ready", status)

    def _seed_installed_release(
        self,
        record_path: Path,
        record: dict[str, object],
        destination_root: Path,
    ) -> Path:
        release_dir = destination_root / str(record["snapshot_release_id"])
        release_dir.mkdir(parents=True)
        files = record["files"]
        assert isinstance(files, list)
        for item in files:
            assert isinstance(item, dict)
            source = self.artifact / str(item["path"])
            destination = release_dir / str(item["path"])
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        shutil.copyfile(record_path, release_dir / "snapshot-release-record.json")
        return release_dir

    def test_install_reuses_complete_approved_artifact_without_network(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "preinstalled"
        release_dir = self._seed_installed_release(record_path, record, destination_root)
        record_url = (
            "https://snapshots.example.test/snapshot-records/v2/"
            f"{record_sha256}.json"
        )

        with mock.patch.object(
            DISTRIBUTION,
            "_download_with_resume",
            side_effect=AssertionError("network download must not run"),
        ) as sequential:
            with mock.patch.object(
                DISTRIBUTION,
                "_download_parallel_ranges",
                side_effect=AssertionError("parallel download must not run"),
            ) as parallel:
                with redirect_stderr(io.StringIO()):
                    installed = DISTRIBUTION.install_release(
                        record_url=record_url,
                        destination_root=destination_root,
                        trusted_keys=self.trusted_keys,
                        approved_record_path=record_path,
                        expected_network="bitcoin",
                        max_height=self.height,
                    )

        self.assertEqual(installed.release_dir, release_dir)
        self.assertFalse((destination_root / ".downloads").exists())
        sequential.assert_not_called()
        parallel.assert_not_called()

    def test_install_rejects_corrupt_approved_artifact_without_overwrite(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "corrupt-preinstalled"
        release_dir = self._seed_installed_release(record_path, record, destination_root)
        snapshot_entry = next(
            item for item in record["files"] if item["role"] == "snapshot_db"
        )
        corrupt_path = release_dir / snapshot_entry["path"]
        corrupt_path.write_bytes(b"corrupt")

        with mock.patch.object(DISTRIBUTION, "_download_with_resume") as sequential:
            with mock.patch.object(DISTRIBUTION, "_download_parallel_ranges") as parallel:
                with redirect_stderr(io.StringIO()):
                    with self.assertRaisesRegex(ValueError, "snapshot file size mismatch"):
                        DISTRIBUTION.install_release(
                            record_url=(
                                "https://snapshots.example.test/snapshot-records/v2/"
                                f"{record_sha256}.json"
                            ),
                            destination_root=destination_root,
                            trusted_keys=self.trusted_keys,
                            approved_record_path=record_path,
                            expected_network="bitcoin",
                            max_height=self.height,
                        )

        self.assertEqual(corrupt_path.read_bytes(), b"corrupt")
        sequential.assert_not_called()
        parallel.assert_not_called()

    def test_install_rejects_symlinked_staging_directory(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "symlinked-staging"
        destination_root.mkdir()
        outside = self.root / "outside-staging"
        outside.mkdir()
        sentinel = outside / "sentinel"
        sentinel.write_bytes(b"unchanged")
        staging = destination_root / f".{record['snapshot_release_id']}.installing"
        staging.symlink_to(outside, target_is_directory=True)

        with self.assertRaisesRegex(ValueError, "staging directory must be a real directory"):
            DISTRIBUTION.install_release(
                record_url=(
                    "https://snapshots.example.test/snapshot-records/v2/"
                    f"{record_sha256}.json"
                ),
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
                approved_record_path=record_path,
            )

        self.assertEqual(sentinel.read_bytes(), b"unchanged")
        self.assertEqual(list(outside.iterdir()), [sentinel])

    def test_install_rejects_symlinked_partial_without_touching_target(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "symlinked-partial"
        staging = destination_root / f".{record['snapshot_release_id']}.installing"
        staging.mkdir(parents=True)
        snapshot_entry = next(
            item for item in record["files"] if item["role"] == "snapshot_db"
        )
        outside = self.root / "outside-partial"
        outside.write_bytes(b"unchanged")
        (staging / f"{snapshot_entry['path']}.part").symlink_to(outside)

        with mock.patch.object(DISTRIBUTION, "_download_with_resume") as download:
            with self.assertRaisesRegex(ValueError, "must be a regular file, not a symlink"):
                DISTRIBUTION.install_release(
                    record_url=(
                        "https://snapshots.example.test/snapshot-records/v2/"
                        f"{record_sha256}.json"
                    ),
                    destination_root=destination_root,
                    trusted_keys=self.trusted_keys,
                    approved_record_path=record_path,
                )

        download.assert_not_called()
        self.assertEqual(outside.read_bytes(), b"unchanged")

    def test_install_rejects_unmanaged_staging_entry(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "unmanaged-staging"
        staging = destination_root / f".{record['snapshot_release_id']}.installing"
        staging.mkdir(parents=True)
        (staging / "operator-note.txt").write_text("unexpected", encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "unexpected entry in snapshot release directory"):
            DISTRIBUTION.install_release(
                record_url=(
                    "https://snapshots.example.test/snapshot-records/v2/"
                    f"{record_sha256}.json"
                ),
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
                approved_record_path=record_path,
            )

    def test_install_rejects_symlinked_download_cache(self) -> None:
        record_path, _record, record_sha256 = self.prepare()
        destination_root = self.root / "symlinked-downloads"
        destination_root.mkdir()
        outside = self.root / "outside-downloads"
        outside.mkdir()
        (destination_root / ".downloads").symlink_to(outside, target_is_directory=True)

        with self.assertRaisesRegex(ValueError, "download directory must be a real directory"):
            DISTRIBUTION.install_release(
                record_url=(
                    "https://snapshots.example.test/snapshot-records/v2/"
                    f"{record_sha256}.json"
                ),
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
            )

        self.assertEqual(list(outside.iterdir()), [])
        self.assertTrue(record_path.is_file())

    def test_install_rejects_extra_file_in_completed_release(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        destination_root = self.root / "extra-installed-entry"
        release_dir = self._seed_installed_release(record_path, record, destination_root)
        (release_dir / "untracked").write_bytes(b"unexpected")

        with self.assertRaisesRegex(ValueError, "unexpected entry in snapshot release directory"):
            DISTRIBUTION.install_release(
                record_url=(
                    "https://snapshots.example.test/snapshot-records/v2/"
                    f"{record_sha256}.json"
                ),
                destination_root=destination_root,
                trusted_keys=self.trusted_keys,
                approved_record_path=record_path,
            )

    def test_sequential_resume_rejects_symlink_target(self) -> None:
        outside = self.root / "sequential-outside"
        outside.write_bytes(b"unchanged")
        destination = self.root / "download.part"
        destination.symlink_to(outside)

        with mock.patch.object(DISTRIBUTION.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "resumable download target must be a regular file"):
                DISTRIBUTION._download_with_resume(
                    "https://snapshots.example.test/snapshot.db",
                    destination,
                )

        run.assert_not_called()
        self.assertEqual(outside.read_bytes(), b"unchanged")

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
            upload_concurrency=16,
            multipart_chunk_size_mib=64,
        )
        output = io.StringIO()
        source_config = self.root / "aws-config"
        source_config.write_text("[profile publisher]\nregion = auto\n", encoding="utf-8")
        original_config = source_config.read_bytes()
        with mock.patch.dict(
            DISTRIBUTION.os.environ,
            {"AWS_CONFIG_FILE": str(source_config)},
            clear=False,
        ):
            with redirect_stderr(output):
                with mock.patch.object(DISTRIBUTION, "_progress_enabled", return_value=True):
                    response = mock.Mock(returncode=0, stdout="", stderr="")
                    with mock.patch.object(DISTRIBUTION.subprocess, "run", return_value=response) as run:
                        client.upload(
                            self.snapshot,
                            "snapshots/v2/test/snapshot_42.db",
                            sha256(self.snapshot),
                            self.snapshot.stat().st_size,
                            "application/octet-stream",
                        )
        calls = run.call_args_list
        self.assertEqual(len(calls), 5)
        configured = {
            call.args[0][-2]: call.args[0][-1]
            for call in calls[:-1]
        }
        self.assertEqual(configured["s3.max_concurrent_requests"], "16")
        self.assertEqual(configured["s3.multipart_threshold"], "64MB")
        self.assertEqual(configured["s3.multipart_chunksize"], "64MB")
        self.assertEqual(configured["s3.preferred_transfer_client"], "classic")
        command = calls[-1].args[0]
        self.assertNotIn("--only-show-errors", command)
        self.assertIs(calls[-1].kwargs["stdout"], output)
        temporary_config = Path(calls[-1].kwargs["env"]["AWS_CONFIG_FILE"])
        self.assertTrue(temporary_config.is_file())
        self.assertEqual(temporary_config.stat().st_mode & 0o777, 0o600)
        self.assertEqual(source_config.read_bytes(), original_config)
        self.assertIn("Upload snapshot_42.db: complete", output.getvalue())
        client.close()
        self.assertFalse(temporary_config.exists())

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

    def test_install_uses_parallel_ranges_only_for_snapshot_database(self) -> None:
        record_path, record, _digest = self.prepare()
        object_root = self.root / "parallel-install-objects"
        published = DISTRIBUTION.upload_release(
            record_path,
            self.artifact,
            FakeAwsClient(object_root),
        )
        sequential_calls: list[str] = []
        parallel_calls: list[str] = []

        def copy_object(url: str, destination: Path) -> None:
            source = object_root / urlparse(url).path.lstrip("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)

        def sequential(url: str, destination: Path, _curl: str) -> None:
            sequential_calls.append(url)
            copy_object(url, destination)

        def parallel(url: str, destination: Path, **_kwargs: object) -> None:
            parallel_calls.append(url)
            copy_object(url, destination)

        with mock.patch.object(DISTRIBUTION, "PARALLEL_DOWNLOAD_MIN_SIZE", 1):
            with mock.patch.object(DISTRIBUTION, "_download_with_resume", side_effect=sequential):
                with mock.patch.object(DISTRIBUTION, "_download_parallel_ranges", side_effect=parallel):
                    with redirect_stderr(io.StringIO()):
                        installed = DISTRIBUTION.install_release(
                            record_url=published["record_url"],
                            destination_root=self.root / "parallel-installed",
                            trusted_keys=self.trusted_keys,
                        )
        snapshot_entry = next(item for item in record["files"] if item["role"] == "snapshot_db")
        self.assertEqual(len(parallel_calls), 1)
        self.assertTrue(parallel_calls[0].endswith(snapshot_entry["object_key"]))
        self.assertFalse(any(url.endswith(snapshot_entry["object_key"]) for url in sequential_calls))
        self.assertEqual(installed.snapshot_file.read_bytes(), self.snapshot.read_bytes())

    def test_public_verifier_checks_record_and_every_object_size(self) -> None:
        record_path, record, record_sha256 = self.prepare()
        calls: list[tuple[str, str]] = []

        def read_public(url: str, *, method: str) -> tuple[bytes, int, str]:
            calls.append((method, url))
            if method == "GET":
                content = record_path.read_bytes()
                return content, len(content), url
            item = next(item for item in record["files"] if url.endswith(item["object_key"]))
            return b"", item["size"], url

        with mock.patch.object(DISTRIBUTION, "_read_public_url", side_effect=read_public):
            with mock.patch.object(DISTRIBUTION, "_probe_public_byte_range") as probe_range:
                verified = DISTRIBUTION.verify_public_release(record_path, self.trusted_keys)
        self.assertEqual(verified["record_sha256"], record_sha256)
        self.assertEqual(verified["verified_file_count"], 4)
        self.assertTrue(verified["snapshot_db_byte_range_verified"])
        self.assertEqual(len(calls), 5)
        self.assertEqual(calls[0][0], "GET")
        self.assertTrue(all(method == "HEAD" for method, _url in calls[1:]))
        snapshot_file = next(item for item in record["files"] if item["role"] == "snapshot_db")
        probe_range.assert_called_once_with(
            f"{record['public_base_url']}/{snapshot_file['object_key']}",
            snapshot_file["size"],
        )

    def test_public_verifier_rejects_incomplete_object(self) -> None:
        record_path, record, _record_sha256 = self.prepare()

        def read_public(url: str, *, method: str) -> tuple[bytes, int, str]:
            if method == "GET":
                content = record_path.read_bytes()
                return content, len(content), url
            item = next(item for item in record["files"] if url.endswith(item["object_key"]))
            return b"", item["size"] - 1, url

        with mock.patch.object(DISTRIBUTION, "_read_public_url", side_effect=read_public):
            with mock.patch.object(DISTRIBUTION, "_probe_public_byte_range"):
                with self.assertRaisesRegex(ValueError, "public snapshot object size mismatch"):
                    DISTRIBUTION.verify_public_release(record_path, self.trusted_keys)

    def test_public_byte_range_probe_requires_partial_content(self) -> None:
        url = "https://snapshots.example/snapshot.db"
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.geturl.return_value = url
        response.status = 206
        response.headers = {"Content-Range": "bytes 0-0/10"}
        response.read.return_value = b"x"
        with mock.patch.object(DISTRIBUTION, "urlopen", return_value=response) as open_url:
            DISTRIBUTION._probe_public_byte_range(url, 10)
        request = open_url.call_args.args[0]
        self.assertEqual(request.get_header("Range"), "bytes=0-0")

        response.status = 200
        with mock.patch.object(DISTRIBUTION, "urlopen", return_value=response):
            with self.assertRaisesRegex(ValueError, "does not support byte ranges"):
                DISTRIBUTION._probe_public_byte_range(url, 10)

    def test_parallel_range_download_resumes_only_missing_chunks(self) -> None:
        chunk_size = 1024 * 1024
        content = b"a" * chunk_size + b"b" * chunk_size + b"c" * (chunk_size // 2)
        destination = self.root / "parallel-snapshot.db.part"
        calls: list[tuple[int, int]] = []

        def run(command: list[str], **_kwargs: object) -> mock.Mock:
            start_text, end_text = command[command.index("--range") + 1].split("-", 1)
            start = int(start_text)
            end = int(end_text)
            output = Path(command[command.index("--output") + 1])
            output.write_bytes(content[start : end + 1])
            calls.append((start, end))
            return mock.Mock(returncode=0, stdout="206", stderr="")

        with mock.patch.object(DISTRIBUTION.subprocess, "run", side_effect=run):
            DISTRIBUTION._download_parallel_ranges(
                "https://snapshots.example/snapshot.db",
                destination,
                expected_size=len(content),
                expected_sha256=hashlib.sha256(content).hexdigest(),
                concurrency=3,
                chunk_size=chunk_size,
            )
        self.assertEqual(destination.read_bytes(), content)
        self.assertEqual(len(calls), 3)

        state_path, _work_dir = DISTRIBUTION._range_download_paths(destination)
        state = DISTRIBUTION._load_json(state_path)
        state["completed_chunks"] = [0, 2]
        DISTRIBUTION._write_range_state(state_path, state)
        with destination.open("r+b") as output:
            output.seek(chunk_size)
            output.write(b"x" * chunk_size)
        calls.clear()
        with mock.patch.object(DISTRIBUTION.subprocess, "run", side_effect=run):
            DISTRIBUTION._download_parallel_ranges(
                "https://snapshots.example/snapshot.db",
                destination,
                expected_size=len(content),
                expected_sha256=hashlib.sha256(content).hexdigest(),
                concurrency=3,
                chunk_size=chunk_size,
            )
        self.assertEqual(calls, [(chunk_size, chunk_size * 2 - 1)])
        self.assertEqual(destination.read_bytes(), content)
        DISTRIBUTION._cleanup_range_download(destination)

    def test_parallel_range_download_rejects_full_object_response(self) -> None:
        content = b"x" * (1024 * 1024)
        destination = self.root / "range-rejected.db.part"

        def run(command: list[str], **_kwargs: object) -> mock.Mock:
            output = Path(command[command.index("--output") + 1])
            output.write_bytes(content)
            return mock.Mock(returncode=0, stdout="200", stderr="")

        with mock.patch.object(DISTRIBUTION.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(ValueError, "did not return HTTP 206"):
                DISTRIBUTION._download_parallel_ranges(
                    "https://snapshots.example/snapshot.db",
                    destination,
                    expected_size=len(content),
                    expected_sha256=hashlib.sha256(content).hexdigest(),
                    concurrency=2,
                    chunk_size=1024 * 1024,
                )

    def test_parallel_range_download_rejects_symlinked_state(self) -> None:
        destination = self.root / "range-state.db.part"
        state_path, _work_dir = DISTRIBUTION._range_download_paths(destination)
        outside = self.root / "outside-range-state"
        outside.write_text("unchanged", encoding="utf-8")
        state_path.symlink_to(outside)

        with self.assertRaisesRegex(ValueError, "parallel download state must be a regular file"):
            DISTRIBUTION._download_parallel_ranges(
                "https://snapshots.example/snapshot.db",
                destination,
                expected_size=1024 * 1024,
                expected_sha256="1" * 64,
                concurrency=2,
                chunk_size=1024 * 1024,
            )

        self.assertEqual(outside.read_text(encoding="utf-8"), "unchanged")

    def test_parallel_range_download_rejects_symlinked_work_directory(self) -> None:
        destination = self.root / "range-work.db.part"
        _state_path, work_dir = DISTRIBUTION._range_download_paths(destination)
        outside = self.root / "outside-range-work"
        outside.mkdir()
        sentinel = outside / "sentinel"
        sentinel.write_text("unchanged", encoding="utf-8")
        work_dir.symlink_to(outside, target_is_directory=True)

        with self.assertRaisesRegex(ValueError, "work directory must be a real directory"):
            DISTRIBUTION._download_parallel_ranges(
                "https://snapshots.example/snapshot.db",
                destination,
                expected_size=1024 * 1024,
                expected_sha256="1" * 64,
                concurrency=2,
                chunk_size=1024 * 1024,
            )

        self.assertEqual(sentinel.read_text(encoding="utf-8"), "unchanged")

    def test_pwrite_all_retries_short_writes(self) -> None:
        writes: list[tuple[bytes, int]] = []

        def pwrite(_descriptor: int, data: memoryview, offset: int) -> int:
            written = min(2, len(data))
            writes.append((bytes(data[:written]), offset))
            return written

        with mock.patch.object(DISTRIBUTION.os, "pwrite", side_effect=pwrite):
            DISTRIBUTION._pwrite_all(7, b"abcde", 11)
        self.assertEqual(writes, [(b"ab", 11), (b"cd", 13), (b"e", 15)])


if __name__ == "__main__":
    unittest.main()
