#!/usr/bin/env python3
"""Select reproducible release bundles from reviewed source inputs and verify delivery."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile

import artifact_signing as signing
import assumeutxo_deployment as deployment
import snapshot_distribution as distribution
from sourcedao_release import safe_file
from validate_network_bundle import validate_network_bundle

INPUT = "release-bootstrap.json"
SCHEMA = "usdb-release-bootstrap:v1"
ROLES = {"manifest", "signature", "trusted_keys", "record"}


def write_input(path: Path, content: bytes, *, replace: bool = False) -> None:
    """Publish small public inputs atomically; reject links and immutable conflicts."""
    for parent in (path, *path.parents):
        signing.require(not parent.is_symlink(), f"Release input path must not be a symlink: {parent}")
    if path.exists() and signing.read_small(path) == content:
        return
    signing.require(replace or not path.exists(), f"Immutable release input differs: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent) as output:
        output.write(content)
        output.flush()
        os.fsync(output.fileno())
        os.chmod(output.name, 0o644)
        if replace:
            # Keep the temporary name owned by NamedTemporaryFile for cleanup.
            staged = Path(output.name + ".ready")
            os.link(output.name, staged)
            try:
                os.replace(staged, path)
            finally:
                staged.unlink(missing_ok=True)
        else:
            os.link(output.name, path)


def load_inputs(source: Path, value: dict | None = None) -> tuple[dict, dict[str, Path]]:
    """Source revisions pin the base network, public keys, URL and publication record."""
    network = validate_network_bundle(source)
    value = signing.parse_json(signing.read_small(source / INPUT)) if value is None else value
    signing.exact(value, {"schema_version", "network_bundle_id", "source_network_sha256", "origin_block_hash", "source_url", "files"}, "release bootstrap input")
    signing.require(value["schema_version"] == SCHEMA, "Unsupported release bootstrap input")
    signing.require(value["network_bundle_id"] == network["network_bundle_id"], "Release bootstrap network mismatch")
    signing.require(value["source_network_sha256"] == hashlib.sha256(signing.read_small(source / "network.json")).hexdigest(),
                    "Base network changed; rerun snapshot deploy before releasing")
    signing.https_url(value["source_url"])
    signing.exact(value["files"], ROLES, "release bootstrap files")
    paths = {}
    for role, entry in value["files"].items():
        signing.exact(entry, {"path", "sha256"}, "release bootstrap file")
        relative = Path(entry["path"])
        signing.require(relative.parts[:2] == ("release-inputs", "assumeutxo"), "Release bootstrap file must be inside its public input directory")
        path = safe_file(source, entry["path"])
        signing.require(hashlib.sha256(signing.read_small(path)).hexdigest() == signing.digest(entry["sha256"]), "Release bootstrap input digest mismatch")
        paths[role] = path
    signing.require(paths["signature"] == Path(str(paths["manifest"]) + ".sig"), "Release bootstrap signature path mismatch")
    return value, paths


def prepare(source: Path, output: Path) -> Path:
    """Use the same deterministic selection in candidate creation and publish revalidation."""
    source = source.expanduser().absolute()
    validate_network_bundle(source)
    if not (source / INPUT).exists() and not (source / INPUT).is_symlink():
        return source
    value, paths = load_inputs(source)
    return deployment.prepare_bundle(source, output.expanduser().absolute(), value["origin_block_hash"], value["source_url"],
                                     paths["manifest"], paths["trusted_keys"], paths["record"])


def register(source: Path, candidate: Path, record: Path, published: dict) -> Path:
    """Record only the public inputs needed by CI; never copy a workspace or its raw UTXO."""
    network = validate_network_bundle(source)
    candidate_network = validate_network_bundle(candidate)
    contract = candidate_network.get("_native_bootstrap")
    signing.require(contract is not None and contract["distribution"]["mode"] == "usdb-signed", "Release integration requires a signed native candidate")
    signing.require(contract["distribution"]["source_url"] == published["source_url"], "Candidate source differs from publication")
    record_content = signing.read_small(record)
    record_sha = hashlib.sha256(record_content).hexdigest()
    signing.require(record_sha == published["record_sha256"], "Release integration record digest mismatch")
    prefix = Path("release-inputs/assumeutxo") / record_sha
    files = {}
    for role in sorted(ROLES):
        path = record if role == "record" else candidate / contract["distribution"][role]["path"]
        name = record_sha + ".json" if role == "record" else path.name
        content = signing.read_small(path)
        relative = prefix / name
        write_input(source / relative, content)
        files[role] = dict(path=relative.as_posix(), sha256=hashlib.sha256(content).hexdigest())
    value = dict(schema_version=SCHEMA, network_bundle_id=network["network_bundle_id"],
                 source_network_sha256=hashlib.sha256(signing.read_small(source / "network.json")).hexdigest(),
                 origin_block_hash=contract["origin_block_hash"], source_url=published["source_url"], files=files)
    value, paths = load_inputs(source, value)
    # Prove that the checked-in inputs can reconstruct a valid bundle before selecting them.
    with tempfile.TemporaryDirectory(prefix="usdb-release-input-check-") as temporary:
        deployment.prepare_bundle(source, Path(temporary) / "bundle", value["origin_block_hash"], value["source_url"],
                                  paths["manifest"], paths["trusted_keys"], paths["record"])
    selection = source / INPUT
    write_input(selection, signing.canonical(value), replace=True)
    print(f"Release input integrated: path={selection}, record_sha256={record_sha}", file=sys.stderr, flush=True)
    return selection


def verify_public(bundle: Path) -> dict:
    """Recheck small public objects and range delivery without downloading 9 GB in CI."""
    network = validate_network_bundle(bundle)
    contract = network.get("_native_bootstrap")
    if contract is None:
        return distribution.verify_public_release(bundle / "snapshots/balance-history-snapshot-release-record.json",
                                                  bundle / network["artifacts"]["snapshot_trusted_keys"]["path"])
    publication = deployment.load_publication(bundle, network, contract)
    if contract["distribution"]["mode"] == "usdb-signed":
        signing.require(publication is not None, "Signed native releases require an integrated publication record")
        record_content = signing.read_small(bundle / publication["path"])
        record = signing.parse_json(record_content)
        items = [(publication["url"], dict(name="record", sha256=publication["sha256"], size_bytes=len(record_content)))]
        items.extend((record["public_base_url"] + "/" + item["object_key"], item)
                     for role, item in record["files"].items() if role != "snapshot")
        with tempfile.TemporaryDirectory(prefix="usdb-public-release-check-") as temporary:
            for index, (url, item) in enumerate(items):
                target = Path(temporary) / str(index)
                signing.require(item["size_bytes"] <= signing.METADATA_LIMIT, "Public release metadata exceeds limit")
                signing.fetch(url, target, limit=item["size_bytes"])
                payload = target.read_bytes()
                signing.require(len(payload) == item["size_bytes"] and hashlib.sha256(payload).hexdigest() == item["sha256"],
                                f"Public release metadata mismatch: {item['name']}")
    source_url = signing.https_url(contract["distribution"]["source_url"])
    distribution._probe_public_byte_range(source_url, deployment.UTXO_SIZE)
    return dict(status="public_delivery_verified", mode="assumeutxo", source_url=source_url,
                record_url=publication["url"] if publication else None,
                note="Metadata and byte-range delivery verified; full UTXO hash is checked by snapshot publish and node download")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    generate = sub.add_parser("prepare")
    generate.add_argument("--source-bundle", type=Path, required=True)
    generate.add_argument("--output-dir", type=Path, required=True)
    verify = sub.add_parser("verify-public")
    verify.add_argument("--bundle-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            print(prepare(args.source_bundle, args.output_dir))
        else:
            print(json.dumps(verify_public(args.bundle_dir), indent=2, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Release bundle operation failed: action={args.command}, error={error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
