#!/usr/bin/env python3
"""Create and validate a digest-pinned USDB cross-repository release candidate."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from validate_network_bundle import (  # noqa: E402
    read_json,
    require,
    require_no_runtime_secrets,
    sha256,
    validate_network_bundle,
)
from snapshot_distribution import validate_release_record  # noqa: E402
from runtime_compatibility import build_runtime_compatibility  # noqa: E402

SCHEMA_VERSION = "usdb-release-manifest:v5"
SNAPSHOT_RECORD_RELATIVE_PATH = Path(
    "snapshots/balance-history-snapshot-release-record.json"
)
RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
HASH_RE = re.compile(r"^[0-9a-f]{64}$")
BLOCK_HASH_RE = re.compile(r"^0x[0-9a-f]{64}$")

REPOSITORIES = {
    "go_ethereum": "buckyos/go-ethereum",
    "usdb": "buckyos/usdb",
    "source_dao": "buckyos/SourceDAO",
}
REPOSITORY_DIRECTORIES = {
    "go_ethereum": "go-ethereum",
    "usdb": "usdb",
    "source_dao": "SourceDAO",
}
IMAGE_SPECS = {
    "usdb_services": {
        "name": "ghcr.io/buckyos/usdb-services",
        "source": "usdb",
        "signer_workflow": "buckyos/usdb/.github/workflows/usdb-services-image.yml",
    },
    "usdb_chain": {
        "name": "ghcr.io/buckyos/usdb-chain",
        "source": "go_ethereum",
        "signer_workflow": "buckyos/go-ethereum/.github/workflows/usdb-chain-image.yml",
    },
    "bitcoin_core": {
        "name": "ghcr.io/buckyos/usdb-bitcoin-core",
        "source": "usdb",
        "signer_workflow": "buckyos/usdb/.github/workflows/usdb-bitcoin-image.yml",
    },
}
REQUIRED_CHECKS = {
    "go_ethereum": [
        "Go canonical and compatibility",
        "Cross-repository golden artifacts",
    ],
    "usdb": ["Rust workspace"],
    "source_dao": ["SourceDAO contracts"],
}


def require_exact_keys(value: dict[str, Any], expected: set[str], context: str) -> None:
    actual = set(value)
    require(
        actual == expected,
        f"{context} keys mismatch: missing={sorted(expected - actual)}, "
        f"unknown={sorted(actual - expected)}",
    )


def require_sha256(value: Any, context: str) -> str:
    require(isinstance(value, str) and HASH_RE.fullmatch(value) is not None, f"{context} must be lowercase SHA-256")
    return value


def require_revision(value: Any, context: str) -> str:
    require(isinstance(value, str) and REVISION_RE.fullmatch(value) is not None, f"{context} must be a full lowercase Git SHA")
    return value


def require_created_at(value: Any) -> str:
    require(isinstance(value, str), "created_at_utc must be a string")
    try:
        parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise ValueError("created_at_utc must use canonical UTC format YYYY-MM-DDTHH:MM:SSZ") from exc
    require(parsed.year >= 2026, "created_at_utc is outside the supported release era")
    return value


def require_image_reference(value: Any, expected_name: str, context: str) -> str:
    require(isinstance(value, str), f"{context} reference must be a string")
    pattern = re.compile(rf"^{re.escape(expected_name)}@sha256:[0-9a-f]{{64}}$")
    require(pattern.fullmatch(value) is not None, f"{context} must use the canonical GHCR digest reference")
    return value


def build_network_identity(bundle_dir: Path) -> dict[str, Any]:
    network = validate_network_bundle(bundle_dir)
    genesis_manifest = read_json(bundle_dir / "artifacts/usdb-genesis.manifest.json")
    genesis_block_hash = genesis_manifest.get("block_hash")
    require(isinstance(genesis_block_hash, str), "genesis manifest block hash must be a string")
    require(BLOCK_HASH_RE.fullmatch(genesis_block_hash) is not None, "genesis block hash must be 0x plus 64 lowercase hex characters")
    artifacts = network["artifacts"]
    return {
        "bundle_id": network["network_bundle_id"],
        "bundle_status": network["status"],
        "network_json_sha256": sha256(bundle_dir / "network.json"),
        "chain_id": network["chain_id"],
        "network_id": network["network_id"],
        "genesis_sha256": artifacts["genesis"]["sha256"],
        "genesis_block_hash": genesis_block_hash,
        "btc_network_id": network["btc_source"]["network_id"],
        "btc_index_origin_height": network["btc_source"]["index_origin_height"],
        "btc_activation_registry_id": network["btc_source"]["activation_registry_id"],
        "snapshot_trusted_keys_sha256": artifacts["snapshot_trusted_keys"]["sha256"],
    }


def _canonical_json(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def build_snapshot_state(bundle_dir: Path) -> dict[str, Any]:
    bootstrap = read_json(bundle_dir / "artifacts/bootstrap-manifest.json")
    mode = bootstrap.get("balance_history_snapshot_mode")
    require(mode == "none", "release-approved snapshot must remain an optional node choice")
    record_path = bundle_dir / SNAPSHOT_RECORD_RELATIVE_PATH
    if not record_path.exists():
        return {"status": "not_used", "bootstrap_mode": "full-sync"}
    require(record_path.is_file() and not record_path.is_symlink(), "snapshot release record must be a regular file")
    record = read_json(record_path)
    validate_release_record(record)
    require(
        record_path.read_bytes() == _canonical_json(record),
        "snapshot release record must use canonical JSON encoding",
    )

    network = validate_network_bundle(bundle_dir)
    network_names = {"btc-mainnet": "bitcoin", "btc-regtest": "regtest"}
    expected_network = network_names.get(network["btc_source"]["network_id"])
    require(expected_network is not None, "network bundle has no snapshot network mapping")
    require(record["network"] == expected_network, "snapshot release record Bitcoin network mismatch")
    require(
        record["height"] <= network["btc_source"]["index_origin_height"],
        "snapshot release record height exceeds BTC index origin",
    )

    trusted_artifact = network["artifacts"]["snapshot_trusted_keys"]
    trusted_path = bundle_dir / trusted_artifact["path"]
    require(
        trusted_path.name == record["trusted_keys"]["file_name"],
        "snapshot release record trusted-key catalog name mismatch",
    )
    trusted_sha256 = sha256(trusted_path)
    require(
        trusted_sha256 == trusted_artifact["sha256"] == record["trusted_keys"]["sha256"],
        "snapshot release record trusted-key catalog hash mismatch",
    )

    record_sha256 = sha256(record_path)
    record_url = (
        f"{record['public_base_url']}/snapshot-records/v2/{record_sha256}.json"
    )
    download_size = sum(item["size"] for item in record["files"])
    snapshot_file = next(item for item in record["files"] if item["role"] == "snapshot_db")
    return {
        "status": "available",
        "bootstrap_mode": "optional-signed-snapshot",
        "record": {
            "path": SNAPSHOT_RECORD_RELATIVE_PATH.as_posix(),
            "url": record_url,
            "sha256": record_sha256,
        },
        "snapshot_release_id": record["snapshot_release_id"],
        "artifact_set_id": record["artifact_set_id"],
        "height": record["height"],
        "btc_block_hash": record["btc_block_hash"],
        "snapshot_id": record["snapshot_id"],
        "download_size_bytes": download_size,
        "snapshot_db_size_bytes": snapshot_file["size"],
        "trusted_keys": {
            "path": trusted_artifact["path"],
            "sha256": trusted_sha256,
            "signing_key_id": record["trusted_keys"]["signing_key_id"],
        },
    }


def build_compatibility_lock(path: Path, revisions: dict[str, str]) -> dict[str, str]:
    lock = read_json(path)
    require_exact_keys(
        lock,
        {"schema_version", "coordinator", "dependencies", "toolchains"},
        "compatibility lock",
    )
    require(lock.get("schema_version") == "usdb-ci-revisions:v2", "unsupported compatibility lock schema")
    coordinator = lock.get("coordinator")
    require(isinstance(coordinator, dict), "compatibility lock coordinator must be an object")
    require_exact_keys(coordinator, {"repository", "directory"}, "compatibility lock coordinator")
    require(
        coordinator.get("repository") == REPOSITORIES["go_ethereum"],
        "compatibility lock coordinator repository mismatch",
    )
    require(
        coordinator.get("directory") == REPOSITORY_DIRECTORIES["go_ethereum"],
        "compatibility lock coordinator directory mismatch",
    )
    dependencies = lock.get("dependencies")
    dependency_keys = {"usdb", "source_dao"}
    require(isinstance(dependencies, dict), "compatibility lock dependencies must be an object")
    require_exact_keys(dependencies, dependency_keys, "compatibility lock dependencies")
    for key in dependency_keys:
        entry = dependencies[key]
        require(isinstance(entry, dict), f"compatibility lock dependencies.{key} must be an object")
        require_exact_keys(
            entry,
            {"repository", "directory", "revision"},
            f"compatibility lock dependencies.{key}",
        )
        require(entry.get("repository") == REPOSITORIES[key], f"compatibility lock repository mismatch for {key}")
        require(
            entry.get("directory") == REPOSITORY_DIRECTORIES[key],
            f"compatibility lock directory mismatch for {key}",
        )
        require_revision(entry.get("revision"), f"compatibility lock dependencies.{key}.revision")
    require(isinstance(lock["toolchains"], dict), "compatibility lock toolchains must be an object")
    for key in ("usdb", "source_dao"):
        require(
            dependencies[key]["revision"] == revisions[key],
            f"selected {key} revision does not match the go-ethereum compatibility lock",
        )
    return {
        "repository": REPOSITORIES["go_ethereum"],
        "path": "scripts/usdb/ci-revisions.json",
        "sha256": sha256(path),
    }


def create_manifest(
    *,
    bundle_dir: Path,
    release_id: str,
    created_at_utc: str,
    compatibility_lock_path: Path,
    revisions: dict[str, str],
    image_references: dict[str, str],
) -> dict[str, Any]:
    require(RELEASE_ID_RE.fullmatch(release_id) is not None, "release_id contains unsupported characters")
    require_created_at(created_at_utc)
    require_exact_keys(revisions, set(REPOSITORIES), "revision inputs")
    require_exact_keys(image_references, set(IMAGE_SPECS), "image inputs")
    network_identity = build_network_identity(bundle_dir)
    require(
        re.fullmatch(
            rf"{re.escape(network_identity['bundle_id'])}-r[1-9][0-9]*",
            release_id,
        )
        is not None,
        "release_id does not belong to the selected network bundle",
    )

    repositories: dict[str, Any] = {}
    for key, repository in REPOSITORIES.items():
        repositories[key] = {
            "repository": repository,
            "revision": require_revision(revisions[key], f"repositories.{key}.revision"),
        }

    images: dict[str, Any] = {}
    for key, spec in IMAGE_SPECS.items():
        source_key = spec["source"]
        images[key] = {
            "reference": require_image_reference(image_references[key], spec["name"], f"images.{key}"),
            "platform": "linux/amd64",
            "source_repository": REPOSITORIES[source_key],
            "source_revision": revisions[source_key],
            "attestation": {
                "repository": REPOSITORIES[source_key],
                "signer_workflow": spec["signer_workflow"],
            },
        }

    manifest = {
        "schema_version": SCHEMA_VERSION,
        "release_id": release_id,
        "stage": "candidate",
        "created_at_utc": created_at_utc,
        "network_bundle": network_identity,
        "compatibility_lock": build_compatibility_lock(compatibility_lock_path, revisions),
        "repositories": repositories,
        "images": images,
        "ci_required_checks": {key: list(value) for key, value in REQUIRED_CHECKS.items()},
        "snapshot": build_snapshot_state(bundle_dir),
        "runtime_compatibility": build_runtime_compatibility(network_identity),
    }
    validate_manifest(manifest, bundle_dir, compatibility_lock_path)
    return manifest


def validate_manifest(manifest: dict[str, Any], bundle_dir: Path, compatibility_lock_path: Path) -> None:
    require_exact_keys(
        manifest,
        {
            "schema_version",
            "release_id",
            "stage",
            "created_at_utc",
            "network_bundle",
            "compatibility_lock",
            "repositories",
            "images",
            "ci_required_checks",
            "snapshot",
            "runtime_compatibility",
        },
        "release manifest",
    )
    require(manifest["schema_version"] == SCHEMA_VERSION, "unsupported release manifest schema")
    require(isinstance(manifest["release_id"], str) and RELEASE_ID_RE.fullmatch(manifest["release_id"]) is not None, "invalid release_id")
    require(manifest["stage"] == "candidate", "release generator only accepts candidate manifests")
    require_created_at(manifest["created_at_utc"])
    require_no_runtime_secrets(manifest)

    network = manifest["network_bundle"]
    require(isinstance(network, dict), "network_bundle must be an object")
    require_exact_keys(
        network,
        {
            "bundle_id",
            "bundle_status",
            "network_json_sha256",
            "chain_id",
            "network_id",
            "genesis_sha256",
            "genesis_block_hash",
            "btc_network_id",
            "btc_index_origin_height",
            "btc_activation_registry_id",
            "snapshot_trusted_keys_sha256",
        },
        "network_bundle",
    )
    require_sha256(network["network_json_sha256"], "network_bundle.network_json_sha256")
    require_sha256(network["genesis_sha256"], "network_bundle.genesis_sha256")
    require_sha256(network["snapshot_trusted_keys_sha256"], "network_bundle.snapshot_trusted_keys_sha256")
    require(isinstance(network["genesis_block_hash"], str) and BLOCK_HASH_RE.fullmatch(network["genesis_block_hash"]) is not None, "invalid genesis block hash")
    expected_network = build_network_identity(bundle_dir)
    require(network == expected_network, "release manifest network identity does not match the bundle")
    require(
        re.fullmatch(
            rf"{re.escape(expected_network['bundle_id'])}-r[1-9][0-9]*",
            manifest["release_id"],
        )
        is not None,
        "release manifest ID does not belong to the network bundle",
    )

    repositories = manifest["repositories"]
    require(isinstance(repositories, dict), "repositories must be an object")
    require_exact_keys(repositories, set(REPOSITORIES), "repositories")
    for key, expected_repository in REPOSITORIES.items():
        entry = repositories[key]
        require(isinstance(entry, dict), f"repositories.{key} must be an object")
        require_exact_keys(entry, {"repository", "revision"}, f"repositories.{key}")
        require(entry["repository"] == expected_repository, f"repositories.{key}.repository mismatch")
        require_revision(entry["revision"], f"repositories.{key}.revision")

    compatibility_lock = manifest["compatibility_lock"]
    require(isinstance(compatibility_lock, dict), "compatibility_lock must be an object")
    require_exact_keys(compatibility_lock, {"repository", "path", "sha256"}, "compatibility_lock")
    require_sha256(compatibility_lock["sha256"], "compatibility_lock.sha256")
    selected_revisions = {key: repositories[key]["revision"] for key in REPOSITORIES}
    require(
        compatibility_lock == build_compatibility_lock(compatibility_lock_path, selected_revisions),
        "release manifest compatibility lock does not match the selected Go artifact",
    )

    images = manifest["images"]
    require(isinstance(images, dict), "images must be an object")
    require_exact_keys(images, set(IMAGE_SPECS), "images")
    for key, spec in IMAGE_SPECS.items():
        entry = images[key]
        require(isinstance(entry, dict), f"images.{key} must be an object")
        require_exact_keys(
            entry,
            {"reference", "platform", "source_repository", "source_revision", "attestation"},
            f"images.{key}",
        )
        source_key = spec["source"]
        source = repositories[source_key]
        require_image_reference(entry["reference"], spec["name"], f"images.{key}")
        require(entry["platform"] == "linux/amd64", f"images.{key}.platform must be linux/amd64")
        require(entry["source_repository"] == source["repository"], f"images.{key}.source_repository mismatch")
        require(entry["source_revision"] == source["revision"], f"images.{key}.source_revision mismatch")
        attestation = entry["attestation"]
        require(isinstance(attestation, dict), f"images.{key}.attestation must be an object")
        require_exact_keys(attestation, {"repository", "signer_workflow"}, f"images.{key}.attestation")
        require(attestation["repository"] == source["repository"], f"images.{key}.attestation repository mismatch")
        require(attestation["signer_workflow"] == spec["signer_workflow"], f"images.{key}.signer workflow mismatch")

    checks = manifest["ci_required_checks"]
    require(isinstance(checks, dict), "ci_required_checks must be an object")
    require(checks == REQUIRED_CHECKS, "ci_required_checks do not match the v1 release gate")
    require(
        manifest["snapshot"] == build_snapshot_state(bundle_dir),
        "candidate snapshot state does not match the network bundle",
    )
    require(
        manifest["runtime_compatibility"] == build_runtime_compatibility(expected_network),
        "runtime compatibility does not match the selected network and service contracts",
    )


def load_manifest(path: Path, bundle_dir: Path, compatibility_lock_path: Path) -> dict[str, Any]:
    manifest = read_json(path)
    validate_manifest(manifest, bundle_dir, compatibility_lock_path)
    return manifest


def write_manifest(path: Path, manifest: dict[str, Any]) -> None:
    require(not path.exists(), f"release manifest already exists: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.partial-{os.getpid()}")
    try:
        with temporary.open("x", encoding="utf-8") as output:
            json.dump(manifest, output, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    create = subparsers.add_parser("create", help="create a candidate manifest")
    create.add_argument("--bundle-dir", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    create.add_argument("--release-id", required=True)
    create.add_argument("--created-at-utc", required=True)
    create.add_argument("--compatibility-lock", type=Path, required=True)
    create.add_argument("--usdb-revision", required=True)
    create.add_argument("--go-ethereum-revision", required=True)
    create.add_argument("--source-dao-revision", required=True)
    create.add_argument("--services-image", required=True)
    create.add_argument("--chain-image", required=True)
    create.add_argument("--bitcoin-image", required=True)

    validate = subparsers.add_parser("validate", help="validate an existing manifest")
    validate.add_argument("--bundle-dir", type=Path, required=True)
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--compatibility-lock", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    bundle_dir = args.bundle_dir.resolve()
    try:
        if args.command == "create":
            manifest = create_manifest(
                bundle_dir=bundle_dir,
                release_id=args.release_id,
                created_at_utc=args.created_at_utc,
                compatibility_lock_path=args.compatibility_lock.resolve(),
                revisions={
                    "go_ethereum": args.go_ethereum_revision,
                    "usdb": args.usdb_revision,
                    "source_dao": args.source_dao_revision,
                },
                image_references={
                    "usdb_services": args.services_image,
                    "usdb_chain": args.chain_image,
                    "bitcoin_core": args.bitcoin_image,
                },
            )
            write_manifest(args.output.resolve(), manifest)
            path = args.output.resolve()
        else:
            path = args.manifest.resolve()
            manifest = load_manifest(path, bundle_dir, args.compatibility_lock.resolve())
        print(
            json.dumps(
                {
                    "release_id": manifest["release_id"],
                    "stage": manifest["stage"],
                    "manifest": str(path),
                    "sha256": sha256(path),
                },
                sort_keys=True,
            )
        )
    except (OSError, ValueError) as exc:
        print(f"release manifest error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
