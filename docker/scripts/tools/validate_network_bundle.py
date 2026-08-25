#!/usr/bin/env python3
"""Fail-closed validation for a USDB network bundle and optional node config."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import Any

EXPECTED_BUNDLE_ID = "usdb-testnet-v0"
EXPECTED_CHAIN_ID = 202608250
EXPECTED_BTC_REGISTRY = "cc47923f4cdff1875f89771d08e1b89fa22295c92bb816073c3271dc53c54c1c"
EXPECTED_ORIGIN = 963800


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

    require(network.get("schema_version") == "usdb-network-bundle:v1", "unexpected network bundle schema")
    require(network.get("network_bundle_id") == EXPECTED_BUNDLE_ID, "unexpected network bundle ID")
    require(network.get("status") == "development-resettable", "testnet-v0 must remain resettable")
    require(network.get("chain_id") == EXPECTED_CHAIN_ID, "unexpected testnet-v0 chain ID")
    require(network.get("network_id") == EXPECTED_CHAIN_ID, "unexpected testnet-v0 network ID")
    require(network.get("p2p_port") == 31303, "testnet-v0 P2P port must be 31303")

    btc_source = network.get("btc_source")
    require(isinstance(btc_source, dict), "network btc_source is required")
    require(btc_source.get("network_id") == "btc-mainnet", "testnet-v0 must consume BTC mainnet")
    require(btc_source.get("index_origin_height") == EXPECTED_ORIGIN, "unexpected BTC index origin")
    require(btc_source.get("activation_registry_id") == EXPECTED_BTC_REGISTRY, "unexpected BTC registry")

    require(env.get("USDB_NETWORK_BUNDLE_ID") == EXPECTED_BUNDLE_ID, "network.env bundle ID mismatch")
    require(env.get("USDB_CHAIN_ID") == str(EXPECTED_CHAIN_ID), "network.env chain ID mismatch")
    require(env.get("USDB_NETWORK_ID") == str(EXPECTED_CHAIN_ID), "network.env network ID mismatch")
    require(env.get("BTC_NETWORK") == "bitcoin", "network.env BTC network must be bitcoin")
    require(env.get("USDB_GENESIS_BLOCK_HEIGHT") == str(EXPECTED_ORIGIN), "network.env origin mismatch")
    require(env.get("BTC_ACTIVATION_REGISTRY_ID") == EXPECTED_BTC_REGISTRY, "network.env registry mismatch")
    require(env.get("USDB_P2P_PORT") == "31303", "network.env P2P port mismatch")

    require(chain.get("schemaVersion") == 2, "chain bootstrap schema must be v2")
    require(chain.get("chainId") == EXPECTED_CHAIN_ID, "chain bootstrap chain ID mismatch")
    chain_btc = chain.get("btcSource")
    require(isinstance(chain_btc, dict), "chain bootstrap btcSource is required")
    require(chain_btc.get("networkId") == "btc-mainnet", "chain bootstrap BTC network mismatch")
    require(chain_btc.get("indexOriginHeight") == EXPECTED_ORIGIN, "chain bootstrap origin mismatch")
    activations = chain.get("usdbConsensus", {}).get("activations")
    require(isinstance(activations, list) and activations, "chain bootstrap activations are required")
    require(activations[0].get("block") == 0, "activation schedule must start at block 0")
    require(activations[0].get("btcActivationRegistryId") == EXPECTED_BTC_REGISTRY, "activation registry mismatch")
    versions = activations[0].get("versions")
    require(isinstance(versions, dict), "activation versions are required")
    require(versions.get("quotePolicyVersion") == 0, "testnet-v0 quote policy must be disabled")
    require(versions.get("auxPoolPolicyVersion") == 0, "testnet-v0 aux pool policy must be disabled")

    require(source_dao.get("chainId") == EXPECTED_CHAIN_ID, "SourceDAO chain ID mismatch")
    predeploys = chain.get("predeploys", {})
    require(source_dao.get("daoAddress") == predeploys.get("dao", {}).get("address"), "DAO address mismatch")
    require(source_dao.get("dividendAddress") == predeploys.get("dividend", {}).get("address"), "Dividend address mismatch")
    require(
        source_dao.get("bootstrapAdminAddress") == chain.get("bootstrapAdmin", {}).get("address"),
        "bootstrap admin mismatch",
    )

    config = genesis.get("config")
    require(isinstance(config, dict), "genesis config is required")
    require(config.get("chainId") == EXPECTED_CHAIN_ID, "genesis chainId mismatch")
    require(config.get("chainId_alt") == EXPECTED_CHAIN_ID, "genesis chainId_alt mismatch")
    genesis_usdb = config.get("usdb")
    require(isinstance(genesis_usdb, dict), "genesis config.usdb is required")
    require(genesis_usdb.get("btcNetworkId") == "btc-mainnet", "genesis BTC network mismatch")
    require(genesis_usdb.get("btcIndexOriginHeight") == EXPECTED_ORIGIN, "genesis BTC origin mismatch")
    require(genesis_usdb.get("activations") == activations, "genesis activation schedule mismatch")

    require(genesis_manifest.get("network_bundle_id") == EXPECTED_BUNDLE_ID, "genesis manifest bundle mismatch")
    require(genesis_manifest.get("chain_id") == EXPECTED_CHAIN_ID, "genesis manifest chain ID mismatch")
    require(genesis_manifest.get("network_id") == EXPECTED_CHAIN_ID, "genesis manifest network ID mismatch")
    require(
        genesis_manifest.get("file_sha256") == sha256(bundle_dir / "artifacts/usdb-genesis.json"),
        "genesis manifest file hash mismatch",
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
    return network


def validate_image_ref(name: str, value: str) -> None:
    require(bool(value), f"{name} is required in node.env")
    lowered = value.lower()
    require(not lowered.endswith(":latest"), f"{name} must not use latest")
    require(not lowered.endswith(":local"), f"{name} must not use a local tag")
    require("replace" not in lowered, f"{name} still contains a placeholder")


def validate_node_env(path: Path, require_runtime: bool) -> None:
    env = read_env(path)
    validate_image_ref("USDB_SERVICES_IMAGE", env.get("USDB_SERVICES_IMAGE", ""))
    validate_image_ref("USDB_CHAIN_IMAGE", env.get("USDB_CHAIN_IMAGE", ""))

    auth_mode = env.get("BTC_AUTH_MODE", "userpass")
    require(auth_mode in {"userpass", "cookie"}, "testnet node BTC auth must be userpass or cookie")
    if auth_mode == "userpass":
        require(bool(env.get("BTC_RPC_USER")), "BTC_RPC_USER is required")
        require(bool(env.get("BTC_RPC_PASSWORD")), "BTC_RPC_PASSWORD is required")

    role = env.get("USDB_NODE_ROLE", "full")
    require(role in {"bootnode", "full", "miner"}, "unsupported USDB_NODE_ROLE")
    if role == "miner":
        require(bool(env.get("USDB_MINER_ADDRESS")), "miner role requires USDB_MINER_ADDRESS")
        require(bool(env.get("USDB_PASS_ID")), "miner role requires USDB_PASS_ID")

    require(env.get("USDB_P2P_BIND_PORT", "31303") == "31303", "testnet-v0 P2P bind port must be 31303")
    if require_runtime:
        snapshot_dir = Path(env.get("BH_SNAPSHOT_HOST_DIR", ""))
        require(snapshot_dir.is_dir(), f"snapshot host directory does not exist: {snapshot_dir}")
        for name in (
            "snapshot_963800.db",
            "snapshot_963800.manifest.json",
            "snapshot_963800.manifest.sig",
        ):
            require((snapshot_dir / name).is_file(), f"required snapshot artifact is missing: {snapshot_dir / name}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--node-env", type=Path)
    parser.add_argument("--require-runtime", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    network = validate_network_bundle(args.bundle_dir.resolve())
    if args.node_env is not None:
        validate_node_env(args.node_env.resolve(), args.require_runtime)
    print(
        json.dumps(
            {
                "network_bundle_id": network["network_bundle_id"],
                "status": network["status"],
                "chain_id": network["chain_id"],
                "network_id": network["network_id"],
                "node_env_checked": args.node_env is not None,
                "runtime_artifacts_checked": args.require_runtime,
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
