#!/usr/bin/env python3

from __future__ import annotations

import random
import unittest
from typing import Any

import audit_bitcoin_core_utxo_sample as audit


SCRIPTS = ("51", "52", "53", "54")


class FakeBitcoinRpc:
    def __init__(self) -> None:
        self.stable_height = 100
        self.scan_height = 102
        self.tip_height = self.scan_height
        self.omit_prevout = False
        self.gettxout_value_override: object | None = None
        self.source_blocks = {
            height: {
                "hash": f"block-{height}",
                "height": height,
                "tx": [{
                    "txid": f"{height:064x}",
                    "vin": [{"coinbase": "00"}],
                    "vout": [{
                        "n": 0,
                        "value": 1,
                        "scriptPubKey": {"hex": script},
                    }],
                }],
            }
            for height, script in zip(range(97, 101), SCRIPTS)
        }
        self.unspents = {
            "52": [{
                "txid": "a" * 64,
                "vout": 0,
                "scriptPubKey": "52",
                "amount": 1,
                "height": 98,
            }],
            "54": [{
                "txid": "b" * 64,
                "vout": 1,
                "scriptPubKey": "54",
                "amount": "2.50000000",
                "height": 100,
            }],
        }

    def gap_block(self, height: int) -> dict[str, Any]:
        if height == 101:
            tx_input: dict[str, Any] = {"txid": "c" * 64, "vout": 0}
            if not self.omit_prevout:
                tx_input["prevout"] = {
                    "scriptPubKey": {"hex": "51"},
                    "value": 1,
                }
            return {
                "hash": "block-101",
                "height": 101,
                "tx": [{
                    "txid": "d" * 64,
                    "vin": [tx_input],
                    "vout": [{
                        "n": 0,
                        "value": 1,
                        "scriptPubKey": {"hex": "55"},
                    }],
                }],
            }
        return {
            "hash": "block-102",
            "height": 102,
            "tx": [{
                "txid": "e" * 64,
                "vin": [{"coinbase": "00"}],
                "vout": [{
                    "n": 0,
                    "value": 0,
                    "scriptPubKey": {"hex": "6a00"},
                }],
            }],
        }

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        params = params or []
        if method == "getblockchaininfo":
            return {"chain": "regtest", "blocks": self.tip_height}
        if method == "getblockhash":
            return f"block-{params[0]}"
        if method == "getblock":
            height = int(str(params[0]).split("-")[1])
            if params[1] == 2:
                return self.source_blocks[height]
            return self.gap_block(height)
        if method == "scantxoutset":
            requested = {descriptor[4:-1] for descriptor in params[1]}
            unspents = [
                dict(unspent)
                for script, entries in self.unspents.items()
                if script in requested
                for unspent in entries
            ]
            return {
                "success": True,
                "height": self.scan_height,
                "bestblock": f"block-{self.scan_height}",
                "unspents": unspents,
            }
        if method == "gettxout":
            for entries in self.unspents.values():
                for unspent in entries:
                    if (unspent["txid"], unspent["vout"]) == (params[0], params[1]):
                        return {
                            "bestblock": f"block-{self.scan_height}",
                            "value": (
                                unspent["amount"]
                                if self.gettxout_value_override is None
                                else self.gettxout_value_override
                            ),
                            "scriptPubKey": {"hex": unspent["scriptPubKey"]},
                        }
            return None
        if method == "getblockcount":
            return self.tip_height
        if method == "getbestblockhash":
            return f"block-{self.tip_height}"
        raise AssertionError(f"unexpected Bitcoin RPC method: {method} {params}")


class FakeBalanceHistoryRpc:
    def __init__(self, bitcoin: FakeBitcoinRpc) -> None:
        self.bitcoin = bitcoin
        self.balance_overrides: dict[str, int] = {}

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        params = params or []
        if method == "get_network_type":
            return "regtest"
        if method == "get_snapshot_info":
            return {
                "stable_height": self.bitcoin.stable_height,
                "stable_block_hash": f"block-{self.bitcoin.stable_height}",
                "stable_lag": self.bitcoin.scan_height - self.bitcoin.stable_height,
                "balance_query_floor": 0,
            }
        if method == "get_addresses_balances":
            hashes = params[0]["script_hashes"]
            script_by_hash = {
                audit.script_hash_from_hex(script): script for script in SCRIPTS
            }
            balances = {
                "51": 0,
                "52": 100_000_000,
                "53": 0,
                "54": 250_000_000,
            }
            return [[{
                "block_height": self.bitcoin.stable_height,
                "balance": self.balance_overrides.get(script_hash, balances[script_by_hash[script_hash]]),
                "delta": 0,
            }] for script_hash in hashes]
        if method == "get_block_commit":
            return {
                "block_height": params[0],
                "btc_block_hash": f"block-{params[0]}",
            }
        raise AssertionError(f"unexpected balance-history RPC method: {method} {params}")


def run_fake_audit(
    bitcoin: FakeBitcoinRpc,
    balance_history: FakeBalanceHistoryRpc,
) -> dict[str, Any]:
    return audit.run_audit(
        bitcoin,
        balance_history,
        expected_network="regtest",
        sample_size=2,
        oversample_factor=3,
        source_lookback_blocks=4,
        source_block_count=4,
        max_gettxout_checks=8,
        seed=7,
    )


class AuditPrimitiveTests(unittest.TestCase):
    def test_amount_conversion_requires_exact_satoshis(self) -> None:
        self.assertEqual(audit.amount_to_sat("1.23456789"), 123_456_789)
        with self.assertRaisesRegex(audit.AuditError, "exact non-negative"):
            audit.amount_to_sat("0.000000001")
        with self.assertRaisesRegex(audit.AuditError, "exact non-negative"):
            audit.amount_to_sat("-1")

    def test_script_hash_and_core_unspendable_rules(self) -> None:
        self.assertEqual(
            audit.script_hash_from_hex("51"),
            "6032c38c0bc0e91e726f1e55e1832e434509001a7aed5cfd881b6ef07215e84a",
        )
        self.assertFalse(audit.is_core_unspendable("51"))
        self.assertTrue(audit.is_core_unspendable("6a00"))
        self.assertTrue(audit.is_core_unspendable("51" * 10_001))

    def test_height_sampling_is_deterministic_and_unique(self) -> None:
        first = audit.deterministic_heights(10, 100, 8, random.Random(99))
        second = audit.deterministic_heights(10, 100, 8, random.Random(99))
        self.assertEqual(first, second)
        self.assertEqual(len(first), len(set(first)))
        self.assertTrue(all(10 <= height <= 100 for height in first))


class AuditFlowTests(unittest.TestCase):
    def test_audit_excludes_lag_window_touches_and_cross_checks_gettxout(self) -> None:
        bitcoin = FakeBitcoinRpc()
        report = run_fake_audit(bitcoin, FakeBalanceHistoryRpc(bitcoin))

        self.assertTrue(report["ok"])
        self.assertEqual(report["verified_script_count"], 2)
        self.assertEqual(report["lag_window_touched_candidate_count"], 1)
        self.assertGreaterEqual(report["gettxout_checked_count"], 1)
        self.assertEqual(report["mismatches"], [])
        self.assertEqual(len(report["samples"]), 2)

    def test_balance_mismatch_is_reported(self) -> None:
        bitcoin = FakeBitcoinRpc()
        balance_history = FakeBalanceHistoryRpc(bitcoin)
        for script in ("52", "53", "54"):
            balance_history.balance_overrides[audit.script_hash_from_hex(script)] = 1

        report = run_fake_audit(bitcoin, balance_history)

        self.assertFalse(report["ok"])
        self.assertGreater(report["mismatch_count"], 0)

    def test_missing_prevout_fails_closed(self) -> None:
        bitcoin = FakeBitcoinRpc()
        bitcoin.omit_prevout = True
        with self.assertRaisesRegex(audit.AuditError, "omitted an input prevout"):
            run_fake_audit(bitcoin, FakeBalanceHistoryRpc(bitcoin))

    def test_gettxout_mismatch_fails_closed(self) -> None:
        bitcoin = FakeBitcoinRpc()
        bitcoin.gettxout_value_override = "9.00000000"
        with self.assertRaisesRegex(audit.AuditError, "gettxout mismatch"):
            run_fake_audit(bitcoin, FakeBalanceHistoryRpc(bitcoin))

    def test_no_live_outpoint_fails_gettxout_cross_check(self) -> None:
        bitcoin = FakeBitcoinRpc()
        bitcoin.unspents = {}
        with self.assertRaisesRegex(audit.AuditError, "no live outpoints"):
            run_fake_audit(bitcoin, FakeBalanceHistoryRpc(bitcoin))

    def test_duplicate_scantxoutset_outpoint_is_rejected(self) -> None:
        candidate = audit.Candidate(
            script_pubkey="52",
            script_hash=audit.script_hash_from_hex("52"),
            source_height=1,
            source_txid="1" * 64,
            source_vout=0,
        )
        unspent = {
            "txid": "a" * 64,
            "vout": 0,
            "scriptPubKey": "52",
            "amount": 1,
        }
        with self.assertRaisesRegex(audit.AuditError, "duplicate outpoint"):
            audit.group_scantxoutset_unspents(
                {"unspents": [unspent, dict(unspent)]}, [candidate]
            )

    def test_tip_change_fails_closed(self) -> None:
        bitcoin = FakeBitcoinRpc()
        bitcoin.tip_height += 1
        with self.assertRaisesRegex(audit.AuditError, "tip height changed"):
            run_fake_audit(bitcoin, FakeBalanceHistoryRpc(bitcoin))


if __name__ == "__main__":
    unittest.main()
