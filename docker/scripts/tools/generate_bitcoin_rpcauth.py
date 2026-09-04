#!/usr/bin/env python3
"""Create a fail-closed Bitcoin Core rpcauth file and one-time client secret."""

from __future__ import annotations

import argparse
import hmac
import json
import os
import re
import secrets
import stat
import sys
import tempfile
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


def _absolute_path_without_following_final_symlink(path: Path) -> Path:
    return Path(os.path.abspath(path.expanduser()))


def _read_private_node_env(node_env: Path) -> str:
    if node_env.is_symlink() or not node_env.is_file():
        raise ValueError(f"node env must be a regular file: {node_env}")
    mode = stat.S_IMODE(node_env.stat().st_mode)
    if mode & 0o077:
        raise ValueError(f"node env must not be group/world accessible: {node_env}")
    return node_env.read_text(encoding="utf-8")


def _node_env_value(content: str, key: str, node_env: Path) -> str:
    values: list[str] = []
    for line in content.splitlines():
        candidate, separator, value = line.partition("=")
        if separator and candidate == key:
            values.append(value)
    if len(values) != 1:
        raise ValueError(f"node env must contain exactly one {key}: {node_env}")
    return values[0]


def _render_node_env(node_env: Path, content: str, updates: dict[str, str]) -> str:
    seen: set[str] = set()
    rendered: list[str] = []
    for line in content.splitlines():
        key = line.split("=", 1)[0]
        if key not in updates:
            rendered.append(line)
            continue
        if key in seen:
            raise ValueError(f"node env contains duplicate {key}: {node_env}")
        seen.add(key)
        rendered.append(f"{key}={updates[key]}")

    missing = sorted(set(updates) - seen)
    if missing:
        raise ValueError(f"node env is missing required fields {', '.join(missing)}: {node_env}")
    return "\n".join(rendered) + "\n"


def _atomic_write_private(path: Path, content: str) -> None:
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.",
        dir=path.parent,
        text=True,
    )
    temporary_path = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o600)
        target = os.fdopen(descriptor, "w", encoding="utf-8")
        descriptor = -1
        with target:
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary_path, path)
    except BaseException:
        if descriptor >= 0:
            os.close(descriptor)
        temporary_path.unlink(missing_ok=True)
        raise


def install_into_node_env(username: str, output: Path, node_env: Path) -> dict[str, str]:
    node_env = _absolute_path_without_following_final_symlink(node_env)
    output = _absolute_path_without_following_final_symlink(output)
    original = _read_private_node_env(node_env)
    configured_rpcauth = _node_env_value(original, "BTC_RPCAUTH_HOST_FILE", node_env)
    _node_env_value(original, "BTC_RPC_USER", node_env)
    _node_env_value(original, "BTC_RPC_PASSWORD", node_env)
    if Path(configured_rpcauth).expanduser().resolve() != output.resolve():
        raise ValueError(
            "rpcauth output does not match BTC_RPCAUTH_HOST_FILE in node env: "
            f"output={output}, configured={configured_rpcauth}"
        )

    credentials = generate(username, output)
    try:
        current = _read_private_node_env(node_env)
        if current != original:
            raise ValueError(f"node env changed while generating rpcauth: {node_env}")
        rendered = _render_node_env(
            node_env,
            current,
            {
                "BTC_RPC_USER": credentials["username"],
                "BTC_RPC_PASSWORD": credentials["password"],
            },
        )
        _atomic_write_private(node_env, rendered)
    except BaseException:
        output.unlink(missing_ok=True)
        raise

    return {
        "username": credentials["username"],
        "rpcauth_file": str(output),
        "node_env": str(node_env),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--username", default="usdb-testnet")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--node-env",
        type=Path,
        required=True,
        help="Private node.env whose BTC RPC credentials will be updated atomically",
    )
    args = parser.parse_args()
    result = install_into_node_env(args.username, args.output, args.node_env)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError) as exc:
        print(f"Bitcoin rpcauth generation failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
