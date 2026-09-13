"""P6.4 capability gates must not confuse foreground data with completed historical validation."""

import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
from check_bitcoin_block_data import assess_block_data, probe


class Core:
    def __init__(self):
        self.info = {"chain": "main", "blocks": 963810, "headers": 970000, "initialblockdownload": True, "pruned": False}
        self.hash = "ab" * 32
        self.block = {"hash": self.hash, "height": 963800, "confirmations": 11, "tx": [
            {"vin": [{"coinbase": "00"}]}, {"vin": [{"prevout": {"value": 0.00000001}}]},
        ]}
        self.calls = []

    def call(self, method):
        self.calls.append(method)
        if method == "getblockchaininfo":
            return self.info
        if method == "getchainstates":
            return {"chainstates": [{"blocks": 105439, "validated": True}, {"blocks": 963810, "validated": False}]}
        raise AssertionError(method)

    def get_block_hash(self, height):
        self.calls.append("getblockhash")
        return self.hash

    def call_result(self, method, params):
        self.calls.append(method)
        assert method == "getblock" and params == [self.hash, 3]
        return self.block


class BlockDataTests(unittest.TestCase):
    def test_foreground_data_does_not_wait_for_background_or_txindex(self):
        core = Core()
        result = probe(core, "main", 963800, core.hash)
        self.assertTrue(result["ready"])
        self.assertFalse(result["txindex_required"])
        self.assertEqual(result["chainstates"][0]["blocks"], 105439)
        self.assertNotIn("getindexinfo", core.calls)

    def test_missing_undo_blocks_even_when_foreground_height_is_ready(self):
        core = Core()
        core.block["tx"][1]["vin"][0].pop("prevout")
        result = probe(core, "main", 963800, core.hash)
        self.assertFalse(result["ready"])
        self.assertIn("undo/prevout unavailable", result["blockers"][0])

    def test_wrong_chain_pruning_missing_height_and_stale_hash_block(self):
        for key, value in [("chain", "regtest"), ("pruned", True), ("blocks", 963799)]:
            core = Core()
            core.info[key] = value
            self.assertFalse(probe(core, "main", 963800, core.hash)["ready"])
        core = Core()
        self.assertFalse(probe(core, "main", 963800, "00" * 32)["ready"])
        core.block["confirmations"] = -1
        self.assertFalse(probe(core, "main", 963800, core.hash)["ready"])

    def test_invalid_values_fail_and_zero_is_retained(self):
        core = Core()
        for value in [-1, True, "1", 0.000000001, float("nan"), float("inf"), 21000001]:
            block = copy.deepcopy(core.block)
            block["tx"][1]["vin"][0]["prevout"]["value"] = value
            with self.assertRaises(ValueError):
                assess_block_data(block, 963800, core.hash)
        core.block["tx"][1]["vin"][0]["prevout"]["value"] = 0
        self.assertEqual(assess_block_data(core.block, 963800, core.hash), 1)


if __name__ == "__main__":
    unittest.main()
