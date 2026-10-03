#!/usr/bin/env python3
"""Check the actual v1 release inputs, reset isolation and tag selection failures."""
import copy
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

import release_bundle
from release_manifest import build_network_identity
from runtime_compatibility import build_runtime_compatibility
from validate_network_bundle import read_env, read_json, validate_network_bundle

NETWORKS = Path(__file__).resolve().parents[2] / "networks"


class TestnetV1BundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="usdb-v1-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.v0 = NETWORKS / "testnet-v0"
        self.v1 = self.root / "testnet-v1"
        shutil.copytree(NETWORKS / "testnet-v1", self.v1)

    def write(self, relative, value):
        (self.v1 / relative).write_text(json.dumps(value, indent=2) + "\n")

    def test_frozen_business_parameters_are_preserved(self):
        old, new = validate_network_bundle(self.v0), validate_network_bundle(self.v1)
        self.assertEqual(new["chain_id"], 202610030)
        self.assertEqual(new["network_id"], 202610030)
        self.assertEqual(old["btc_source"]["index_origin_height"], new["btc_source"]["index_origin_height"])
        self.assertEqual(new["btc_source"]["rules_scope"], "usdb-testnet-v1")
        old_genesis = read_json(self.v0 / "artifacts/usdb-genesis.json")
        new_genesis = read_json(self.v1 / "artifacts/usdb-genesis.json")
        system_account = "0000000000000000000000000000000000001000"
        for address, account in old_genesis["alloc"].items():
            if address == system_account:
                self.assertNotEqual(account, new_genesis["alloc"][address])
            else:
                self.assertEqual(account, new_genesis["alloc"][address])
        self.assertEqual(old_genesis["difficulty"], new_genesis["difficulty"])
        before = read_json(self.v0 / "artifacts/sourcedao-bootstrap-config.json")
        after = read_json(self.v1 / "artifacts/sourcedao-bootstrap-config.json")
        before["chainId"] = after["chainId"]
        self.assertEqual(before, after)
        for role in ("sourcedao_contract_golden", "sourcedao_bootstrap_source", "sourcedao_bootstrap_imported", "snapshot_trusted_keys"):
            self.assertEqual(old["artifacts"][role], new["artifacts"][role])

    def test_same_bitcoin_data_but_distinct_network_state(self):
        old = build_network_identity(self.v0)
        new = build_network_identity(self.v1)
        self.assertNotEqual(old["genesis_block_hash"], new["genesis_block_hash"])
        old_contract = build_runtime_compatibility(old)["services"]
        new_contract = build_runtime_compatibility(new)["services"]
        for service in ("bitcoin_core", "balance_history"):
            self.assertEqual(old_contract[service], new_contract[service])
        for service in ("usdb_indexer", "usdb_chain", "control_plane"):
            self.assertNotEqual(old_contract[service], new_contract[service])
        old_env, new_env = read_env(self.v0 / "node.env.example"), read_env(self.v1 / "node.env.example")
        for key in ("USDB_INDEXER_DATA_HOST_DIR", "USDB_CHAIN_DATA_HOST_DIR", "CONTROL_PLANE_DATA_HOST_DIR"):
            self.assertNotEqual(old_env[key], new_env[key])

    def test_wrong_identity_is_rejected(self):
        original = read_json(self.v1 / "network.json")
        for key, value in (("chain_id", 202608250), ("network_id", 202608250), ("network_bundle_id", "usdb-testnet-v99")):
            with self.subTest(key=key):
                network = copy.deepcopy(original)
                network[key] = value
                self.write("network.json", network)
                with self.assertRaises(ValueError): validate_network_bundle(self.v1)
        self.write("network.json", original)
        for key, value in (("rules_scope", "legacy"), ("activation_registry_id", "a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224")):
            with self.subTest(key=key):
                network = copy.deepcopy(original)
                network["btc_source"][key] = value
                self.write("network.json", network)
                with self.assertRaises(ValueError): validate_network_bundle(self.v1)

    def test_catalog_mutation_fails_even_with_updated_inventory_hash(self):
        network = read_json(self.v1 / "network.json")
        artifact = network["artifacts"]["btc_activation_registry_catalog"]
        catalog = read_json(self.v1 / artifact["path"])
        catalog["registries"][0]["scope"]["rules_scope"] = "attacker-scope"
        self.write(artifact["path"], catalog)
        artifact["sha256"] = hashlib.sha256((self.v1 / artifact["path"]).read_bytes()).hexdigest()
        self.write("network.json", network)
        with self.assertRaisesRegex(ValueError, "frozen registry catalog"):
            validate_network_bundle(self.v1)

    def test_old_system_storage_is_rejected(self):
        genesis = read_json(self.v1 / "artifacts/usdb-genesis.json")
        genesis["alloc"] = read_json(self.v0 / "artifacts/usdb-genesis.json")["alloc"]
        self.write("artifacts/usdb-genesis.json", genesis)
        with self.assertRaisesRegex(ValueError, "genesis.*hash mismatch"):
            validate_network_bundle(self.v1)

    def test_tag_selection_never_falls_back_to_v0(self):
        self.assertEqual(release_bundle.source_for_release(NETWORKS, "usdb-testnet-v1-r1"), NETWORKS / "testnet-v1")
        self.assertEqual(release_bundle.source_for_release(NETWORKS, "usdb-testnet-v0-r25"), self.v0)
        for release_id, expected in (("usdb-testnet-v1-r1", "usdb-testnet-v0"), ("usdb-testnet-v99-r1", None), ("../../testnet-v0", None)):
            with self.subTest(release_id=release_id), self.assertRaises(ValueError):
                release_bundle.source_for_release(NETWORKS, release_id, expected)

    def test_signed_bitcoin_bootstrap_reused_with_new_state_paths(self):
        old = release_bundle.prepare(self.v0, self.root / "v0-release")
        new = release_bundle.prepare(self.v1, self.root / "v1-release")
        old_network, new_network = validate_network_bundle(old), validate_network_bundle(new)
        self.assertEqual(old_network["_native_bootstrap"], new_network["_native_bootstrap"])
        old_identity, new_identity = build_network_identity(old), build_network_identity(new)
        old_contract, new_contract = build_runtime_compatibility(old_identity), build_runtime_compatibility(new_identity)
        self.assertEqual(old_contract["services"]["balance_history"], new_contract["services"]["balance_history"])
        self.assertNotEqual(old_contract["services"]["usdb_indexer"], new_contract["services"]["usdb_indexer"])
        new_env = read_env(new / "node.env.example")
        self.assertEqual(new_env["SNAPSHOT_MODE"], "assumeutxo")
        self.assertIn("usdb-testnet-v1", new_env["USDB_CHAIN_DATA_HOST_DIR"])
        selection = read_json(self.v1 / "release-bootstrap.json")
        selection["source_network_sha256"] = "0" * 64
        self.write("release-bootstrap.json", selection)
        with self.assertRaisesRegex(ValueError, "Base network changed"):
            release_bundle.prepare(self.v1, self.root / "bad-release")


if __name__ == "__main__":
    unittest.main()
