"""Guard V2 configuration generation, replay identity, and simulator source selection."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "src/btc/usdb-indexer/scripts"
sys.path.insert(0, str(SCRIPTS))
from compare_world_replay import prepare_configs
from regtest_world_simulator import RegtestWorldSimulator


class MatrixConfigTests(unittest.TestCase):
    def test_generated_standalone_configs_pin_v2_outside_the_json_heredoc(self):
        for script in ("regtest_e2e_smoke.sh", "regtest_live_ord_e2e.sh", "regtest_world_sim.sh"):
            with self.subTest(script=script), tempfile.TemporaryDirectory() as temp:
                source = (SCRIPTS / script).read_text()
                start = source.index("create_usdb_indexer_config() {")
                end = source.index("\n}\n\n", start) + 2
                env = dict(os.environ, REPO_ROOT=str(ROOT), USDB_INDEXER_ROOT=temp, BITCOIN_DIR=temp,
                           BTC_RPC_PORT="1", ORD_RPC_PORT="2", ORD_SERVER_PORT="2", BH_RPC_PORT="3",
                           USDB_INDEXER_RPC_PORT="4", INSCRIPTION_SOURCE="bitcoind",
                           INSCRIPTION_FIXTURE_FILE="", USDB_UPSTREAM_POLL_INTERVAL_MS="200")
                result = subprocess.run(["bash", "-euc", source[start:end] + "\ncreate_usdb_indexer_config"],
                                        env=env, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                config = json.loads((Path(temp) / "config.json").read_text())
                self.assertEqual(config["usdb"]["rules_scope"], "miner-pass-v2-fixture")
                catalog = Path(temp) / config["usdb"]["activation_registry_catalog_file"]
                self.assertEqual(json.loads(catalog.read_text())["current_registry_id"],
                                 config["usdb"]["activation_registry_id"])

    def test_fresh_replay_preserves_relative_and_absolute_catalog_bytes(self):
        for absolute in (False, True):
            with self.subTest(absolute=absolute), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                balance, usdb, scratch = (root / name for name in ("balance", "indexer", "replay"))
                for path in (balance, usdb, scratch):
                    path.mkdir()
                (balance / "config.toml").write_text('root_dir = "original"\n[rpc_server]\nport = 1234\n')
                catalog = usdb / "catalog.json"
                catalog.write_bytes((ROOT / "tests/fixtures/miner-pass-v2/catalog.json").read_bytes())
                config = dict(bitcoin=dict(network="regtest"), balance_history=dict(rpc_url="original"),
                              usdb=dict(genesis_block_height=1, inscription_source="bitcoind",
                                        activation_registry_catalog_file=str(catalog) if absolute else catalog.name))
                (usdb / "config.json").write_text(json.dumps(config))
                (usdb / "state-marker").write_text("must not copy state")
                _, replay = prepare_configs(balance, usdb, scratch, 4001, 4002)
                copied = json.loads((replay / "config.json").read_text())
                self.assertEqual((replay / copied["usdb"]["activation_registry_catalog_file"]).read_bytes(),
                                 catalog.read_bytes())
                self.assertFalse((replay / "state-marker").exists())
                self.assertEqual(json.loads((usdb / "config.json").read_text()), config)


class SimulatorSourceTests(unittest.TestCase):
    def test_missing_cardinal_owner_coin_blocks_mint_even_if_wallet_has_other_funds(self):
        sim = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        actor = SimpleNamespace(agent_id=1)
        sim.load_spendable_owner_output = Mock(return_value=None)
        self.assertFalse(sim.is_action_viable(actor, "standard_mint", {1}, 50))
        sim.load_spendable_owner_output.return_value = ("source", 1, 99_999)
        self.assertFalse(sim.is_action_viable(actor, "standard_mint", {1}, 51))
        sim.load_spendable_owner_output.return_value = ("source", 1, 100_000)
        self.assertTrue(sim.is_action_viable(actor, "standard_mint", {1}, 52))

    def test_source_cache_refreshes_after_height_change_and_is_discardable_on_reorg(self):
        sim = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        actor = SimpleNamespace(agent_id=1)
        sim.load_spendable_owner_output = Mock(side_effect=[("old", 0, 100_000), None, ("new", 1, 100_000)])
        self.assertEqual(sim.mint_source_output(actor, 50)[0], "old")
        self.assertEqual(sim.mint_source_output(actor, 50)[0], "old")
        self.assertIsNone(sim.mint_source_output(actor, 51))
        sim.mint_source_cache = {}
        self.assertEqual(sim.mint_source_output(actor, 51)[0], "new")


if __name__ == "__main__":
    unittest.main()
