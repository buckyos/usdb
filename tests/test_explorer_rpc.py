#!/usr/bin/env python3
"""Check identity, sampled receipt consistency and the boundaries of explorer readiness claims."""
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import check_explorer_rpc as CHECK
from common.explorer_rpc import ExplorerRpcFixture


class ExplorerRpcTests(unittest.TestCase):
    def test_samples_usdb_payload_without_claiming_explorer_or_public_readiness(self):
        rpc = ExplorerRpcFixture()
        report = CHECK.inspect_rpc(rpc, rpc.identity, transaction=rpc.transaction)
        self.assertTrue(report["basic_rpc_passed"])
        self.assertEqual(report["extra_data_bytes"], 111)
        self.assertTrue(report["sample_transaction"]["canonical_at_observation"])
        self.assertEqual(report["explorer_qualification"], "not_run")
        self.assertEqual(report["public_endpoint_qualification"], "not_run")
        self.assertIn("admin", report["advertised_namespaces"])
        self.assertNotIn("debug_traceTransaction", [method for method, _ in rpc.calls])

    def test_wrong_identity_stops_before_sampling_accounts_or_traces(self):
        for key in ("chain_id", "genesis_block_hash"):
            with self.subTest(key=key):
                rpc = ExplorerRpcFixture()
                identity = {**rpc.identity, key: 1 if key == "chain_id" else "0x" + "ff" * 32}
                with self.assertRaisesRegex(ValueError, "identity"):
                    CHECK.inspect_rpc(rpc, identity, transaction=rpc.transaction, trace=True)
                self.assertTrue(all(method in {"eth_getBlockByNumber", "eth_chainId"} for method, _ in rpc.calls))

    def test_reorg_or_missing_receipt_prevents_passing_basic_samples(self):
        for mutation in (lambda rpc: setattr(rpc, "reorg", True),
                         lambda rpc: rpc.failures.update(eth_getTransactionReceipt=None),
                         lambda rpc: rpc.tx_block.update(transactions=[])):
            with self.subTest(mutation=mutation):
                rpc = ExplorerRpcFixture()
                mutation(rpc)
                self.assertFalse(CHECK.inspect_rpc(rpc, rpc.identity, transaction=rpc.transaction)["basic_rpc_passed"])

    def test_optional_archive_and_fee_gaps_remain_visible(self):
        rpc = ExplorerRpcFixture()
        def call(method, params):
            if method == "eth_getBalance" and params[-1] == "0x2c":
                raise ValueError("historical state unavailable")
            return rpc(method, params)
        rpc.failures["eth_feeHistory"] = ValueError("unsupported")
        report = CHECK.inspect_rpc(call, rpc.identity)
        self.assertTrue(report["basic_rpc_passed"])
        self.assertEqual({item["name"] for item in report["checks"] if item["status"] == "failed"},
                         {"historical_balance_sample", "fee_history"})
        self.assertTrue(any("No transaction sample" in line for line in report["limitations"]))

    def test_malformed_or_inconsistent_transaction_identity_does_not_pass(self):
        for method, patch in (("eth_getTransactionByHash", {"hash": None}),
                              ("eth_getTransactionReceipt", {"transactionHash": 12}),
                              ("eth_getTransactionByHash", {"blockNumber": "0xb"}),
                              ("eth_getTransactionReceipt", {"blockNumber": "0x12d"})):
            with self.subTest(method=method, patch=patch):
                rpc = ExplorerRpcFixture()
                value = rpc(method, [rpc.transaction])
                rpc.failures[method] = {**value, **patch}
                report = CHECK.inspect_rpc(rpc, rpc.identity, transaction=rpc.transaction)
                self.assertFalse(report["basic_rpc_passed"])

    def test_trace_is_opt_in_and_failure_is_not_hidden(self):
        rpc = ExplorerRpcFixture()
        rpc.failures["debug_traceTransaction"] = ValueError("RPC method unavailable")
        report = CHECK.inspect_rpc(rpc, rpc.identity, transaction=rpc.transaction, trace=True)
        trace = next(item for item in report["checks"] if item["name"] == "transaction_trace")
        self.assertEqual(trace["status"], "failed")
        self.assertEqual(next(params for method, params in rpc.calls if method == "debug_traceTransaction"),
                         [rpc.transaction, {"tracer": "callTracer", "timeout": "5s"}])

    def test_malformed_hex_values_and_response_shapes_fail_samples(self):
        for method, value in (("eth_getBalance", "garbage"), ("eth_getCode", "0x1"),
                              ("eth_getBlockByHash", None), ("eth_getLogs", {})):
            with self.subTest(method=method):
                rpc = ExplorerRpcFixture()
                rpc.failures[method] = value
                self.assertFalse(CHECK.inspect_rpc(rpc, rpc.identity)["basic_rpc_passed"])

    def test_probe_transport_refuses_writes_and_credential_urls(self):
        for url in ("file:///etc/passwd", "http://user:secret@localhost:8545", "http://localhost:8545/?secret=value"):
            with self.assertRaises(ValueError):
                CHECK.ReadRpc(url, 1)
        rpc = CHECK.ReadRpc("http://127.0.0.1:8545", 1)
        with mock.patch.object(rpc.opener, "open") as send:
            for method in ("eth_sendRawTransaction", "eth_sendTransaction", "miner_stop", "admin_addPeer"):
                with self.assertRaises(ValueError):
                    rpc(method, [])
            send.assert_not_called()


if __name__ == "__main__":
    unittest.main()
