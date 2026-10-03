"""Safety boundaries for the shared real-service MinerPass V2 fixture tools."""

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "miner_pass_regtest", Path(__file__).parent / "common/miner_pass_regtest.py"
)
HELPER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPER)


class MinerPassRegtestTest(unittest.TestCase):
    def test_select_only_confirmed_cardinal_coins_of_source(self):
        base = dict(txid="funding", vout=0, address="D", amount=1,
                    confirmations=1, spendable=True, safe=True)
        coins = [dict(base, txid="wrong-owner", address="E", amount=100),
                 dict(base, txid="pass", amount=90),
                 dict(base, txid="unconfirmed", confirmations=0, amount=80),
                 dict(base, txid="unsafe", safe=False, amount=70),
                 dict(base, txid="watch-only", spendable=False, amount=60), base]
        inscriptions = [dict(inscription="old-pass", location="pass:0:1000")]
        self.assertEqual(HELPER.select_satpoint(coins, inscriptions, "D"), "funding:0:0")
        with self.assertRaisesRegex(ValueError, "No confirmed cardinal funding coin"):
            HELPER.select_satpoint(coins[:-1], inscriptions, "D")
        with self.assertRaisesRegex(ValueError, "min_sats=100000"):
            HELPER.select_satpoint([dict(base, amount="0.00099999")], [], "D")

    def test_source_selection_is_deterministic(self):
        base = dict(vout=0, address="D", amount="0.00100000",
                    confirmations=1, spendable=True, safe=True)
        coins = [dict(base, txid="b"), dict(base, txid="a")]
        self.assertEqual(HELPER.select_satpoint(coins, [], "D"), "a:0:0")
        self.assertEqual(HELPER.select_satpoint(coins[::-1], [], "D"), "a:0:0")

    def test_indexer_pins_fixture_and_rejects_other_networks_without_writing(self):
        with tempfile.TemporaryDirectory() as temp:
            config = Path(temp) / "config.json"
            for network in ("mainnet", "testnet", "signet"):
                original = json.dumps(dict(bitcoin=dict(network=network), usdb={}))
                config.write_text(original)
                with self.assertRaisesRegex(ValueError, "requires regtest"):
                    HELPER.configure_indexer(config)
                self.assertEqual(config.read_text(), original)
                self.assertFalse((Path(temp) / "miner-pass-v2-catalog.json").exists())
            config.write_text(json.dumps(dict(bitcoin=dict(network="regtest"), usdb={})))
            HELPER.configure_indexer(config)
            usdb = json.loads(config.read_text())["usdb"]
            self.assertEqual(usdb["activation_registry_id"], HELPER.REGISTRY_ID)
            self.assertEqual(usdb["rules_scope"], HELPER.RULES_SCOPE)
            self.assertEqual((Path(temp) / usdb["activation_registry_catalog_file"]).read_bytes(),
                             HELPER.CATALOG.read_bytes())

    def test_genesis_requires_fresh_regtest_scope(self):
        data = dict(config=dict(usdb=dict(btcNetworkId="btc-regtest", btcIndexOriginHeight=1,
                    activations=[dict(block=0, btcActivationRegistryId="old")])))
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "genesis.json"
            for key, value in (("btcNetworkId", "btc-mainnet"), ("btcIndexOriginHeight", 0),
                               ("activations", [dict(block=1)])):
                bad = copy.deepcopy(data)
                bad["config"]["usdb"][key] = value
                original = json.dumps(bad)
                path.write_text(original)
                with self.assertRaises(ValueError):
                    HELPER.configure_genesis(path)
                self.assertEqual(path.read_text(), original)
            path.write_text(json.dumps(data))
            HELPER.configure_genesis(path)
            self.assertEqual(json.loads(path.read_text())["config"]["usdb"]["activations"][0]
                             ["btcActivationRegistryId"], HELPER.REGISTRY_ID)


if __name__ == "__main__":
    unittest.main()
