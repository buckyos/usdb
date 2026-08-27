#!/usr/bin/env python3
"""Inspect or wait for a USDB JSON-RPC service consensus readiness state."""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--expected-service", required=True)
    parser.add_argument("--require-consensus-ready", action="store_true")
    parser.add_argument("--wait-timeout-secs", type=int, default=0)
    parser.add_argument("--poll-interval-secs", type=float, default=10.0)
    parser.add_argument("--request-timeout-secs", type=float, default=5.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.wait_timeout_secs < 0:
        raise ValueError("wait timeout must not be negative")
    if args.poll_interval_secs <= 0 or args.request_timeout_secs <= 0:
        raise ValueError("poll and request timeouts must be positive")

    deadline = time.monotonic() + args.wait_timeout_secs
    last_error: Exception | None = None
    while True:
        try:
            result = fetch_readiness(args.url, args.request_timeout_secs)
            ready = validate_readiness(result, args.expected_service)
            print(json.dumps(result, indent=2, sort_keys=True))
            if not args.require_consensus_ready or ready:
                return 0
            last_error = ValueError("consensus_ready is false")
        except (OSError, ValueError, urllib.error.URLError, json.JSONDecodeError) as exc:
            last_error = exc

        if args.wait_timeout_secs == 0 or time.monotonic() >= deadline:
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
