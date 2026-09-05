#!/usr/bin/env python3
"""Prepare, publish, and install immutable balance-history snapshot releases."""

from __future__ import annotations

import argparse
import concurrent.futures
import fcntl
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import quote, urlparse
from urllib.request import Request, urlopen


RECORD_SCHEMA_VERSION = "usdb-snapshot-release-record:v3"
ARTIFACT_TYPE = "balance-history-split"
CORE_MANIFEST_VERSION = "balance-history-core-snapshot-manifest:v1"
REGISTRY_MANIFEST_VERSION = "balance-history-script-registry-manifest:v1"
# Keep in sync with the Rust snapshot builder's state::COMPLETE_MARKER_VERSION.
COMPLETE_MARKER_VERSION = 2
SIGNATURE_SCHEME = "ed25519"
DEFAULT_BUCKET = "usdb-snapshot"
DEFAULT_ENDPOINT_URL = "https://87e0bdf811b13ee87fd0bcec7a4fd1e7.r2.cloudflarestorage.com"
DEFAULT_PUBLIC_BASE_URL = "https://usdb-snapshot.tbudr.top"
DEFAULT_AWS_REGION = "auto"
DEFAULT_S3_UPLOAD_CONCURRENCY = 16
DEFAULT_S3_CHUNK_SIZE_MIB = 64
DEFAULT_DOWNLOAD_CONCURRENCY = 8
DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB = 64
PARALLEL_DOWNLOAD_MIN_SIZE = 128 * 1024 * 1024
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
RELEASE_ID_RE = re.compile(r"^balance-history-[a-z0-9-]+-h[0-9]+-[0-9a-f]{16}$")
EXPECTED_COMPONENTS = ("core", "script_registry")
EXPECTED_FILE_ROLES = (
    "database",
    "manifest",
    "signature",
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


def _progress_enabled() -> bool:
    return sys.stderr.isatty() or os.environ.get("USDB_SNAPSHOT_FORCE_PROGRESS") == "1"


def _human_bytes(value: float) -> str:
    units = ("B", "KiB", "MiB", "GiB", "TiB")
    size = max(value, 0.0)
    for unit in units[:-1]:
        if size < 1024.0:
            return f"{size:.1f} {unit}"
        size /= 1024.0
    return f"{size:.1f} {units[-1]}"


class _FileReadProgress:
    def __init__(self, label: str, total_bytes: int) -> None:
        self.label = label
        self.total_bytes = total_bytes
        self.started = time.monotonic()
        self.last_rendered = 0.0
        self.enabled = _progress_enabled()

    def update(self, processed_bytes: int, *, force: bool = False, status: str = "") -> None:
        if not self.enabled:
            return
        now = time.monotonic()
        if not force and now - self.last_rendered < 0.25:
            return
        elapsed = max(now - self.started, 0.001)
        fraction = 1.0 if self.total_bytes == 0 else min(processed_bytes / self.total_bytes, 1.0)
        width = 32
        completed = min(int(fraction * width), width)
        bar = "#" * completed + "-" * (width - completed)
        rate = processed_bytes / elapsed
        remaining = max(self.total_bytes - processed_bytes, 0)
        eta = remaining / rate if rate > 0 else 0.0
        suffix = f" {status}" if status else ""
        sys.stderr.write(
            f"\r{self.label} [{bar}] {fraction * 100:6.2f}% "
            f"{_human_bytes(processed_bytes)}/{_human_bytes(self.total_bytes)} "
            f"{_human_bytes(rate)}/s ETA {eta:,.0f}s{suffix}"
        )
        sys.stderr.flush()
        self.last_rendered = now

    def finish(self, processed_bytes: int, *, success: bool) -> None:
        if not self.enabled:
            return
        self.update(
            processed_bytes,
            force=True,
            status="complete" if success else "failed",
        )
        sys.stderr.write("\n")
        sys.stderr.flush()


def _sha256(path: Path, *, progress_label: str | None = None) -> str:
    digest = hashlib.sha256()
    total_bytes = path.stat().st_size
    progress = _FileReadProgress(progress_label, total_bytes) if progress_label else None
    processed_bytes = 0
    if progress is not None:
        progress.update(0, force=True)
    with path.open("rb") as source:
        try:
            for chunk in iter(lambda: source.read(8 * 1024 * 1024), b""):
                digest.update(chunk)
                processed_bytes += len(chunk)
                if progress is not None:
                    progress.update(processed_bytes)
        except BaseException:
            if progress is not None:
                progress.finish(processed_bytes, success=False)
            raise
    if progress is not None:
        progress.finish(processed_bytes, success=True)
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


def _path_exists(path: Path) -> bool:
    """Returns whether a directory entry exists without following symlinks."""
    return os.path.lexists(path)


def _ensure_managed_directory(path: Path, label: str) -> None:
    """Creates a tool-managed directory or validates its existing entry type."""
    if _path_exists(path):
        _require(
            path.is_dir() and not path.is_symlink(),
            f"{label} must be a real directory, not a symlink: {path}",
        )
        return
    path.mkdir(mode=0o755)


def _require_managed_file(path: Path, label: str) -> None:
    """Rejects a present tool-managed entry unless it is a regular file."""
    if _path_exists(path):
        _require(
            path.is_file() and not path.is_symlink(),
            f"{label} must be a regular file, not a symlink: {path}",
        )


def _write_new_or_identical(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if _path_exists(path):
        _require_managed_file(path, "existing immutable file")
        _require(path.read_bytes() == content, f"refusing to replace different existing file: {path}")
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


def _validate_completion_marker(
    marker: dict[str, Any],
    *,
    component_name: str,
    height: int,
    network: str,
    btc_block_hash: str,
    core_snapshot_id: str,
    artifact_id: str,
    database_name: str,
    manifest_name: str,
    signature_name: str,
    database_sha256: str,
) -> int:
    common = {
        "btc_block_hash",
        "completed_at",
        "file_sha256",
        "height",
        "manifest_file",
        "network",
        "signature_file",
        "version",
    }
    if component_name == "core":
        expected = common | {
            "balance_history_count",
            "block_commit_count",
            "core_artifact_id",
            "snapshot_file",
            "snapshot_id",
            "utxo_count",
        }
        artifact_field = "core_artifact_id"
        database_field = "snapshot_file"
        snapshot_field = "snapshot_id"
        count_fields = ("balance_history_count", "utxo_count", "block_commit_count")
    else:
        expected = common | {
            "core_snapshot_id",
            "entry_count",
            "registry_artifact_id",
            "registry_file",
        }
        artifact_field = "registry_artifact_id"
        database_field = "registry_file"
        snapshot_field = "core_snapshot_id"
        count_fields = ("entry_count",)
    _require_exact_keys(marker, expected, f"{component_name} completion marker")
    _require(
        type(marker["version"]) is int and marker["version"] == COMPLETE_MARKER_VERSION,
        f"unsupported {component_name} completion marker version: expected {COMPLETE_MARKER_VERSION}",
    )
    _require(
        (
            marker["height"],
            marker["network"],
            marker["btc_block_hash"],
            marker[snapshot_field],
            marker[artifact_field],
        )
        == (height, network, btc_block_hash, core_snapshot_id, artifact_id),
        f"{component_name} completion marker identity mismatch",
    )
    _require(
        (
            marker[database_field],
            marker["manifest_file"],
            marker["signature_file"],
            marker["file_sha256"],
        )
        == (database_name, manifest_name, signature_name, database_sha256),
        f"{component_name} completion marker file identity mismatch",
    )
    for field in count_fields:
        _require(
            isinstance(marker[field], int)
            and not isinstance(marker[field], bool)
            and marker[field] >= 0,
            f"{component_name} completion marker {field} must be a non-negative integer",
        )
    completed_at = marker["completed_at"]
    _require(
        isinstance(completed_at, int)
        and not isinstance(completed_at, bool)
        and completed_at >= 0,
        f"{component_name} completion time must be a Unix timestamp",
    )
    return completed_at


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

    finalization_path = finalization_marker.expanduser().resolve()
    _require(finalization_path.is_file() and not finalization_path.is_symlink(), f"artifact finalization marker is missing or not regular: {finalization_path}")
    finalization = _load_json(finalization_path)
    _require_exact_keys(
        finalization,
        {
            "artifacts",
            "finalized_at_utc",
            "finalizer_revision",
            "producer_revision",
            "schema_version",
            "trusted_keys_sha256",
        },
        "split artifact finalization marker",
    )
    _require(finalization["schema_version"] == "usdb-snapshot-artifact-finalization:v2", "unsupported split artifact finalization marker")
    _require(finalization["producer_revision"] == producer_revision, "finalization marker producer revision mismatch")
    _require(isinstance(finalization["finalized_at_utc"], str) and bool(finalization["finalized_at_utc"]), "artifact finalization timestamp is required")
    _require(isinstance(finalization["finalizer_revision"], str) and REVISION_RE.fullmatch(finalization["finalizer_revision"]) is not None, "invalid finalizer revision")

    trusted_keys_path = trusted_keys.expanduser().resolve()
    _require(trusted_keys_path.is_file() and not trusted_keys_path.is_symlink(), f"trusted-key catalog does not exist: {trusted_keys_path}")
    trusted_keys_sha256 = _sha256(trusted_keys_path)
    _require(finalization["trusted_keys_sha256"] == trusted_keys_sha256, "finalization marker trusted-key catalog mismatch")

    artifacts = finalization["artifacts"]
    _require(isinstance(artifacts, dict), "finalization marker artifacts must be an object")
    _require_exact_keys(artifacts, set(EXPECTED_COMPONENTS), "finalization marker artifacts")
    _require(isinstance(artifacts["core"], dict), "finalization marker must contain core artifact")
    _require(artifacts["script_registry"] is None or isinstance(artifacts["script_registry"], dict), "finalization marker script_registry must be an object or null")

    components: dict[str, Any] = {}
    height: int | None = None
    network: str | None = None
    btc_block_hash: str | None = None
    core_snapshot_id: str | None = None
    for component_name in EXPECTED_COMPONENTS:
        finalized = artifacts[component_name]
        if finalized is None:
            continue
        expected_component = "core" if component_name == "core" else "script_registry"
        _require_exact_keys(
            finalized,
            {
                "artifact_dir",
                "artifact_id",
                "btc_block_hash",
                "component",
                "core_snapshot_id",
                "file",
                "file_sha256",
                "height",
                "manifest_file",
                "network",
                "signature_file",
                "signing_key_id",
                "trusted_keys_sha256",
            },
            f"{component_name} finalization report",
        )
        _require(finalized["component"] == expected_component, f"{component_name} finalization component mismatch")
        component_height = _require_u32(finalized["height"], f"{component_name} height")
        component_network = finalized["network"]
        _require(isinstance(component_network, str) and re.fullmatch(r"[a-z0-9-]+", component_network) is not None, f"invalid {component_name} network")
        component_block_hash = _require_sha256(finalized["btc_block_hash"], f"{component_name} BTC block hash")
        component_snapshot_id = _require_sha256(finalized["core_snapshot_id"], f"{component_name} core snapshot ID")
        artifact_id = _require_sha256(finalized["artifact_id"], f"{component_name} artifact ID")
        db_sha256 = _require_sha256(finalized["file_sha256"], f"{component_name} DB SHA-256")
        signing_key_id = finalized["signing_key_id"]
        _require(isinstance(signing_key_id, str) and bool(signing_key_id), f"{component_name} signing key ID is required")
        _require(finalized["trusted_keys_sha256"] == trusted_keys_sha256, f"{component_name} trusted-key digest mismatch")
        _validate_trusted_catalog(trusted_keys_path, signing_key_id)
        if height is None:
            height = component_height
            network = component_network
            btc_block_hash = component_block_hash
            core_snapshot_id = component_snapshot_id
        _require((component_height, component_network, component_block_hash, component_snapshot_id) == (height, network, btc_block_hash, core_snapshot_id), f"{component_name} identity differs from core")

        component_dir_name = "core" if component_name == "core" else "script-registry"
        reported_dir_value = finalized["artifact_dir"]
        _require(
            isinstance(reported_dir_value, str) and bool(reported_dir_value),
            f"invalid {component_name} finalization artifact directory",
        )
        reported_dir = PurePosixPath(reported_dir_value)
        _require(
            not reported_dir.is_absolute()
            and ".." not in reported_dir.parts
            and reported_dir.name == component_dir_name,
            f"invalid {component_name} finalization artifact directory",
        )
        component_dir = artifact / component_dir_name
        _require(component_dir.is_dir() and not component_dir.is_symlink(), f"{component_name} artifact directory is missing: {component_dir}")
        complete_path = component_dir / "complete.json"
        db_name = _safe_basename(finalized["file"], f"{component_name} database")
        manifest_name = _safe_basename(finalized["manifest_file"], f"{component_name} manifest")
        signature_name = _safe_basename(finalized["signature_file"], f"{component_name} signature")
        paths = [component_dir / db_name, component_dir / manifest_name, component_dir / signature_name, complete_path]
        for path in paths:
            _require(path.is_file() and not path.is_symlink(), f"{component_name} artifact file is missing or not regular: {path}")
        manifest = _load_json(component_dir / manifest_name)
        expected_manifest_version = CORE_MANIFEST_VERSION if component_name == "core" else REGISTRY_MANIFEST_VERSION
        expected_artifact_type = "balance_history_core" if component_name == "core" else "balance_history_script_registry"
        id_field = "core_artifact_id" if component_name == "core" else "registry_artifact_id"
        _require(manifest.get("manifest_version") == expected_manifest_version, f"unsupported {component_name} manifest version")
        _require(manifest.get("artifact_type") == expected_artifact_type, f"{component_name} artifact_type mismatch")
        _require(manifest.get(id_field) == artifact_id, f"{component_name} artifact ID differs from manifest")
        _require(manifest.get("file_name") == db_name and manifest.get("file_sha256") == db_sha256, f"{component_name} file identity differs from manifest")
        _require(manifest.get("signature_scheme") == SIGNATURE_SCHEME and manifest.get("signing_key_id") == signing_key_id, f"{component_name} signing identity differs from manifest")
        if component_name == "core":
            _require(manifest.get("core_snapshot_id") == core_snapshot_id, "core snapshot ID differs from manifest")
        else:
            base = manifest.get("base")
            _require(isinstance(base, dict), "registry manifest base is required")
            _require((base.get("base_height"), base.get("btc_network"), base.get("base_block_hash"), base.get("core_snapshot_id")) == (height, network, btc_block_hash, core_snapshot_id), "registry manifest base differs from core")
        _require((component_dir / manifest_name).with_suffix(".sig").name == signature_name, f"{component_name} signature basename mismatch")

        completed_at = _validate_completion_marker(
            _load_json(complete_path),
            component_name=component_name,
            height=height,
            network=network,
            btc_block_hash=btc_block_hash,
            core_snapshot_id=core_snapshot_id,
            artifact_id=artifact_id,
            database_name=db_name,
            manifest_name=manifest_name,
            signature_name=signature_name,
            database_sha256=db_sha256,
        )
        preliminary = list(zip(EXPECTED_FILE_ROLES, paths, strict=True))
        files: list[dict[str, Any]] = []
        object_prefix = f"snapshots/v3/balance-history/{network}/{height:012d}/{component_dir_name}/{artifact_id}"
        for role, path in preliminary:
            digest = db_sha256 if role == "database" else _sha256(path)
            relative_path = f"{component_dir_name}/{path.name}"
            files.append({
                "object_key": f"{object_prefix}/{path.name}",
                "path": relative_path,
                "role": role,
                "sha256": digest,
                "size": path.stat().st_size,
            })
        components[component_name] = {
            "artifact_id": artifact_id,
            "completed_at": completed_at,
            "files": files,
            "object_prefix": object_prefix,
            "signing_key_id": signing_key_id,
        }

    assert height is not None and network is not None and btc_block_hash is not None and core_snapshot_id is not None
    components.setdefault("script_registry", None)
    artifact_set_id = _sha256_bytes(
        json.dumps(
            {
                "btc_block_hash": btc_block_hash,
                "components": components,
                "core_snapshot_id": core_snapshot_id,
                "height": height,
                "network": network,
            },
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
    )
    release_id = f"balance-history-{network}-h{height}-{artifact_set_id[:16]}"
    record = {
        "artifact_set_id": artifact_set_id,
        "artifact_type": ARTIFACT_TYPE,
        "btc_block_hash": btc_block_hash,
        "components": components,
        "core_snapshot_id": core_snapshot_id,
        "height": height,
        "network": network,
        "producer": {
            "artifact_finalization_marker_sha256": _sha256(finalization_path),
            "artifact_finalized_at_utc": finalization["finalized_at_utc"],
            "artifact_finalizer_revision": finalization["finalizer_revision"],
            "artifact_producer_revision": producer_revision,
        },
        "public_base_url": public_base,
        "schema_version": RECORD_SCHEMA_VERSION,
        "snapshot_release_id": release_id,
        "trusted_keys": {
            "file_name": trusted_keys_path.name,
            "sha256": trusted_keys_sha256,
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
            "artifact_set_id",
            "artifact_type",
            "btc_block_hash",
            "components",
            "core_snapshot_id",
            "height",
            "network",
            "producer",
            "public_base_url",
            "schema_version",
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
    btc_block_hash = _require_sha256(record["btc_block_hash"], "BTC block hash")
    core_snapshot_id = _require_sha256(record["core_snapshot_id"], "core snapshot ID")
    artifact_set_id = _require_sha256(record["artifact_set_id"], "artifact set ID")
    release_id = record["snapshot_release_id"]
    _require(isinstance(release_id, str) and RELEASE_ID_RE.fullmatch(release_id) is not None, "invalid snapshot release ID")
    _require(release_id == f"balance-history-{network}-h{height}-{artifact_set_id[:16]}", "snapshot release ID does not match artifact identity")
    _normalize_https_base(record["public_base_url"], "public base URL")

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
    _require_exact_keys(trusted, {"file_name", "sha256"}, "trusted_keys")
    _safe_basename(trusted["file_name"], "trusted-key catalog file name")
    _require_sha256(trusted["sha256"], "trusted-key catalog SHA-256")

    components = record["components"]
    _require(isinstance(components, dict), "snapshot components must be an object")
    _require_exact_keys(components, set(EXPECTED_COMPONENTS), "snapshot components")
    _require(isinstance(components["core"], dict), "snapshot core component is required")
    _require(components["script_registry"] is None or isinstance(components["script_registry"], dict), "snapshot script_registry component must be an object or null")
    all_paths: set[str] = set()
    for component_name in EXPECTED_COMPONENTS:
        component = components[component_name]
        if component is None:
            continue
        _require_exact_keys(component, {"artifact_id", "completed_at", "files", "object_prefix", "signing_key_id"}, f"snapshot component {component_name}")
        artifact_id = _require_sha256(component["artifact_id"], f"{component_name} artifact ID")
        _require(isinstance(component["completed_at"], int) and not isinstance(component["completed_at"], bool) and component["completed_at"] >= 0, f"{component_name} completion time must be a Unix timestamp")
        _require(isinstance(component["signing_key_id"], str) and bool(component["signing_key_id"]), f"{component_name} signing key ID is required")
        component_dir = "core" if component_name == "core" else "script-registry"
        expected_prefix = f"snapshots/v3/balance-history/{network}/{height:012d}/{component_dir}/{artifact_id}"
        _require(component["object_prefix"] == expected_prefix, f"{component_name} object prefix mismatch")
        files = component["files"]
        _require(isinstance(files, list) and len(files) == len(EXPECTED_FILE_ROLES), f"{component_name} must contain exactly four files")
        roles: set[str] = set()
        for index, item in enumerate(files):
            _require(isinstance(item, dict), f"{component_name} file entry {index} must be an object")
            _require_exact_keys(item, {"object_key", "path", "role", "sha256", "size"}, f"{component_name} file entry {index}")
            role = item["role"]
            _require(role == EXPECTED_FILE_ROLES[index] and role not in roles, f"{component_name} file roles are not canonical")
            path = _safe_object_key(item["path"], f"{component_name} file path")
            path_parts = PurePosixPath(path).parts
            _require(len(path_parts) == 2 and path_parts[0] == component_dir, f"{component_name} file path must stay inside {component_dir}")
            _safe_basename(path_parts[1], f"{component_name} file basename")
            _require(path not in all_paths, "snapshot release file path is duplicated")
            _require(item["object_key"] == f"{expected_prefix}/{path_parts[1]}", f"{component_name} object key mismatch")
            _safe_object_key(item["object_key"], f"{component_name} object key")
            _require_sha256(item["sha256"], f"{component_name} file SHA-256")
            _require(isinstance(item["size"], int) and not isinstance(item["size"], bool) and item["size"] > 0, f"{component_name} file size must be positive")
            roles.add(role)
            all_paths.add(path)
        _require(roles == set(EXPECTED_FILE_ROLES), f"{component_name} file roles are incomplete")
    expected_artifact_set_id = _sha256_bytes(
        json.dumps(
            {
                "btc_block_hash": btc_block_hash,
                "components": components,
                "core_snapshot_id": core_snapshot_id,
                "height": height,
                "network": network,
            },
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
    )
    _require(artifact_set_id == expected_artifact_set_id, "artifact set ID does not match file inventory")


def _component(record: dict[str, Any], component_name: str) -> dict[str, Any]:
    _require(component_name in EXPECTED_COMPONENTS, f"unsupported snapshot component {component_name}")
    component = record["components"][component_name]
    _require(component is not None, f"snapshot release has no {component_name} component")
    return component


def _all_files(record: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        item
        for component_name in EXPECTED_COMPONENTS
        if record["components"][component_name] is not None
        for item in record["components"][component_name]["files"]
    ]


def _validate_local_files(record: dict[str, Any], source_dir: Path) -> None:
    for item in _all_files(record):
        path = source_dir / item["path"]
        _require(path.is_file() and not path.is_symlink(), f"snapshot release source file is missing or not regular: {path}")
        _require(path.stat().st_size == item["size"], f"snapshot release source file size mismatch: {path}")
        _require(
            _sha256(path, progress_label=f"Local verify {path.name}") == item["sha256"],
            f"snapshot release source file SHA-256 mismatch: {path}",
        )


class AwsCliClient:
    def __init__(
        self,
        *,
        endpoint_url: str,
        bucket: str,
        region: str,
        profile: str | None,
        upload_concurrency: int = DEFAULT_S3_UPLOAD_CONCURRENCY,
        multipart_chunk_size_mib: int = DEFAULT_S3_CHUNK_SIZE_MIB,
        executable: str = "aws",
    ) -> None:
        self.endpoint_url = _normalize_https_base(endpoint_url, "S3 endpoint URL")
        _require(re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", bucket) is not None, "invalid S3 bucket name")
        _require(isinstance(region, str) and bool(region) and re.fullmatch(r"[A-Za-z0-9-]+", region) is not None, "invalid AWS region")
        _require(1 <= upload_concurrency <= 64, "S3 upload concurrency must be between 1 and 64")
        _require(5 <= multipart_chunk_size_mib <= 1024, "S3 multipart chunk size must be between 5 and 1024 MiB")
        self.bucket = bucket
        self.region = region
        self.profile = profile
        self.upload_concurrency = upload_concurrency
        self.multipart_chunk_size_mib = multipart_chunk_size_mib
        self.executable = executable
        self._temporary_config: tempfile.TemporaryDirectory[str] | None = None
        self._upload_environment: dict[str, str] | None = None

    def _base_command(self) -> list[str]:
        command = [self.executable]
        if self.profile:
            command.extend(["--profile", self.profile])
        command.extend(["--region", self.region, "--endpoint-url", self.endpoint_url])
        return command

    def _configure_upload_environment(self) -> dict[str, str]:
        if self._upload_environment is not None:
            return self._upload_environment
        self._temporary_config = tempfile.TemporaryDirectory(prefix="usdb-aws-config-")
        temporary_config = Path(self._temporary_config.name) / "config"
        configured_path = Path(
            os.environ.get("AWS_CONFIG_FILE", str(Path.home() / ".aws/config"))
        ).expanduser()
        if configured_path.is_file():
            shutil.copyfile(configured_path, temporary_config)
        else:
            temporary_config.write_text("", encoding="utf-8")
        temporary_config.chmod(0o600)
        environment = os.environ.copy()
        environment["AWS_CONFIG_FILE"] = str(temporary_config)
        command = [self.executable]
        if self.profile:
            command.extend(["--profile", self.profile])
        settings = {
            "s3.max_concurrent_requests": str(self.upload_concurrency),
            "s3.multipart_threshold": f"{self.multipart_chunk_size_mib}MB",
            "s3.multipart_chunksize": f"{self.multipart_chunk_size_mib}MB",
            "s3.preferred_transfer_client": "classic",
        }
        for key, value in settings.items():
            result = subprocess.run(
                [*command, "configure", "set", key, value],
                check=False,
                capture_output=True,
                text=True,
                env=environment,
            )
            if result.returncode != 0:
                raise ValueError(
                    f"AWS CLI failed to configure temporary upload setting {key}: "
                    f"{result.stderr.strip()}"
                )
        self._upload_environment = environment
        return environment

    def close(self) -> None:
        if self._temporary_config is not None:
            self._temporary_config.cleanup()
        self._temporary_config = None
        self._upload_environment = None

    def __enter__(self) -> "AwsCliClient":
        return self

    def __exit__(self, _exc_type: Any, _exc: Any, _traceback: Any) -> None:
        self.close()

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
        show_progress = _progress_enabled()
        command = [
            *self._base_command(),
            "s3",
            "cp",
            str(source),
            f"s3://{self.bucket}/{object_key}",
            "--content-type",
            content_type,
            "--cache-control",
            "public,max-age=31536000,immutable",
            "--metadata",
            f"usdb-sha256={sha256},usdb-size={size}",
        ]
        if not show_progress:
            command.append("--only-show-errors")
        if show_progress:
            print(
                f"Upload {source.name}: size={_human_bytes(size)} object={object_key} "
                f"concurrency={self.upload_concurrency} chunk={self.multipart_chunk_size_mib}MiB",
                file=sys.stderr,
                flush=True,
            )
        subprocess.run(
            command,
            check=True,
            stdout=sys.stderr if show_progress else subprocess.DEVNULL,
            env=self._configure_upload_environment(),
        )
        if show_progress:
            print(f"Upload {source.name}: complete", file=sys.stderr, flush=True)


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
        if _progress_enabled():
            print(f"Upload {source.name}: already exists, skipping", file=sys.stderr, flush=True)
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
    for item in _all_files(record):
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
    record_key = f"snapshot-records/v3/{record_sha256}.json"
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


def _read_public_url(url: str, *, method: str) -> tuple[bytes, int, str]:
    last_error: OSError | None = None
    for attempt in range(4):
        try:
            request = Request(url, method=method, headers={"User-Agent": "usdb-snapshot-verifier/1"})
            with urlopen(request, timeout=30) as response:  # noqa: S310 - URL is validated below.
                final_url = response.geturl()
                _require(urlparse(final_url).scheme == "https", "public snapshot redirect must remain HTTPS")
                length_value = response.headers.get("Content-Length")
                _require(length_value is not None and length_value.isdigit(), f"public object has no valid Content-Length: {url}")
                return (response.read() if method == "GET" else b"", int(length_value), final_url)
        except OSError as error:
            last_error = error
            if attempt < 3:
                time.sleep(2**attempt)
    assert last_error is not None
    raise ValueError(f"public snapshot request failed for {url}: {last_error}") from last_error


def _probe_public_byte_range(url: str, expected_size: int) -> None:
    last_error: OSError | None = None
    for attempt in range(4):
        try:
            request = Request(
                url,
                method="GET",
                headers={
                    "Range": "bytes=0-0",
                    "User-Agent": "usdb-snapshot-verifier/1",
                },
            )
            with urlopen(request, timeout=30) as response:  # noqa: S310 - URL is validated by the signed record.
                _require(
                    urlparse(response.geturl()).scheme == "https",
                    "public snapshot redirect must remain HTTPS",
                )
                _require(response.status == 206, f"public snapshot object does not support byte ranges: {url}")
                _require(
                    response.headers.get("Content-Range") == f"bytes 0-0/{expected_size}",
                    f"public snapshot object returned an invalid Content-Range: {url}",
                )
                _require(len(response.read(2)) == 1, f"public snapshot byte-range response is invalid: {url}")
                return
        except OSError as error:
            last_error = error
            if attempt < 3:
                time.sleep(2**attempt)
    assert last_error is not None
    raise ValueError(f"public snapshot byte-range request failed for {url}: {last_error}") from last_error


def verify_public_release(record_path: Path, trusted_keys: Path) -> dict[str, Any]:
    record_file = record_path.expanduser().resolve()
    record = _load_json(record_file)
    validate_release_record(record)
    record_content = record_file.read_bytes()
    _require(record_content == _canonical_json(record), "snapshot release record is not canonical JSON")

    trusted_path = trusted_keys.expanduser().resolve()
    _require(trusted_path.is_file() and not trusted_path.is_symlink(), f"trusted-key catalog does not exist: {trusted_path}")
    _require(trusted_path.name == record["trusted_keys"]["file_name"], "trusted-key catalog file name does not match snapshot release record")
    _require(_sha256(trusted_path) == record["trusted_keys"]["sha256"], "trusted-key catalog does not match snapshot release record")
    for component_name in EXPECTED_COMPONENTS:
        component = record["components"][component_name]
        if component is not None:
            _validate_trusted_catalog(trusted_path, component["signing_key_id"])

    record_sha256 = _sha256_bytes(record_content)
    record_url = f"{record['public_base_url']}/snapshot-records/v3/{record_sha256}.json"
    downloaded_record, record_size, _ = _read_public_url(record_url, method="GET")
    _require(record_size == len(record_content), "public snapshot release record size mismatch")
    _require(downloaded_record == record_content, "public snapshot release record content mismatch")

    verified_size = 0
    database_ranges = 0
    files = _all_files(record)
    for item in files:
        object_url = _quoted_object_url(record["public_base_url"], item["object_key"])
        _, size, _ = _read_public_url(object_url, method="HEAD")
        _require(size == item["size"], f"public snapshot object size mismatch: {item['object_key']}")
        if item["role"] == "database":
            _probe_public_byte_range(object_url, item["size"])
            database_ranges += 1
        verified_size += size
    return {
        "record_url": record_url,
        "record_sha256": record_sha256,
        "snapshot_release_id": record["snapshot_release_id"],
        "verified_file_count": len(files),
        "verified_size_bytes": verified_size,
        "database_byte_ranges_verified": database_ranges,
    }


def _quoted_object_url(base_url: str, object_key: str) -> str:
    return f"{base_url}/{'/'.join(quote(part, safe='') for part in PurePosixPath(object_key).parts)}"


def _download_with_resume(url: str, destination_part: Path, curl_executable: str = "curl") -> None:
    _ensure_managed_directory(destination_part.parent, "download directory")
    _require_managed_file(destination_part, "resumable download target")
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


def _range_download_paths(destination_part: Path) -> tuple[Path, Path]:
    return (
        destination_part.with_name(destination_part.name + ".ranges.json"),
        destination_part.with_name(destination_part.name + ".ranges"),
    )


def _write_range_state(path: Path, state: dict[str, Any]) -> None:
    _ensure_managed_directory(path.parent, "parallel download directory")
    _require_managed_file(path, "parallel download state")
    temporary = path.with_name(f".{path.name}.partial-{os.getpid()}")
    content = _canonical_json(state)
    try:
        with temporary.open("xb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


def _cleanup_range_download(destination_part: Path) -> None:
    state_path, work_dir = _range_download_paths(destination_part)
    _require_managed_file(state_path, "parallel download state")
    state_path.unlink(missing_ok=True)
    if _path_exists(work_dir):
        _require(work_dir.is_dir() and not work_dir.is_symlink(), f"invalid range download work directory: {work_dir}")
        shutil.rmtree(work_dir)


def _chunk_length(index: int, chunk_size: int, expected_size: int) -> int:
    return min(chunk_size, expected_size - index * chunk_size)


def _pwrite_all(descriptor: int, data: bytes, offset: int) -> None:
    view = memoryview(data)
    while view:
        written = os.pwrite(descriptor, view, offset)
        _require(written > 0, "parallel snapshot range write made no progress")
        view = view[written:]
        offset += written


def _download_range_chunk(
    *,
    url: str,
    index: int,
    chunk_size: int,
    expected_size: int,
    work_dir: Path,
    destination_descriptor: int,
    curl_executable: str,
) -> tuple[int, int]:
    start = index * chunk_size
    length = _chunk_length(index, chunk_size, expected_size)
    end = start + length - 1
    chunk_path = work_dir / f"{index:08d}.part"
    _require_managed_file(chunk_path, "parallel download chunk")
    chunk_path.unlink(missing_ok=True)
    result = subprocess.run(
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
            "--silent",
            "--show-error",
            "--range",
            f"{start}-{end}",
            "--max-filesize",
            str(length),
            "--output",
            str(chunk_path),
            "--write-out",
            "%{http_code}",
            url,
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(
            f"parallel snapshot range {start}-{end} failed: {result.stderr.strip()}"
        )
    _require(result.stdout.strip() == "206", f"parallel snapshot range {start}-{end} did not return HTTP 206")
    _require(
        chunk_path.is_file() and chunk_path.stat().st_size == length,
        f"parallel snapshot range {start}-{end} returned the wrong length",
    )
    offset = start
    with chunk_path.open("rb") as source:
        for block in iter(lambda: source.read(8 * 1024 * 1024), b""):
            _pwrite_all(destination_descriptor, block, offset)
            offset += len(block)
    chunk_path.unlink()
    return index, length


def _download_parallel_ranges(
    url: str,
    destination_part: Path,
    *,
    expected_size: int,
    expected_sha256: str,
    concurrency: int,
    chunk_size: int,
    curl_executable: str = "curl",
) -> None:
    _require(2 <= concurrency <= 64, "parallel download concurrency must be between 2 and 64")
    _require(chunk_size >= 1024 * 1024, "parallel download chunk size must be at least 1 MiB")
    _require(expected_size > 0, "parallel download expected size must be positive")
    _require(SHA256_RE.fullmatch(expected_sha256) is not None, "parallel download expected SHA-256 is invalid")
    _ensure_managed_directory(destination_part.parent, "parallel download directory")
    state_path, work_dir = _range_download_paths(destination_part)
    url_sha256 = _sha256_bytes(url.encode("utf-8"))

    _require_managed_file(destination_part, "parallel download target")
    _require_managed_file(state_path, "parallel download state")
    if _path_exists(state_path) and not destination_part.is_file():
        _cleanup_range_download(destination_part)
    if _path_exists(state_path):
        state = _load_json(state_path)
        _require_exact_keys(
            state,
            {
                "chunk_size_bytes",
                "completed_chunks",
                "expected_sha256",
                "expected_size",
                "url_sha256",
                "version",
            },
            "parallel download state",
        )
        _require(state["version"] == 1, "unsupported parallel download state version")
        _require(state["url_sha256"] == url_sha256, "parallel download URL identity mismatch")
        _require(state["expected_size"] == expected_size, "parallel download size identity mismatch")
        _require(state["expected_sha256"] == expected_sha256, "parallel download hash identity mismatch")
        _require(
            isinstance(state["chunk_size_bytes"], int)
            and not isinstance(state["chunk_size_bytes"], bool)
            and state["chunk_size_bytes"] >= 1024 * 1024,
            "parallel download chunk size is invalid",
        )
        chunk_size = state["chunk_size_bytes"]
        _require(
            destination_part.stat().st_size == expected_size,
            "parallel download preallocated file size mismatch",
        )
    else:
        if _path_exists(destination_part):
            destination_part.unlink()
        state = {
            "version": 1,
            "url_sha256": url_sha256,
            "expected_size": expected_size,
            "expected_sha256": expected_sha256,
            "chunk_size_bytes": chunk_size,
            "completed_chunks": [],
        }
        descriptor = os.open(destination_part, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o644)
        try:
            os.ftruncate(descriptor, expected_size)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        _write_range_state(state_path, state)

    chunk_count = (expected_size + chunk_size - 1) // chunk_size
    completed_value = state["completed_chunks"]
    _require(isinstance(completed_value, list), "parallel download completed chunk set is invalid")
    _require(
        all(isinstance(index, int) and not isinstance(index, bool) and 0 <= index < chunk_count for index in completed_value),
        "parallel download completed chunk index is invalid",
    )
    completed = set(completed_value)
    _require(len(completed) == len(completed_value), "parallel download completed chunk set is duplicated")
    _ensure_managed_directory(work_dir, "parallel download work directory")
    progress = _FileReadProgress(f"Download {destination_part.name}", expected_size)
    completed_bytes = sum(_chunk_length(index, chunk_size, expected_size) for index in completed)
    progress.update(completed_bytes, force=True)
    remaining = [index for index in range(chunk_count) if index not in completed]
    descriptor = os.open(destination_part, os.O_RDWR)
    pending: list[tuple[int, int]] = []
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
            futures = {
                executor.submit(
                    _download_range_chunk,
                    url=url,
                    index=index,
                    chunk_size=chunk_size,
                    expected_size=expected_size,
                    work_dir=work_dir,
                    destination_descriptor=descriptor,
                    curl_executable=curl_executable,
                ): index
                for index in remaining
            }
            try:
                for future in concurrent.futures.as_completed(futures):
                    pending.append(future.result())
                    if len(pending) < concurrency:
                        continue
                    # A chunk becomes resumable only after its bytes are durable.
                    os.fsync(descriptor)
                    for index, length in pending:
                        completed.add(index)
                        completed_bytes += length
                    pending.clear()
                    state["completed_chunks"] = sorted(completed)
                    _write_range_state(state_path, state)
                    progress.update(completed_bytes)
            except BaseException:
                for future in futures:
                    future.cancel()
                raise
        if pending:
            os.fsync(descriptor)
            for index, length in pending:
                completed.add(index)
                completed_bytes += length
            state["completed_chunks"] = sorted(completed)
            _write_range_state(state_path, state)
        _require(len(completed) == chunk_count, "parallel download did not complete every chunk")
        progress.finish(expected_size, success=True)
    except BaseException:
        progress.finish(completed_bytes, success=False)
        raise
    finally:
        os.close(descriptor)


def _verify_download(
    path: Path,
    expected_size: int,
    expected_sha256: str,
    *,
    progress_label: str,
) -> None:
    _require(path.is_file() and not path.is_symlink(), f"downloaded snapshot file is missing or not regular: {path}")
    _require(path.stat().st_size == expected_size, f"downloaded snapshot file size mismatch: {path}")
    _require(
        _sha256(path, progress_label=progress_label) == expected_sha256,
        f"downloaded snapshot file SHA-256 mismatch: {path}",
    )


def _fsync_path(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _validate_release_directory(
    files: list[dict[str, Any]],
    directory: Path,
    *,
    allow_transient: bool,
) -> None:
    """Ensures a release directory contains only regular managed entries."""
    expected_files = {PurePosixPath(item["path"]).name for item in files}
    expected_files.add("snapshot-release-record.json")
    transient_files: set[str] = set()
    transient_directories: set[str] = set()
    for item in files:
        part_name = f"{PurePosixPath(item['path']).name}.part"
        transient_files.add(part_name)
        transient_files.add(f"{part_name}.ranges.json")
        transient_directories.add(f"{part_name}.ranges")

    seen: set[str] = set()
    for child in directory.iterdir():
        name = child.name
        seen.add(name)
        if name in expected_files or (allow_transient and name in transient_files):
            _require(
                child.is_file() and not child.is_symlink(),
                f"snapshot release entry must be a regular file, not a symlink: {child}",
            )
            continue
        if allow_transient and name in transient_directories:
            _require(
                child.is_dir() and not child.is_symlink(),
                f"snapshot release work entry must be a real directory, not a symlink: {child}",
            )
            continue
        raise ValueError(f"unexpected entry in snapshot release directory: {child}")

    if not allow_transient:
        _require(
            seen == expected_files,
            "snapshot release directory inventory mismatch: "
            f"missing={sorted(expected_files - seen)}, extra={sorted(seen - expected_files)}",
        )


@dataclass(frozen=True)
class InstalledSnapshot:
    component: str
    artifact_id: str
    release_id: str
    release_dir: Path
    record_path: Path
    snapshot_file: Path
    manifest_file: Path
    signature_file: Path
    height: int
    network: str


def _installed_snapshot(
    record: dict[str, Any], component_name: str, release_dir: Path
) -> InstalledSnapshot:
    component = _component(record, component_name)
    by_role = {
        item["role"]: release_dir / PurePosixPath(item["path"]).name
        for item in component["files"]
    }
    return InstalledSnapshot(
        component=component_name,
        artifact_id=component["artifact_id"],
        release_id=record["snapshot_release_id"],
        release_dir=release_dir,
        record_path=release_dir / "snapshot-release-record.json",
        snapshot_file=by_role["database"],
        manifest_file=by_role["manifest"],
        signature_file=by_role["signature"],
        height=record["height"],
        network=record["network"],
    )


def _verify_installed_release(
    record: dict[str, Any],
    component_name: str,
    release_dir: Path,
    record_content: bytes,
) -> InstalledSnapshot:
    component = _component(record, component_name)
    _require(release_dir.is_dir() and not release_dir.is_symlink(), f"installed snapshot release is missing or invalid: {release_dir}")
    _validate_release_directory(component["files"], release_dir, allow_transient=False)
    installed_record = release_dir / "snapshot-release-record.json"
    _require(installed_record.read_bytes() == record_content, f"installed snapshot release record mismatch: {release_dir}")
    for item in component["files"]:
        _verify_download(
            release_dir / PurePosixPath(item["path"]).name,
            item["size"],
            item["sha256"],
            progress_label=f"Verify cached {item['path']}",
        )
    return _installed_snapshot(record, component_name, release_dir)


def install_release(
    *,
    record_url: str,
    destination_root: Path,
    trusted_keys: Path,
    component: str = "core",
    approved_record_path: Path | None = None,
    curl_executable: str = "curl",
    expected_network: str | None = None,
    max_height: int | None = None,
    download_concurrency: int = DEFAULT_DOWNLOAD_CONCURRENCY,
    download_chunk_size_mib: int = DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
) -> InstalledSnapshot:
    _require(1 <= download_concurrency <= 64, "download concurrency must be between 1 and 64")
    _require(1 <= download_chunk_size_mib <= 1024, "download chunk size must be between 1 and 1024 MiB")
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
    _require_managed_file(lock_path, "snapshot distribution lock")
    lock_flags = os.O_RDWR | os.O_CREAT | os.O_APPEND
    if hasattr(os, "O_NOFOLLOW"):
        lock_flags |= os.O_NOFOLLOW
    lock_descriptor = os.open(lock_path, lock_flags, 0o600)
    with os.fdopen(lock_descriptor, "a+b") as lock:
        _require(
            stat.S_ISREG(os.fstat(lock.fileno()).st_mode),
            f"snapshot distribution lock is not a regular file: {lock_path}",
        )
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        if approved_record_path is not None:
            approved_record = approved_record_path.expanduser()
            _require(
                approved_record.is_file() and not approved_record.is_symlink(),
                f"approved snapshot release record is missing or invalid: {approved_record}",
            )
            record_cache = approved_record.resolve()
            _require(
                _sha256(record_cache) == expected_record_sha256,
                "approved snapshot release record SHA-256 does not match its URL",
            )
        else:
            downloads = root / ".downloads"
            _ensure_managed_directory(downloads, "snapshot record download directory")
            record_cache = downloads / record_name
            _require_managed_file(record_cache, "snapshot release record cache")
            if not record_cache.is_file() or _sha256(record_cache) != expected_record_sha256:
                record_part = record_cache.with_name(record_cache.name + ".part")
                _require_managed_file(record_part, "snapshot release record partial")
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
        selected = _component(record, component)
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
        _validate_trusted_catalog(trusted_path, selected["signing_key_id"])

        release_id = record["snapshot_release_id"]
        destination_name = release_id if component == "core" else selected["artifact_id"]
        destination = root / destination_name
        if _path_exists(destination):
            print(
                f"Snapshot artifact exists; verifying before reuse: {destination}",
                file=sys.stderr,
                flush=True,
            )
            return _verify_installed_release(record, component, destination, record_content)
        staging = root / f".{destination_name}.{component}.installing"
        _ensure_managed_directory(staging, "snapshot release staging directory")
        _validate_release_directory(selected["files"], staging, allow_transient=True)
        for item in selected["files"]:
            file_name = PurePosixPath(item["path"]).name
            final_path = staging / file_name
            _require_managed_file(final_path, "staged snapshot file")
            if _path_exists(final_path):
                try:
                    _verify_download(
                        final_path,
                        item["size"],
                        item["sha256"],
                        progress_label=f"Verify staged {item['path']}",
                    )
                    continue
                except ValueError:
                    final_path.unlink(missing_ok=True)
            part_path = final_path.with_name(final_path.name + ".part")
            _require_managed_file(part_path, "snapshot download partial")
            range_state_path, _range_work_dir = _range_download_paths(part_path)
            _require_managed_file(range_state_path, "parallel download state")
            range_state_exists = _path_exists(range_state_path)
            if (
                part_path.is_file()
                and not range_state_exists
                and part_path.stat().st_size == item["size"]
                and _sha256(
                    part_path,
                    progress_label=f"Verify completed {item['path']}",
                )
                == item["sha256"]
            ):
                part_path.replace(final_path)
                continue
            if part_path.is_file() and not range_state_exists and part_path.stat().st_size >= item["size"]:
                part_path.unlink()
            object_url = _quoted_object_url(record_base, item["object_key"])
            use_parallel_ranges = (
                item["role"] == "database"
                and item["size"] >= PARALLEL_DOWNLOAD_MIN_SIZE
                and download_concurrency > 1
            )
            legacy_partial = (
                part_path.is_file()
                and not range_state_exists
                and 0 < part_path.stat().st_size < item["size"]
            )
            print(
                f"Downloading {component} snapshot artifact: {file_name}",
                file=sys.stderr,
                flush=True,
            )
            if use_parallel_ranges and not legacy_partial:
                _download_parallel_ranges(
                    object_url,
                    part_path,
                    expected_size=item["size"],
                    expected_sha256=item["sha256"],
                    concurrency=download_concurrency,
                    chunk_size=download_chunk_size_mib * 1024 * 1024,
                    curl_executable=curl_executable,
                )
            else:
                _download_with_resume(object_url, part_path, curl_executable)
            try:
                _verify_download(
                    part_path,
                    item["size"],
                    item["sha256"],
                    progress_label=f"Verify downloaded {item['path']}",
                )
            except ValueError:
                part_path.unlink(missing_ok=True)
                _cleanup_range_download(part_path)
                raise
            _cleanup_range_download(part_path)
            part_path.replace(final_path)
        _write_new_or_identical(staging / "snapshot-release-record.json", record_content)
        _validate_release_directory(selected["files"], staging, allow_transient=False)
        for item in selected["files"]:
            _fsync_path(staging / PurePosixPath(item["path"]).name)
        _fsync_path(staging / "snapshot-release-record.json")
        _fsync_path(staging)
        print(
            f"Snapshot files verified; publishing atomically: {destination}",
            file=sys.stderr,
            flush=True,
        )
        _require(not _path_exists(destination), f"snapshot release destination appeared during install: {destination}")
        os.replace(staging, destination)
        _fsync_path(root)
        # All files were hashed and fsynced immediately before the atomic rename.
        # Rehashing the immutable 250+ GiB database here would double install time.
        print(f"Snapshot artifact ready: {destination}", file=sys.stderr, flush=True)
        return _installed_snapshot(record, component, destination)


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
    upload.add_argument(
        "--s3-upload-concurrency",
        type=int,
        default=DEFAULT_S3_UPLOAD_CONCURRENCY,
    )
    upload.add_argument(
        "--s3-chunk-size-mib",
        type=int,
        default=DEFAULT_S3_CHUNK_SIZE_MIB,
    )
    upload.add_argument("--progress", action="store_true")
    upload.add_argument("--aws-executable", default="aws", help=argparse.SUPPRESS)

    install = subparsers.add_parser("install", help="Resume, verify, and atomically install one split snapshot component")
    install.add_argument("--record-url", required=True)
    install.add_argument("--destination-root", type=Path, required=True)
    install.add_argument("--trusted-keys", type=Path, required=True)
    install.add_argument("--component", choices=EXPECTED_COMPONENTS, default="core")
    install.add_argument("--expected-network")
    install.add_argument("--max-height", type=int)
    install.add_argument(
        "--download-concurrency",
        type=int,
        default=DEFAULT_DOWNLOAD_CONCURRENCY,
    )
    install.add_argument(
        "--download-chunk-size-mib",
        type=int,
        default=DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
    )
    install.add_argument("--curl-executable", default="curl", help=argparse.SUPPRESS)
    verify_public = subparsers.add_parser(
        "verify-public",
        help="Verify the content-addressed record and public object availability",
    )
    verify_public.add_argument("--record", type=Path, required=True)
    verify_public.add_argument("--trusted-keys", type=Path, required=True)
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
                    "record_object_key": f"snapshot-records/v3/{digest}.json",
                    "record_path": str(path),
                    "record_sha256": digest,
                    "record_url": f"{record['public_base_url']}/snapshot-records/v3/{digest}.json",
                    "snapshot_release_id": record["snapshot_release_id"],
                }
            )
        elif args.command == "upload":
            if args.progress:
                os.environ["USDB_SNAPSHOT_FORCE_PROGRESS"] = "1"
            with AwsCliClient(
                endpoint_url=args.endpoint_url,
                bucket=args.bucket,
                region=args.aws_region,
                profile=args.aws_profile,
                upload_concurrency=args.s3_upload_concurrency,
                multipart_chunk_size_mib=args.s3_chunk_size_mib,
                executable=args.aws_executable,
            ) as client:
                _print_json(upload_release(args.record, args.source_dir, client))
        elif args.command == "install":
            installed = install_release(
                record_url=args.record_url,
                destination_root=args.destination_root,
                trusted_keys=args.trusted_keys,
                component=args.component,
                curl_executable=args.curl_executable,
                expected_network=args.expected_network,
                max_height=args.max_height,
                download_concurrency=args.download_concurrency,
                download_chunk_size_mib=args.download_chunk_size_mib,
            )
            _print_json(
                {
                    "artifact_id": installed.artifact_id,
                    "component": installed.component,
                    "height": installed.height,
                    "manifest_file": str(installed.manifest_file),
                    "network": installed.network,
                    "record_path": str(installed.record_path),
                    "release_dir": str(installed.release_dir),
                    "snapshot_file": str(installed.snapshot_file),
                    "snapshot_release_id": installed.release_id,
                }
            )
        else:
            _print_json(verify_public_release(args.record, args.trusted_keys))
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Snapshot distribution failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
