#!/usr/bin/env python3
"""Create, finalize and publish unified BH baselines through the existing snapshot workflow."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

import artifact_signing as signing
import bitcoin_assumeutxo as workspace
import snapshot_distribution as distribution
import assumeutxo_release
from assumeutxo_release import immutable, stamp, verify_public_file

RECORD_SCHEMA = "usdb-baseline-snapshot-release-record:v1"
FINALIZED_SCHEMA = "usdb-baseline-snapshot-finalized:v1"
MANIFEST_SCHEMA = "balance-history-baseline-manifest:v1"
ROLES = ("database", "manifest", "signature", "trusted_keys")
TRUST_FILE = "snapshot.trusted-keys.json"


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def file_item(path: Path) -> dict:
    return dict(name=path.name, size_bytes=path.stat().st_size,
                sha256=hashlib.sha256(signing.read_small(path)).hexdigest())


def validate_record(value: dict) -> None:
    """Validate a content-addressed distribution inventory; installation still authenticates its manifest."""
    signing.exact(value, {"schema_version", "artifact_type", "identity", "logical_sha256", "signing_key_id",
                         "public_base_url", "files"}, "baseline release record")
    signing.require(value["schema_version"] == RECORD_SCHEMA and value["artifact_type"] == "balance-history-baseline",
                    "Unsupported baseline release record")
    identity = value["identity"]
    signing.exact(identity, {"network", "height", "block_hash", "data_model_version", "commit_protocol_version",
                            "registry_policy", "balance_query_floor", "history_query_floor"}, "baseline identity")
    height = identity["height"]
    signing.require(type(height) is int and 0 < height < 0xFFFFFFFF, "Invalid baseline height")
    signing.require(identity["network"] in ("bitcoin", "testnet", "testnet4", "signet", "regtest"), "Invalid baseline network")
    signing.digest(identity["block_hash"])
    signing.require(type(identity["balance_query_floor"]) is int and type(identity["history_query_floor"]) is int
                    and identity["balance_query_floor"] == height and identity["history_query_floor"] == height + 1,
                    "Invalid baseline query floors")
    signing.require(identity["data_model_version"] == "balance-history-data-model:bip30-generations-core-unspendable-v2"
                    and identity["commit_protocol_version"] == "1.0.0", "Unsupported baseline data model or commit protocol")
    signing.require(identity["registry_policy"] == "live_utxos_and_genesis_outputs:v1", "Invalid baseline registry policy")
    signing.digest(value["logical_sha256"])
    signing.key_id(value["signing_key_id"])
    base = distribution._normalize_https_base(value["public_base_url"], "baseline public base")
    signing.require(base == value["public_base_url"], "Baseline public base must be normalized")
    signing.exact(value["files"], set(ROLES), "baseline file inventory")
    names = dict(database=f"balance_history_baseline_{height}.db",
                 manifest=f"balance_history_baseline_{height}.manifest.json",
                 signature=f"balance_history_baseline_{height}.manifest.sig", trusted_keys=TRUST_FILE)
    prefix = f"balance-history/baseline/{identity['network']}/{height}/{value['files']['database']['sha256']}"
    for role in ROLES:
        item = value["files"][role]
        signing.exact(item, {"name", "size_bytes", "sha256", "object_key"}, f"baseline {role}")
        signing.require(item["name"] == names[role], "Baseline file name differs from its role")
        signing.require(type(item["size_bytes"]) is int and item["size_bytes"] > 0, "Invalid baseline file length")
        signing.digest(item["sha256"])
        signing.require(item["object_key"] == f"{prefix}/{item['name']}", "Baseline object key identity mismatch")


class Release:
    """Freeze public inventories while keeping job cursors, source paths and private keys local."""

    def __init__(self, root: Path, builder: Path, tool: Path, height: int, block_hash: str, trust: Path, signer: str):
        signing.require(type(height) is int and 0 < height < 0xFFFFFFFF, "Invalid baseline height")
        self.height = height
        self.block_hash = signing.digest(block_hash)
        self.signer = signing.key_id(signer)
        self.builder, self.tool, self.trust = (path.expanduser().absolute() for path in (builder, tool, trust))
        self.root = root.expanduser().absolute() / f"{height:012}-{block_hash}"
        self.job = self.builder / "jobs" / self.root.name
        self.finalized = self.root / "artifact-finalized.json"

    def call(self, command: str, extra: list[str] | None = None) -> dict:
        args = [str(self.tool), "--root-dir", str(self.builder), "--json", "baseline", command,
                "--height", str(self.height), "--expected-block-hash", self.block_hash, *(extra or [])]
        print(f"Baseline producer started: action={command}, height={self.height}, job={self.job}", file=sys.stderr, flush=True)
        # Persisted logs and long-running progress remain visible while stdout holds just JSON.
        completed = subprocess.run(args, stdout=subprocess.PIPE, text=True, check=True)
        return signing.parse_json(completed.stdout.encode())

    def create(self, secret: Path, source_args: list[str], batch_size: int) -> dict:
        signing.require(not secret.expanduser().resolve().is_relative_to(self.root.resolve()), "Private signer must be outside the release directory")
        command = "create" if source_args else "resume"
        return self.call(command, [*source_args, "--signing-key", str(secret), "--trusted-keys", str(self.trust),
                                   "--batch-size", str(batch_size)])

    def verified(self) -> tuple[dict, dict[str, Path], dict]:
        """Authenticate once with Rust, then bind the exact verified file inventory."""
        directory = self.job / "artifact"
        names = dict(database=f"balance_history_baseline_{self.height}.db",
                     manifest=f"balance_history_baseline_{self.height}.manifest.json",
                     signature=f"balance_history_baseline_{self.height}.manifest.sig")
        files = {role: directory / name for role, name in names.items()}
        before = {role: stamp(path) for role, path in files.items()}
        trust_before = stamp(self.trust)
        manifest = self.call("verify", ["--trusted-keys", str(self.trust)])
        signing.require(all(stamp(path) == before[role] for role, path in files.items()) and stamp(self.trust) == trust_before,
                        "Baseline inputs changed during verification")
        signing.require(manifest["manifest_version"] == MANIFEST_SCHEMA, "Unexpected baseline manifest schema")
        signing.require(manifest["state"]["identity"]["height"] == self.height
                        and manifest["state"]["identity"]["block_hash"] == self.block_hash,
                        "Verified baseline differs from selected genesis")
        signing.require(manifest["signing_key_id"] == self.signer, "Baseline signer differs from selected snapshot signer")
        catalog = signing.read_small(self.trust)
        packaged_trust = self.root / TRUST_FILE
        immutable(packaged_trust, catalog)
        files["trusted_keys"] = packaged_trust
        inventory = {role: file_item(path) for role, path in files.items() if role != "database"}
        inventory["database"] = dict(name=names["database"], size_bytes=manifest["file_size"], sha256=manifest["file_sha256"])
        signing.require(inventory["database"]["size_bytes"] == before["database"][2], "Baseline DB length changed")
        return manifest, files, inventory

    def final_identity(self, manifest: dict, inventory: dict) -> dict:
        return dict(schema_version=FINALIZED_SCHEMA, identity=manifest["state"]["identity"],
                    logical_sha256=manifest["logical_sha256"], signing_key_id=self.signer, files=inventory)

    def finalizer(self) -> dict:
        repository = Path(__file__).resolve().parents[3]
        revision = subprocess.run(["git", "-C", str(repository), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
        tools = [Path(__file__), Path(distribution.__file__), Path(assumeutxo_release.__file__),
                 Path(signing.__file__), Path(workspace.__file__),
                 repository / "src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"]
        return dict(revision=revision, snapshot_tool_sha256=distribution._sha256(self.tool),
                    scripts_sha256={path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in tools})

    def check_finalized(self, expected: dict) -> dict:
        value = signing.parse_json(signing.read_small(self.finalized))
        signing.exact(value, {*expected, "finalizer", "finalized_at_utc"}, "baseline finalization")
        signing.require({name: value[name] for name in expected} == expected, "Baseline finalization identity changed")
        signing.require(signing.read_small(self.finalized) == signing.canonical(value), "Baseline finalization must be canonical")
        return value

    def finalize(self) -> dict:
        manifest, _, inventory = self.verified()
        expected = self.final_identity(manifest, inventory)
        if self.finalized.exists():
            value = self.check_finalized(expected)
        else:
            value = dict(expected, finalizer=self.finalizer(), finalized_at_utc=now())
            immutable(self.finalized, signing.canonical(value))
        return dict(status="finalized", finalization_file=str(self.finalized), **value)

    def record(self, base: str) -> tuple[dict, dict[str, Path]]:
        manifest, files, inventory = self.verified()
        self.check_finalized(self.final_identity(manifest, inventory))
        base = distribution._normalize_https_base(base, "baseline public base")
        prefix = f"balance-history/baseline/{manifest['state']['identity']['network']}/{self.height}/{manifest['file_sha256']}"
        record = dict(schema_version=RECORD_SCHEMA, artifact_type="balance-history-baseline", identity=manifest["state"]["identity"],
                      logical_sha256=manifest["logical_sha256"], signing_key_id=self.signer, public_base_url=base,
                      files={role: dict(item, object_key=f"{prefix}/{item['name']}") for role, item in inventory.items()})
        validate_record(record)
        return record, files

    def save_record(self, record: dict) -> dict:
        data = signing.canonical(record)
        digest = hashlib.sha256(data).hexdigest()
        path = self.root / "records" / f"{digest}.json"
        immutable(path, data)
        return dict(record_file=str(path), record_sha256=digest,
                    record_url=f"{record['public_base_url']}/snapshot-records/baseline/v1/{digest}.json",
                    manifest_url=f"{record['public_base_url']}/{record['files']['manifest']['object_key']}",
                    snapshot_url=f"{record['public_base_url']}/{record['files']['database']['object_key']}")

    def publish(self, base: str, client) -> dict:
        record, files = self.record(base)
        result = self.save_record(record)
        stamps = {role: stamp(path) for role, path in files.items()}
        statuses = {}
        # The same immutable S3/multipart and anonymous full-download checks serve UTXO releases.
        # Publish the record last, after all four allowlisted public files are verified.
        for role, path in files.items():
            item = record["files"][role]
            statuses[role] = distribution._publish_object(client, path, item["object_key"], item["sha256"], item["size_bytes"],
                                                          "application/json" if path.suffix == ".json" else "application/octet-stream")
            signing.require(stamp(path) == stamps[role], "Baseline file changed during upload")
            verify_public_file(f"{record['public_base_url']}/{item['object_key']}", item)
        signing.require(all(stamp(path) == stamps[role] for role, path in files.items()), "Baseline file changed during publication")
        signing.require(signing.read_small(self.trust) == signing.read_small(files["trusted_keys"]), "Snapshot trust changed during publication")
        path = Path(result["record_file"])
        item = file_item(path)
        key = f"snapshot-records/baseline/v1/{path.name}"
        statuses["record"] = distribution._publish_object(client, path, key, item["sha256"], item["size_bytes"], "application/json")
        verify_public_file(result["record_url"], item)
        result.update(status="published", public_verified_at_utc=now(), objects=statuses)
        workspace.save_json(self.root / "publish-result.json", result)
        return result

    def verify_published(self, base: str) -> dict:
        record, _ = self.record(base)
        result = self.save_record(record)
        for item in record["files"].values():
            verify_public_file(f"{record['public_base_url']}/{item['object_key']}", item)
        verify_public_file(result["record_url"], file_item(Path(result["record_file"])))
        return dict(result, status="public_verified", public_verified_at_utc=now())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["paths", "create", "resume", "resume-verify", "status", "verify", "finalize",
                                          "prepare-release", "publish", "verify-published"])
    parser.add_argument("--root-dir", type=Path, required=True)
    parser.add_argument("--builder-root", type=Path, required=True)
    parser.add_argument("--tool", type=Path, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--block-hash", "--expected-block-hash", required=True)
    parser.add_argument("--network", default="bitcoin")
    parser.add_argument("--signing-key", type=Path, required=True)
    parser.add_argument("--trusted-keys", type=Path, required=True)
    parser.add_argument("--signer-id", required=True)
    for name in ("source-root", "core-manifest", "registry-manifest", "genesis-block", "config"):
        parser.add_argument("--" + name, type=Path)
    parser.add_argument("--batch-size", type=int, default=20000)
    parser.add_argument("--public-base-url", default=distribution.DEFAULT_PUBLIC_BASE_URL)
    parser.add_argument("--bucket", default=distribution.DEFAULT_BUCKET)
    parser.add_argument("--endpoint-url", default=distribution.DEFAULT_ENDPOINT_URL)
    parser.add_argument("--aws-region", default=distribution.DEFAULT_AWS_REGION)
    parser.add_argument("--aws-profile", default="usdb-snapshot-publisher")
    parser.add_argument("--aws-executable", default="aws")
    parser.add_argument("--s3-upload-concurrency", type=int, default=distribution.DEFAULT_S3_UPLOAD_CONCURRENCY)
    parser.add_argument("--s3-chunk-size-mib", type=int, default=distribution.DEFAULT_S3_CHUNK_SIZE_MIB)
    parser.add_argument("--progress", action="store_true")
    args = parser.parse_args()
    try:
        release = Release(args.root_dir, args.builder_root, args.tool, args.height, args.block_hash, args.trusted_keys, args.signer_id)
        source_args = []
        for name in ("source-root", "core-manifest", "registry-manifest", "genesis-block", "config"):
            value = getattr(args, name.replace("-", "_"))
            if value is not None:
                source_args.extend(["--" + name, str(value)])
        signing.require(not source_args or args.command == "create", "Source options are accepted only by create")
        if args.command == "paths":
            result = dict(release_root=str(release.root), job_dir=str(release.job), tool=str(release.tool),
                          signer_id=release.signer, trusted_keys=str(release.trust))
        elif args.command == "status":
            result = release.call("status")
        else:
            with workspace.exclusive_directory(release.root):
                if args.command in ("create", "resume", "resume-verify"):
                    if source_args:
                        source_args += ["--network", args.network]
                    result = release.create(args.signing_key, source_args, args.batch_size)
                elif args.command == "verify":
                    result = release.call("verify", ["--trusted-keys", str(release.trust)])
                elif args.command == "finalize":
                    result = release.finalize()
                elif args.command == "prepare-release":
                    record, _ = release.record(args.public_base_url)
                    result = release.save_record(record)
                elif args.command == "verify-published":
                    result = release.verify_published(args.public_base_url)
                else:
                    if args.progress:
                        os.environ["USDB_SNAPSHOT_FORCE_PROGRESS"] = "1"
                    with distribution.AwsCliClient(endpoint_url=args.endpoint_url, bucket=args.bucket, region=args.aws_region,
                            profile=args.aws_profile or None, upload_concurrency=args.s3_upload_concurrency,
                            multipart_chunk_size_mib=args.s3_chunk_size_mib, executable=args.aws_executable) as client:
                        result = release.publish(args.public_base_url, client)
        print(json.dumps(result, indent=2, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"Baseline snapshot operation failed: action={args.command}, root={args.root_dir}, error={error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
