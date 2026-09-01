#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("runtime_compatibility.py")
SPEC = importlib.util.spec_from_file_location("runtime_compatibility", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNTIME = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNTIME)


def network_identity(bundle_id: str = "usdb-testnet-v0") -> dict:
    return {
        "bundle_id": bundle_id,
        "chain_id": 202608250,
        "genesis_block_hash": "0x" + "1" * 64,
        "btc_network_id": "btc-mainnet",
        "btc_index_origin_height": 963800,
        "btc_activation_registry_id": "2" * 64,
    }


class RuntimeCompatibilityTests(unittest.TestCase):
    def test_contract_and_paths_are_deterministic(self) -> None:
        network = network_identity()
        contract = RUNTIME.build_runtime_compatibility(network)
        self.assertEqual(contract, RUNTIME.build_runtime_compatibility(network))
        self.assertRegex(contract["compatibility_id"], r"^[0-9a-f]{64}$")

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = RUNTIME.build_persistent_data_paths(root, network, contract)
            self.assertEqual(
                paths["BTC_NODE_DATA_HOST_DIR"],
                root / "datasets/bitcoin/btc-mainnet",
            )
            self.assertEqual(
                paths["USDB_CHAIN_DATA_HOST_DIR"],
                root / "networks/usdb-testnet-v0/usdb-chain",
            )
            self.assertEqual(len(paths["BH_DATA_HOST_DIR"].name), 64)
            self.assertEqual(len(paths["USDB_INDEXER_DATA_HOST_DIR"].name), 64)

    def test_new_generation_reuses_only_matching_source_datasets(self) -> None:
        first = network_identity()
        second = copy.deepcopy(first)
        second["bundle_id"] = "usdb-testnet-v1"
        second["chain_id"] += 1
        second["genesis_block_hash"] = "0x" + "3" * 64
        first_contract = RUNTIME.build_runtime_compatibility(first)
        second_contract = RUNTIME.build_runtime_compatibility(second)
        root = Path("/data/usdb")
        first_paths = RUNTIME.build_persistent_data_paths(root, first, first_contract)
        second_paths = RUNTIME.build_persistent_data_paths(root, second, second_contract)

        for key in (
            "BTC_NODE_DATA_HOST_DIR",
            "BH_DATA_HOST_DIR",
            "USDB_INDEXER_DATA_HOST_DIR",
        ):
            self.assertEqual(first_paths[key], second_paths[key])
        for key in ("USDB_CHAIN_DATA_HOST_DIR", "CONTROL_PLANE_DATA_HOST_DIR"):
            self.assertNotEqual(first_paths[key], second_paths[key])

    def test_derivation_change_gets_a_distinct_indexer_dataset(self) -> None:
        first = network_identity()
        second = copy.deepcopy(first)
        second["btc_activation_registry_id"] = "4" * 64
        first_contract = RUNTIME.build_runtime_compatibility(first)
        second_contract = RUNTIME.build_runtime_compatibility(second)
        root = Path("/data/usdb")
        first_paths = RUNTIME.build_persistent_data_paths(root, first, first_contract)
        second_paths = RUNTIME.build_persistent_data_paths(root, second, second_contract)
        self.assertEqual(
            first_paths["BH_DATA_HOST_DIR"], second_paths["BH_DATA_HOST_DIR"]
        )
        self.assertNotEqual(
            first_paths["USDB_INDEXER_DATA_HOST_DIR"],
            second_paths["USDB_INDEXER_DATA_HOST_DIR"],
        )


if __name__ == "__main__":
    unittest.main()
