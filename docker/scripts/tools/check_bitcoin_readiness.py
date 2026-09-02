#!/usr/bin/env python3
"""Wait for a release Bitcoin Core node to satisfy the USDB sync contract."""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class BitcoinReadiness:
    chain: str
    blocks: int
    headers: int
    initial_block_download: bool
    pruned: bool
    txindex_synced: bool
    txindex_height: int
    tip_time: int
    tip_age_secs: int
    network_active: bool
    connections: int
    verification_progress: float


def require_type(value: Any, expected: type, path: str) -> Any:
    if type(value) is not expected:
        raise ValueError(f"{path} must be {expected.__name__}")
    return value


def require_number(value: Any, path: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{path} must be a number")
    return float(value)


def assess_readiness(
    blockchain_info: dict[str, Any],
    index_info: dict[str, Any],
    network_info: dict[str, Any],
    expected_chain: str,
    minimum_height: int,
    maximum_tip_age_secs: int,
    minimum_connections: int,
    now_timestamp: int | None = None,
) -> tuple[BitcoinReadiness, list[str]]:
    chain = require_type(blockchain_info.get("chain"), str, "getblockchaininfo.chain")
    blocks = require_type(blockchain_info.get("blocks"), int, "getblockchaininfo.blocks")
    headers = require_type(blockchain_info.get("headers"), int, "getblockchaininfo.headers")
    initial_block_download = require_type(
        blockchain_info.get("initialblockdownload"),
        bool,
        "getblockchaininfo.initialblockdownload",
    )
    pruned = require_type(blockchain_info.get("pruned"), bool, "getblockchaininfo.pruned")
    tip_time = require_type(blockchain_info.get("time"), int, "getblockchaininfo.time")
    verification_progress = require_number(
        blockchain_info.get("verificationprogress"),
        "getblockchaininfo.verificationprogress",
    )
    if not 0.0 <= verification_progress <= 1.0:
        raise ValueError("getblockchaininfo.verificationprogress must be between 0 and 1")
    now = int(time.time()) if now_timestamp is None else now_timestamp
    tip_age_secs = max(0, now - tip_time)

    txindex = index_info.get("txindex")
    if not isinstance(txindex, dict):
        raise ValueError("getindexinfo.txindex is missing; start Bitcoin Core with txindex=1")
    txindex_synced = require_type(txindex.get("synced"), bool, "getindexinfo.txindex.synced")
    txindex_height = require_type(
        txindex.get("best_block_height"),
        int,
        "getindexinfo.txindex.best_block_height",
    )
    network_active = require_type(network_info.get("networkactive"), bool, "getnetworkinfo.networkactive")
    connections = require_type(network_info.get("connections"), int, "getnetworkinfo.connections")

    failures: list[str] = []
    if chain != expected_chain:
        failures.append(f"chain={chain}, expected {expected_chain}")
    if pruned:
        failures.append("pruned=true, a full non-pruned node is required")
    if initial_block_download:
        failures.append("initialblockdownload=true")
    if blocks < minimum_height:
        failures.append(f"blocks={blocks}, minimum {minimum_height}")
    if blocks != headers:
        failures.append(f"blocks={blocks} does not match headers={headers}")
    if not txindex_synced:
        failures.append("txindex.synced=false")
    if txindex_height != blocks:
        failures.append(f"txindex height={txindex_height} does not match blocks={blocks}")
    if tip_age_secs > maximum_tip_age_secs:
        failures.append(f"tip age={tip_age_secs}s exceeds maximum {maximum_tip_age_secs}s")
    if not network_active:
        failures.append("networkactive=false")
    if connections < minimum_connections:
        failures.append(f"connections={connections}, minimum {minimum_connections}")
    return BitcoinReadiness(
        chain=chain,
        blocks=blocks,
        headers=headers,
        initial_block_download=initial_block_download,
        pruned=pruned,
        txindex_synced=txindex_synced,
        txindex_height=txindex_height,
        tip_time=tip_time,
        tip_age_secs=tip_age_secs,
        network_active=network_active,
        connections=connections,
        verification_progress=verification_progress,
    ), failures


def evaluate_readiness(
    blockchain_info: dict[str, Any],
    index_info: dict[str, Any],
    network_info: dict[str, Any],
    expected_chain: str,
    minimum_height: int,
    maximum_tip_age_secs: int,
    minimum_connections: int,
    now_timestamp: int | None = None,
) -> BitcoinReadiness:
    status, failures = assess_readiness(
        blockchain_info,
        index_info,
        network_info,
        expected_chain,
        minimum_height,
        maximum_tip_age_secs,
        minimum_connections,
        now_timestamp,
    )
    if failures:
        raise ValueError("; ".join(failures))
    return status


def readiness_report(
    status: BitcoinReadiness | None,
    blockers: list[str],
) -> dict[str, Any]:
    return {
        "schema_version": "usdb-bitcoin-readiness:v1",
        "ready": status is not None and not blockers,
        "status": asdict(status) if status is not None else None,
        "blockers": blockers,
    }


def _duration_text(seconds: float) -> str:
    elapsed = max(0, int(seconds))
    hours, remainder = divmod(elapsed, 3600)
    minutes, secs = divmod(remainder, 60)
    return f"{hours:02d}:{minutes:02d}:{secs:02d}"


def format_wait_progress(
    status: BitcoinReadiness | None,
    blockers: list[str],
    elapsed_secs: float,
) -> str:
    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    prefix = f"[{timestamp}] [usdb-node] Bitcoin readiness waiting: elapsed={_duration_text(elapsed_secs)}"
    if status is None:
        return f"{prefix}, detail={'; '.join(blockers)}"
    txindex = "synced" if status.txindex_synced else "indexing"
    detail = "; ".join(blockers)
    return (
        f"{prefix}, blocks={status.blocks}/{status.headers}, "
        f"verification={status.verification_progress * 100:.2f}%, "
        f"txindex={txindex}@{status.txindex_height}, peers={status.connections}, "
        f"blockers={detail}"
    )


class BitcoinRpc:
    def __init__(self, url: str, user: str, password: str, timeout_secs: float) -> None:
        self.url = url
        self.timeout_secs = timeout_secs
        token = base64.b64encode(f"{user}:{password}".encode()).decode()
        self.authorization = f"Basic {token}"

    def call(self, method: str) -> dict[str, Any]:
        body = json.dumps({"jsonrpc": "2.0", "id": "usdb-readiness", "method": method, "params": []}).encode()
        request = urllib.request.Request(
            self.url,
            data=body,
            headers={"Authorization": self.authorization, "Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_secs) as response:
                value = json.load(response)
        except (OSError, urllib.error.URLError, json.JSONDecodeError) as exc:
            raise ValueError(f"Bitcoin RPC {method} failed: {exc}") from exc
        if not isinstance(value, dict):
            raise ValueError(f"Bitcoin RPC {method} returned a non-object response")
        if value.get("error") is not None:
            raise ValueError(f"Bitcoin RPC {method} returned error: {value['error']}")
        result = value.get("result")
        if not isinstance(result, dict):
            raise ValueError(f"Bitcoin RPC {method} result must be an object")
        return result


def read_password(args: argparse.Namespace) -> str:
    if args.password_file:
        try:
            password = Path(args.password_file).read_text(encoding="utf-8").strip()
        except OSError as exc:
            raise ValueError(f"failed to read Bitcoin RPC password file: {exc}") from exc
    else:
        password = os.environ.get("BTC_RPC_PASSWORD", "")
    if not password:
        raise ValueError("Bitcoin RPC password is required")
    return password


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", default=os.environ.get("BTC_RPC_URL", "http://127.0.0.1:8332"))
    parser.add_argument("--user", default=os.environ.get("BTC_RPC_USER", ""))
    parser.add_argument("--password-file", default=os.environ.get("BTC_RPC_PASSWORD_FILE", ""))
    parser.add_argument("--expected-chain", default=os.environ.get("BTC_READY_EXPECTED_CHAIN", "main"))
    parser.add_argument(
        "--minimum-height",
        type=int,
        default=int(os.environ.get("BTC_MIN_READY_HEIGHT", "963800")),
    )
    parser.add_argument(
        "--maximum-tip-age-secs",
        type=int,
        default=int(os.environ.get("BTC_MAX_TIP_AGE_SECS", "7200")),
    )
    parser.add_argument(
        "--minimum-connections",
        type=int,
        default=int(os.environ.get("BTC_MIN_CONNECTIONS", "1")),
    )
    parser.add_argument("--rpc-timeout-secs", type=float, default=5.0)
    parser.add_argument("--wait-timeout-secs", type=float, default=0.0)
    parser.add_argument("--poll-interval-secs", type=float, default=5.0)
    parser.add_argument(
        "--progress-interval-secs",
        type=float,
        default=float(os.environ.get("BTC_READY_PROGRESS_INTERVAL_SECS", "60")),
    )
    parser.add_argument(
        "--status-json",
        action="store_true",
        help="emit one structured ready/not-ready report without failing for sync lag",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.user:
        raise ValueError("Bitcoin RPC user is required")
    if args.minimum_height < 0:
        raise ValueError("minimum height must be non-negative")
    if args.maximum_tip_age_secs < 0:
        raise ValueError("maximum tip age must be non-negative")
    if args.minimum_connections < 0:
        raise ValueError("minimum connections must be non-negative")
    if args.rpc_timeout_secs <= 0 or args.poll_interval_secs <= 0:
        raise ValueError("RPC timeout and poll interval must be positive")
    if args.progress_interval_secs < 0:
        raise ValueError("progress interval must be non-negative")
    password = read_password(args)
    rpc = BitcoinRpc(args.url, args.user, password, args.rpc_timeout_secs)
    started_at = time.monotonic()
    deadline = time.monotonic() + args.wait_timeout_secs
    next_progress_at = started_at
    last_status: BitcoinReadiness | None = None
    last_blockers = ["readiness check did not run"]

    while True:
        try:
            status, blockers = assess_readiness(
                rpc.call("getblockchaininfo"),
                rpc.call("getindexinfo"),
                rpc.call("getnetworkinfo"),
                args.expected_chain,
                args.minimum_height,
                args.maximum_tip_age_secs,
                args.minimum_connections,
            )
            last_status = status
            last_blockers = blockers
        except ValueError as exc:
            last_status = None
            last_blockers = [str(exc)]

        if args.status_json:
            print(json.dumps(readiness_report(last_status, last_blockers), sort_keys=True))
            return 0
        if last_status is not None and not last_blockers:
            print(json.dumps(asdict(last_status), sort_keys=True))
            return 0

        now = time.monotonic()
        if (
            args.wait_timeout_secs > 0
            and args.progress_interval_secs > 0
            and now >= next_progress_at
        ):
            print(
                format_wait_progress(last_status, last_blockers, now - started_at),
                file=sys.stderr,
                flush=True,
            )
            next_progress_at = now + args.progress_interval_secs
        if now >= deadline:
            raise ValueError("; ".join(last_blockers))
        time.sleep(args.poll_interval_secs)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as exc:
        print(f"Bitcoin readiness failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
