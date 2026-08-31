#!/usr/bin/env python3
"""Prepare, publish, and install immutable balance-history snapshot releases."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import quote, urlparse


RECORD_SCHEMA_VERSION = "usdb-snapshot-release-record:v2"
ARTIFACT_TYPE = "balance-history"
SNAPSHOT_MANIFEST_VERSION = "balance-history-snapshot-manifest:v3"
SIGNATURE_SCHEME = "ed25519"
DEFAULT_BUCKET = "usdb-snapshot"
DEFAULT_ENDPOINT_URL = "https://87e0bdf811b13ee87fd0bcec7a4fd1e7.r2.cloudflarestorage.com"
DEFAULT_PUBLIC_BASE_URL = "https://usdb-snapshot.tbudr.top"
DEFAULT_AWS_REGION = "auto"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
RELEASE_ID_RE = re.compile(r"^balance-history-[a-z0-9-]+-h[0-9]+-[0-9a-f]{16}$")
EXPECTED_FILE_ROLES = (
    "snapshot_db",
    "snapshot_manifest",
    "snapshot_signature",
    "completion_marker",
)


def _strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_strict_object)
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"failed to read strict JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"JSON document must be an object: {path}")
    return value


def _canonical_json(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(8 * 1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _require_exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    _require(actual == expected, f"{label} fields mismatch: missing={sorted(expected - actual)}, extra={sorted(actual - expected)}")


def _require_sha256(value: Any, label: str) -> str:
    _require(isinstance(value, str) and SHA256_RE.fullmatch(value) is not None, f"{label} must be lowercase SHA-256")
    return value


def _require_u32(value: Any, label: str) -> int:
    _require(isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= 0xFFFFFFFF, f"{label} must be a u32")
    return value


def _safe_basename(value: Any, label: str) -> str:
    _require(isinstance(value, str) and bool(value), f"{label} is required")
    path = PurePosixPath(value)
    _require(not path.is_absolute() and len(path.parts) == 1 and value not in {".", ".."}, f"{label} must be a safe basename")
    return value


def _normalize_https_base(value: str, label: str) -> str:
    normalized = value.rstrip("/")
    parsed = urlparse(normalized)
    _require(
        parsed.scheme == "https"
        and bool(parsed.netloc)
        and parsed.username is None
        and parsed.password is None
        and not parsed.query
        and not parsed.fragment,
        f"{label} must be an HTTPS URL without credentials, query, or fragment",
    )
    return normalized


def _safe_object_key(value: Any, label: str) -> str:
    _require(isinstance(value, str) and bool(value), f"{label} is required")
    path = PurePosixPath(value)
    _require(not path.is_absolute() and ".." not in path.parts and all(part not in {"", "."} for part in path.parts), f"{label} is unsafe")
    return value


def _write_new_or_identical(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        _require(path.is_file() and path.read_bytes() == content, f"refusing to replace different existing file: {path}")
        return
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
    try:
        with os.fdopen(descriptor, "wb") as target:
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _validate_trusted_catalog(path: Path, signing_key_id: str) -> str:
    catalog = _load_json(path)
    _require_exact_keys(catalog, {"keys"}, "trusted-key catalog")
    keys = catalog["keys"]
    _require(isinstance(keys, list) and bool(keys), "trusted-key catalog keys must be a non-empty array")
    matches = 0
    seen: set[str] = set()
    for item in keys:
        _require(isinstance(item, dict), "trusted-key catalog entry must be an object")
        _require_exact_keys(item, {"key_id", "public_key_base64"}, "trusted-key catalog entry")
        key_id = item["key_id"]
        _require(isinstance(key_id, str) and bool(key_id) and key_id not in seen, "trusted-key catalog contains an invalid or duplicate key ID")
        _require(isinstance(item["public_key_base64"], str) and bool(item["public_key_base64"]), "trusted-key catalog public key is required")
        seen.add(key_id)
        matches += int(key_id == signing_key_id)
    _require(matches == 1, f"snapshot signer {signing_key_id!r} is not uniquely trusted by {path}")
    return _sha256(path)


def prepare_release_record(
    *,
    artifact_dir: Path,
    trusted_keys: Path,
    finalization_marker: Path,
    public_base_url: str,
    producer_revision: str,
    output_dir: Path,
) -> tuple[Path, dict[str, Any], str]:
    artifact = artifact_dir.expanduser().resolve()
    _require(artifact.is_dir(), f"snapshot artifact directory does not exist: {artifact}")
    _require(REVISION_RE.fullmatch(producer_revision) is not None, "producer revision must be a lowercase 40-character Git commit")
    public_base = _normalize_https_base(public_base_url, "public base URL")

    complete_path = artifact / "complete.json"
    _require(complete_path.is_file() and not complete_path.is_symlink(), f"snapshot completion marker is missing or not regular: {complete_path}")
    complete = _load_json(complete_path)
    _require_exact_keys(
        complete,
        {
            "balance_history_count",
            "block_commit_count",
            "btc_block_hash",
            "completed_at",
            "file_sha256",
            "height",
            "manifest_file",
            "network",
            "script_registry_count",
            "signature_file",
            "snapshot_file",
            "snapshot_id",
            "utxo_count",
            "version",
        },
        "snapshot completion marker",
    )
    _require(complete["version"] == 1, "unsupported snapshot completion marker version")
    for count_key in (
        "balance_history_count",
        "block_commit_count",
        "script_registry_count",
        "utxo_count",
    ):
        count = complete[count_key]
        _require(isinstance(count, int) and not isinstance(count, bool) and count >= 0, f"snapshot completion marker {count_key} must be non-negative")
    _require(isinstance(complete["completed_at"], int) and not isinstance(complete["completed_at"], bool) and complete["completed_at"] >= 0, "snapshot completion time must be a Unix timestamp")
    height = _require_u32(complete["height"], "snapshot height")
    network = complete["network"]
    _require(isinstance(network, str) and re.fullmatch(r"[a-z0-9-]+", network) is not None, "invalid snapshot network")
    btc_block_hash = _require_sha256(complete["btc_block_hash"], "BTC block hash")
    snapshot_id = _require_sha256(complete["snapshot_id"], "snapshot ID")
    expected_db_sha256 = _require_sha256(complete["file_sha256"], "snapshot DB SHA-256")
    snapshot_name = _safe_basename(complete["snapshot_file"], "snapshot file")
    manifest_name = _safe_basename(complete["manifest_file"], "snapshot manifest")
    signature_name = _safe_basename(complete["signature_file"], "snapshot signature")
    snapshot_path = artifact / snapshot_name
    manifest_path = artifact / manifest_name
    signature_path = artifact / signature_name
    for path in (snapshot_path, manifest_path, signature_path):
        _require(path.is_file() and not path.is_symlink(), f"snapshot artifact file is missing or not regular: {path}")

    manifest = _load_json(manifest_path)
    _require(manifest.get("manifest_version") == SNAPSHOT_MANIFEST_VERSION, "unsupported snapshot manifest version")
    _require(manifest.get("file_name") == snapshot_name, "snapshot manifest file_name mismatch")
    _require(manifest.get("file_sha256") == expected_db_sha256, "snapshot DB digest differs between manifest and completion marker")
    _require(manifest.get("signature_scheme") == SIGNATURE_SCHEME, "public snapshot must use an Ed25519 signature")
    signing_key_id = manifest.get("signing_key_id")
    _require(isinstance(signing_key_id, str) and bool(signing_key_id), "signed snapshot is missing signing_key_id")
    state_ref = manifest.get("state_ref")
    db_identity = manifest.get("db_identity")
    _require(isinstance(state_ref, dict), "snapshot manifest state_ref is required")
    _require(isinstance(db_identity, dict), "snapshot manifest db_identity is required")
    _require(state_ref.get("block_height") == height, "snapshot manifest height mismatch")
    _require(state_ref.get("stable_block_hash") == btc_block_hash, "snapshot manifest BTC block hash mismatch")
    _require(state_ref.get("snapshot_id") == snapshot_id, "snapshot manifest snapshot ID mismatch")
    _require(db_identity.get("btc_network") == network, "snapshot manifest network mismatch")
    _require(manifest.get("balance_query_floor") == height, "snapshot balance query floor mismatch")
    _require(manifest.get("history_query_floor") == min(height + 1, 0xFFFFFFFF), "snapshot history query floor mismatch")
    _require(manifest_path.with_suffix(".sig").name == signature_name, "snapshot signature basename does not match manifest")

    trusted_keys_path = trusted_keys.expanduser().resolve()
    _require(trusted_keys_path.is_file() and not trusted_keys_path.is_symlink(), f"trusted-key catalog does not exist: {trusted_keys_path}")
    trusted_keys_sha256 = _validate_trusted_catalog(trusted_keys_path, signing_key_id)

    finalization_path = finalization_marker.expanduser().resolve()
    _require(finalization_path.is_file() and not finalization_path.is_symlink(), f"artifact finalization marker is missing or not regular: {finalization_path}")
    finalization = _load_json(finalization_path)
    _require_exact_keys(
        finalization,
        {
            "btc_block_hash",
            "file_sha256",
            "finalized_at_utc",
            "finalizer_revision",
            "height",
            "manifest_file",
            "network",
            "producer_revision",
            "signature_file",
            "signing_key_id",
            "snapshot_file",
            "snapshot_id",
            "trusted_keys_sha256",
            "version",
        },
        "artifact finalization marker",
    )
    _require(
        finalization.get("version") == 1
        and finalization.get("height") == height
        and finalization.get("network") == network
        and finalization.get("btc_block_hash") == btc_block_hash
        and finalization.get("snapshot_id") == snapshot_id
        and finalization.get("snapshot_file") == snapshot_name
        and finalization.get("manifest_file") == manifest_name
        and finalization.get("signature_file") == signature_name
        and finalization.get("file_sha256") == expected_db_sha256
        and finalization.get("signing_key_id") == signing_key_id
        and finalization.get("trusted_keys_sha256") == trusted_keys_sha256
        and finalization.get("producer_revision") == producer_revision,
        "artifact finalization marker does not match snapshot artifact",
    )
    _require(isinstance(finalization.get("finalized_at_utc"), str) and bool(finalization["finalized_at_utc"]), "artifact finalization timestamp is required")

    preliminary_files = [
        ("snapshot_db", snapshot_path),
        ("snapshot_manifest", manifest_path),
        ("snapshot_signature", signature_path),
        ("completion_marker", complete_path),
    ]
    identity_files = []
    for role, path in preliminary_files:
        digest = expected_db_sha256 if role == "snapshot_db" else _sha256(path)
        identity_files.append(
            {"path": path.name, "role": role, "sha256": digest, "size": path.stat().st_size}
        )
    snapshot_entry = next(item for item in identity_files if item["role"] == "snapshot_db")
    _require(snapshot_entry["sha256"] == expected_db_sha256, "snapshot DB SHA-256 identity mismatch")
    artifact_set_id = _sha256_bytes(
        json.dumps(
            {
                "files": identity_files,
                "height": height,
                "network": network,
                "snapshot_id": snapshot_id,
            },
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
    )
    release_id = f"balance-history-{network}-h{height}-{artifact_set_id[:16]}"
    object_prefix = f"snapshots/v2/balance-history/{network}/{height:012d}/{artifact_set_id}"
    files = [
        {**item, "object_key": f"{object_prefix}/{item['path']}"}
        for item in identity_files
    ]

    record = {
        "artifact_completed_at": complete["completed_at"],
        "artifact_set_id": artifact_set_id,
        "artifact_type": ARTIFACT_TYPE,
        "btc_block_hash": btc_block_hash,
        "files": files,
        "height": height,
        "network": network,
        "object_prefix": object_prefix,
        "producer": {
            "artifact_finalization_marker_sha256": _sha256(finalization_path),
            "artifact_finalized_at_utc": finalization["finalized_at_utc"],
            "artifact_finalizer_revision": finalization["finalizer_revision"],
            "artifact_producer_revision": producer_revision,
        },
        "public_base_url": public_base,
        "schema_version": RECORD_SCHEMA_VERSION,
        "snapshot_id": snapshot_id,
        "snapshot_release_id": release_id,
        "trusted_keys": {
            "file_name": trusted_keys_path.name,
            "sha256": trusted_keys_sha256,
            "signing_key_id": signing_key_id,
        },
    }
    validate_release_record(record)
    content = _canonical_json(record)
    record_sha256 = _sha256_bytes(content)
    output_path = output_dir.expanduser().resolve() / f"snapshot-release-record-{record_sha256}.json"
    _write_new_or_identical(output_path, content)
    return output_path, record, record_sha256


def validate_release_record(record: dict[str, Any]) -> None:
    _require_exact_keys(
        record,
        {
            "artifact_completed_at",
            "artifact_set_id",
            "artifact_type",
            "btc_block_hash",
            "files",
            "height",
            "network",
            "object_prefix",
            "producer",
            "public_base_url",
            "schema_version",
            "snapshot_id",
            "snapshot_release_id",
            "trusted_keys",
        },
        "snapshot release record",
    )
    _require(record["schema_version"] == RECORD_SCHEMA_VERSION, "unsupported snapshot release record schema")
    _require(record["artifact_type"] == ARTIFACT_TYPE, "unsupported snapshot artifact type")
    network = record["network"]
    _require(isinstance(network, str) and re.fullmatch(r"[a-z0-9-]+", network) is not None, "invalid snapshot network")
    height = _require_u32(record["height"], "snapshot height")
    _require_sha256(record["btc_block_hash"], "BTC block hash")
    snapshot_id = _require_sha256(record["snapshot_id"], "snapshot ID")
    artifact_set_id = _require_sha256(record["artifact_set_id"], "artifact set ID")
    release_id = record["snapshot_release_id"]
    _require(isinstance(release_id, str) and RELEASE_ID_RE.fullmatch(release_id) is not None, "invalid snapshot release ID")
    _require(release_id == f"balance-history-{network}-h{height}-{artifact_set_id[:16]}", "snapshot release ID does not match artifact identity")
    expected_prefix = f"snapshots/v2/balance-history/{network}/{height:012d}/{artifact_set_id}"
    _require(record["object_prefix"] == expected_prefix, "snapshot object prefix does not match artifact identity")
    _normalize_https_base(record["public_base_url"], "public base URL")
    _require(isinstance(record["artifact_completed_at"], int) and not isinstance(record["artifact_completed_at"], bool) and record["artifact_completed_at"] >= 0, "artifact completion time must be a Unix timestamp")

    producer = record["producer"]
    _require(isinstance(producer, dict), "snapshot producer must be an object")
    _require_exact_keys(
        producer,
        {
            "artifact_finalization_marker_sha256",
            "artifact_finalized_at_utc",
            "artifact_finalizer_revision",
            "artifact_producer_revision",
        },
        "snapshot producer",
    )
    _require_sha256(producer["artifact_finalization_marker_sha256"], "artifact finalization marker SHA-256")
    _require(isinstance(producer["artifact_finalized_at_utc"], str) and bool(producer["artifact_finalized_at_utc"]), "artifact finalization timestamp is required")
    _require(isinstance(producer["artifact_producer_revision"], str) and REVISION_RE.fullmatch(producer["artifact_producer_revision"]) is not None, "invalid artifact producer revision")
    _require(isinstance(producer["artifact_finalizer_revision"], str) and REVISION_RE.fullmatch(producer["artifact_finalizer_revision"]) is not None, "invalid artifact finalizer revision")

    trusted = record["trusted_keys"]
    _require(isinstance(trusted, dict), "trusted_keys must be an object")
    _require_exact_keys(trusted, {"file_name", "sha256", "signing_key_id"}, "trusted_keys")
    _safe_basename(trusted["file_name"], "trusted-key catalog file name")
    _require_sha256(trusted["sha256"], "trusted-key catalog SHA-256")
    _require(isinstance(trusted["signing_key_id"], str) and bool(trusted["signing_key_id"]), "trusted signing key ID is required")

    files = record["files"]
    _require(isinstance(files, list) and len(files) == len(EXPECTED_FILE_ROLES), "snapshot release record must contain exactly four files")
    roles: set[str] = set()
    paths: set[str] = set()
    identity_files: list[dict[str, Any]] = []
    for index, item in enumerate(files):
        _require(isinstance(item, dict), f"snapshot file entry {index} must be an object")
        _require_exact_keys(item, {"object_key", "path", "role", "sha256", "size"}, f"snapshot file entry {index}")
        role = item["role"]
        _require(role == EXPECTED_FILE_ROLES[index] and role not in roles, "snapshot file roles are not in canonical order or are duplicated")
        path = _safe_basename(item["path"], "snapshot release file path")
        _require(path not in paths, "snapshot release file path is duplicated")
        _require(item["object_key"] == f"{expected_prefix}/{path}", "snapshot object key does not match record prefix")
        _safe_object_key(item["object_key"], "snapshot object key")
        sha256 = _require_sha256(item["sha256"], "snapshot file SHA-256")
        _require(isinstance(item["size"], int) and not isinstance(item["size"], bool) and item["size"] > 0, "snapshot file size must be positive")
        roles.add(role)
        paths.add(path)
        identity_files.append({"path": path, "role": role, "sha256": sha256, "size": item["size"]})
    _require(roles == set(EXPECTED_FILE_ROLES), "snapshot release record file roles are incomplete")
    expected_artifact_set_id = _sha256_bytes(
        json.dumps(
            {"files": identity_files, "height": height, "network": network, "snapshot_id": snapshot_id},
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
    )
    _require(artifact_set_id == expected_artifact_set_id, "artifact set ID does not match file inventory")


def _validate_local_files(record: dict[str, Any], source_dir: Path) -> None:
    for item in record["files"]:
        path = source_dir / item["path"]
        _require(path.is_file() and not path.is_symlink(), f"snapshot release source file is missing or not regular: {path}")
        _require(path.stat().st_size == item["size"], f"snapshot release source file size mismatch: {path}")
        _require(_sha256(path) == item["sha256"], f"snapshot release source file SHA-256 mismatch: {path}")


class AwsCliClient:
    def __init__(self, *, endpoint_url: str, bucket: str, region: str, profile: str | None, executable: str = "aws") -> None:
        self.endpoint_url = _normalize_https_base(endpoint_url, "S3 endpoint URL")
        _require(re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", bucket) is not None, "invalid S3 bucket name")
        _require(isinstance(region, str) and bool(region) and re.fullmatch(r"[A-Za-z0-9-]+", region) is not None, "invalid AWS region")
        self.bucket = bucket
        self.region = region
        self.profile = profile
        self.executable = executable

    def _base_command(self) -> list[str]:
        command = [self.executable]
        if self.profile:
            command.extend(["--profile", self.profile])
        command.extend(["--region", self.region, "--endpoint-url", self.endpoint_url])
        return command

    def head(self, object_key: str) -> dict[str, Any] | None:
        result = subprocess.run(
            [*self._base_command(), "s3api", "head-object", "--bucket", self.bucket, "--key", object_key, "--output", "json"],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            try:
                value = json.loads(result.stdout, object_pairs_hook=_strict_object)
            except json.JSONDecodeError as error:
                raise ValueError(f"AWS CLI returned invalid head-object JSON for {object_key}: {error}") from error
            _require(isinstance(value, dict), f"AWS CLI head-object result is not an object: {object_key}")
            return value
        if any(token in result.stderr for token in ("404", "Not Found", "NoSuchKey")):
            return None
        raise ValueError(f"AWS CLI head-object failed for {object_key}: {result.stderr.strip()}")

    def upload(self, source: Path, object_key: str, sha256: str, size: int, content_type: str) -> None:
        subprocess.run(
            [
                *self._base_command(),
                "s3",
                "cp",
                str(source),
                f"s3://{self.bucket}/{object_key}",
                "--only-show-errors",
                "--content-type",
                content_type,
                "--cache-control",
                "public,max-age=31536000,immutable",
                "--metadata",
                f"usdb-sha256={sha256},usdb-size={size}",
            ],
            check=True,
        )


def _head_matches(head: dict[str, Any], sha256: str, size: int) -> bool:
    metadata = head.get("Metadata")
    return (
        head.get("ContentLength") == size
        and isinstance(metadata, dict)
        and metadata.get("usdb-sha256") == sha256
        and metadata.get("usdb-size") == str(size)
    )


def _publish_object(client: Any, source: Path, object_key: str, sha256: str, size: int, content_type: str) -> str:
    existing = client.head(object_key)
    if existing is not None:
        _require(_head_matches(existing, sha256, size), f"refusing to replace existing S3 object with different identity: {object_key}")
        return "existing"
    client.upload(source, object_key, sha256, size, content_type)
    published = client.head(object_key)
    _require(published is not None and _head_matches(published, sha256, size), f"uploaded S3 object failed metadata/size verification: {object_key}")
    return "uploaded"


def upload_release(record_path: Path, source_dir: Path, client: Any) -> dict[str, Any]:
    record_file = record_path.expanduser().resolve()
    record = _load_json(record_file)
    validate_release_record(record)
    source = source_dir.expanduser().resolve()
    _require(source.is_dir(), f"snapshot release source directory does not exist: {source}")
    _validate_local_files(record, source)
    statuses: dict[str, str] = {}
    for item in record["files"]:
        suffix = Path(item["path"]).suffix.lower()
        content_type = "application/json" if suffix == ".json" else "application/octet-stream"
        statuses[item["object_key"]] = _publish_object(
            client,
            source / item["path"],
            item["object_key"],
            item["sha256"],
            item["size"],
            content_type,
        )
    record_content = record_file.read_bytes()
    record_sha256 = _sha256_bytes(record_content)
    _require(record_content == _canonical_json(record), "snapshot release record is not canonical JSON")
    record_key = f"snapshot-records/v2/{record_sha256}.json"
    statuses[record_key] = _publish_object(
        client,
        record_file,
        record_key,
        record_sha256,
        len(record_content),
        "application/json",
    )
    return {
        "objects": statuses,
        "record_object_key": record_key,
        "record_sha256": record_sha256,
        "record_url": f"{record['public_base_url']}/{record_key}",
        "snapshot_release_id": record["snapshot_release_id"],
    }


def _quoted_object_url(base_url: str, object_key: str) -> str:
    return f"{base_url}/{'/'.join(quote(part, safe='') for part in PurePosixPath(object_key).parts)}"


def _download_with_resume(url: str, destination_part: Path, curl_executable: str = "curl") -> None:
    destination_part.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            curl_executable,
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--retry",
            "8",
            "--retry-delay",
            "2",
            "--retry-all-errors",
            "--connect-timeout",
            "30",
            "--continue-at",
            "-",
            "--output",
            str(destination_part),
            url,
        ],
        check=True,
    )


def _verify_download(path: Path, expected_size: int, expected_sha256: str) -> None:
    _require(path.is_file() and not path.is_symlink(), f"downloaded snapshot file is missing or not regular: {path}")
    _require(path.stat().st_size == expected_size, f"downloaded snapshot file size mismatch: {path}")
    _require(_sha256(path) == expected_sha256, f"downloaded snapshot file SHA-256 mismatch: {path}")


def _fsync_path(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class InstalledSnapshot:
    release_id: str
    release_dir: Path
    record_path: Path
    snapshot_file: Path
    manifest_file: Path
    signature_file: Path
    height: int
    network: str


def _installed_snapshot(record: dict[str, Any], release_dir: Path) -> InstalledSnapshot:
    by_role = {item["role"]: release_dir / item["path"] for item in record["files"]}
    return InstalledSnapshot(
        release_id=record["snapshot_release_id"],
        release_dir=release_dir,
        record_path=release_dir / "snapshot-release-record.json",
        snapshot_file=by_role["snapshot_db"],
        manifest_file=by_role["snapshot_manifest"],
        signature_file=by_role["snapshot_signature"],
        height=record["height"],
        network=record["network"],
    )


def _verify_installed_release(record: dict[str, Any], release_dir: Path, record_content: bytes) -> InstalledSnapshot:
    _require(release_dir.is_dir() and not release_dir.is_symlink(), f"installed snapshot release is missing or invalid: {release_dir}")
    installed_record = release_dir / "snapshot-release-record.json"
    _require(installed_record.is_file() and installed_record.read_bytes() == record_content, f"installed snapshot release record mismatch: {release_dir}")
    for item in record["files"]:
        _verify_download(release_dir / item["path"], item["size"], item["sha256"])
    return _installed_snapshot(record, release_dir)


def install_release(
    *,
    record_url: str,
    destination_root: Path,
    trusted_keys: Path,
    curl_executable: str = "curl",
    expected_network: str | None = None,
    max_height: int | None = None,
) -> InstalledSnapshot:
    parsed_record_url = urlparse(record_url)
    _require(
        parsed_record_url.scheme == "https"
        and bool(parsed_record_url.netloc)
        and parsed_record_url.username is None
        and parsed_record_url.password is None
        and not parsed_record_url.query
        and not parsed_record_url.fragment,
        "snapshot release record URL must be HTTPS without credentials, query, or fragment",
    )
    record_name = PurePosixPath(parsed_record_url.path).name
    match = re.fullmatch(r"([0-9a-f]{64})\.json", record_name)
    _require(match is not None, "snapshot release record URL must end in its lowercase SHA-256 digest")
    expected_record_sha256 = match.group(1)

    root = destination_root.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)
    lock_path = root / ".snapshot-distribution.lock"
    with lock_path.open("a+b") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        downloads = root / ".downloads"
        record_cache = downloads / record_name
        if not record_cache.is_file() or _sha256(record_cache) != expected_record_sha256:
            record_part = record_cache.with_name(record_cache.name + ".part")
            if record_part.is_file() and _sha256(record_part) == expected_record_sha256:
                record_part.replace(record_cache)
            else:
                # Release records are small, so a bad partial is restarted
                # instead of carrying an unknown prefix into a Range request.
                record_part.unlink(missing_ok=True)
                _download_with_resume(record_url, record_part, curl_executable)
                if _sha256(record_part) != expected_record_sha256:
                    record_part.unlink(missing_ok=True)
                    raise ValueError("downloaded snapshot release record SHA-256 mismatch")
                record_part.replace(record_cache)
        record = _load_json(record_cache)
        validate_release_record(record)
        record_content = record_cache.read_bytes()
        _require(record_content == _canonical_json(record), "downloaded snapshot release record is not canonical JSON")
        if expected_network is not None:
            _require(record["network"] == expected_network, f"snapshot network mismatch: expected {expected_network}, got {record['network']}")
        if max_height is not None:
            _require(record["height"] <= max_height, f"snapshot height {record['height']} exceeds allowed maximum {max_height}")
        record_base = _normalize_https_base(record["public_base_url"], "snapshot public base URL")
        _require(urlparse(record_base).netloc == parsed_record_url.netloc, "snapshot record and artifact public origins do not match")

        trusted_path = trusted_keys.expanduser().resolve()
        _require(trusted_path.is_file() and not trusted_path.is_symlink(), f"trusted-key catalog does not exist: {trusted_path}")
        _require(trusted_path.name == record["trusted_keys"]["file_name"], "local trusted-key catalog file name does not match snapshot release record")
        _require(_sha256(trusted_path) == record["trusted_keys"]["sha256"], "local trusted-key catalog does not match snapshot release record")
        _validate_trusted_catalog(trusted_path, record["trusted_keys"]["signing_key_id"])

        release_id = record["snapshot_release_id"]
        destination = root / release_id
        if destination.exists():
            return _verify_installed_release(record, destination, record_content)
        staging = root / f".{release_id}.installing"
        staging.mkdir(mode=0o755, exist_ok=True)
        for item in record["files"]:
            final_path = staging / item["path"]
            if final_path.exists():
                try:
                    _verify_download(final_path, item["size"], item["sha256"])
                    continue
                except ValueError:
                    final_path.unlink(missing_ok=True)
            part_path = final_path.with_name(final_path.name + ".part")
            if part_path.is_file() and part_path.stat().st_size == item["size"] and _sha256(part_path) == item["sha256"]:
                part_path.replace(final_path)
                continue
            if part_path.is_file() and part_path.stat().st_size >= item["size"]:
                part_path.unlink()
            _download_with_resume(_quoted_object_url(record_base, item["object_key"]), part_path, curl_executable)
            try:
                _verify_download(part_path, item["size"], item["sha256"])
            except ValueError:
                part_path.unlink(missing_ok=True)
                raise
            part_path.replace(final_path)
        _write_new_or_identical(staging / "snapshot-release-record.json", record_content)
        for item in record["files"]:
            _fsync_path(staging / item["path"])
        _fsync_path(staging / "snapshot-release-record.json")
        _fsync_path(staging)
        os.replace(staging, destination)
        _fsync_path(root)
        return _verify_installed_release(record, destination, record_content)


def _print_json(value: dict[str, Any]) -> None:
    print(json.dumps(value, indent=2, sort_keys=True))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="Build a content-addressed release record from one finalized snapshot")
    prepare.add_argument("--artifact-dir", type=Path, required=True)
    prepare.add_argument("--trusted-keys", type=Path, required=True)
    prepare.add_argument("--finalization-marker", type=Path, required=True)
    prepare.add_argument("--producer-revision", required=True)
    prepare.add_argument("--public-base-url", default=DEFAULT_PUBLIC_BASE_URL)
    prepare.add_argument("--output-dir", type=Path, required=True)

    upload = subparsers.add_parser("upload", help="Idempotently publish release objects through the AWS CLI")
    upload.add_argument("--record", type=Path, required=True)
    upload.add_argument("--source-dir", type=Path, required=True)
    upload.add_argument("--bucket", default=DEFAULT_BUCKET)
    upload.add_argument("--endpoint-url", default=DEFAULT_ENDPOINT_URL)
    upload.add_argument("--aws-region", default=DEFAULT_AWS_REGION)
    upload.add_argument("--aws-profile")
    upload.add_argument("--aws-executable", default="aws", help=argparse.SUPPRESS)

    install = subparsers.add_parser("install", help="Resume, verify, and atomically install one public snapshot release")
    install.add_argument("--record-url", required=True)
    install.add_argument("--destination-root", type=Path, required=True)
    install.add_argument("--trusted-keys", type=Path, required=True)
    install.add_argument("--expected-network")
    install.add_argument("--max-height", type=int)
    install.add_argument("--curl-executable", default="curl", help=argparse.SUPPRESS)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.command == "prepare":
            path, record, digest = prepare_release_record(
                artifact_dir=args.artifact_dir,
                trusted_keys=args.trusted_keys,
                finalization_marker=args.finalization_marker,
                public_base_url=args.public_base_url,
                producer_revision=args.producer_revision,
                output_dir=args.output_dir,
            )
            _print_json(
                {
                    "record_object_key": f"snapshot-records/v2/{digest}.json",
                    "record_path": str(path),
                    "record_sha256": digest,
                    "record_url": f"{record['public_base_url']}/snapshot-records/v2/{digest}.json",
                    "snapshot_release_id": record["snapshot_release_id"],
                }
            )
        elif args.command == "upload":
            client = AwsCliClient(
                endpoint_url=args.endpoint_url,
                bucket=args.bucket,
                region=args.aws_region,
                profile=args.aws_profile,
                executable=args.aws_executable,
            )
            _print_json(upload_release(args.record, args.source_dir, client))
        else:
            installed = install_release(
                record_url=args.record_url,
                destination_root=args.destination_root,
                trusted_keys=args.trusted_keys,
                curl_executable=args.curl_executable,
                expected_network=args.expected_network,
                max_height=args.max_height,
            )
            _print_json(
                {
                    "height": installed.height,
                    "manifest_file": str(installed.manifest_file),
                    "network": installed.network,
                    "record_path": str(installed.record_path),
                    "release_dir": str(installed.release_dir),
                    "snapshot_file": str(installed.snapshot_file),
                    "snapshot_release_id": installed.release_id,
                }
            )
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Snapshot distribution failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
