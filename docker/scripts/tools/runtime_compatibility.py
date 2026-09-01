#!/usr/bin/env python3
"""Build deterministic runtime storage contracts and host data paths."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "usdb-runtime-compatibility:v1"
DATA_LAYOUT_VERSION = "usdb-node-data-layout:v2"
DATASET_IDENTITY_VERSION = "usdb-dataset-identity:v1"
DATASET_IDENTITY_FILE = ".usdb-dataset-identity.json"

# These values are deployment compatibility contracts. Bump one only after the
# owning service has defined and tested reuse, migration, or rebuild behavior.
SERVICE_STORAGE_SCHEMAS = {
    "bitcoin_core": "bitcoin-core-datadir:v1",
    "balance_history": "balance-history-rocksdb-schema:v1",
    "usdb_indexer": "usdb-indexer-data:v1",
    "usdb_chain": "geth-blockchain-db:v8",
    "control_plane": "usdb-control-plane-data:v1",
}
BALANCE_HISTORY_DATA_MODEL = (
    "balance-history-data-model:bip30-generations-core-unspendable-v2"
)

PERSISTENT_DATA_SERVICES = {
    "BTC_NODE_DATA_HOST_DIR": "bitcoin_core",
    "BH_DATA_HOST_DIR": "balance_history",
    "USDB_INDEXER_DATA_HOST_DIR": "usdb_indexer",
    "USDB_CHAIN_DATA_HOST_DIR": "usdb_chain",
    "CONTROL_PLANE_DATA_HOST_DIR": "control_plane",
}

LEGACY_PERSISTENT_DATA_PATHS = {
    "BTC_NODE_DATA_HOST_DIR": "bitcoin/mainnet",
    "BH_DATA_HOST_DIR": "balance-history",
    "USDB_INDEXER_DATA_HOST_DIR": "usdb-indexer",
    "USDB_CHAIN_DATA_HOST_DIR": "usdb-chain",
    "CONTROL_PLANE_DATA_HOST_DIR": "control-plane",
}


def _canonical_json(value: Any) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


def _content_id(value: Any) -> str:
    return hashlib.sha256(_canonical_json(value)).hexdigest()


def _require_network_identity(network: dict[str, Any]) -> None:
    required = {
        "bundle_id",
        "chain_id",
        "genesis_block_hash",
        "btc_network_id",
        "btc_index_origin_height",
        "btc_activation_registry_id",
    }
    missing = required - set(network)
    if missing:
        raise ValueError(f"runtime compatibility network identity is missing: {sorted(missing)}")


def build_runtime_compatibility(network: dict[str, Any]) -> dict[str, Any]:
    """Build the exact service data contracts accepted by one release."""

    _require_network_identity(network)
    services = {
        "bitcoin_core": {
            "storage_schema": SERVICE_STORAGE_SCHEMAS["bitcoin_core"],
            "identity": {"btc_network_id": network["btc_network_id"]},
        },
        "balance_history": {
            "storage_schema": SERVICE_STORAGE_SCHEMAS["balance_history"],
            "data_model": BALANCE_HISTORY_DATA_MODEL,
            "identity": {"btc_network_id": network["btc_network_id"]},
        },
        "usdb_indexer": {
            "storage_schema": SERVICE_STORAGE_SCHEMAS["usdb_indexer"],
            "identity": {
                "btc_network_id": network["btc_network_id"],
                "btc_index_origin_height": network["btc_index_origin_height"],
                "btc_activation_registry_id": network["btc_activation_registry_id"],
            },
        },
        "usdb_chain": {
            "storage_schema": SERVICE_STORAGE_SCHEMAS["usdb_chain"],
            "identity": {
                "chain_id": network["chain_id"],
                "genesis_block_hash": network["genesis_block_hash"],
            },
        },
        "control_plane": {
            "storage_schema": SERVICE_STORAGE_SCHEMAS["control_plane"],
            "identity": {"network_bundle_id": network["bundle_id"]},
        },
    }
    unsigned = {
        "schema_version": SCHEMA_VERSION,
        "data_layout_version": DATA_LAYOUT_VERSION,
        "mismatch_action": "rebuild",
        "migration_support": "none",
        "services": services,
    }
    return {**unsigned, "compatibility_id": _content_id(unsigned)}


def service_contract_id(service: str, compatibility: dict[str, Any]) -> str:
    services = compatibility.get("services")
    if not isinstance(services, dict) or service not in services:
        raise ValueError(f"runtime compatibility is missing service contract: {service}")
    return _content_id({"service": service, "contract": services[service]})


def build_dataset_identity(service: str, compatibility: dict[str, Any]) -> dict[str, Any]:
    services = compatibility["services"]
    return {
        "schema_version": DATASET_IDENTITY_VERSION,
        "service": service,
        "service_contract_id": service_contract_id(service, compatibility),
        "contract": services[service],
    }


def build_persistent_data_paths(
    data_root: Path,
    network: dict[str, Any],
    compatibility: dict[str, Any],
) -> dict[str, Path]:
    """Separate source-derived datasets from network-generation state."""

    _require_network_identity(network)
    root = data_root.expanduser().resolve()
    btc_network = network["btc_network_id"]
    indexer_contract_id = service_contract_id("usdb_indexer", compatibility)
    balance_contract_id = service_contract_id("balance_history", compatibility)
    return {
        "BTC_NODE_DATA_HOST_DIR": root / "datasets/bitcoin" / btc_network,
        "BH_DATA_HOST_DIR": root
        / "datasets/balance-history"
        / btc_network
        / balance_contract_id,
        "USDB_INDEXER_DATA_HOST_DIR": root
        / "datasets/usdb-indexer"
        / indexer_contract_id,
        "USDB_CHAIN_DATA_HOST_DIR": root
        / "networks"
        / network["bundle_id"]
        / "usdb-chain",
        "CONTROL_PLANE_DATA_HOST_DIR": root
        / "networks"
        / network["bundle_id"]
        / "control-plane",
    }


def build_legacy_data_paths(data_root: Path) -> dict[str, Path]:
    root = data_root.expanduser().resolve()
    return {
        key: root / relative for key, relative in LEGACY_PERSISTENT_DATA_PATHS.items()
    }


def network_secure_dir(data_root: Path, bundle_id: str) -> Path:
    return data_root.expanduser().resolve() / "networks" / bundle_id / "secure"


def snapshot_artifact_dir(data_root: Path) -> Path:
    return data_root.expanduser().resolve() / "artifacts/balance-history"
