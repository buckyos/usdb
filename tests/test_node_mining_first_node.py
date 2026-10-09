"""Genesis cold start with connected peers, durable authority and execution races."""

from contextlib import redirect_stderr
from copy import deepcopy
import io
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as node
import usdb_mining as mining
import usdb_peers as peers
from common.mining import ADDRESS, SEED, MiningFixture, genesis_peer


class Interrupted(BaseException):
    """Interrupt a durable phase without invoking failure rollback."""


class FirstNodeTests(unittest.TestCase):
    def test_explicit_genesis_start_allows_connected_peers_and_preserves_seeds(self):
        for seed in ("", SEED):
            for count in (0, 1, 2):
                with self.subTest(seed=bool(seed), count=count), MiningFixture() as f:
                    f.update_env(USDB_BOOTNODES=seed)
                    f.chain["peers"] = count
                    original = f.layout.node_env.read_bytes()
                    plan = mining.preflight(f.layout, ADDRESS, first_node=True)
                    self.assertEqual(plan["peer"]["declaration"], "new")
                    self.assertEqual(plan["peer"]["genesis_peer_count"], count)
                    self.assertEqual(f.layout.node_env.read_bytes(), original)
                    self.assertFalse(mining.state_path(f.layout).exists())
                    output = io.StringIO()
                    mining.print_report(plan, output=output)
                    self.assertIn(f"genesis peers={count}", output.getvalue())
                    self.assertIn("network initializer must confirm", output.getvalue())
                    with redirect_stderr(io.StringIO()):
                        f.enable()
                        self.assertEqual(f.run(), 0)
                    self.assertEqual(node.read_env(f.layout.node_env)["USDB_BOOTNODES"], seed)
                    self.assertEqual(mining.read_state(f.layout)["first_node"]["node_id"], f.chain["node_id"])
                    self.assertEqual(f.calls, [("run_testnet_runtime.sh", ("stop-chain",)),
                                               ("run_testnet_runtime.sh", ("recreate-chain",))])

    def test_any_non_genesis_peer_blocks_declaration_with_context(self):
        with MiningFixture() as f:
            genesis = f.layout.network_identity["genesis_block_hash"]
            f.chain["peers"] = 2
            f.peers = [genesis_peer(genesis), genesis_peer("0x" + "12" * 32, "22" * 32)]
            before = f.layout.node_env.read_bytes()
            with self.assertRaisesRegex(ValueError, "FIRST_NODE_CONFLICT.*peer=" + "22" * 32) as error:
                f.enable()
            self.assertIn("expected_genesis=" + genesis, str(error.exception))
            self.assertEqual(f.layout.node_env.read_bytes(), before)
            self.assertFalse(mining.state_path(f.layout).exists())
            self.assertEqual(f.calls, [])

    def test_unknown_or_invalid_peer_evidence_never_authorizes_cold_start(self):
        cases = [None, {}, "invalid", [None], [{}], [{"protocols": "invalid"}],
                 [{"protocols": {"eth": "handshake"}}], [{"protocols": {"snap": {"version": 1}}}],
                 [{"protocols": {"eth": {"version": True, "head": "0x" + "ab" * 32}}}],
                 [{"protocols": {"eth": {"version": 67, "head": "bad\nPRIVATE"}}}]]
        for value in cases:
            with self.subTest(value=value), MiningFixture() as f:
                f.chain["peers"] = 1
                original_rpc = f.rpc
                with mock.patch.object(mining, "rpc", side_effect=lambda layout, method, *args, **kw:
                                       value if method == "admin_peers" else original_rpc(layout, method, *args, **kw)):
                    with self.assertRaisesRegex(ValueError, "FIRST_NODE_PEERS_UNCONFIRMED") as error:
                        f.enable()
                self.assertNotIn("PRIVATE", str(error.exception))
                self.assertEqual(f.calls, [])
                self.assertFalse(mining.state_path(f.layout).exists())

    def test_rpc_failure_and_changing_peer_inventory_request_retry(self):
        with MiningFixture() as f:
            f.fail_rpc = "admin_peers"
            with self.assertRaisesRegex(ValueError, "FIRST_NODE_PEERS_UNCONFIRMED.*admin_peers failed"):
                f.enable()
        for count, observed in ((1, 0), (0, 1)):
            with self.subTest(count=count, observed=observed), MiningFixture() as f:
                f.chain["peers"] = count
                f.peers = [genesis_peer(f.layout.network_identity["genesis_block_hash"])] * observed
                with self.assertRaisesRegex(ValueError, f"peer inventory changed.*before={count}, observed={observed}"):
                    f.enable()
                self.assertEqual(f.calls, [])

    def test_local_progress_identity_and_connection_changes_during_peer_probe_are_rejected(self):
        for changes, code in ((dict(height=1), "FIRST_NODE_CONFLICT"),
                              (dict(syncing={"currentBlock": "0x0"}), "FIRST_NODE_CONFLICT"),
                              (dict(peers=1), "FIRST_NODE_PEERS_UNCONFIRMED"),
                              (dict(node_id="ff" * 32), "NODE_IDENTITY_CHANGED")):
            with self.subTest(changes=changes), MiningFixture() as f:
                f.after_rpc = lambda method: f.chain.update(changes) if method == "admin_peers" else None
                with self.assertRaisesRegex(ValueError, code):
                    f.enable()
                self.assertEqual(f.calls, [])

    def test_connected_peers_do_not_bypass_pass_or_readiness_checks(self):
        for mutation, code in (("pass", "INELIGIBLE_CANDIDATE"), ("readiness", "INDEXER_NOT_READY")):
            with self.subTest(mutation=mutation), MiningFixture() as f:
                f.chain["peers"] = 1
                if mutation == "pass":
                    f.candidate["pass"]["state"] = "invalid"
                else:
                    f.ready["consensus_ready"] = False
                with self.assertRaisesRegex(ValueError, code):
                    f.enable()
                self.assertEqual(f.calls, [])

    def test_peer_first_block_after_queue_is_rejected_before_any_runtime_change(self):
        with MiningFixture() as f, redirect_stderr(io.StringIO()):
            f.chain["peers"] = 1
            f.enable()
            f.peers = [genesis_peer("0x" + "12" * 32)]
            self.assertEqual(f.run(), node.CONTROLLER_MANUAL_EXIT_CODE)
            result = mining.read_state(f.layout)
            self.assertEqual((result["phase"], result["rollback"]), ("FAILED", "not_needed"))
            self.assertIsNone(result["first_node"])
            self.assertEqual(f.calls, [])
            mining.submit(f.layout, disable=True, yes=True)
            self.assertIsNone(mining.read_state(f.layout)["first_node"])

    def test_first_block_during_slow_eligibility_check_is_rechecked_before_stop(self):
        with MiningFixture() as f, redirect_stderr(io.StringIO()):
            f.enable()
            before = f.layout.node_env.read_bytes()
            f.after_rpc = lambda method: f.chain.update(height=1) if method == "resolve_miner_candidate" else None
            self.assertEqual(f.run(), node.CONTROLLER_MANUAL_EXIT_CODE)
            result = mining.read_state(f.layout)
            self.assertIn("height=1", result["error"])
            self.assertEqual(result["rollback"], "not_needed")
            self.assertEqual(f.layout.node_env.read_bytes(), before)
            self.assertEqual(f.calls, [])
            with self.assertRaisesRegex(ValueError, "FIRST_NODE_CONFLICT"):
                f.enable()

    def test_interrupted_stopping_rechecks_peers_before_touching_running_full_node(self):
        with MiningFixture() as f, redirect_stderr(io.StringIO()):
            f.chain["peers"] = 1
            f.enable()
            original_phase = mining._phase
            def interrupt(layout, operation, phase, **fields):
                original_phase(layout, operation, phase, **fields)
                if phase == "STOPPING":
                    raise Interrupted()
            with mock.patch.object(mining, "_phase", side_effect=interrupt), self.assertRaises(Interrupted):
                f.run()
            f.peers = [genesis_peer("0x" + "12" * 32)]
            self.assertEqual(f.run(), node.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])
            self.assertEqual(mining.read_state(f.layout)["rollback"], "not_needed")

    def test_connected_cold_start_resumes_after_interruption_in_each_phase(self):
        for phase in ("QUEUED", "STOPPING", "STOPPED", "CONFIGURED", "STARTING"):
            with self.subTest(phase=phase), MiningFixture() as f, redirect_stderr(io.StringIO()):
                f.update_env(USDB_BOOTNODES=SEED)
                f.chain["peers"] = 1
                f.enable()
                if phase != "QUEUED":
                    original_phase = mining._phase
                    def interrupt(layout, operation, current, **fields):
                        original_phase(layout, operation, current, **fields)
                        if current == phase:
                            raise Interrupted()
                    with mock.patch.object(mining, "_phase", side_effect=interrupt), self.assertRaises(Interrupted):
                        f.run()
                self.assertEqual(f.run(), 0)
                self.assertEqual(mining.read_state(f.layout)["phase"], "APPLIED")
                self.assertEqual(f.container_number, 1)
                self.assertEqual(node.read_env(f.layout.node_env)["USDB_BOOTNODES"], SEED)

    def test_recorded_founder_with_seeds_can_restart_after_blocks_without_peers(self):
        with MiningFixture() as f, redirect_stderr(io.StringIO()):
            f.update_env(USDB_BOOTNODES=SEED)
            f.chain["peers"] = 1
            f.enable()
            self.assertEqual(f.run(), 0)
            record = deepcopy(mining.read_state(f.layout)["first_node"])
            f.chain.update(height=12, peers=0)
            f.fail_rpc = "admin_peers"
            mining.validate_start(f.layout)
            plan = mining.preflight(f.layout, ADDRESS)
            self.assertEqual(plan["peer"]["declaration"], "recorded")
            self.assertEqual(plan["peer"]["record"], record)
            self.assertEqual(peers.membership(f.layout, node.read_env(f.layout.node_env),
                syncing=False, peer_count=0, node_id=f.chain["node_id"])[:2], ("READY", "FIRST_NODE"))
            self.assertEqual(mining.submit(f.layout, address=ADDRESS, yes=True)["outcome"], "already_applied")
            self.assertEqual(f.container_number, 1)

    def test_new_node_or_network_cannot_reuse_founder_record(self):
        for changed in ("node", "network", "dataset"):
            with self.subTest(changed=changed), MiningFixture() as f, redirect_stderr(io.StringIO()):
                f.enable()
                f.run()
                f.chain["height"] = 12
                if changed == "node":
                    f.chain["node_id"] = "ff" * 32
                elif changed == "network":
                    f.layout.network_identity["genesis_block_hash"] = "0x" + "12" * 32
                else:
                    Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], node.DATASET_IDENTITY_FILE).write_text('{"dataset":"replacement"}')
                with self.assertRaisesRegex(ValueError, "FIRST_NODE_CONFLICT.*height=12"):
                    mining.preflight(f.layout, ADDRESS, first_node=True)


if __name__ == "__main__":
    unittest.main()
