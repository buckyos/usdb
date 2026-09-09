#!/usr/bin/env python3
"""Accept persistent peer management without resetting data or upstream services."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE
import usdb_mining as MINING
import usdb_peers as PEERS
from common.mining import ADDRESS
from common.peers import PeerFixture
from common.enode import PUBLIC_KEY, V4, V6, DNS


class Interrupted(BaseException):
    pass


class PeerTests(unittest.TestCase):
    def test_address_normalization_preserves_one_nodes_multiple_endpoints(self):
        expanded = f"enode://{PUBLIC_KEY.upper()}@[2001:0DB8:0:0:0:0:0:1]:31303?discport=31303"
        self.assertEqual(PEERS.normalize_enode(expanded), V6)
        self.assertEqual(PEERS.normalize_enode(DNS.replace("seed.example.org", "SEED.EXAMPLE.ORG.")), DNS)
        self.assertEqual(PEERS.parse_seeds(f" {V4},{expanded},{V6},{DNS} "), [V4, V6, DNS])
        self.assertEqual(PEERS.normalize_enode(V4 + "?discport=31304"), V4 + "?discport=31304")

    def test_invalid_addresses_never_write_a_task(self):
        bad = ["enode://example", V4 + "/path", V4 + "?discport=0", V4 + "?discport=65536",
               V4.replace("31303", "0"), V4.replace("31303", "65536"), V4 + "#fragment",
               V4.replace(PUBLIC_KEY, "00" * 64), V4.replace("192.0.2.1", "0.0.0.0"),
               V6.replace("2001:db8::1", "fe80::1%eth0"), V4 + "\n", V4.replace("enode:", "http:")]
        with PeerFixture() as f:
            original = f.layout.node_env.read_bytes()
            for value in bad:
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, "INVALID_PEER_SOURCE"):
                    f.edit(enode=value)
            self.assertFalse(PEERS.state_path(f.layout).exists())
            self.assertEqual(f.layout.node_env.read_bytes(), original)
            self.assertEqual(f.calls, [])

    def test_request_can_queue_while_bootstrap_holds_the_node_lock(self):
        with PeerFixture() as f:
            original = f.layout.node_env.read_bytes()
            with NODE.node_operation_lock(f.layout, "up"):
                first = f.edit()
                f.edit(enode=V6)
                f.edit(enode=V6)
                f.edit("remove", V4)
            self.assertEqual(first["outcome"], "controller_submitted")
            self.assertEqual(PEERS.observe(f.layout)["configured"], [V6])
            self.assertEqual(PEERS.observe(f.layout)["applied_config"], [])
            self.assertEqual(f.layout.node_env.read_bytes(), original)
            self.assertEqual(f.calls, [])

    def test_apply_recreates_only_chain_and_survives_controller_restart(self):
        with PeerFixture() as f:
            before = MINING.binding(f.layout, f.env)
            secret = NODE.read_env(f.layout.node_env)["BTC_RPC_PASSWORD"]
            f.edit()
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(PEERS.read_state(f.layout)["phase"], "APPLIED")
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_BOOTNODES"], V4)
            self.assertEqual(f.runtime["environment"]["USDB_BOOTNODES"], V4)
            self.assertEqual(f.calls, [("run_testnet_runtime.sh", ("stop-chain",)),
                                       ("run_testnet_runtime.sh", ("recreate-chain",))])
            self.assertEqual(MINING.binding(f.layout, f.env), before)
            self.assertNotIn(secret, PEERS.state_path(f.layout).read_text())
            self.assertEqual(PEERS.state_path(f.layout).stat().st_mode & 0o777, 0o600)
            self.assertEqual(f.edit()["outcome"], "unchanged")
            self.assertEqual(f.container_number, 1)

    def test_stopped_chain_saves_seeds_without_starting_services(self):
        with PeerFixture() as f:
            f.runtime = {"state": "absent", "environment": {}, "argv": []}
            f.edit()
            with mock.patch.object(NODE, "up_node", side_effect=AssertionError("unexpected bootstrap")):
                self.assertEqual(NODE.run_bootstrap_controller(f.layout, sync_timeout_secs=1, pull=False), 0)
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_BOOTNODES"], V4)
            self.assertEqual(f.calls, [])

    def test_applied_miner_authorization_is_rebound_without_new_authority(self):
        with PeerFixture() as f:
            f.enable()
            self.assertEqual(f.run(), 0)
            original = MINING.read_state(f.layout)
            f.edit()
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            after = MINING.read_state(f.layout)
            self.assertEqual(after["operation_id"], original["operation_id"])
            self.assertEqual(after["first_node"], original["first_node"])
            self.assertEqual(after["plan"], original["plan"])
            self.assertEqual(after["authorization_config"]["USDB_BOOTNODES"], V4)
            MINING.validate_start(f.layout)
            self.assertTrue(MINING.runtime_matches(f.layout, NODE.read_env(f.layout.node_env), f.runtime))

    def test_pending_mining_finishes_before_queued_peer_change(self):
        with PeerFixture() as f:
            f.enable()
            f.edit()
            # Reattaching an existing mining intent remains available.
            self.assertEqual(f.enable()["outcome"], "controller_submitted")
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_BOOTNODES"], "")
            self.assertEqual(f.run(), 0)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(f.runtime["environment"]["USDB_NODE_ROLE"], "miner")

    def test_incomplete_seed_application_blocks_new_enable(self):
        with PeerFixture() as f:
            f.edit()
            with self.assertRaisesRegex(ValueError, "PEER_OPERATION_PENDING"):
                f.enable()
            self.assertFalse(MINING.state_path(f.layout).exists())

    def test_partial_config_write_recovers_receipt_and_keeps_identity(self):
        with PeerFixture() as f:
            f.enable()
            f.run()
            f.edit()
            original_write = NODE._atomic_write_private
            def interrupt(path, content):
                original_write(path, content)
                if path == f.layout.node_env:
                    raise Interrupted()
            with mock.patch.object(NODE, "_atomic_write_private", side_effect=interrupt):
                with self.assertRaises(Interrupted):
                    f.run_peers()
            self.assertEqual(PEERS.read_state(f.layout)["phase"], "CONFIGURING")
            with self.assertRaisesRegex(ValueError, "MINING_AUTHORIZATION_REQUIRED"):
                MINING.validate_start(f.layout)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            MINING.validate_start(f.layout)

    def test_external_seed_drift_is_not_overwritten(self):
        with PeerFixture() as f:
            f.edit()
            f.update_env(USDB_BOOTNODES=V6)
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertIn("PEER_CONFIG_CHANGED", PEERS.read_state(f.layout)["error"])
            self.assertEqual(f.calls, [])

    def test_replaced_mining_receipt_is_not_overwritten_after_partial_write(self):
        with PeerFixture() as f:
            f.enable()
            f.run()
            f.edit()
            original = NODE._atomic_write_private
            def interrupt(path, content):
                if path == f.layout.node_env:
                    raise Interrupted()
                original(path, content)
            with mock.patch.object(NODE, "_atomic_write_private", side_effect=interrupt):
                with self.assertRaises(Interrupted):
                    f.run_peers()
            receipt = MINING.read_state(f.layout)
            receipt["operation_id"] = "replacement-operation"
            MINING.write_state(f.layout, receipt)
            saved = MINING.state_path(f.layout).read_bytes()
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertIn("MINING_OPERATION_CHANGED", PEERS.read_state(f.layout)["error"])
            self.assertEqual(MINING.state_path(f.layout).read_bytes(), saved)

    def test_failed_target_is_visible_and_explicit_apply_can_retry(self):
        with PeerFixture() as f:
            f.edit()
            f.run_peers()
            f.runtime.update(state="restarting", exit_code=1, argv=[])
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(PEERS.observe(f.layout, connected=True)["state"], "BLOCKED")
            self.assertEqual(NODE._chain_component(f.layout, f.env, f.runtime)["state"], "BLOCKED")
            self.assertEqual(f.container_number, 1)
            PEERS.submit(f.layout, "apply")
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(f.container_number, 2)

    def test_actual_geth_seed_arguments_are_required_for_applied(self):
        with PeerFixture() as f:
            f.edit()
            f.run_peers()
            f.runtime["argv"][f.runtime["argv"].index("--bootnodes") + 1] = ""
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertIn("CHAIN_ARGUMENT_MISMATCH", PEERS.read_state(f.layout)["error"])

    def test_down_preserves_seeds_without_resuming_bootstrap_or_chain(self):
        for partial in (False, True):
            with self.subTest(partial=partial), PeerFixture() as f:
                f.edit()
                PEERS.request_bootstrap(f.layout)
                if partial:
                    f.run_peers()
                # Model an explicit down after the controller has been stopped.
                f.runtime = {"state": "absent", "environment": {}, "argv": []}
                PEERS.pause_bootstrap(f.layout)
                calls = deepcopy(f.calls)
                with mock.patch.object(NODE, "up_node", side_effect=AssertionError("unexpected bootstrap")):
                    self.assertEqual(NODE.run_bootstrap_controller(f.layout, sync_timeout_secs=1, pull=False), 0)
                self.assertEqual(f.calls, calls)
                self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_BOOTNODES"], V4)

    def test_old_applied_bootstrap_intent_is_not_inherited_by_new_edit(self):
        with PeerFixture() as f:
            f.edit()
            PEERS.request_bootstrap(f.layout)
            f.run_peers()
            f.run_peers()
            f.edit(enode=V6)
            self.assertFalse(PEERS.read_state(f.layout)["resume_bootstrap"])

    def test_status_distinguishes_rpc_timeout_from_wrong_network(self):
        with PeerFixture() as f:
            for error, expected in (("RPC timeout", "WAITING"), ("CHAIN_IDENTITY_MISMATCH: wrong genesis", "BLOCKED")):
                with mock.patch.object(MINING, "chain_view", side_effect=ValueError(error)):
                    self.assertEqual(PEERS.observe(f.layout, connected=True)["state"], expected)

    def test_last_joiner_miner_seed_cannot_be_removed(self):
        with PeerFixture() as f:
            f.update_env(USDB_BOOTNODES=V4)
            f.adopt()
            f.chain.update(peers=1, height=50)
            MINING.submit(f.layout, address=ADDRESS, yes=True)
            f.run()
            f.edit("remove", V4)
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertIn("LAST_MINER_SEED", PEERS.read_state(f.layout)["error"])
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_BOOTNODES"], V4)

    def test_disable_cancels_mining_but_retains_peer_intent(self):
        with PeerFixture() as f:
            f.enable()
            f.run()
            f.edit()
            f.run_peers()
            MINING.submit(f.layout, disable=True, yes=True)
            self.assertEqual(PEERS.read_state(f.layout)["phase"], "QUEUED")
            self.assertEqual(f.run(), 0)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(f.runtime["environment"]["USDB_NODE_ROLE"], "full")
            self.assertEqual(f.runtime["environment"]["USDB_BOOTNODES"], V4)

    def test_controller_bootstrap_completion_does_not_drop_new_peer_intent(self):
        for outcome, code in (("ready", 0), ("manual_action_required", 1)):
            with self.subTest(outcome=outcome), PeerFixture() as f:
                def finish(*args, **kwargs):
                    f.edit()
                    return {"outcome": outcome}, code
                with mock.patch.object(NODE, "up_node", side_effect=finish), mock.patch.object(NODE, "print_up_result"):
                    self.assertEqual(NODE.run_bootstrap_controller(f.layout, sync_timeout_secs=1, pull=False), 1)

    def test_membership_is_waiting_until_seed_and_connection_exist(self):
        with PeerFixture() as f:
            env = NODE.read_env(f.layout.node_env)
            args = {"syncing": False, "peer_count": 0, "node_id": f.chain["node_id"]}
            self.assertEqual(PEERS.membership(f.layout, env, **args)[:2], ("WAITING", "SEED_REQUIRED"))
            env["USDB_BOOTNODES"] = V6
            self.assertEqual(PEERS.membership(f.layout, env, **args)[:2], ("WAITING", "WAITING_FOR_PEERS"))
            args["peer_count"] = 1
            self.assertEqual(PEERS.membership(f.layout, env, **args)[:2], ("READY", "CONNECTED"))
            args["syncing"] = {"currentBlock": "0x1"}
            self.assertEqual(PEERS.membership(f.layout, env, **args)[0], "SYNCING")

    def test_first_node_exception_is_bound_to_node_key_and_live_id(self):
        with PeerFixture() as f:
            f.enable()
            f.run()
            env = NODE.read_env(f.layout.node_env)
            args = {"syncing": False, "peer_count": 0, "node_id": f.chain["node_id"]}
            self.assertEqual(PEERS.membership(f.layout, env, **args)[:2], ("READY", "FIRST_NODE"))
            self.assertEqual(PEERS.membership(f.layout, env, **{**args, "syncing": {}})[0], "SYNCING")
            args["node_id"] = "ff" * 32
            self.assertEqual(PEERS.membership(f.layout, env, **args)[1], "SEED_REQUIRED")
            args["node_id"] = f.chain["node_id"]
            Path(env["USDB_CHAIN_DATA_HOST_DIR"], "geth/nodekey").write_text("different key")
            self.assertEqual(PEERS.membership(f.layout, env, **args)[1], "SEED_REQUIRED")

    def test_cli_parses_public_commands(self):
        for command in (["list"], ["status", "--watch"], ["add", V6], ["remove", V6], ["apply"]):
            args = NODE.build_parser().parse_args(["peers", *command, "--json"])
            self.assertEqual(args.command, "peers")
            self.assertTrue(args.json)


if __name__ == "__main__":
    unittest.main()
