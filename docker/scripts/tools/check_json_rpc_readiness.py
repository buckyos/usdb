#!/usr/bin/env python3
"""Inspect or wait for a USDB JSON-RPC service consensus readiness state."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from typing import Any


def fetch_readiness(url: str, timeout_secs: float) -> dict[str, Any]:
    payload = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": "get_readiness", "params": []}
    ).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout_secs) as response:
        body = json.load(response)
    if not isinstance(body, dict):
        raise ValueError("JSON-RPC response must be an object")
    if body.get("error") is not None:
        raise ValueError(f"JSON-RPC returned an error: {body['error']}")
    result = body.get("result")
    if not isinstance(result, dict):
        raise ValueError("JSON-RPC readiness result must be an object")
    return result


def validate_readiness(result: dict[str, Any], expected_service: str) -> bool:
    service = result.get("service")
    if service != expected_service:
        raise ValueError(f"readiness service mismatch: expected {expected_service}, got {service}")
    ready = result.get("consensus_ready")
    if not isinstance(ready, bool):
        raise ValueError("readiness consensus_ready must be a boolean")
    return ready


def validate_balance_history_origin(
    result: dict[str, Any],
    expected_service: str,
    minimum_stable_height: int,
) -> bool:
    """Accept query-complete balance-history state without requiring tip catch-up."""
    if expected_service != "balance-history":
        raise ValueError("minimum stable height is only valid for balance-history")
    validate_readiness(result, expected_service)
    query_ready = result.get("query_ready")
    if not isinstance(query_ready, bool):
        raise ValueError("readiness query_ready must be a boolean")
    stable_height = result.get("stable_height")
    if not isinstance(stable_height, int) or isinstance(stable_height, bool):
        raise ValueError("readiness stable_height must be an integer")
    stable_block_hash = result.get("stable_block_hash")
    latest_block_commit = result.get("latest_block_commit")
    blockers = result.get("blockers")
    if not isinstance(blockers, list):
        raise ValueError("readiness blockers must be an array")

    def is_sha256(value: Any) -> bool:
        if not isinstance(value, str) or len(value) != 64 or value != value.lower():
            return False
        try:
            int(value, 16)
        except ValueError:
            return False
        return True

    return (
        query_ready
        and stable_height >= minimum_stable_height
        and is_sha256(stable_block_hash)
        and is_sha256(latest_block_commit)
        and "SnapshotInstallUnverified" not in blockers
    )


def _duration_text(seconds: float) -> str:
    elapsed = max(0, int(seconds))
    hours, remainder = divmod(elapsed, 3600)
    minutes, secs = divmod(remainder, 60)
    return f"{hours:02d}:{minutes:02d}:{secs:02d}"


def _progress_text(result: dict[str, Any]) -> str:
    current = result.get("current")
    total = result.get("total")
    fields: list[str] = []
    if isinstance(current, int) and not isinstance(current, bool):
        progress = f"progress={current}"
        if isinstance(total, int) and not isinstance(total, bool):
            progress += f"/{total}"
            if total > 0:
                progress += f" ({min(100.0, current * 100.0 / total):.2f}%)"
        fields.append(progress)
    for key in ("phase", "stable_height", "synced_block_height", "balance_history_stable_height"):
        value = result.get(key)
        if isinstance(value, (str, int)) and not isinstance(value, bool):
            fields.append(f"{key}={value}")
    blockers = result.get("blockers")
    if isinstance(blockers, list) and blockers:
        fields.append("blockers=" + ",".join(str(item) for item in blockers))
    message = result.get("message")
    if isinstance(message, str) and message:
        fields.append(f"message={message}")
    return ", ".join(fields) if fields else "readiness response has no progress fields"


def format_wait_progress(
    expected_service: str,
    elapsed_secs: float,
    result: dict[str, Any] | None,
    error: Exception | None,
) -> str:
    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    prefix = (
        f"[{timestamp}] [usdb-node] {expected_service} readiness waiting: "
        f"elapsed={_duration_text(elapsed_secs)}"
    )
    if result is not None:
        return f"{prefix}, {_progress_text(result)}"
    return f"{prefix}, detail={error}"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--expected-service", required=True)
    parser.add_argument("--require-consensus-ready", action="store_true")
    parser.add_argument(
        "--minimum-stable-height",
        type=int,
        help=(
            "accept query-ready balance-history after it has committed this height; "
            "does not require consensus-ready tip catch-up"
        ),
    )
    parser.add_argument("--wait-timeout-secs", type=int, default=0)
    parser.add_argument("--poll-interval-secs", type=float, default=10.0)
    parser.add_argument("--request-timeout-secs", type=float, default=5.0)
    parser.add_argument(
        "--progress-interval-secs",
        type=float,
        default=float(os.environ.get("USDB_READINESS_PROGRESS_INTERVAL_SECS", "30")),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.wait_timeout_secs < 0:
        raise ValueError("wait timeout must not be negative")
    minimum_stable_height = getattr(args, "minimum_stable_height", None)
    if minimum_stable_height is not None and minimum_stable_height < 0:
        raise ValueError("minimum stable height must not be negative")
    if minimum_stable_height is not None and args.require_consensus_ready:
        raise ValueError(
            "minimum stable height and require-consensus-ready are separate gates"
        )
    if (
        args.poll_interval_secs <= 0
        or args.request_timeout_secs <= 0
        or args.progress_interval_secs < 0
    ):
        raise ValueError(
            "poll and request intervals must be positive; progress must be non-negative"
        )

    started_at = time.monotonic()
    deadline = time.monotonic() + args.wait_timeout_secs
    next_progress_at = started_at
    last_error: Exception | None = None
    last_result: dict[str, Any] | None = None
    while True:
        try:
            result = fetch_readiness(args.url, args.request_timeout_secs)
            consensus_ready = validate_readiness(result, args.expected_service)
            if minimum_stable_height is not None:
                ready = validate_balance_history_origin(
                    result,
                    args.expected_service,
                    minimum_stable_height,
                )
            else:
                ready = not args.require_consensus_ready or consensus_ready
            if ready:
                print(json.dumps(result, indent=2, sort_keys=True))
                return 0
            last_result = result
            last_error = ValueError(
                f"balance-history has not committed origin height {minimum_stable_height}"
                if minimum_stable_height is not None
                else "consensus_ready is false"
            )
        except (OSError, ValueError, urllib.error.URLError, json.JSONDecodeError) as exc:
            last_result = None
            last_error = exc

        now = time.monotonic()
        if (
            args.wait_timeout_secs > 0
            and args.progress_interval_secs > 0
            and now >= next_progress_at
        ):
            print(
                format_wait_progress(
                    args.expected_service,
                    now - started_at,
                    last_result,
                    last_error,
                ),
                file=sys.stderr,
                flush=True,
            )
            next_progress_at = now + args.progress_interval_secs
        if args.wait_timeout_secs == 0 or now >= deadline:
            print(
                f"{args.expected_service} readiness check failed: {last_error}",
                file=sys.stderr,
            )
            return 1
        time.sleep(min(args.poll_interval_secs, max(0.0, deadline - time.monotonic())))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as exc:
        print(f"readiness configuration error: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
