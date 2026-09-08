#!/usr/bin/env python3
"""Sample a USDB node's explorer RPC capabilities without signing or sending transactions."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

from release_manifest import build_network_identity

SCHEMA = "usdb-explorer-rpc-check:v1"
READ_METHODS = {"web3_clientVersion", "rpc_modules", "eth_chainId", "net_version", "eth_syncing",
                "eth_blockNumber", "eth_getBlockByNumber", "eth_getBlockByHash", "eth_getBalance",
                "eth_getCode", "eth_getTransactionCount", "eth_call", "eth_estimateGas", "eth_gasPrice",
                "eth_maxPriorityFeePerGas", "eth_feeHistory", "eth_getLogs", "eth_getTransactionByHash",
                "eth_getTransactionReceipt", "debug_traceTransaction"}
HASH = re.compile(r"0x[0-9a-fA-F]{64}")


def quantity(value) -> int:
    if not isinstance(value, str) or re.fullmatch(r"0x(?:0|[1-9a-fA-F][0-9a-fA-F]*)", value) is None:
        raise ValueError("RPC returned an invalid hex quantity")
    return int(value, 16)


def hex_data(value) -> str:
    if not isinstance(value, str) or re.fullmatch(r"0x(?:[0-9a-fA-F]{2})*", value) is None:
        raise ValueError("RPC returned invalid hex data")
    return value


def same_hash(value, expected: str) -> bool:
    """Compare RPC hashes without accepting malformed values or coercing their types."""
    return isinstance(value, str) and HASH.fullmatch(value) is not None and value.lower() == expected.lower()


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Keep all probes on the operator-selected endpoint."""
    def redirect_request(self, *args, **kwargs):
        return None


class ReadRpc:
    def __init__(self, url: str, timeout: float):
        parsed = urllib.parse.urlsplit(url)
        if (parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username is not None or
                parsed.password is not None or parsed.query or parsed.fragment or url != url.strip()):
            raise ValueError("RPC URL must be HTTP(S) without credentials, query or fragment")
        self.url, self.timeout = url, timeout
        self.opener = urllib.request.build_opener(NoRedirect)

    def __call__(self, method: str, params: list):
        if method not in READ_METHODS:
            raise ValueError("Probe refuses non-read-only RPC method")
        payload = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        request = urllib.request.Request(self.url, data=json.dumps(payload).encode(), headers={"Content-Type": "application/json"})
        try:
            with self.opener.open(request, timeout=self.timeout) as response:
                body = response.read(8 * 1024 * 1024 + 1)
            if len(body) > 8 * 1024 * 1024:
                raise ValueError("RPC response exceeds the probe size limit")
            value = json.loads(body)
        except urllib.error.HTTPError as error:
            raise ValueError(f"RPC HTTP status {error.code}") from error
        except (OSError, urllib.error.URLError) as error:
            raise ValueError(f"RPC transport unavailable ({type(error).__name__})") from error
        if not isinstance(value, dict) or value.get("jsonrpc") != "2.0" or value.get("id") != 1:
            raise ValueError("RPC returned an invalid response envelope")
        if "error" in value:
            code = value["error"].get("code") if isinstance(value["error"], dict) else None
            raise ValueError(f"RPC {method} failed with code {code}")
        if "result" not in value:
            raise ValueError("RPC response has no result")
        return value["result"]


def inspect_rpc(rpc, identity: dict, *, transaction: str | None = None, historical_block: int | None = None, trace=False) -> dict:
    """Pin samples to one observed head; a passing sample is not explorer or archive qualification."""
    genesis = rpc("eth_getBlockByNumber", ["0x0", False])
    chain_id = quantity(rpc("eth_chainId", []))
    if (chain_id != identity["chain_id"] or not isinstance(genesis, dict) or not isinstance(genesis.get("hash"), str) or
            genesis["hash"].lower() != identity["genesis_block_hash"].lower()):
        raise ValueError("RPC network identity differs from the selected release bundle")
    block = rpc("eth_getBlockByNumber", ["latest", False])
    if not isinstance(block, dict) or HASH.fullmatch(str(block.get("hash"))) is None:
        raise ValueError("RPC latest block is unavailable")
    number, block_hash = quantity(block.get("number")), block["hash"]
    tag = hex(number)
    address = block.get("miner")
    if not isinstance(address, str) or re.fullmatch(r"0x[0-9a-fA-F]{40}", address) is None:
        raise ValueError("RPC block miner address is invalid")
    report = {"schema_version": SCHEMA, "observed_at": datetime.now(timezone.utc).isoformat(),
              "network": identity, "checkpoint": {"number": number, "hash": block_hash},
              "extra_data_bytes": (len(hex_data(block.get("extraData"))) - 2) // 2,
              "checks": [], "limitations": ["Read-only samples do not qualify Blockscout indexing, browser wallet transactions or public RPC access controls.",
                                            "USDB rewards, supply and selector payloads require explicit explorer verification."]}

    def probe(name, method, params, validate, *, required=True):
        try:
            value = rpc(method, params)
            valid = validate(value)
            if valid is False or valid is None:
                raise ValueError("RPC sample has an unexpected value or shape")
            report["checks"].append({"name": name, "method": method, "required": required, "status": "passed"})
            return value
        except (ValueError, OSError, TypeError, KeyError) as error:
            report["checks"].append({"name": name, "method": method, "required": required,
                                     "status": "failed", "detail": str(error)})
            return None

    report["client_version"] = probe("client", "web3_clientVersion", [], lambda v: isinstance(v, str) and bool(v))
    probe("network_id", "net_version", [], lambda v: v == str(identity["network_id"]))
    probe("synced", "eth_syncing", [], lambda v: v is False)
    probe("head_height", "eth_blockNumber", [], lambda v: quantity(v) >= number)
    modules = probe("namespaces", "rpc_modules", [], lambda v: isinstance(v, dict), required=False)
    report["advertised_namespaces"] = sorted(modules) if isinstance(modules, dict) else None
    probe("block_by_hash", "eth_getBlockByHash", [block_hash, False],
          lambda v: isinstance(v, dict) and v.get("hash") == block_hash and quantity(v.get("number")) == number)
    probe("balance", "eth_getBalance", [address, tag], quantity)
    probe("nonce", "eth_getTransactionCount", [address, "pending"], quantity)
    probe("code", "eth_getCode", [address, tag], hex_data)
    probe("call", "eth_call", [{"to": "0x" + "00" * 20, "data": "0x"}, tag], hex_data)
    probe("gas_price", "eth_gasPrice", [], quantity)
    probe("estimate_transfer", "eth_estimateGas", [{"from": address, "to": "0x" + "00" * 20, "value": "0x0"}],
          lambda v: quantity(v) >= 21000)
    probe("priority_fee", "eth_maxPriorityFeePerGas", [], quantity, required=False)
    probe("fee_history", "eth_feeHistory", ["0x1", tag, []],
          lambda v: isinstance(v, dict) and len(v.get("baseFeePerGas", [])) == 2, required=False)
    probe("one_block_logs", "eth_getLogs", [{"fromBlock": tag, "toBlock": tag}], lambda v: isinstance(v, list))
    historical = historical_block if historical_block is not None else max(0, number - 256)
    if historical < 0 or historical > number:
        raise ValueError("Historical sample height must be within the observed chain")
    report["historical_sample_block"] = historical
    probe("historical_balance_sample", "eth_getBalance", [address, hex(historical)], quantity, required=False)
    report["limitations"].append("One successful historical balance sample does not prove complete archive state or replay coverage.")

    if transaction is not None:
        if HASH.fullmatch(transaction) is None:
            raise ValueError("Sample transaction must be a 32-byte hex hash")
        tx = probe("transaction", "eth_getTransactionByHash", [transaction],
                   lambda v: isinstance(v, dict) and same_hash(v.get("hash"), transaction)
                   and quantity(v.get("blockNumber")) <= number)
        receipt = probe("receipt", "eth_getTransactionReceipt", [transaction],
                        lambda v: isinstance(v, dict) and same_hash(v.get("transactionHash"), transaction)
                        and HASH.fullmatch(str(v.get("blockHash"))) is not None and quantity(v["blockNumber"]) <= number)
        if receipt and tx:
            tx_block = probe("receipt_block", "eth_getBlockByNumber", [receipt["blockNumber"], False],
                             lambda v: isinstance(v, dict) and v.get("hash") == receipt["blockHash"] == tx.get("blockHash")
                             and quantity(v.get("number")) == quantity(receipt["blockNumber"]) == quantity(tx["blockNumber"])
                             and isinstance(v.get("transactions"), list)
                             and any(same_hash(h, transaction) for h in v["transactions"]))
            report["sample_transaction"] = {"hash": transaction, "block_number": quantity(receipt["blockNumber"]),
                                             "canonical_at_observation": tx_block is not None}
        if trace:
            probe("transaction_trace", "debug_traceTransaction", [transaction, {"tracer": "callTracer", "timeout": "5s"}],
                  lambda v: isinstance(v, dict) and isinstance(v.get("type"), str), required=False)
    else:
        report["limitations"].append("No transaction sample selected; receipt and transaction indexing remain untested.")
    if not trace:
        report["limitations"].append("Tracing was not executed; use --trace with --transaction only on a private tracing endpoint.")
    probe("checkpoint_still_canonical", "eth_getBlockByNumber", [tag, False],
          lambda v: isinstance(v, dict) and v.get("hash") == block_hash)
    report["basic_rpc_passed"] = all(item["status"] == "passed" for item in report["checks"] if item["required"])
    report["explorer_qualification"] = "not_run"
    report["public_endpoint_qualification"] = "not_run"
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle-dir", type=Path, required=True, help="Verified network bundle from the selected release")
    parser.add_argument("--rpc-url", default="http://127.0.0.1:8545", help="Read-only probe target; never printed in reports")
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--transaction", help="An existing mined transaction hash to sample")
    parser.add_argument("--historical-block", type=int, help="Historical balance sample (default: head minus 256)")
    parser.add_argument("--trace", action="store_true", help="Opt in to tracing the selected transaction; private endpoint only")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0 or (args.trace and not args.transaction):
        parser.error("timeout must be positive and finite; --trace requires --transaction")
    try:
        identity = build_network_identity(args.bundle_dir.resolve())
        report = inspect_rpc(ReadRpc(args.rpc_url, args.timeout), identity, transaction=args.transaction,
                             historical_block=args.historical_block, trace=args.trace)
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0 if report["basic_rpc_passed"] else 1
    except (ValueError, OSError) as error:
        print(f"Explorer RPC check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
