#!/usr/bin/env python3
"""Bind native bootstrap inputs to a new immutable network bundle and node paths."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from artifact_signing import canonical, digest, exact, https_url, parse_json, read_small, require
from assumeutxo_bootstrap import checkpoint_metadata, validate_environment
from bitcoin_release import UTXO_FILE, UTXO_SIZE, validate_utxo
from resource_policy import MIB, memory_bytes

SCHEMA = "usdb-assumeutxo-deployment:v1"
ARTIFACT = "assumeutxo_bootstrap"
RELATIVE = "artifacts/assumeutxo-bootstrap.json"
PUBLICATION_ARTIFACT = "assumeutxo_release_record"
PUBLICATION_PATH = "artifacts/assumeutxo-release-record.json"


def validate_contract(contract: dict, origin: int) -> None:
    """Distribution metadata cannot change the compiled snapshot or the network's origin."""
    exact(contract, {"schema_version", "snapshot", "origin_height", "origin_block_hash", "distribution"}, "native deployment")
    require(contract["schema_version"] == SCHEMA, "Unsupported native deployment schema")
    require(canonical(contract["snapshot"]) == canonical(checkpoint_metadata(935000)), "Native deployment checkpoint mismatch")
    require(type(contract["origin_height"]) is int and contract["origin_height"] == origin, "Native deployment origin differs from network")
    env = dict(SNAPSHOT_MODE="assumeutxo", USDB_GENESIS_BLOCK_HEIGHT=str(origin),
               BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH=contract["origin_block_hash"], BH_ASSUMEUTXO_SNAPSHOT_FILE="/data/assumeutxo/" + UTXO_FILE)
    validate_environment(env)
    distribution = contract["distribution"]
    exact(distribution, {"mode", "source_url", "manifest", "signature", "trusted_keys"}, "native distribution")
    if distribution["source_url"]:
        https_url(distribution["source_url"])
    require(distribution["mode"] in {"pinned", "usdb-signed"}, "Unsupported native distribution mode")
    if distribution["mode"] == "pinned":
        require(all(distribution[key] is None for key in ("manifest", "signature", "trusted_keys")), "Pinned deployment cannot supply signature files")
    else:
        for key in ("manifest", "signature", "trusted_keys"):
            entry = distribution[key]
            exact(entry, {"path", "sha256"}, "native distribution file")
            relative = Path(entry["path"])
            require(not relative.is_absolute() and ".." not in relative.parts and len(relative.parts) == 2
                    and relative.parts[0] in {"artifacts", "trust"}, "Native distribution path must be inside bundle artifacts/trust")
            from artifact_signing import digest
            digest(entry["sha256"])
        require(distribution["signature"]["path"] == distribution["manifest"]["path"] + ".sig", "Native detached signature path mismatch")
        require(distribution["trusted_keys"]["path"] == "trust/bitcoin-artifacts.trusted-keys.json", "Native trusted catalog path mismatch")


def load_contract(bundle: Path, network: dict) -> dict | None:
    entry = network.get("artifacts", {}).get(ARTIFACT)
    if entry is None:
        return None
    require(entry.get("path") == RELATIVE, "Native deployment artifact path mismatch")
    content = read_small(bundle / RELATIVE)
    require(hashlib.sha256(content).hexdigest() == entry.get("sha256"), "Native deployment artifact hash mismatch")
    contract = parse_json(content)
    validate_contract(contract, network["btc_source"]["index_origin_height"])
    distribution = contract["distribution"]
    if distribution["mode"] == "usdb-signed":
        for key in ("manifest", "signature", "trusted_keys"):
            item = distribution[key]
            content = read_small(bundle / item["path"])
            require(hashlib.sha256(content).hexdigest() == item["sha256"], "Native distribution file hash mismatch")
        from artifact_signing import load_manifest
        manifest = load_manifest(path=bundle / distribution["manifest"]["path"],
                                 trust_path=bundle / distribution["trusted_keys"]["path"], artifact_type="bitcoin-assumeutxo")
        validate_utxo(manifest)
    load_publication(bundle, network, contract)
    return contract


def load_publication(bundle: Path, network: dict, contract: dict) -> dict | None:
    """Bind an optional published UTXO record to the bundle's signed distribution."""
    entry = network["artifacts"].get(PUBLICATION_ARTIFACT)
    if entry is None:
        return None
    require(entry["path"] == PUBLICATION_PATH, "Native publication record path mismatch")
    content = read_small(bundle / PUBLICATION_PATH)
    record_sha = hashlib.sha256(content).hexdigest()
    require(record_sha == entry["sha256"], "Native publication record digest mismatch")
    record = parse_json(content)
    exact(record, {"schema_version", "artifact_type", "snapshot", "signing_key_id", "public_base_url", "files"}, "native publication")
    require(content == canonical(record) and record["schema_version"] == "usdb-assumeutxo-release-record:v1"
            and record["artifact_type"] == "bitcoin-assumeutxo", "Unsupported native publication record")
    require(record["snapshot"] == contract["snapshot"], "Native publication checkpoint mismatch")
    distribution = contract["distribution"]
    require(distribution["mode"] == "usdb-signed", "Native publication requires a signed distribution")
    manifest = parse_json(read_small(bundle / distribution["manifest"]["path"]))
    require(record["signing_key_id"] == manifest["signing_key_id"], "Native publication signer mismatch")
    base = https_url(record["public_base_url"])
    require(not base.endswith("/"), "Native publication base URL must not end in slash")
    files = record["files"]
    exact(files, {"snapshot", "manifest", "signature", "trusted_keys", "finalization"}, "native publication files")
    names = dict(snapshot=UTXO_FILE, manifest=files["manifest"]["sha256"] + ".json",
                 signature=files["manifest"]["sha256"] + ".json.sig", trusted_keys="bitcoin-artifacts.trusted-keys.json",
                 finalization="artifact-finalized.json")
    prefix = f"bitcoin/utxo/{contract['snapshot']['base_height']}/{digest(files['finalization']['sha256'])}"
    for role, item in files.items():
        exact(item, {"name", "size_bytes", "sha256", "object_key"}, "native publication file")
        digest(item["sha256"])
        require(type(item["size_bytes"]) is int and item["size_bytes"] > 0, "Invalid native publication file size")
        require(item["name"] == names[role] and item["object_key"] == prefix + "/" + names[role], "Native publication object path mismatch")
        if role in {"manifest", "signature", "trusted_keys"}:
            payload = read_small(bundle / distribution[role]["path"])
            require(len(payload) == item["size_bytes"] and hashlib.sha256(payload).hexdigest() == item["sha256"],
                    "Native publication differs from packaged signing materials")
    require(files["snapshot"]["sha256"] == contract["snapshot"]["file_sha256"]
            and files["snapshot"]["size_bytes"] == UTXO_SIZE, "Native publication file identity mismatch")
    require(distribution["source_url"] == base + "/" + files["snapshot"]["object_key"], "Native publication source URL mismatch")
    return dict(path=PUBLICATION_PATH, sha256=record_sha, url=f"{base}/snapshot-records/assumeutxo/v1/{record_sha}.json")


def state_identity(contract: dict) -> dict:
    """Keep transport/signer rotation out of the persistent balance database identity."""
    return {key: contract[key] for key in ("snapshot", "origin_height", "origin_block_hash")}


def paths(data_root: Path, bundle_id: str) -> dict[str, str]:
    return dict(BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR=str(data_root / "artifacts/assumeutxo/mainnet-935000"),
                BTC_ASSUMEUTXO_STATE_HOST_DIR=str(data_root / "networks" / bundle_id / "assumeutxo"))


def environment(contract: dict, data_root: Path, bundle_id: str) -> dict[str, str]:
    distribution = contract["distribution"]
    return {
        "SNAPSHOT_MODE": "assumeutxo", "BTC_TXINDEX": "0", "BH_ASSUMEUTXO_BASE_HEIGHT": "935000",
        "USDB_GENESIS_BLOCK_HEIGHT": str(contract["origin_height"]),
        "BTC_MIN_READY_HEIGHT": str(contract["origin_height"]), "BTC_NETWORK": "bitcoin",
        "BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH": contract["origin_block_hash"],
        "BH_ASSUMEUTXO_SNAPSHOT_FILE": "/data/assumeutxo/" + UTXO_FILE,
        "BTC_ASSUMEUTXO_SNAPSHOT_FILE": "/data/assumeutxo/" + UTXO_FILE,
        "BTC_ASSUMEUTXO_DISTRIBUTION_MODE": distribution["mode"],
        "BTC_ASSUMEUTXO_SOURCE_URL": distribution["source_url"],
        "BTC_ASSUMEUTXO_MANIFEST_URL": "",
        "BTC_ASSUMEUTXO_MANIFEST_FILE": "/network/" + Path(distribution["manifest"]["path"]).name if distribution["manifest"] else "",
        "BH_SYNC_LOCAL_LOADER_THRESHOLD": "500", "BH_SCRIPT_REGISTRY_ENABLED": "0",
        "BH_SCRIPT_REGISTRY_RECORD_URL": "", "BH_SCRIPT_REGISTRY_ARTIFACT_ID": "",
        "BH_SNAPSHOT_FILE": "", "BH_SNAPSHOT_MANIFEST": "", "USDB_INDEXER_CHECKPOINT_MANIFEST": "",
        "INSCRIPTION_SOURCE": "bitcoind", "INSCRIPTION_SOURCE_SHADOW_COMPARE": "false",
        **paths(data_root, bundle_id),
    }


def validate_node(contract: dict, env: dict, network: dict) -> None:
    expected = environment(contract, Path(env["USDB_DATA_ROOT"]), network["network_bundle_id"])
    # Cache tuning and txindex retention remain operator choices; identities and
    # mount paths always come from the release contract.
    for key, value in expected.items():
        if key not in {"BH_SYNC_LOCAL_LOADER_THRESHOLD", "BTC_TXINDEX"}:
            require(env.get(key, "") == value, f"{key} differs from the release native bootstrap contract")
    require(env.get("BTC_TXINDEX") in {"0", "1"}, "Native BTC_TXINDEX must be explicit")
    require(memory_bytes(env.get("BTC_BOOTSTRAP_MEMORY_LIMIT", "128m"), "BTC_BOOTSTRAP_MEMORY_LIMIT") == 128 * MIB,
            "Native Core observer requires its fixed 128 MiB budget")
    validate_environment(env)
    for key in ("BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR", "BTC_ASSUMEUTXO_STATE_HOST_DIR"):
        path = Path(env[key])
        require(path.is_dir() and not path.is_symlink(), f"Native artifact/state directory is missing or unsafe: {key}")


def prepare_bundle(source: Path, output: Path, origin_hash: str, source_url: str, manifest: Path | None, trusted: Path | None,
                   publication: Path | None = None) -> Path:
    """Create a candidate bundle without editing a published bundle or release checksum."""
    from validate_network_bundle import validate_network_bundle
    from sourcedao_release import copy_public_bundle
    network = validate_network_bundle(source)
    require("_native_bootstrap" not in network, "Prepare a native candidate from the base network bundle")
    require(not output.exists(), "Native candidate output must not exist")
    require((manifest is None) == (trusted is None), "Manifest and trusted keys must be provided together")
    distribution = dict(mode="usdb-signed" if manifest else "pinned", source_url=source_url, manifest=None, signature=None, trusted_keys=None)
    contract = dict(schema_version=SCHEMA, snapshot=checkpoint_metadata(935000),
                    origin_height=network["btc_source"]["index_origin_height"], origin_block_hash=origin_hash, distribution=distribution)
    sources = {}
    if manifest:
        from artifact_signing import load_manifest
        validate_utxo(load_manifest(path=manifest, trust_path=trusted, artifact_type="bitcoin-assumeutxo"))
        for key, path, relative in (
            ("manifest", manifest, "artifacts/assumeutxo-distribution.json"),
            ("signature", Path(str(manifest) + ".sig"), "artifacts/assumeutxo-distribution.json.sig"),
            ("trusted_keys", trusted, "trust/bitcoin-artifacts.trusted-keys.json"),
        ):
            content = read_small(path)
            distribution[key] = dict(path=relative, sha256=hashlib.sha256(content).hexdigest())
            sources[key] = content
    validate_contract(contract, contract["origin_height"])
    copy_public_bundle(source, output, network)
    for key, content in sources.items():
        path = output / distribution[key]["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        network["artifacts"]["assumeutxo_" + key] = distribution[key]
    (output / RELATIVE).write_bytes(canonical(contract))
    network["artifacts"][ARTIFACT] = dict(path=RELATIVE, sha256=hashlib.sha256(canonical(contract)).hexdigest())
    if publication is not None:
        content = read_small(publication)
        (output / PUBLICATION_PATH).write_bytes(content)
        network["artifacts"][PUBLICATION_ARTIFACT] = dict(path=PUBLICATION_PATH, sha256=hashlib.sha256(content).hexdigest())
    (output / "network.json").write_text(json.dumps(network, indent=2) + "\n")
    from runtime_compatibility import build_runtime_compatibility, build_persistent_data_paths
    from usdb_node import upsert_env
    from validate_network_bundle import read_env
    genesis = parse_json(read_small(output / "artifacts/usdb-genesis.manifest.json"))
    identity = dict(bundle_id=network["network_bundle_id"], chain_id=network["chain_id"], genesis_block_hash=genesis["block_hash"],
                    btc_network_id=network["btc_source"]["network_id"], btc_index_origin_height=contract["origin_height"],
                    btc_activation_registry_id=network["btc_source"]["activation_registry_id"], balance_history_bootstrap=state_identity(contract))
    compatibility = build_runtime_compatibility(identity)
    template = output / "node.env.example"
    root = Path(read_env(template)["USDB_DATA_ROOT"])
    updates = {"USDB_RUNTIME_COMPATIBILITY_ID": compatibility["compatibility_id"],
               **{key: str(value) for key, value in build_persistent_data_paths(root, identity, compatibility).items()},
               **environment(contract, root, network["network_bundle_id"])}
    template.write_text(upsert_env(template.read_text(), updates))
    validate_network_bundle(output)
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-bundle", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--origin-block-hash", required=True)
    parser.add_argument("--source-url", default="", help="Empty requires pre-positioning the pinned UTXO file in the generated artifact directory")
    parser.add_argument("--manifest-file", type=Path)
    parser.add_argument("--trusted-keys", type=Path)
    args = parser.parse_args()
    try:
        print(prepare_bundle(args.source_bundle, args.output_dir, args.origin_block_hash, args.source_url, args.manifest_file, args.trusted_keys))
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"Native bundle preparation failed: {error}") from error


if __name__ == "__main__":
    main()
