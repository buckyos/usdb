#!/usr/bin/env python3

"""Audit a balance-history stable snapshot against Bitcoin Core's UTXO set."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import random
import sys
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Protocol
from urllib import error, request


SATOSHIS_PER_BTC = Decimal("100000000")
MAX_SCRIPT_SIZE = 10_000
OP_RETURN = 0x6A


class AuditError(RuntimeError):
    """Raised when the audit cannot produce a trustworthy result."""


class Rpc(Protocol):
    def call(self, method: str, params: list[Any] | None = None) -> Any:
        """Calls one JSON-RPC method and returns its result."""


class JsonRpcClient:
    """Small JSON-RPC 1.0/2.0 HTTP client with optional basic authentication."""

    def __init__(
        self,
        url: str,
        *,
        username: str | None = None,
        password: str | None = None,
        timeout: float = 30.0,
    ) -> None:
        self.url = url
        self.timeout = timeout
        self.authorization: str | None = None
        if username is not None or password is not None:
            if username is None or password is None:
                raise AuditError("RPC username and password must be supplied together")
            token = base64.b64encode(f"{username}:{password}".encode()).decode()
            self.authorization = f"Basic {token}"

    def call(self, method: str, params: list[Any] | None = None) -> Any:
        payload = json.dumps(
            {"jsonrpc": "2.0", "id": "balance-history-utxo-audit", "method": method,
             "params": params or []}
        ).encode()
        headers = {"content-type": "application/json"}
        if self.authorization is not None:
            headers["authorization"] = self.authorization
        rpc_request = request.Request(self.url, data=payload, headers=headers, method="POST")

        try:
            with request.urlopen(rpc_request, timeout=self.timeout) as response:
                body = response.read()
        except error.HTTPError as exc:
            body = exc.read()
            if not body:
                raise AuditError(
                    f"RPC {method} failed with HTTP status {exc.code}: {exc.reason}"
                ) from exc
        except (error.URLError, TimeoutError, OSError) as exc:
            raise AuditError(f"RPC {method} transport failure at {self.url}: {exc}") from exc

        try:
            decoded = json.loads(body)
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            raise AuditError(f"RPC {method} returned invalid JSON") from exc
        if decoded.get("error") is not None:
            raise AuditError(
                f"RPC {method} returned error: {json.dumps(decoded['error'], sort_keys=True)}"
            )
        if "result" not in decoded:
            raise AuditError(f"RPC {method} response has no result field")
        return decoded["result"]


@dataclass(frozen=True)
class Candidate:
    script_pubkey: str
    script_hash: str
    source_height: int
    source_txid: str
    source_vout: int


def normalize_hex(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise AuditError(f"{field} must be a hex string")
    normalized = value.lower()
    if len(normalized) % 2 != 0:
        raise AuditError(f"{field} has odd-length hex")
    try:
        bytes.fromhex(normalized)
    except ValueError as exc:
        raise AuditError(f"{field} contains invalid hex") from exc
    return normalized


def script_hash_from_hex(script_pubkey: str) -> str:
    """Returns balance-history's Electrum-compatible reversed SHA-256 hash."""

    script = bytes.fromhex(normalize_hex(script_pubkey, "scriptPubKey"))
    return hashlib.sha256(script).digest()[::-1].hex()


def is_core_unspendable(script_pubkey: str) -> bool:
    """Matches Bitcoin Core's script UTXO exclusion rule."""

    script = bytes.fromhex(normalize_hex(script_pubkey, "scriptPubKey"))
    return len(script) > MAX_SCRIPT_SIZE or bool(script and script[0] == OP_RETURN)


def amount_to_sat(value: object) -> int:
    try:
        amount = Decimal(str(value))
    except (InvalidOperation, ValueError) as exc:
        raise AuditError(f"Invalid BTC amount: {value!r}") from exc
    satoshis = amount * SATOSHIS_PER_BTC
    integral = satoshis.to_integral_value()
    if not amount.is_finite() or satoshis != integral or integral < 0:
        raise AuditError(f"BTC amount is not an exact non-negative satoshi value: {value!r}")
    return int(integral)


def deterministic_heights(
    start_height: int,
    end_height: int,
    block_count: int,
    rng: random.Random,
) -> list[int]:
    if start_height < 0 or end_height < start_height:
        raise AuditError(
            f"Invalid candidate source height range: {start_height}..{end_height}"
        )
    available = end_height - start_height + 1
    count = min(block_count, available)
    if count <= 0:
        raise AuditError("source block count must be greater than zero")
    return sorted(rng.sample(range(start_height, end_height + 1), count))


def collect_candidates(
    bitcoin: Rpc,
    heights: list[int],
    max_candidates: int,
    rng: random.Random,
) -> list[Candidate]:
    by_script: dict[str, Candidate] = {}
    for height in heights:
        block_hash = bitcoin.call("getblockhash", [height])
        block = bitcoin.call("getblock", [block_hash, 2])
        if block.get("height") != height or block.get("hash") != block_hash:
            raise AuditError(f"Bitcoin Core returned inconsistent block data at height {height}")
        for transaction in block.get("tx", []):
            txid = transaction.get("txid")
            if not isinstance(txid, str):
                raise AuditError(f"Block {height} contains a transaction without txid")
            for output in transaction.get("vout", []):
                script = output.get("scriptPubKey") or {}
                script_hex = normalize_hex(script.get("hex"), "scriptPubKey.hex")
                # raw() descriptors require a concrete hex payload. Empty scripts are
                # valid but vanishingly rare and are not useful as an address sample.
                if not script_hex or is_core_unspendable(script_hex):
                    continue
                vout = output.get("n")
                if not isinstance(vout, int) or vout < 0:
                    raise AuditError(f"Transaction {txid} contains an invalid vout index")
                by_script.setdefault(
                    script_hex,
                    Candidate(
                        script_pubkey=script_hex,
                        script_hash=script_hash_from_hex(script_hex),
                        source_height=height,
                        source_txid=txid,
                        source_vout=vout,
                    ),
                )

    candidates = list(by_script.values())
    rng.shuffle(candidates)
    return candidates[:max_candidates]


def collect_touched_scripts(bitcoin: Rpc, start_height: int, end_height: int) -> set[str]:
    """Collects scripts created or spent in an inclusive recent block range."""

    touched: set[str] = set()
    for height in range(start_height, end_height + 1):
        block_hash = bitcoin.call("getblockhash", [height])
        block = bitcoin.call("getblock", [block_hash, 3])
        if block.get("height") != height or block.get("hash") != block_hash:
            raise AuditError(f"Bitcoin Core returned inconsistent block data at height {height}")
        for transaction in block.get("tx", []):
            for output in transaction.get("vout", []):
                script = output.get("scriptPubKey") or {}
                script_hex = normalize_hex(script.get("hex"), "scriptPubKey.hex")
                if not is_core_unspendable(script_hex):
                    touched.add(script_hex)
            for tx_input in transaction.get("vin", []):
                if "coinbase" in tx_input:
                    continue
                prevout = tx_input.get("prevout")
                if not isinstance(prevout, dict):
                    raise AuditError(
                        "getblock verbosity=3 omitted an input prevout; recent undo data is "
                        "required to compare stable-lag snapshots without an unbounded txindex scan"
                    )
                script = prevout.get("scriptPubKey") or {}
                script_hex = normalize_hex(script.get("hex"), "vin.prevout.scriptPubKey.hex")
                if not is_core_unspendable(script_hex):
                    touched.add(script_hex)
    return touched


def group_scantxoutset_unspents(
    scan_result: dict[str, Any], candidates: list[Candidate]
) -> tuple[dict[str, int], dict[str, list[dict[str, Any]]]]:
    candidate_scripts = {candidate.script_pubkey for candidate in candidates}
    balances = {script: 0 for script in candidate_scripts}
    unspents_by_script = {script: [] for script in candidate_scripts}
    seen_outpoints: set[tuple[str, int]] = set()

    for unspent in scan_result.get("unspents", []):
        script = normalize_hex(unspent.get("scriptPubKey"), "unspent.scriptPubKey")
        if script not in candidate_scripts:
            raise AuditError(f"scantxoutset returned an unrequested script: {script}")
        txid = unspent.get("txid")
        vout = unspent.get("vout")
        if not isinstance(txid, str) or not isinstance(vout, int) or vout < 0:
            raise AuditError("scantxoutset returned an invalid outpoint")
        outpoint = (txid, vout)
        if outpoint in seen_outpoints:
            raise AuditError(f"scantxoutset returned duplicate outpoint {txid}:{vout}")
        seen_outpoints.add(outpoint)
        value_sat = amount_to_sat(unspent.get("amount"))
        balances[script] += value_sat
        normalized = dict(unspent)
        normalized["scriptPubKey"] = script
        normalized["value_sat"] = value_sat
        unspents_by_script[script].append(normalized)

    return balances, unspents_by_script


def query_balance_history_balances(
    balance_history: Rpc,
    candidates: list[Candidate],
    stable_height: int,
) -> dict[str, int]:
    rows = balance_history.call(
        "get_addresses_balances",
        [{
            "script_hashes": [candidate.script_hash for candidate in candidates],
            "block_height": stable_height,
            "block_range": None,
        }],
    )
    if not isinstance(rows, list) or len(rows) != len(candidates):
        raise AuditError("get_addresses_balances returned an unexpected result count")

    balances: dict[str, int] = {}
    for candidate, history in zip(candidates, rows):
        if not isinstance(history, list) or len(history) != 1:
            raise AuditError(
                f"get_addresses_balances returned an invalid row for {candidate.script_hash}"
            )
        balance = history[0].get("balance")
        if not isinstance(balance, int) or balance < 0:
            raise AuditError(
                f"get_addresses_balances returned an invalid balance for {candidate.script_hash}"
            )
        balances[candidate.script_pubkey] = balance
    return balances


def select_gettxout_checks(
    candidates: list[Candidate],
    unspents_by_script: dict[str, list[dict[str, Any]]],
    limit: int,
    rng: random.Random,
) -> tuple[list[dict[str, Any]], int]:
    all_unspents = [
        unspent
        for candidate in candidates
        for unspent in unspents_by_script[candidate.script_pubkey]
    ]
    total = len(all_unspents)
    if limit < 0:
        raise AuditError("max gettxout checks cannot be negative")
    if total > limit:
        all_unspents = rng.sample(all_unspents, limit)
    return sorted(all_unspents, key=lambda item: (item["txid"], item["vout"])), total


def verify_gettxout(bitcoin: Rpc, checks: list[dict[str, Any]], scan_hash: str) -> None:
    for expected in checks:
        actual = bitcoin.call("gettxout", [expected["txid"], expected["vout"], False])
        if actual is None:
            raise AuditError(
                f"gettxout no longer reports scanned outpoint {expected['txid']}:{expected['vout']}"
            )
        actual_script = normalize_hex(
            (actual.get("scriptPubKey") or {}).get("hex"), "gettxout.scriptPubKey.hex"
        )
        actual_sat = amount_to_sat(actual.get("value"))
        if actual_script != expected["scriptPubKey"] or actual_sat != expected["value_sat"]:
            raise AuditError(
                f"gettxout mismatch for {expected['txid']}:{expected['vout']}: "
                f"expected script={expected['scriptPubKey']} value_sat={expected['value_sat']}, "
                f"actual script={actual_script} value_sat={actual_sat}"
            )
        if actual.get("bestblock") != scan_hash:
            raise AuditError(
                f"Bitcoin Core tip changed during gettxout verification: "
                f"expected {scan_hash}, got {actual.get('bestblock')}"
            )


def expected_core_chain(network: str) -> str:
    mapping = {
        "bitcoin": "main",
        "testnet": "test",
        "testnet4": "testnet4",
        "signet": "signet",
        "regtest": "regtest",
    }
    try:
        return mapping[network]
    except KeyError as exc:
        raise AuditError(f"Unsupported expected network: {network}") from exc


def run_audit(
    bitcoin: Rpc,
    balance_history: Rpc,
    *,
    expected_network: str,
    sample_size: int,
    oversample_factor: int,
    source_lookback_blocks: int,
    source_block_count: int,
    max_gettxout_checks: int,
    seed: int,
) -> dict[str, Any]:
    if sample_size <= 0 or oversample_factor <= 0:
        raise AuditError("sample size and oversample factor must be greater than zero")
    if source_lookback_blocks <= 0 or source_block_count <= 0:
        raise AuditError("source lookback and source block count must be greater than zero")

    core_info = bitcoin.call("getblockchaininfo")
    core_chain = core_info.get("chain")
    if core_chain != expected_core_chain(expected_network):
        raise AuditError(
            f"Bitcoin Core network mismatch: expected {expected_core_chain(expected_network)}, "
            f"got {core_chain}"
        )
    service_network = balance_history.call("get_network_type")
    if service_network != expected_network:
        raise AuditError(
            f"balance-history network mismatch: expected {expected_network}, got {service_network}"
        )

    snapshot = balance_history.call("get_snapshot_info")
    stable_height = snapshot.get("stable_height")
    stable_hash = snapshot.get("stable_block_hash")
    stable_lag = snapshot.get("stable_lag")
    balance_floor = snapshot.get("balance_query_floor")
    if (
        not isinstance(stable_height, int)
        or stable_height < 0
        or not isinstance(stable_hash, str)
        or not isinstance(stable_lag, int)
        or stable_lag < 0
        or not isinstance(balance_floor, int)
        or balance_floor < 0
    ):
        raise AuditError("get_snapshot_info returned incomplete stable snapshot metadata")
    if bitcoin.call("getblockhash", [stable_height]) != stable_hash:
        raise AuditError("Bitcoin Core canonical hash does not match balance-history stable hash")

    source_start = max(balance_floor, stable_height - source_lookback_blocks + 1)
    candidate_rng = random.Random(seed)
    heights = deterministic_heights(
        source_start, stable_height, source_block_count, candidate_rng
    )
    candidate_limit = sample_size * oversample_factor
    candidates = collect_candidates(bitcoin, heights, candidate_limit, candidate_rng)
    if len(candidates) < sample_size:
        raise AuditError(
            f"Only {len(candidates)} unique spendable scripts were found; "
            f"need at least {sample_size}"
        )

    descriptors = [f"raw({candidate.script_pubkey})" for candidate in candidates]
    scan = bitcoin.call("scantxoutset", ["start", descriptors])
    if not isinstance(scan, dict) or scan.get("success") is not True:
        raise AuditError("scantxoutset did not complete successfully")
    scan_height = scan.get("height")
    scan_hash = scan.get("bestblock")
    if not isinstance(scan_height, int) or scan_height < stable_height or not isinstance(scan_hash, str):
        raise AuditError("scantxoutset returned an invalid scan anchor")
    if bitcoin.call("getblockhash", [scan_height]) != scan_hash:
        raise AuditError("scantxoutset bestblock is no longer canonical")

    core_balances, unspents_by_script = group_scantxoutset_unspents(scan, candidates)
    touched = collect_touched_scripts(bitcoin, stable_height + 1, scan_height)
    retained = [candidate for candidate in candidates if candidate.script_pubkey not in touched]
    selected = retained[:sample_size]
    if len(selected) < sample_size:
        raise AuditError(
            f"Only {len(selected)} of {len(candidates)} candidate scripts were untouched "
            f"between stable height {stable_height} and scan height {scan_height}; "
            f"increase the oversample factor"
        )

    history_balances = query_balance_history_balances(
        balance_history, selected, stable_height
    )
    mismatches = []
    for candidate in selected:
        core_balance = core_balances[candidate.script_pubkey]
        history_balance = history_balances[candidate.script_pubkey]
        if core_balance != history_balance:
            mismatches.append({
                "script_hash": candidate.script_hash,
                "script_pubkey": candidate.script_pubkey,
                "source_height": candidate.source_height,
                "source_outpoint": f"{candidate.source_txid}:{candidate.source_vout}",
                "balance_history_sat": history_balance,
                "bitcoin_core_sat": core_balance,
            })

    check_rng = random.Random(seed ^ 0x5554584F)
    checks, total_selected_unspents = select_gettxout_checks(
        selected, unspents_by_script, max_gettxout_checks, check_rng
    )
    if max_gettxout_checks > 0 and total_selected_unspents == 0:
        raise AuditError(
            "Selected scripts have no live outpoints, so gettxout could not be cross-checked; "
            "increase the sample size or source lookback"
        )
    verify_gettxout(bitcoin, checks, scan_hash)

    if bitcoin.call("getblockcount") != scan_height:
        raise AuditError("Bitcoin Core tip height changed during the audit; rerun the audit")
    if bitcoin.call("getbestblockhash") != scan_hash:
        raise AuditError("Bitcoin Core best block changed during the audit; rerun the audit")
    if bitcoin.call("getblockhash", [stable_height]) != stable_hash:
        raise AuditError("Bitcoin Core stable-height block changed during the audit")
    stable_commit = balance_history.call("get_block_commit", [stable_height])
    if not isinstance(stable_commit, dict) or stable_commit.get("btc_block_hash") != stable_hash:
        raise AuditError("balance-history stable block commit changed during the audit")

    samples = [{
        "script_hash": candidate.script_hash,
        "script_pubkey": candidate.script_pubkey,
        "source_height": candidate.source_height,
        "source_outpoint": f"{candidate.source_txid}:{candidate.source_vout}",
        "balance_history_sat": history_balances[candidate.script_pubkey],
        "bitcoin_core_sat": core_balances[candidate.script_pubkey],
        "live_utxo_count": len(unspents_by_script[candidate.script_pubkey]),
    } for candidate in selected]
    report = {
        "schema": "balance-history-bitcoin-core-utxo-audit:v1",
        "ok": not mismatches,
        "expected_network": expected_network,
        "bitcoin_core_pruned": core_info.get("pruned"),
        "seed": seed,
        "stable_height": stable_height,
        "stable_block_hash": stable_hash,
        "stable_lag": stable_lag,
        "balance_history_api_version": snapshot.get("balance_history_api_version"),
        "balance_history_semantics_version": snapshot.get(
            "balance_history_semantics_version"
        ),
        "commit_protocol_version": snapshot.get("commit_protocol_version"),
        "scan_height": scan_height,
        "scan_block_hash": scan_hash,
        "source_height_start": source_start,
        "source_height_end": stable_height,
        "source_block_heights": heights,
        "candidate_count": len(candidates),
        "lag_window_touched_candidate_count": len(candidates) - len(retained),
        "verified_script_count": len(selected),
        "selected_unspent_count": total_selected_unspents,
        "gettxout_checked_count": len(checks),
        "mismatch_count": len(mismatches),
        "mismatches": mismatches,
        "samples": samples,
    }
    return report


def load_cookie(path: Path) -> tuple[str, str]:
    try:
        value = path.read_text().strip()
    except OSError as exc:
        raise AuditError(f"Failed to read Bitcoin RPC cookie {path}: {exc}") from exc
    if ":" not in value:
        raise AuditError(f"Bitcoin RPC cookie {path} has invalid format")
    username, password = value.split(":", 1)
    return username, password


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitcoin-rpc-url", required=True)
    parser.add_argument("--balance-history-url", required=True)
    parser.add_argument(
        "--expected-network",
        required=True,
        choices=("bitcoin", "testnet", "testnet4", "signet", "regtest"),
    )
    auth = parser.add_mutually_exclusive_group(required=True)
    auth.add_argument("--bitcoin-cookie-file", type=Path)
    auth.add_argument("--bitcoin-rpc-user")
    parser.add_argument("--bitcoin-rpc-password")
    parser.add_argument("--sample-size", type=int, default=32)
    parser.add_argument("--oversample-factor", type=int, default=4)
    parser.add_argument("--source-lookback-blocks", type=int, default=2016)
    parser.add_argument("--source-block-count", type=int, default=24)
    parser.add_argument("--max-gettxout-checks", type=int, default=256)
    parser.add_argument("--seed", type=int, default=20260827)
    parser.add_argument("--timeout", type=float, default=1800.0)
    parser.add_argument("--output", type=Path)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.bitcoin_cookie_file is not None:
            username, password = load_cookie(args.bitcoin_cookie_file)
        else:
            username = args.bitcoin_rpc_user
            password = args.bitcoin_rpc_password
            if password is None:
                raise AuditError(
                    "--bitcoin-rpc-password is required with --bitcoin-rpc-user"
                )
        bitcoin = JsonRpcClient(
            args.bitcoin_rpc_url,
            username=username,
            password=password,
            timeout=args.timeout,
        )
        balance_history = JsonRpcClient(args.balance_history_url, timeout=args.timeout)
        report = run_audit(
            bitcoin,
            balance_history,
            expected_network=args.expected_network,
            sample_size=args.sample_size,
            oversample_factor=args.oversample_factor,
            source_lookback_blocks=args.source_lookback_blocks,
            source_block_count=args.source_block_count,
            max_gettxout_checks=args.max_gettxout_checks,
            seed=args.seed,
        )
        encoded = json.dumps(report, indent=2, sort_keys=True)
        if args.output is not None:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded + "\n")
        print(encoded)
        return 0 if report["ok"] else 1
    except (AuditError, OSError) as exc:
        failure = {
            "schema": "balance-history-bitcoin-core-utxo-audit:v1",
            "ok": False,
            "error": str(exc),
        }
        if args.output is not None:
            try:
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(
                    json.dumps(failure, indent=2, sort_keys=True) + "\n"
                )
            except OSError as output_error:
                print(
                    f"failed to write audit failure report {args.output}: {output_error}",
                    file=sys.stderr,
                )
        print(f"balance-history UTXO audit failed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
