"""Build a tiny valid split-snapshot release record for repository-local tests."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def install_split_snapshot_record(bundle: Path) -> dict[str, object]:
    height = 963_800
    network = "bitcoin"
    block_hash = "00" * 31 + "42"
    core_snapshot_id = "22" * 32
    signer_catalog = bundle / "trust/usdb-mainnet-snapshot-v1.trusted-keys.json"
    signer_id = json.loads(signer_catalog.read_text(encoding="utf-8"))["keys"][0]["key_id"]

    def component(name: str, artifact_id: str) -> dict[str, object]:
        directory = "core" if name == "core" else "script-registry"
        database = (
            "balance_history_core_963800.db"
            if name == "core"
            else "script_registry_963800.db"
        )
        manifest = database.removesuffix(".db") + ".manifest.json"
        paths = (database, manifest, manifest.removesuffix(".json") + ".sig", "complete.json")
        roles = ("database", "manifest", "signature", "completion_marker")
        prefix = (
            f"snapshots/v3/balance-history/{network}/{height:012d}/"
            f"{directory}/{artifact_id}"
        )
        files = [
            {
                "object_key": f"{prefix}/{path}",
                "path": f"{directory}/{path}",
                "role": role,
                "sha256": hashlib.sha256(f"{name}:{role}".encode()).hexdigest(),
                "size": index + 8,
            }
            for index, (role, path) in enumerate(zip(roles, paths, strict=True))
        ]
        return {
            "artifact_id": artifact_id,
            "completed_at": 1_788_157_548,
            "files": files,
            "object_prefix": prefix,
            "signing_key_id": signer_id,
        }

    components = {
        "core": component("core", "33" * 32),
        "script_registry": component("script_registry", "44" * 32),
    }
    artifact_set_id = hashlib.sha256(
        json.dumps(
            {
                "btc_block_hash": block_hash,
                "components": components,
                "core_snapshot_id": core_snapshot_id,
                "height": height,
                "network": network,
            },
            separators=(",", ":"),
            sort_keys=True,
        ).encode()
    ).hexdigest()
    record = {
        "artifact_set_id": artifact_set_id,
        "artifact_type": "balance-history-split",
        "btc_block_hash": block_hash,
        "components": components,
        "core_snapshot_id": core_snapshot_id,
        "height": height,
        "network": network,
        "producer": {
            "artifact_finalization_marker_sha256": "55" * 32,
            "artifact_finalized_at_utc": "2026-09-05T00:00:00Z",
            "artifact_finalizer_revision": "aa" * 20,
            "artifact_producer_revision": "bb" * 20,
        },
        "public_base_url": "https://snapshots.example.test",
        "schema_version": "usdb-snapshot-release-record:v3",
        "snapshot_release_id": (
            f"balance-history-{network}-h{height}-{artifact_set_id[:16]}"
        ),
        "trusted_keys": {
            "file_name": signer_catalog.name,
            "sha256": _sha256(signer_catalog),
        },
    }
    target = bundle / "snapshots/balance-history-snapshot-release-record.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return record
