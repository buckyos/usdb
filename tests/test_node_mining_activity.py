"""Mining dashboard evidence, reorg handling, precision and bounded observation costs."""

from copy import deepcopy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_mining_activity as activity
import usdb_node as node
from node_progress_render import _usdb_amount, render_node_progress
from common.mining import ADDRESS, PASS_ID, MiningFixture
from common.node_progress import ready_miner_progress


class MiningActivityTests(unittest.TestCase):
    def setUp(self):
        self.fixture = self.enterContext(MiningFixture())
        self.fixture.update_env(USDB_NODE_ROLE="miner", USDB_MINER_ADDRESS=ADDRESS)
        self.fixture.adopt()
        self.mining = dict(state="ACTIVE", applied=True, configured={"USDB_NODE_ROLE": "miner",
                           "USDB_MINER_ADDRESS": ADDRESS, "USDB_MINER_THREADS": "1"},
                           runtime=deepcopy(self.fixture.runtime),
                           eligibility={"candidate": deepcopy(self.fixture.candidate)})
        self.hash = "0x" + "12" * 32
        self.component = node._component_progress("usdb_chain", "READY", "peers=1", current=120)
        self.component["head"] = dict(number=120, hash=self.hash, timestamp=1790250200)
        self.service = dict(state="running", started_at="2026-09-24T11:00:00+00:00")
        self.block = dict(hash=self.hash, number="0x78", timestamp=hex(1790250200), miner=ADDRESS)
        self.economics = dict(schema_version="usdb-block-economics:v1", status="verified", block_hash=self.hash,
                              block_number="120", miner=ADDRESS, selector=dict(pass_id=PASS_ID, btc_height=970644),
                              amounts=dict(miner_emission_atoms="688003910372432", miner_fees_atoms="1"))
        self.canonical_hash = self.hash
        self.calls = []
        self.observer = activity.MiningActivityObserver()
        self.clock = self.enterContext(mock.patch.object(activity.time, "monotonic", return_value=1000))
        self.logs = self.enterContext(mock.patch.object(activity.subprocess, "run", return_value=
            subprocess.CompletedProcess([], 0, json.dumps(dict(msg="Successfully sealed new block", hash=self.hash)), "")))
        self.rpc = self.enterContext(mock.patch.object(node, "_json_rpc_batch", side_effect=self.rpc_response))

    def rpc_response(self, url, calls, timeout_secs):
        self.assertGreater(timeout_secs, 0)
        self.assertLessEqual(timeout_secs, 2)
        method, params = calls[0]
        self.calls.append((method, params))
        if method == "eth_getBlockByHash":
            value = self.block
        elif method == "eth_getBlockByNumber":
            value = {**self.block, "hash": self.canonical_hash}
        elif method == "eth_getUSDBBlockEconomics":
            value = self.economics
        else:
            self.fail(f"Unexpected display RPC: {method}")
        return {method: deepcopy(value)}

    def observe(self):
        return self.observer.observe(self.fixture.layout, self.mining, self.component, self.service)

    def test_verified_income_is_exact_and_reuses_hash_bound_cache(self):
        report = self.observe()
        self.assertEqual(report["state"], "canonical")
        self.assertEqual(report["reward"]["total_atoms"], "688003910372433")
        original = deepcopy(report)
        report["reward"]["total_atoms"] = "tampered"
        count = len(self.calls)
        self.assertEqual(self.observe(), original)
        self.assertEqual(len(self.calls), count)
        self.clock.return_value += 16
        self.assertEqual(self.observe()["state"], "canonical")
        self.assertEqual(sum(method == "eth_getUSDBBlockEconomics" for method, _ in self.calls), 1)

    def test_network_head_and_recipient_alone_never_prove_a_local_seal(self):
        for log in ("", json.dumps(dict(msg="Successfully sealed new block", hash="0x123...456")),
                    json.dumps(dict(msg="Imported new chain segment", hash=self.hash)), "[]\nnull\nbad json"):
            self.logs.return_value.stdout = log
            self.observer = activity.MiningActivityObserver()
            self.assertEqual(self.observe()["state"], "not_observed")
        self.rpc.assert_not_called()

    def test_same_height_reorg_withdraws_income_even_during_cache_ttl(self):
        self.observe()
        self.canonical_hash = "0x" + "aa" * 32
        self.component["head"]["hash"] = self.canonical_hash
        report = self.observe()
        self.assertEqual(report["state"], "orphaned")
        self.assertNotIn("reward", report)

    def test_reorg_during_economics_query_does_not_show_income(self):
        def changed(*args, **kwargs):
            result = self.rpc_response(*args, **kwargs)
            if "eth_getUSDBBlockEconomics" in result:
                self.canonical_hash = "0x" + "aa" * 32
            return result
        self.rpc.side_effect = changed
        self.assertEqual(self.observe()["state"], "orphaned")
        self.assertIsNone(self.observer.reward)

    def test_missing_canonical_block_is_unknown_not_proof_of_an_orphan(self):
        self.canonical_hash = None
        self.assertEqual(self.observe()["state"], "unavailable")

    def test_probe_budget_limits_followup_calls(self):
        def slow(*args, **kwargs):
            result = self.rpc_response(*args, **kwargs)
            self.clock.return_value += activity.PROBE_BUDGET_SECS
            return result
        self.rpc.side_effect = slow
        self.assertEqual(self.observe()["state"], "unavailable")
        self.assertEqual(len(self.calls), 1)

    def test_display_preserves_small_and_uint256_amounts_without_floats(self):
        for amount in (0, 1, 10**18, 10**18 + 1, 2**256 - 1):
            text = _usdb_amount(str(amount)).replace(",", "")
            whole, _, fraction = text.partition(".")
            self.assertEqual(int(whole) * 10**18 + int(fraction.ljust(18, "0")), amount)

    def test_invalid_economics_is_not_replaced_with_estimates_or_zero_income(self):
        variants = [dict(status="estimated"), dict(block_hash="0x" + "aa" * 32),
                    dict(block_number="119"), dict(miner="0x" + "11" * 20),
                    dict(selector=[]), dict(amounts=[1]),
                    dict(amounts=dict(miner_emission_atoms="-1", miner_fees_atoms="0")),
                    dict(amounts=dict(miner_emission_atoms=True, miner_fees_atoms="0")),
                    dict(amounts=dict(miner_emission_atoms=str(2**256), miner_fees_atoms="0"))]
        for fields in variants:
            with self.subTest(fields=fields):
                self.observer = activity.MiningActivityObserver()
                with mock.patch.dict(self.economics, fields):
                    report = self.observe()
                self.assertEqual(report["state"], "canonical")
                self.assertEqual(report["reward"]["state"], "unavailable")
                self.assertNotIn("total_atoms", report["reward"])

    def test_rpc_failure_backoff_and_timeout_do_not_block_mining_readiness(self):
        def unavailable(*args, **kwargs):
            if args[1][0][0] == "eth_getUSDBBlockEconomics":
                self.calls.append(("eth_getUSDBBlockEconomics", []))
                raise ValueError("-32601 method not found at http://secret:credential@example.invalid")
            return self.rpc_response(*args, **kwargs)
        self.rpc.side_effect = unavailable
        report = self.observe()
        self.assertIn("unavailable in this chain release", report["reward"]["detail"])
        self.assertNotIn("credential", json.dumps(report))
        self.clock.return_value += 16
        self.assertEqual(self.observe()["reward"]["detail"], report["reward"]["detail"])
        self.assertEqual(sum(method == "eth_getUSDBBlockEconomics" for method, _ in self.calls), 1)
        self.logs.side_effect = subprocess.TimeoutExpired("docker logs", 2)
        self.clock.return_value += 16
        with mock.patch.object(node, "_mining_status", return_value=self.mining), \
                mock.patch.object(activity, "OBSERVER", self.observer):
            mining, overall = node._mining_progress(self.fixture.layout, {"usdb-chain": self.service},
                                                    self.component, [self.component], "READY")
        self.assertEqual((overall, mining["state"]), ("READY", "ACTIVE"))
        self.assertEqual(mining["activity"]["state"], "unavailable")

    def test_restart_or_network_change_drops_retained_local_observation(self):
        for change in ("container", "started_at", "genesis", "chain_id"):
            with self.subTest(change=change):
                self.observer = activity.MiningActivityObserver()
                self.logs.return_value.stdout = json.dumps(dict(msg="Successfully sealed new block", hash=self.hash))
                self.observe()
                self.logs.return_value.stdout = ""
                if change == "container":
                    self.mining["runtime"]["id"] += "new"
                elif change == "started_at":
                    self.service["started_at"] += "new"
                elif change == "genesis":
                    self.fixture.layout.network_identity["genesis_block_hash"] = "0x" + "cd" * 32
                else:
                    self.fixture.layout.network_identity["chain_id"] += 1
                self.assertEqual(self.observe()["state"], "not_observed")

    def test_no_optional_probes_for_full_unapplied_or_unready_node(self):
        for fields in (dict(applied=False), dict(drift=True)):
            with mock.patch.dict(self.mining, fields):
                self.assertIsNone(self.observe())
        with mock.patch.dict(self.mining["configured"], USDB_NODE_ROLE="full"):
            self.assertIsNone(self.observe())
        with mock.patch.dict(self.component, state="WAITING"):
            self.assertIsNone(self.observe())
        self.logs.assert_not_called()
        self.rpc.assert_not_called()

    def test_dashboard_identifiers_income_and_historical_pass_are_distinct(self):
        report = ready_miner_progress()
        report["mining"] = {**self.mining, "activity": self.observe()}
        self.mining["eligibility"]["candidate"]["pass"]["pass_id"] = "aa" * 32 + "i1"
        report["observed_at"] = "2026-09-24T11:43:58+00:00"
        report["components"][-1] = self.component
        original = deepcopy(report)
        for width in (40, 80, 120):
            text = render_node_progress(report, width=width)
            self.assertTrue(all(len(line) <= width for line in text.splitlines()))
            compact = " ".join(text.split())
            for label in ("Miner address:", "Candidate Pass:", "Head:", "Local block: #120", "Block income:",
                          "0.000688003910372433 USDB", "Block Pass:", "effective energy"):
                self.assertIn(label, compact)
        detailed = render_node_progress(report, details=True, width=150)
        self.assertIn(self.hash, detailed)
        self.assertIn(PASS_ID, detailed)
        self.assertEqual(report, original)

    def test_orphaned_or_stale_observations_do_not_render_earnings(self):
        report = ready_miner_progress()
        report["mining"] = {**self.mining, "activity": self.observe()}
        report["components"][-1] = self.component
        report["mining"]["activity"]["state"] = "orphaned"
        self.assertNotIn("Block income:", render_node_progress(report))
        report["components"][-1]["observation_unavailable"] = True
        self.assertNotIn("Local block:", render_node_progress(report))


if __name__ == "__main__":
    unittest.main()
