#!/usr/bin/env python3
"""Check one canonical block's historical input capability; this is not USDB consensus readiness."""

from __future__ import annotations

import argparse
from decimal import Decimal
import json
from pathlib import Path

from check_bitcoin_readiness import BitcoinRpc, require_type


def assess_block_data(block: dict, height: int, block_hash: str) -> int:
    """Require complete undo-backed prevouts for the requested active-chain block."""
    if block.get("hash") != block_hash or type(block.get("height")) is not int or block["height"] != height:
        raise ValueError("block identity does not match the requested height/hash")
    if require_type(block.get("confirmations"), int, "block.confirmations") <= 0:
        raise ValueError("requested block is not in the active chain")
    transactions = require_type(block.get("tx"), list, "block.tx")
    if not transactions:
        raise ValueError("block has no transactions")
    count = 0
    for position, tx in enumerate(transactions):
        inputs = require_type(tx.get("vin"), list, "transaction.vin")
        if not inputs:
            raise ValueError("transaction has no inputs")
        for vin in inputs:
            if position == 0 and len(inputs) == 1 and "coinbase" in vin:
                continue
            prevout = vin.get("prevout")
            if not isinstance(prevout, dict):
                raise ValueError("block undo/prevout unavailable; retain unpruned block and undo data")
            value = prevout.get("value")
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise ValueError("prevout.value must be a BTC amount")
            sats = Decimal(str(value)) * 100_000_000
            if not sats.is_finite() or sats < 0 or sats > 2_100_000_000_000_000 or sats != sats.to_integral_value():
                raise ValueError("prevout.value is not a valid satoshi amount")
            count += 1
    return count


def probe(rpc: BitcoinRpc, chain: str, height: int, expected_hash: str) -> dict:
    """Report foreground and background separately; txindex progress is not an input-data gate."""
    info = rpc.call("getblockchaininfo")
    states = rpc.call("getchainstates")
    report = {
        "schema_version": "usdb-bitcoin-block-data-readiness:v1",
        "scope": "one_block_historical_inputs",
        "ready": False,
        "requested_height": height,
        "requested_hash": expected_hash,
        "foreground": {key: info.get(key) for key in ("chain", "blocks", "headers", "initialblockdownload", "pruned")},
        "chainstates": states.get("chainstates"),
        "txindex_required": False,
        "blockers": [],
    }
    try:
        if info.get("chain") != chain:
            raise ValueError("Bitcoin network mismatch")
        if require_type(info.get("pruned"), bool, "getblockchaininfo.pruned"):
            raise ValueError("pruned node cannot guarantee historical block and undo retention")
        if require_type(info.get("blocks"), int, "getblockchaininfo.blocks") < height:
            raise ValueError("requested height has not reached the active chain")
        if rpc.get_block_hash(height) != expected_hash:
            raise ValueError("canonical block hash mismatch")
        block = rpc.call_result("getblock", [expected_hash, 3])
        report["input_count"] = assess_block_data(block, height, expected_hash)
        if rpc.get_block_hash(height) != expected_hash:
            raise ValueError("canonical block changed during input-data check")
        report["ready"] = True
    except ValueError as exc:
        report["blockers"].append(str(exc))
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--cookie-file", required=True, type=Path)
    parser.add_argument("--expected-chain", default="main")
    parser.add_argument("--height", required=True, type=int)
    parser.add_argument("--block-hash", required=True)
    parser.add_argument("--rpc-timeout-secs", type=float, default=30)
    args = parser.parse_args()
    if args.height < 0 or args.rpc_timeout_secs <= 0:
        parser.error("height must be nonnegative and RPC timeout positive")
    if len(args.block_hash) != 64 or any(c not in "0123456789abcdef" for c in args.block_hash):
        parser.error("block hash must contain 64 lowercase hexadecimal characters")
    user, password = args.cookie_file.read_text().strip().split(":", 1)
    rpc = BitcoinRpc(args.url, user, password, args.rpc_timeout_secs)
    result = probe(rpc, args.expected_chain, args.height, args.block_hash)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["ready"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, OSError) as exc:
        print(json.dumps({"ready": False, "blockers": [str(exc)]}))
        raise SystemExit(1) from exc
