#!/usr/bin/env python3
"""Validate native bootstrap deployment inputs and render service configs."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import tempfile
from typing import Mapping


def unsigned(env: Mapping[str, str], key: str, default: int | None = None, *, minimum: int = 0, maximum: int = 0xFFFFFFFF) -> int:
    value = env.get(key, str(default) if default is not None else "")
    if not re.fullmatch(r"[0-9]+", value):
        raise ValueError(f"{key} must be an unsigned integer")
    number = int(value)
    if not minimum <= number <= maximum:
        raise ValueError(f"{key} must be between {minimum} and {maximum}")
    return number


def checkpoint_metadata(base_height: int) -> dict:
    """Read the same source catalog embedded in Rust; deployment cannot supply a commit seed."""
    if base_height != 935000:
        raise ValueError("This deployment version supports only mainnet AssumeUTXO height 935000")
    tools_dir = Path(__file__).resolve().parent
    packaged = tools_dir.parent / "data/assumeutxo/mainnet-935000.json"
    source = tools_dir.parents[2] / "src/btc/balance-history/src/bootstrap/checkpoints/mainnet-935000.json"
    path = packaged if packaged.is_file() else source
    checkpoint = json.loads(path.read_text(encoding="utf-8"))
    snapshot = checkpoint["snapshot"]
    if snapshot["network"] != "bitcoin" or snapshot["base_height"] != base_height:
        raise ValueError("Packaged AssumeUTXO catalog has a mismatched network or height")
    return snapshot


def validate_environment(env: Mapping[str, str], *, service: str = "balance-history") -> dict:
    """Reject incompatible bootstrap sources before changing config or starting a process."""
    if env.get("SNAPSHOT_MODE") != "assumeutxo":
        raise ValueError("Native bootstrap requires SNAPSHOT_MODE=assumeutxo")
    if env.get("BTC_NETWORK", "bitcoin") != "bitcoin":
        raise ValueError("Native deployment currently requires BTC_NETWORK=bitcoin")
    if env.get("INSCRIPTION_SOURCE", "bitcoind") != "bitcoind":
        raise ValueError("Native deployment requires INSCRIPTION_SOURCE=bitcoind")
    if env.get("INSCRIPTION_SOURCE_SHADOW_COMPARE", "false") != "false":
        raise ValueError("Native deployment requires INSCRIPTION_SOURCE_SHADOW_COMPARE=false")
    if env.get("BH_SCRIPT_REGISTRY_ENABLED", "0") != "0":
        raise ValueError("Native deployment does not install a script-registry sidecar")
    for key in (
        "BH_SNAPSHOT_FILE", "BH_SNAPSHOT_MANIFEST", "USDB_INDEXER_CHECKPOINT_MANIFEST",
        "BH_SCRIPT_REGISTRY_RECORD_URL", "BH_SCRIPT_REGISTRY_ARTIFACT_ID", "INSCRIPTION_FIXTURE_FILE",
    ):
        if env.get(key):
            raise ValueError(f"SNAPSHOT_MODE=assumeutxo requires empty {key}")
    base = unsigned(env, "BH_ASSUMEUTXO_BASE_HEIGHT", 935000)
    snapshot = checkpoint_metadata(base)
    genesis = unsigned(env, "USDB_GENESIS_BLOCK_HEIGHT", minimum=base, maximum=0xFFFFFFFE)
    result = dict(snapshot=snapshot, origin_height=genesis)
    if service == "indexer":
        return result
    origin_hash = env.get("BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH", "")
    if not re.fullmatch(r"[0-9a-f]{64}", origin_hash) or origin_hash == "0" * 64:
        raise ValueError("BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH must be a nonzero lowercase BTC block hash")
    if genesis == base and origin_hash != snapshot["base_hash"]:
        raise ValueError("G=B requires the origin block hash to equal the snapshot base hash")
    source = env.get("BH_ASSUMEUTXO_SNAPSHOT_FILE", "")
    if not source or not Path(source).is_absolute():
        raise ValueError("BH_ASSUMEUTXO_SNAPSHOT_FILE must be an absolute path")
    # A sealed native service intentionally does not reopen the source file on restart.
    # Rust verifies the file when importing, and owns schema/seal/canonical-chain checks.
    result.update(origin_block_hash=origin_hash, snapshot_file=source)
    limit = unsigned(env, "BH_SYNC_MAX_SYNC_BLOCK_HEIGHT", 0xFFFFFFFF)
    if limit < genesis:
        raise ValueError("BH_SYNC_MAX_SYNC_BLOCK_HEIGHT must be at least USDB_GENESIS_BLOCK_HEIGHT")
    return result


def render_config(env: Mapping[str, str]) -> str:
    """Render native TOML with escaped strings; include snapshot identity but no commit override."""
    identity = validate_environment(env)
    root = env.get("BH_ROOT_DIR", "/data/balance-history")
    if not Path(root).is_absolute() or Path(root) == Path("/"):
        raise ValueError("BH_ROOT_DIR must be an absolute dedicated service directory")
    sections = []

    def value_text(value: object) -> str:
        # TOML forbids literal DEL; retain full Unicode scalars instead of JSON surrogate pairs.
        return json.dumps(value, ensure_ascii=False).replace("\x7f", "\\u007f")

    def table(name: str, values: dict) -> None:
        sections.append(f"[{name}]\n" + "\n".join(f"{key} = {value_text(value)}" for key, value in values.items()))

    btc = dict(network="bitcoin", data_dir=env.get("BTC_DATA_DIR", "/data/bitcoin"),
               rpc_url=env.get("BTC_RPC_URL", "http://btc-node:8332"))
    auth = btc_auth(env)
    if isinstance(auth, str):
        btc["auth"] = auth
    table("btc", btc)
    if isinstance(auth, dict):
        table("btc.auth", auth)
    table("ordinals", dict(rpc_url=env.get("ORD_RPC_URL", "http://ord-server:28030")))
    table("electrs", dict(rpc_url=env.get("ELECTRS_RPC_URL", "tcp://electrs:50001")))
    table("sync", dict(
        local_loader_threshold=unsigned(env, "BH_SYNC_LOCAL_LOADER_THRESHOLD", 500),
        batch_size=unsigned(env, "BH_SYNC_BATCH_SIZE", 128, minimum=1),
        utxo_max_cache_bytes=unsigned(env, "BH_SYNC_UTXO_MAX_CACHE_BYTES", 2147483648, minimum=1048576, maximum=2**63-1),
        balance_max_cache_bytes=unsigned(env, "BH_SYNC_BALANCE_MAX_CACHE_BYTES", 6442450944, minimum=1048576, maximum=2**63-1),
        max_memory_percent=unsigned(env, "BH_SYNC_MAX_MEMORY_PERCENT", 85, minimum=20, maximum=95),
        max_sync_block_height=unsigned(env, "BH_SYNC_MAX_SYNC_BLOCK_HEIGHT", 0xFFFFFFFF),
    ))
    table("rpc_server", dict(host=env.get("BH_RPC_HOST", "0.0.0.0"), port=unsigned(env, "BH_RPC_PORT", 28010, minimum=1, maximum=65535)))
    table("script_registry", dict(
        cache_size_kib=unsigned(env, "BH_SCRIPT_REGISTRY_CACHE_SIZE_KIB", 65536, minimum=1),
        query_batch_size=unsigned(env, "BH_SCRIPT_REGISTRY_QUERY_BATCH_SIZE", 256, minimum=1, maximum=1000),
        slow_query_ms=unsigned(env, "BH_SCRIPT_REGISTRY_SLOW_QUERY_MS", 250, minimum=1),
    ))
    table("bootstrap", dict(
        snapshot_file=identity["snapshot_file"],
        import_batch_size=unsigned(env, "BH_ASSUMEUTXO_IMPORT_BATCH_SIZE", 20000, minimum=1, maximum=1000000),
        replay_batch_size=unsigned(env, "BH_ASSUMEUTXO_REPLAY_BATCH_SIZE", 20, minimum=1, maximum=100),
    ))
    table("bootstrap.identity", {key: identity[key] for key in ("origin_height", "origin_block_hash")})
    table("bootstrap.identity.snapshot", identity["snapshot"])
    return f"root_dir = {value_text(root)}\n\n" + "\n\n".join(sections) + "\n"


def btc_auth(env: Mapping[str, str]) -> dict | str | None:
    """Share credential validation across TOML and JSON without logging their values."""
    mode = env.get("BTC_AUTH_MODE", "cookie")
    if mode == "none":
        return "None"
    if mode == "cookie":
        return {"CookieFile": env["BTC_COOKIE_FILE"]} if env.get("BTC_COOKIE_FILE") else None
    if mode == "userpass":
        if not env.get("BTC_RPC_USER") or not env.get("BTC_RPC_PASSWORD"):
            raise ValueError("BTC_RPC_USER and BTC_RPC_PASSWORD are required for userpass authentication")
        return {"UserPass": [env["BTC_RPC_USER"], env["BTC_RPC_PASSWORD"]]}
    raise ValueError("BTC_AUTH_MODE must be cookie, userpass or none")


def boolean(env: Mapping[str, str], key: str, default: bool) -> bool:
    """Accept only explicit JSON boolean values for service flags."""
    value = env.get(key, str(default).lower())
    if value not in {"true", "false"}:
        raise ValueError(f"{key} must be true or false")
    return value == "true"


def render_indexer_config(env: Mapping[str, str]) -> str:
    """Render the native indexer path using the same G and Bitcoin input requirements."""
    identity = validate_environment(env, service="indexer")
    root = Path(env.get("USDB_INDEXER_ROOT_DIR", "/data/usdb-indexer"))
    if not root.is_absolute() or root == Path("/"):
        raise ValueError("USDB_INDEXER_ROOT_DIR must be an absolute dedicated service directory")
    btc = dict(network="bitcoin", data_dir=env.get("BTC_DATA_DIR", "/data/bitcoind"),
               rpc_url=env.get("BTC_RPC_URL", "http://btc-node:8332"), block_magic=None)
    auth = btc_auth(env)
    if auth is not None:
        btc["auth"] = auth
    config = dict(
        isolate=None,
        bitcoin=btc,
        ordinals=dict(rpc_url=env.get("ORD_RPC_URL", "http://ord-server:28030")),
        balance_history=dict(rpc_url=env.get("BALANCE_HISTORY_RPC_URL", "http://balance-history:28010")),
        usdb=dict(
            genesis_block_height=identity["origin_height"],
            active_address_page_size=unsigned(env, "ACTIVE_ADDRESS_PAGE_SIZE", 1024, minimum=1),
            balance_query_batch_size=unsigned(env, "BALANCE_QUERY_BATCH_SIZE", 1024, minimum=1),
            balance_query_concurrency=unsigned(env, "BALANCE_QUERY_CONCURRENCY", 4, minimum=1),
            balance_query_timeout_ms=unsigned(env, "BALANCE_QUERY_TIMEOUT_MS", 10000, minimum=1),
            balance_query_max_retries=unsigned(env, "BALANCE_QUERY_MAX_RETRIES", 2),
            inscription_source="bitcoind",
            inscription_fixture_file=None,
            inscription_source_shadow_compare=False,
            inscription_source_shadow_fail_fast=boolean(env, "INSCRIPTION_SOURCE_SHADOW_FAIL_FAST", False),
            rpc_server_host=env.get("USDB_INDEXER_RPC_HOST", "0.0.0.0"),
            rpc_server_port=unsigned(env, "USDB_INDEXER_RPC_PORT", 28020, minimum=1, maximum=65535),
            rpc_server_enabled=boolean(env, "USDB_INDEXER_RPC_SERVER_ENABLED", True),
            pass_energy_leaderboard_cache_enabled=boolean(env, "PASS_ENERGY_LEADERBOARD_CACHE_ENABLED", True),
            pass_energy_leaderboard_cache_top_k=unsigned(env, "PASS_ENERGY_LEADERBOARD_CACHE_TOP_K", 1000, minimum=1),
        ),
    )
    return json.dumps(config, ensure_ascii=False, indent=2) + "\n"


def write_config(path: Path, text: str) -> None:
    """Replace only a fully validated config and keep RPC credentials private."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, prefix=".native-config-", delete=False) as output:
            temporary = Path(output.name)
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate", help="Validate bootstrap inputs without opening service data")
    validate.add_argument("--service", choices=["balance-history", "indexer"], default="balance-history")
    render = commands.add_parser("render", help="Atomically render balance-history native config")
    render.add_argument("--output", required=True, type=Path)
    indexer = commands.add_parser("render-indexer", help="Atomically render native indexer config")
    indexer.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "validate":
            if args.service == "indexer":
                render_indexer_config(os.environ)
            else:
                render_config(os.environ)
        elif args.command == "render-indexer":
            write_config(args.output, render_indexer_config(os.environ))
        else:
            write_config(args.output, render_config(os.environ))
    except (ValueError, OSError, KeyError) as error:
        parser.exit(1, f"AssumeUTXO deployment configuration rejected: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
