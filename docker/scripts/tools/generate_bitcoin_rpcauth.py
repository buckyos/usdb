#!/usr/bin/env python3
"""Create a fail-closed Bitcoin Core rpcauth file and one-time client secret."""

from __future__ import annotations

import argparse
import hmac
import json
import os
import re
import secrets
import sys
from pathlib import Path


def generate(username: str, output: Path) -> dict[str, str]:
    if re.fullmatch(r"[A-Za-z0-9._-]+", username) is None:
        raise ValueError("username contains unsupported characters")
    if output.exists():
        raise ValueError(f"refusing to replace existing rpcauth file: {output}")
    output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    password = secrets.token_urlsafe(32)
    salt = secrets.token_hex(16)
    digest = hmac.new(salt.encode(), password.encode(), "sha256").hexdigest()
    value = f"{username}:{salt}${digest}"
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as target:
        target.write(value + "\n")
    return {"username": username, "password": password, "rpcauth_file": str(output)}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--username", default="usdb-testnet")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(generate(args.username, args.output.expanduser().resolve()), sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as exc:
        print(f"Bitcoin rpcauth generation failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
