#!/usr/bin/env python3
"""Fail-closed validation for a USDB network bundle and optional node config."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import re
import sys
from pathlib import Path, PurePosixPath
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from runtime_compatibility import (  # noqa: E402
    DATA_LAYOUT_VERSION,
    LEGACY_PERSISTENT_DATA_PATHS,
    PERSISTENT_DATA_SERVICES,
    build_persistent_data_paths,
    build_runtime_compatibility,
    network_secure_dir,
    snapshot_artifact_dir,
)

EXPECTED_BUNDLE_ID = "usdb-testnet-v0"
EXPECTED_CHAIN_ID = 202608250
EXPECTED_BTC_REGISTRY = "a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224"
EXPECTED_BOOTSTRAP_ADMIN = "0x0b5223FD31cDc1536f31b3627e6D7025b52310c9"
EXPECTED_SNAPSHOT_MANIFEST_VERSION = "balance-history-snapshot-manifest:v3"
EXPECTED_SNAPSHOT_SIGNATURE_SCHEME = "ed25519"
EXPECTED_INDEXER_CHECKPOINT_MANIFEST_VERSION = "usdb-indexer-checkpoint-manifest:v1"
EXPECTED_INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION = "usdb-indexer-data:v1"
PUBLIC_DEPLOYMENT_TIERS = frozenset({"testnet", "mainnet"})
SUPPORTED_DEPLOYMENT_TIERS = frozenset({"development"}) | PUBLIC_DEPLOYMENT_TIERS
SUPPORTED_FIREWALL_MODES = frozenset({"external", "managed"})
DEFAULT_BITCOIN_RESOURCE_PROFILE = "balanced-32g"
BITCOIN_RESOURCE_PROFILES = {
    "balanced-32g": {
        "memory_limit": "5g",
        "dbcache_mb": "3072",
    },
    "performance-64g": {
        "memory_limit": "16g",
        "dbcache_mb": "12288",
    },
}
KNOWN_DEVELOPMENT_BOOTSTRAP_ADMINS = frozenset(
    {
        "0xabcd35afbb4561213feaff01b5f91e18f8df7c37",
    }
)
PERSISTENT_DATA_PATHS = LEGACY_PERSISTENT_DATA_PATHS


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_object)
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        raise ValueError(f"failed to load strict JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object in {path}")
    return value


def read_env(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise ValueError(f"failed to read env file {path}: {exc}") from exc
    for line_number, raw in enumerate(lines, start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"invalid env line {path}:{line_number}")
        key, value = line.split("=", 1)
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError(f"invalid env key {key!r} at {path}:{line_number}")
        if key in result:
            raise ValueError(f"duplicate env key {key!r} in {path}")
        result[key] = value
    return result


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def require_u32(value: Any, message: str) -> int:
    require(
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= 0xFFFFFFFF,
        message,
    )
    return value


def network_index_origin_height(network: dict[str, Any]) -> int:
    btc_source = network.get("btc_source")
    require(isinstance(btc_source, dict), "network btc_source is required")
    return require_u32(
        btc_source.get("index_origin_height"),
        "BTC index origin must be a u32",
    )


def snapshot_container_path(
    value: str,
    label: str,
    expected_suffix: str,
) -> PurePosixPath:
    require(bool(value), f"{label} is required in balance-history snapshot mode")
    path = PurePosixPath(value)
    snapshots_root = PurePosixPath("/snapshots")
    require(
        path.is_absolute()
        and path.name not in {"", ".", ".."}
        and (
            path.parent == snapshots_root
            or (
                path.parent.parent == snapshots_root
                and path.parent.name not in {"", ".", ".."}
            )
        ),
        f"{label} must be a direct child of /snapshots or one immutable release directory below it",
    )
    require(
        path.name.endswith(expected_suffix),
        f"{label} must end with {expected_suffix}",
    )
    return path


def validate_runtime_snapshot(
    snapshot_dir: Path,
    snapshot_file: PurePosixPath,
    snapshot_manifest: PurePosixPath,
    index_origin_height: int,
    btc_network: str,
    allow_above_origin: bool = False,
) -> dict[str, Any]:
    require(
        snapshot_dir.is_dir(),
        f"snapshot host directory does not exist: {snapshot_dir}",
    )
    snapshot_root = PurePosixPath("/snapshots")
    file_relative = snapshot_file.relative_to(snapshot_root)
    manifest_relative = snapshot_manifest.relative_to(snapshot_root)
    require(
        file_relative.parent == manifest_relative.parent,
        "snapshot file and manifest must belong to the same release directory",
    )
    file_path = snapshot_dir.joinpath(*file_relative.parts)
    manifest_path = snapshot_dir.joinpath(*manifest_relative.parts)
    signature_path = manifest_path.with_suffix(".sig")
    for path in (file_path, manifest_path, signature_path):
        require(
            path.is_file() and not path.is_symlink(),
            f"required snapshot artifact is missing or not regular: {path}",
        )

    manifest = read_json(manifest_path)
    require(
        manifest.get("manifest_version") == EXPECTED_SNAPSHOT_MANIFEST_VERSION,
        "unsupported balance-history snapshot manifest version",
    )
    require(
        manifest.get("file_name") == snapshot_file.name,
        "snapshot manifest file_name mismatch",
    )
    require(
        manifest.get("signature_scheme") == EXPECTED_SNAPSHOT_SIGNATURE_SCHEME
        and isinstance(manifest.get("signing_key_id"), str)
        and bool(manifest["signing_key_id"]),
        "release snapshot manifest must declare an Ed25519 signer",
    )

    state_ref = manifest.get("state_ref")
    require(isinstance(state_ref, dict), "snapshot manifest state_ref is required")
    snapshot_height = require_u32(
        state_ref.get("block_height"),
        "snapshot height must be a u32",
    )
    if not allow_above_origin:
        require(
            snapshot_height <= index_origin_height,
            f"snapshot height {snapshot_height} exceeds network index origin {index_origin_height}",
        )
    require(
        manifest.get("balance_query_floor") == snapshot_height,
        "snapshot balance_query_floor must equal snapshot height",
    )
    require(
        manifest.get("history_query_floor") == min(snapshot_height + 1, 0xFFFFFFFF),
        "snapshot history_query_floor must immediately follow snapshot height",
    )
    db_identity = manifest.get("db_identity")
    require(isinstance(db_identity, dict), "snapshot manifest db_identity is required")
    require(db_identity.get("btc_network") == btc_network, "snapshot Bitcoin network mismatch")
    return manifest


def checkpoint_container_path(value: str) -> PurePosixPath:
    require(bool(value), "USDB_INDEXER_CHECKPOINT_MANIFEST is required in paired-checkpoint mode")
    path = PurePosixPath(value)
    require(
        path.is_absolute()
        and path.parent.parent == PurePosixPath("/snapshots")
        and path.name == "usdb-indexer-checkpoint.manifest.json"
        and path.parent.name not in {"", ".", ".."},
        "USDB_INDEXER_CHECKPOINT_MANIFEST must be /snapshots/<artifact-dir>/usdb-indexer-checkpoint.manifest.json",
    )
    return path


def nested_string(value: dict[str, Any], *keys: str) -> str | None:
    current: Any = value
    for key in keys:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return current if isinstance(current, str) else None


def validate_runtime_checkpoint(
    snapshot_dir: Path,
    checkpoint_path: PurePosixPath,
    balance_history_manifest_path: Path,
    balance_history_manifest: dict[str, Any],
    network: dict[str, Any],
    index_origin_height: int,
    btc_network: str,
) -> None:
    artifact_dir = snapshot_dir / checkpoint_path.parent.name
    manifest_path = artifact_dir / checkpoint_path.name
    signature_path = artifact_dir / "usdb-indexer-checkpoint.manifest.sig"
    data_dir = artifact_dir / "data"
    require(manifest_path.is_file(), f"indexer checkpoint manifest is missing: {manifest_path}")
    require(signature_path.is_file(), f"indexer checkpoint signature is missing: {signature_path}")
    require(data_dir.is_dir(), f"indexer checkpoint data directory is missing: {data_dir}")

    manifest = read_json(manifest_path)
    require(
        manifest.get("manifest_version") == EXPECTED_INDEXER_CHECKPOINT_MANIFEST_VERSION,
        "unsupported usdb-indexer checkpoint manifest version",
    )
    require(
        manifest.get("data_schema_version") == EXPECTED_INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION,
        "unsupported usdb-indexer checkpoint data schema",
    )
    require(
        isinstance(manifest.get("tool_version"), str) and bool(manifest["tool_version"]),
        "indexer checkpoint tool_version is required",
    )
    require(
        manifest.get("signature_scheme") == EXPECTED_SNAPSHOT_SIGNATURE_SCHEME
        and isinstance(manifest.get("signing_key_id"), str)
        and bool(manifest["signing_key_id"]),
        "release indexer checkpoint must declare an Ed25519 signer",
    )
    require(
        isinstance(manifest.get("operation_id"), str)
        and re.fullmatch(r"[0-9a-f]{64}", manifest["operation_id"]) is not None,
        "invalid paired checkpoint operation ID",
    )
    require(
        manifest.get("artifact_dir_name") == checkpoint_path.parent.name,
        "checkpoint artifact directory name mismatch",
    )
    require(
        manifest.get("network_bundle_id") == network.get("network_bundle_id"),
        "checkpoint network bundle mismatch",
    )
    require(manifest.get("chain_id") == network.get("chain_id"), "checkpoint chain ID mismatch")
    require(
        manifest.get("index_origin_height") == index_origin_height,
        "checkpoint index origin mismatch",
    )
    require(
        manifest.get("btc_network") == btc_network,
        "checkpoint Bitcoin network mismatch",
    )
    checkpoint_height = require_u32(
        manifest.get("checkpoint_height"),
        "checkpoint height must be a u32",
    )
    require(
        checkpoint_height >= index_origin_height,
        "paired indexer checkpoint height must not precede network index origin",
    )

    upstream = manifest.get("balance_history")
    require(isinstance(upstream, dict), "checkpoint balance_history binding is required")
    upstream_state = upstream.get("state_ref")
    snapshot_state = balance_history_manifest.get("state_ref")
    require(isinstance(upstream_state, dict), "checkpoint upstream state_ref is required")
    require(upstream_state == snapshot_state, "checkpoint upstream state_ref mismatch")
    require(
        upstream.get("manifest_file_name") == balance_history_manifest_path.name
        and upstream.get("manifest_sha256") == sha256(balance_history_manifest_path),
        "checkpoint balance-history manifest binding mismatch",
    )
    require(
        upstream.get("snapshot_file_name") == balance_history_manifest.get("file_name")
        and upstream.get("snapshot_file_sha256") == balance_history_manifest.get("file_sha256"),
        "checkpoint balance-history snapshot binding mismatch",
    )
    require(
        upstream.get("balance_query_floor") == balance_history_manifest.get("balance_query_floor")
        and upstream.get("history_query_floor") == balance_history_manifest.get("history_query_floor"),
        "checkpoint balance-history retention floor mismatch",
    )
    require(
        isinstance(snapshot_state, dict) and snapshot_state.get("block_height") == checkpoint_height,
        "paired checkpoint heights do not match",
    )

    identity = manifest.get("state_identity")
    indexer_state_ref = manifest.get("indexer_state_ref")
    require(isinstance(identity, dict), "checkpoint state_identity is required")
    require(isinstance(indexer_state_ref, dict), "checkpoint full indexer_state_ref is required")
    require(identity.get("block_height") == checkpoint_height, "checkpoint identity height mismatch")
    require(
        identity.get("stable_block_hash") == snapshot_state.get("stable_block_hash")
        and identity.get("latest_block_commit") == snapshot_state.get("latest_block_commit")
        and identity.get("snapshot_id") == snapshot_state.get("snapshot_id"),
        "checkpoint normalized upstream identity mismatch",
    )
    require(
        identity.get("stable_block_hash")
        == nested_string(indexer_state_ref, "snapshot_info", "stable_block_hash")
        and identity.get("latest_block_commit")
        == nested_string(indexer_state_ref, "snapshot_info", "latest_block_commit")
        and identity.get("snapshot_id")
        == nested_string(indexer_state_ref, "snapshot_info", "snapshot_id")
        and identity.get("activation_registry_id")
        == nested_string(indexer_state_ref, "local_state_commit_info", "activation_registry_id")
        and identity.get("active_version_set_id")
        == nested_string(indexer_state_ref, "local_state_commit_info", "active_version_set_id")
        and identity.get("local_state_commit")
        == nested_string(indexer_state_ref, "local_state_commit_info", "local_state_commit")
        and identity.get("system_state_id")
        == nested_string(indexer_state_ref, "system_state_info", "system_state_id"),
        "checkpoint normalized identity does not match full state-ref",
    )

    files = manifest.get("files")
    require(isinstance(files, list) and bool(files), "checkpoint file inventory is required")
    seen: set[str] = set()
    for item in files:
        require(isinstance(item, dict), "checkpoint file entry must be an object")
        relative_text = item.get("path")
        require(isinstance(relative_text, str) and bool(relative_text), "checkpoint file path is required")
        relative = PurePosixPath(relative_text)
        require(
            not relative.is_absolute() and ".." not in relative.parts and relative_text not in seen,
            "checkpoint file path is unsafe or duplicated",
        )
        seen.add(relative_text)
        file_path = data_dir.joinpath(*relative.parts)
        require(file_path.is_file(), f"checkpoint data file is missing: {file_path}")
        require(item.get("size") == file_path.stat().st_size, f"checkpoint file size mismatch: {relative_text}")
        require(item.get("sha256") == sha256(file_path), f"checkpoint file SHA-256 mismatch: {relative_text}")


def require_no_runtime_secrets(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, item in value.items():
            lowered = key.lower().replace("_", "")
            require(
                not any(token in lowered for token in ("privatekey", "password", "mnemonic", "secret")),
                f"runtime secret field is forbidden in bundle JSON: {path}.{key}",
            )
            require_no_runtime_secrets(item, f"{path}.{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            require_no_runtime_secrets(item, f"{path}[{index}]")


def validate_bootstrap_admin(deployment_tier: str, address: Any) -> str:
    require(
        isinstance(deployment_tier, str) and deployment_tier in SUPPORTED_DEPLOYMENT_TIERS,
        "unsupported deployment tier",
    )
    require(
        isinstance(address, str)
        and re.fullmatch(r"0x[0-9a-fA-F]{40}", address) is not None
        and int(address[2:], 16) != 0,
        "bootstrap admin must be a non-zero EVM address",
    )
    if deployment_tier in PUBLIC_DEPLOYMENT_TIERS:
        require(
            address.lower() not in KNOWN_DEVELOPMENT_BOOTSTRAP_ADMINS,
            f"{deployment_tier} bootstrap admin must not use a known development address",
        )
    return address


def validate_artifact_hashes(bundle_dir: Path, network: dict[str, Any]) -> None:
    artifacts = network.get("artifacts")
    require(isinstance(artifacts, dict), "network.json artifacts must be an object")
    for name, artifact in artifacts.items():
        require(isinstance(artifact, dict), f"artifact {name} must be an object")
        relative_path = artifact.get("path")
        require(isinstance(relative_path, str) and relative_path, f"artifact {name} path is required")
        relative = Path(relative_path)
        require(
            not relative.is_absolute() and ".." not in relative.parts,
            f"artifact {name} path escapes bundle",
        )
        path = bundle_dir / relative
        require(path.is_file(), f"artifact {name} is missing: {path}")
        expected_hash = artifact.get("sha256")
        if expected_hash is not None:
            require(sha256(path) == expected_hash, f"artifact {name} SHA-256 mismatch")


def validate_network_bundle(bundle_dir: Path) -> dict[str, Any]:
    network = read_json(bundle_dir / "network.json")
    env = read_env(bundle_dir / "network.env")
    chain = read_json(bundle_dir / "artifacts/usdb-chain-bootstrap-config.json")
    source_dao = read_json(bundle_dir / "artifacts/sourcedao-bootstrap-config.json")
    genesis = read_json(bundle_dir / "artifacts/usdb-genesis.json")
    genesis_manifest = read_json(bundle_dir / "artifacts/usdb-genesis.manifest.json")
    bootstrap_manifest = read_json(bundle_dir / "artifacts/bootstrap-manifest.json")

    for value in (network, chain, source_dao, genesis_manifest, bootstrap_manifest):
        require_no_runtime_secrets(value)

    require(network.get("schema_version") == "usdb-network-bundle:v2", "unexpected network bundle schema")
    require(network.get("network_bundle_id") == EXPECTED_BUNDLE_ID, "unexpected network bundle ID")
    require(network.get("status") == "development-resettable", "testnet-v0 must remain resettable")
    require(network.get("deployment_tier") == "testnet", "testnet-v0 deployment tier must be testnet")
    require(network.get("chain_id") == EXPECTED_CHAIN_ID, "unexpected testnet-v0 chain ID")
    require(network.get("network_id") == EXPECTED_CHAIN_ID, "unexpected testnet-v0 network ID")
    require(network.get("p2p_port") == 31303, "testnet-v0 P2P port must be 31303")

    btc_source = network.get("btc_source")
    require(isinstance(btc_source, dict), "network btc_source is required")
    index_origin_height = network_index_origin_height(network)
    require(btc_source.get("network_id") == "btc-mainnet", "testnet-v0 must consume BTC mainnet")
    require(btc_source.get("activation_registry_id") == EXPECTED_BTC_REGISTRY, "unexpected BTC registry")

    require(env.get("USDB_NETWORK_BUNDLE_ID") == EXPECTED_BUNDLE_ID, "network.env bundle ID mismatch")
    require(env.get("USDB_CHAIN_ID") == str(EXPECTED_CHAIN_ID), "network.env chain ID mismatch")
    require(env.get("USDB_NETWORK_ID") == str(EXPECTED_CHAIN_ID), "network.env network ID mismatch")
    require(env.get("BTC_NETWORK") == "bitcoin", "network.env BTC network must be bitcoin")
    require(env.get("BTC_MIN_READY_HEIGHT") == str(index_origin_height), "network.env Bitcoin readiness height mismatch")
    require(env.get("BTC_MAX_TIP_AGE_SECS") == "7200", "network.env Bitcoin maximum tip age mismatch")
    require(env.get("BTC_MIN_CONNECTIONS") == "1", "network.env Bitcoin minimum connections mismatch")
    require(env.get("USDB_GENESIS_BLOCK_HEIGHT") == str(index_origin_height), "network.env origin mismatch")
    require(env.get("BTC_ACTIVATION_REGISTRY_ID") == EXPECTED_BTC_REGISTRY, "network.env registry mismatch")
    require(env.get("USDB_P2P_PORT") == "31303", "network.env P2P port mismatch")

    require(chain.get("schemaVersion") == 2, "chain bootstrap schema must be v2")
    require(chain.get("chainId") == EXPECTED_CHAIN_ID, "chain bootstrap chain ID mismatch")
    chain_btc = chain.get("btcSource")
    require(isinstance(chain_btc, dict), "chain bootstrap btcSource is required")
    require(chain_btc.get("networkId") == "btc-mainnet", "chain bootstrap BTC network mismatch")
    require(chain_btc.get("indexOriginHeight") == index_origin_height, "chain bootstrap origin mismatch")
    activations = chain.get("usdbConsensus", {}).get("activations")
    require(isinstance(activations, list) and activations, "chain bootstrap activations are required")
    require(activations[0].get("block") == 0, "activation schedule must start at block 0")
    require(activations[0].get("btcActivationRegistryId") == EXPECTED_BTC_REGISTRY, "activation registry mismatch")
    versions = activations[0].get("versions")
    require(isinstance(versions, dict), "activation versions are required")
    require(versions.get("quotePolicyVersion") == 0, "testnet-v0 quote policy must be disabled")
    require(versions.get("auxPoolPolicyVersion") == 0, "testnet-v0 aux pool policy must be disabled")

    bootstrap_admin = validate_bootstrap_admin(
        network["deployment_tier"],
        chain.get("bootstrapAdmin", {}).get("address"),
    )
    require(bootstrap_admin == EXPECTED_BOOTSTRAP_ADMIN, "unexpected testnet-v0 bootstrap admin")

    require(source_dao.get("chainId") == EXPECTED_CHAIN_ID, "SourceDAO chain ID mismatch")
    require("rpcUrl" not in source_dao, "SourceDAO release config must not freeze an RPC URL")
    require("artifactsDir" not in source_dao, "SourceDAO release config must not freeze an artifacts path")
    predeploys = chain.get("predeploys", {})
    require(source_dao.get("daoAddress") == predeploys.get("dao", {}).get("address"), "DAO address mismatch")
    require(source_dao.get("dividendAddress") == predeploys.get("dividend", {}).get("address"), "Dividend address mismatch")
    require(
        source_dao.get("bootstrapAdminAddress") == bootstrap_admin,
        "bootstrap admin mismatch",
    )
    require(
        bootstrap_manifest.get("balance_history_snapshot_mode") == "none",
        "testnet-v0 default balance-history bootstrap mode must be none",
    )
    require(
        bootstrap_manifest.get("balance_history_snapshot_height") is None,
        "full-sync bootstrap must not declare a snapshot height",
    )

    config = genesis.get("config")
    require(isinstance(config, dict), "genesis config is required")
    require(config.get("chainId") == EXPECTED_CHAIN_ID, "genesis chainId mismatch")
    require(config.get("chainId_alt") == EXPECTED_CHAIN_ID, "genesis chainId_alt mismatch")
    genesis_usdb = config.get("usdb")
    require(isinstance(genesis_usdb, dict), "genesis config.usdb is required")
    require(genesis_usdb.get("btcNetworkId") == "btc-mainnet", "genesis BTC network mismatch")
    require(genesis_usdb.get("btcIndexOriginHeight") == index_origin_height, "genesis BTC origin mismatch")
    require(genesis_usdb.get("activations") == activations, "genesis activation schedule mismatch")

    alloc = genesis.get("alloc")
    require(isinstance(alloc, dict), "genesis alloc is required")
    admin_alloc = alloc.get(bootstrap_admin[2:].lower())
    require(isinstance(admin_alloc, dict), "genesis bootstrap admin allocation is missing")
    expected_admin_balance = chain.get("bootstrapAdmin", {}).get("balanceWei")
    require(
        isinstance(expected_admin_balance, str) and expected_admin_balance.isdecimal(),
        "bootstrap admin balanceWei must be decimal",
    )
    actual_admin_balance = admin_alloc.get("balance")
    try:
        parsed_admin_balance = int(actual_admin_balance, 0)
    except (TypeError, ValueError) as exc:
        raise ValueError("genesis bootstrap admin balance is invalid") from exc
    require(
        parsed_admin_balance == int(expected_admin_balance, 10),
        "genesis bootstrap admin balance mismatch",
    )
    for development_admin in KNOWN_DEVELOPMENT_BOOTSTRAP_ADMINS:
        require(
            development_admin[2:] not in alloc,
            "genesis alloc must not fund a known development bootstrap admin",
        )

    require(
        genesis_manifest.get("schema_version") == "usdb-genesis-manifest:v2",
        "unexpected genesis manifest schema",
    )
    require(genesis_manifest.get("network_bundle_id") == EXPECTED_BUNDLE_ID, "genesis manifest bundle mismatch")
    require(genesis_manifest.get("chain_id") == EXPECTED_CHAIN_ID, "genesis manifest chain ID mismatch")
    require(genesis_manifest.get("network_id") == EXPECTED_CHAIN_ID, "genesis manifest network ID mismatch")
    require(
        genesis_manifest.get("file_sha256") == sha256(bundle_dir / "artifacts/usdb-genesis.json"),
        "genesis manifest file hash mismatch",
    )
    require(
        isinstance(genesis_manifest.get("block_hash"), str)
        and re.fullmatch(r"0x[0-9a-f]{64}", genesis_manifest["block_hash"]) is not None,
        "invalid genesis manifest block hash",
    )
    require(
        genesis_manifest.get("bootstrap_config_sha256")
        == sha256(bundle_dir / "artifacts/usdb-chain-bootstrap-config.json"),
        "genesis manifest bootstrap config hash mismatch",
    )
    require(
        genesis_manifest.get("sourcedao_config_sha256")
        == sha256(bundle_dir / "artifacts/sourcedao-bootstrap-config.json"),
        "genesis manifest SourceDAO config hash mismatch",
    )

    validate_artifact_hashes(bundle_dir, network)
    template = read_env(bundle_dir / "node.env.example")
    data_root = Path(template.get("USDB_DATA_ROOT", ""))
    require(data_root.is_absolute(), "node.env.example USDB_DATA_ROOT must be absolute")
    network_identity = {
        "bundle_id": network["network_bundle_id"],
        "chain_id": network["chain_id"],
        "genesis_block_hash": genesis_manifest["block_hash"],
        "btc_network_id": btc_source["network_id"],
        "btc_index_origin_height": index_origin_height,
        "btc_activation_registry_id": btc_source["activation_registry_id"],
    }
    compatibility = build_runtime_compatibility(network_identity)
    require(
        template.get("USDB_DATA_LAYOUT") == DATA_LAYOUT_VERSION,
        "node.env.example data layout version mismatch",
    )
    require(
        template.get("USDB_RUNTIME_COMPATIBILITY_ID")
        == compatibility["compatibility_id"],
        "node.env.example runtime compatibility ID mismatch",
    )
    for key, expected_path in build_persistent_data_paths(
        data_root,
        network_identity,
        compatibility,
    ).items():
        require(
            Path(template.get(key, "")) == expected_path,
            f"node.env.example {key} mismatch",
        )
    require(
        Path(template.get("BTC_RPCAUTH_HOST_FILE", ""))
        == network_secure_dir(data_root, network["network_bundle_id"])
        / "bitcoin-mainnet-rpcauth",
        "node.env.example Bitcoin rpcauth path mismatch",
    )
    require(
        Path(template.get("BH_SNAPSHOT_HOST_DIR", ""))
        == snapshot_artifact_dir(data_root),
        "node.env.example snapshot artifact path mismatch",
    )
    require(
        template.get("USDB_FIREWALL_MODE") == "external",
        "node.env.example must default to externally managed host firewall",
    )
    return network


def validate_image_ref(name: str, value: str, expected_image: str) -> None:
    require(bool(value), f"{name} is required in node.env")
    lowered = value.lower()
    require(not lowered.endswith(":latest"), f"{name} must not use latest")
    require(not lowered.endswith(":local"), f"{name} must not use a local tag")
    require("replace" not in lowered, f"{name} still contains a placeholder")
    require(
        re.fullmatch(rf"{re.escape(expected_image)}@sha256:[0-9a-f]{{64}}", value) is not None,
        f"{name} must use the canonical GHCR digest reference",
    )


def validate_node_env(
    path: Path,
    network: dict[str, Any],
    require_runtime: bool,
    require_bitcoin_runtime: bool = False,
    expected_data_paths: dict[str, Path] | None = None,
    expected_compatibility_id: str | None = None,
) -> None:
    env = read_env(path)
    index_origin_height = network_index_origin_height(network)
    validate_image_ref(
        "USDB_SERVICES_IMAGE",
        env.get("USDB_SERVICES_IMAGE", ""),
        "ghcr.io/buckyos/usdb-services",
    )
    validate_image_ref(
        "USDB_CHAIN_IMAGE",
        env.get("USDB_CHAIN_IMAGE", ""),
        "ghcr.io/buckyos/usdb-chain",
    )
    validate_image_ref(
        "USDB_BITCOIN_IMAGE",
        env.get("USDB_BITCOIN_IMAGE", ""),
        "ghcr.io/buckyos/usdb-bitcoin-core",
    )

    auth_mode = env.get("BTC_AUTH_MODE", "userpass")
    require(auth_mode == "userpass", "testnet release Bitcoin RPC auth must use userpass")
    require(bool(env.get("BTC_RPC_USER")), "BTC_RPC_USER is required")
    require(bool(env.get("BTC_RPC_PASSWORD")), "BTC_RPC_PASSWORD is required")
    require(env.get("BTC_RPC_URL") == "http://btc-node:8332", "BTC_RPC_URL must use the private btc-node endpoint")
    data_root = Path(env.get("USDB_DATA_ROOT", ""))
    require(data_root.is_absolute(), "USDB_DATA_ROOT must be an absolute path")
    layout_version = env.get("USDB_DATA_LAYOUT", "")
    if not layout_version:
        expected_paths = {
            key: data_root / relative
            for key, relative in LEGACY_PERSISTENT_DATA_PATHS.items()
        }
    else:
        require(
            layout_version == DATA_LAYOUT_VERSION,
            f"unsupported USDB_DATA_LAYOUT: {layout_version}",
        )
        compatibility_id = env.get("USDB_RUNTIME_COMPATIBILITY_ID", "")
        require(
            re.fullmatch(r"[0-9a-f]{64}", compatibility_id) is not None,
            "USDB_RUNTIME_COMPATIBILITY_ID must be lowercase SHA-256",
        )
        if expected_compatibility_id is not None:
            require(
                compatibility_id == expected_compatibility_id,
                "node runtime compatibility ID does not match the selected release",
            )
        expected_paths = expected_data_paths
    for key in PERSISTENT_DATA_SERVICES:
        path_value = Path(env.get(key, ""))
        require(path_value.is_absolute(), f"{key} must be an absolute path")
        if expected_paths is not None:
            require(
                path_value == expected_paths[key],
                f"{key} must match the selected data layout and runtime contract",
            )
        elif layout_version == DATA_LAYOUT_VERSION:
            require(
                path_value.is_relative_to(data_root),
                f"{key} must be below USDB_DATA_ROOT",
            )
        if require_runtime:
            require(path_value.is_dir(), f"persistent data directory does not exist: {path_value}")
    if layout_version == DATA_LAYOUT_VERSION and expected_paths is None:
        btc_network_id = network["btc_source"]["network_id"]
        bundle_id = network["network_bundle_id"]
        require(
            Path(env["BTC_NODE_DATA_HOST_DIR"])
            == data_root / "datasets/bitcoin" / btc_network_id,
            "BTC_NODE_DATA_HOST_DIR has invalid source-dataset scope",
        )
        balance_path = Path(env["BH_DATA_HOST_DIR"])
        require(
            balance_path.parent == data_root / "datasets/balance-history" / btc_network_id
            and re.fullmatch(r"[0-9a-f]{64}", balance_path.name) is not None,
            "BH_DATA_HOST_DIR has invalid source-dataset scope",
        )
        indexer_path = Path(env["USDB_INDEXER_DATA_HOST_DIR"])
        require(
            indexer_path.parent == data_root / "datasets/usdb-indexer"
            and re.fullmatch(r"[0-9a-f]{64}", indexer_path.name) is not None,
            "USDB_INDEXER_DATA_HOST_DIR has invalid derivation-dataset scope",
        )
        require(
            Path(env["USDB_CHAIN_DATA_HOST_DIR"])
            == data_root / "networks" / bundle_id / "usdb-chain",
            "USDB_CHAIN_DATA_HOST_DIR has invalid network scope",
        )
        require(
            Path(env["CONTROL_PLANE_DATA_HOST_DIR"])
            == data_root / "networks" / bundle_id / "control-plane",
            "CONTROL_PLANE_DATA_HOST_DIR has invalid network scope",
        )
    require(
        env.get("BTC_P2P_BIND_ADDRESS") in {"127.0.0.1", "0.0.0.0"},
        "BTC_P2P_BIND_ADDRESS must be explicit loopback-only or public IPv4",
    )
    require(env.get("BTC_P2P_BIND_PORT", "8333") == "8333", "Bitcoin mainnet P2P bind port must be 8333")
    bitcoin_profile = env.get(
        "BTC_RESOURCE_PROFILE",
        DEFAULT_BITCOIN_RESOURCE_PROFILE,
    )
    require(
        bitcoin_profile in BITCOIN_RESOURCE_PROFILES,
        "BTC_RESOURCE_PROFILE must be balanced-32g or performance-64g",
    )
    expected_bitcoin_resources = BITCOIN_RESOURCE_PROFILES[bitcoin_profile]
    require(
        env.get("BTC_MEMORY_LIMIT") == expected_bitcoin_resources["memory_limit"],
        "BTC_MEMORY_LIMIT does not match BTC_RESOURCE_PROFILE",
    )
    require(
        env.get("BTC_DBCACHE_MB") == expected_bitcoin_resources["dbcache_mb"],
        "BTC_DBCACHE_MB does not match BTC_RESOURCE_PROFILE",
    )
    # Configurations created before this field existed had mandatory managed
    # UFW behavior, so absence retains that fail-closed meaning during upgrade.
    firewall_mode = env.get("USDB_FIREWALL_MODE", "managed")
    require(
        firewall_mode in SUPPORTED_FIREWALL_MODES,
        "USDB_FIREWALL_MODE must be external or managed",
    )
    ssh_port = env.get("USDB_OPERATOR_SSH_PORT", "")
    require(re.fullmatch(r"[0-9]+", ssh_port) is not None, "USDB_OPERATOR_SSH_PORT must be a decimal port")
    require(1 <= int(ssh_port) <= 65535, "USDB_OPERATOR_SSH_PORT must be between 1 and 65535")

    role = env.get("USDB_NODE_ROLE", "full")
    require(role in {"bootnode", "full", "miner"}, "unsupported USDB_NODE_ROLE")
    if role == "miner":
        miner_address = env.get("USDB_MINER_ADDRESS", "")
        require(bool(miner_address), "miner role requires USDB_MINER_ADDRESS")
        require(
            re.fullmatch(r"0x[0-9a-fA-F]{40}", miner_address) is not None
            and int(miner_address[2:], 16) != 0,
            "USDB_MINER_ADDRESS must be a non-zero EVM address",
        )

    require(env.get("USDB_P2P_BIND_ADDRESS") == "0.0.0.0", "testnet-v0 P2P must bind public IPv4")
    require(env.get("USDB_P2P_BIND_PORT", "31303") == "31303", "testnet-v0 P2P bind port must be 31303")
    for key in (
        "USDB_HTTP_BIND_ADDRESS",
        "USDB_WS_BIND_ADDRESS",
        "BH_BIND_ADDRESS",
        "USDB_INDEXER_BIND_ADDRESS",
        "CONTROL_PLANE_BIND_ADDRESS",
    ):
        require(env.get(key) == "127.0.0.1", f"{key} must be loopback-only")
    if require_bitcoin_runtime:
        data_dir = Path(env.get("BTC_NODE_DATA_HOST_DIR", ""))
        require(data_dir.is_dir(), f"Bitcoin data directory does not exist: {data_dir}")
        rpcauth_file = Path(env.get("BTC_RPCAUTH_HOST_FILE", ""))
        require(rpcauth_file.is_absolute(), "BTC_RPCAUTH_HOST_FILE must be an absolute path")
        require(rpcauth_file.is_file(), f"Bitcoin rpcauth file does not exist: {rpcauth_file}")
        require(rpcauth_file.stat().st_mode & 0o077 == 0, "Bitcoin rpcauth file must not be group/world accessible")
        rpcauth = rpcauth_file.read_text(encoding="utf-8").strip()
        match = re.fullmatch(r"([A-Za-z0-9._-]+):([0-9a-fA-F]{32})\$([0-9a-fA-F]{64})", rpcauth)
        require(match is not None, "Bitcoin rpcauth file contains an invalid value")
        assert match is not None
        username, salt, expected_hmac = match.groups()
        require(username == env.get("BTC_RPC_USER"), "Bitcoin rpcauth username does not match BTC_RPC_USER")
        actual_hmac = hmac.new(
            salt.encode("utf-8"),
            env["BTC_RPC_PASSWORD"].encode("utf-8"),
            "sha256",
        ).hexdigest()
        require(hmac.compare_digest(actual_hmac, expected_hmac.lower()), "Bitcoin rpcauth does not match RPC password")
    snapshot_mode = env.get("SNAPSHOT_MODE", "none")
    require(
        snapshot_mode in {"none", "balance-history", "paired-checkpoint"},
        "unsupported SNAPSHOT_MODE",
    )
    snapshot_dir = Path(env.get("BH_SNAPSHOT_HOST_DIR", ""))
    require(snapshot_dir.is_absolute(), "BH_SNAPSHOT_HOST_DIR must be an absolute path")
    snapshot_file = env.get("BH_SNAPSHOT_FILE", "")
    snapshot_manifest = env.get("BH_SNAPSHOT_MANIFEST", "")
    checkpoint_manifest = env.get("USDB_INDEXER_CHECKPOINT_MANIFEST", "")
    if snapshot_mode == "none":
        require(not snapshot_file, "SNAPSHOT_MODE=none requires empty BH_SNAPSHOT_FILE")
        require(not snapshot_manifest, "SNAPSHOT_MODE=none requires empty BH_SNAPSHOT_MANIFEST")
        require(
            not checkpoint_manifest,
            "SNAPSHOT_MODE=none requires empty USDB_INDEXER_CHECKPOINT_MANIFEST",
        )
    else:
        require(
            env.get("BH_SNAPSHOT_TRUST_MODE", "signed") == "signed",
            "release snapshot mode must use signed trust",
        )
        snapshot_file_path = snapshot_container_path(snapshot_file, "BH_SNAPSHOT_FILE", ".db")
        snapshot_manifest_path = snapshot_container_path(
            snapshot_manifest,
            "BH_SNAPSHOT_MANIFEST",
            ".manifest.json",
        )
        require(
            snapshot_manifest_path == snapshot_file_path.with_suffix(".manifest.json"),
            "balance-history snapshot file and manifest basenames must match",
        )
        if snapshot_mode == "balance-history":
            require(
                not checkpoint_manifest,
                "SNAPSHOT_MODE=balance-history requires empty USDB_INDEXER_CHECKPOINT_MANIFEST",
            )
        else:
            checkpoint_manifest_path = checkpoint_container_path(checkpoint_manifest)
    if require_runtime and snapshot_mode in {"balance-history", "paired-checkpoint"}:
        balance_history_runtime_manifest = validate_runtime_snapshot(
            snapshot_dir,
            snapshot_file_path,
            snapshot_manifest_path,
            index_origin_height,
            env.get("BTC_NETWORK", "bitcoin"),
            allow_above_origin=snapshot_mode == "paired-checkpoint",
        )
        if snapshot_mode == "paired-checkpoint":
            validate_runtime_checkpoint(
                snapshot_dir,
                checkpoint_manifest_path,
                snapshot_dir / snapshot_manifest_path.name,
                balance_history_runtime_manifest,
                network,
                index_origin_height,
                env.get("BTC_NETWORK", "bitcoin"),
            )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--node-env", type=Path)
    parser.add_argument("--require-runtime", action="store_true")
    parser.add_argument("--require-bitcoin-runtime", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    network = validate_network_bundle(args.bundle_dir.resolve())
    if args.node_env is not None:
        validate_node_env(
            args.node_env.resolve(),
            network,
            args.require_runtime,
            args.require_bitcoin_runtime,
        )
    print(
        json.dumps(
            {
                "network_bundle_id": network["network_bundle_id"],
                "status": network["status"],
                "deployment_tier": network["deployment_tier"],
                "chain_id": network["chain_id"],
                "network_id": network["network_id"],
                "node_env_checked": args.node_env is not None,
                "runtime_artifacts_checked": args.require_runtime,
                "bitcoin_runtime_checked": args.require_bitcoin_runtime,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as exc:
        raise SystemExit(f"network bundle validation failed: {exc}") from exc
