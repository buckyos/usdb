#!/usr/bin/env python3
"""Build and validate the self-contained host-side USDB release node kit."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import tempfile
from pathlib import Path

from usdb_node import load_release_layout


NODE_KIT_FILES = (
    "docker/compose.bitcoin.yml",
    "docker/compose.runtime.yml",
    "docker/scripts/tools/check_json_rpc_readiness.py",
    "docker/scripts/tools/generate_bitcoin_rpcauth.py",
    "docker/scripts/tools/install_usdb_node.sh",
    "docker/scripts/tools/prepare_usdb_firewall.sh",
    "docker/scripts/tools/prepare_usdb_host.sh",
    "docker/scripts/tools/release_manifest.py",
    "docker/scripts/tools/runtime_compatibility.py",
    "docker/scripts/tools/run_testnet_bitcoin.sh",
    "docker/scripts/tools/run_testnet_runtime.sh",
    "docker/scripts/tools/snapshot_distribution.py",
    "docker/scripts/tools/usdb_node.py",
    "docker/scripts/tools/validate_network_bundle.py",
)


def build_node_kit(
    *,
    repository_root: Path,
    bundle_dir: Path,
    manifest_path: Path,
    manifest_checksum_path: Path,
    output_dir: Path,
) -> Path:
    root = repository_root.resolve()
    bundle = bundle_dir.resolve()
    manifest = manifest_path.resolve()
    manifest_checksum = manifest_checksum_path.resolve()
    destination = output_dir.resolve()
    if destination.exists():
        raise ValueError(f"refusing to replace existing node kit: {destination}")
    for path in (bundle, manifest, manifest_checksum):
        if not path.exists():
            raise ValueError(f"required node kit input is missing: {path}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging_parent = Path(tempfile.mkdtemp(prefix=".usdb-node-kit.", dir=destination.parent))
    staging = staging_parent / "usdb-node-kit"
    try:
        for relative_name in NODE_KIT_FILES:
            source = root / relative_name
            if not source.is_file():
                raise ValueError(f"required node kit file is missing: {source}")
            target = staging / relative_name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        release_dir = staging / "release"
        release_dir.mkdir(parents=True)
        shutil.copy2(manifest, release_dir / "usdb-release-manifest.json")
        shutil.copy2(
            manifest_checksum,
            release_dir / "usdb-release-manifest.json.sha256",
        )

        manifest_value = json.loads(manifest.read_text(encoding="utf-8"))
        network_identity = manifest_value.get("network_bundle")
        bundle_id = network_identity.get("bundle_id") if isinstance(network_identity, dict) else None
        if not isinstance(bundle_id, str) or re.fullmatch(
            r"usdb-(?:testnet|mainnet)-v[0-9]+", bundle_id
        ) is None:
            raise ValueError("release manifest has an invalid network bundle ID")
        target_bundle = staging / "docker/networks" / bundle_id
        target_bundle.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(bundle, target_bundle, ignore=shutil.ignore_patterns("node.env"))
        if (target_bundle / "node.env").exists():
            raise ValueError("private node.env must not enter the release node kit")
        layout = load_release_layout(staging)
        if layout.bundle_dir != target_bundle:
            raise ValueError("prepared node kit bundle path is inconsistent")
        os.replace(staging, destination)
    except BaseException:
        shutil.rmtree(staging_parent, ignore_errors=True)
        raise
    shutil.rmtree(staging_parent, ignore_errors=True)
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository-root", type=Path, required=True)
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--manifest-checksum", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        path = build_node_kit(
            repository_root=args.repository_root,
            bundle_dir=args.bundle_dir,
            manifest_path=args.manifest,
            manifest_checksum_path=args.manifest_checksum,
            output_dir=args.output_dir,
        )
    except (OSError, ValueError) as error:
        raise SystemExit(f"USDB node kit preparation failed: {error}") from error
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
