"""Validate peer endpoints and read release defaults without controller dependencies."""
from __future__ import annotations

import ipaddress
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

BOOTNODES_FILE = "bootnodes.json"
BOOTNODES_SCHEMA = "usdb-bootnodes:v1"
MAX_SEEDS = 64
MAX_BOOTNODES_BYTES = 128 * 1024


def normalize_enode(value: str) -> str:
    """Validate a complete enode without DNS/IO; retain distinct address families."""
    try:
        if not isinstance(value, str) or len(value) > 1024 or any(c.isspace() for c in value):
            raise ValueError("whitespace or excessive length")
        parsed = urlsplit(value)
        key = parsed.username or ""
        if (parsed.scheme != "enode" or not re.fullmatch(r"[0-9a-fA-F]{128}", key)
                or parsed.password is not None or parsed.path or parsed.fragment
                or not parsed.hostname or not parsed.port):
            raise ValueError("expected enode://PUBLIC_KEY@HOST:PORT")
        # An arbitrary 128-digit string is not necessarily a secp256k1 public key.
        prime = 2**256 - 2**32 - 977
        x, y = int(key[:64], 16), int(key[64:], 16)
        if x >= prime or y >= prime or (y * y - x * x * x - 7) % prime:
            raise ValueError("public key is not on secp256k1")
        host = parsed.hostname
        if "%" in host:
            raise ValueError("scoped/escaped addresses cannot be shared")
        try:
            address = ipaddress.ip_address(host)
        except ValueError:
            host = host.encode("idna").decode("ascii").lower().rstrip(".")
            if (len(host) > 253 or ":" in host or not all(
                    re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", part)
                    for part in host.split("."))):
                raise ValueError("invalid DNS hostname")
        else:
            if address.is_unspecified or address.is_multicast or address.is_link_local:
                raise ValueError("use a routable address or an explicit loopback/LAN test address")
            host = f"[{address.compressed}]" if address.version == 6 else str(address)
        query = ""
        if parsed.query:
            match = re.fullmatch(r"discport=([0-9]+)", parsed.query)
            if not match or not 1 <= int(match[1]) <= 65535:
                raise ValueError("invalid discovery port")
            if int(match[1]) != parsed.port:
                query = f"?discport={int(match[1])}"
        return f"enode://{key.lower()}@{host}:{parsed.port}{query}"
    except (ValueError, UnicodeError) as error:
        raise ValueError(f"INVALID_PEER_SOURCE: {error}") from error


def parse_seeds(value: str) -> list[str]:
    """Normalize a bounded persistent list without collapsing one node's endpoints."""
    values = value.split(",")
    if len(values) > MAX_SEEDS:
        raise ValueError(f"INVALID_PEER_SOURCE: at most {MAX_SEEDS} seed endpoints are supported")
    return list(dict.fromkeys(normalize_enode(v.strip()) for v in values if v.strip()))


def _unique_object(pairs: list[tuple]) -> dict:
    """Reject ambiguous duplicate JSON fields before validating the catalog."""
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_bootnodes(bundle_dir: Path, bundle_id: str) -> list[str]:
    """Read optional deployment defaults; legacy bundles without a catalog stay empty.

    The catalog is protected by the node-kit archive checksum, not the chain
    identity. Reading it must never perform DNS queries or change node.env.
    """
    path = bundle_dir / BOOTNODES_FILE
    try:
        if path.is_symlink():
            raise ValueError("symlinks are not allowed")
        if not path.exists():
            return []
        with path.open("rb") as source:
            raw = source.read(MAX_BOOTNODES_BYTES + 1)
        if len(raw) > MAX_BOOTNODES_BYTES:
            raise ValueError("catalog exceeds size limit")
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=_unique_object)
        if not isinstance(value, dict) or set(value) != {"schema_version", "network_bundle_id", "bootnodes"}:
            raise ValueError("expected schema_version, network_bundle_id and bootnodes")
        if value["schema_version"] != BOOTNODES_SCHEMA:
            raise ValueError("unsupported schema_version")
        if value["network_bundle_id"] != bundle_id:
            raise ValueError("network_bundle_id does not match the selected network")
        entries = value["bootnodes"]
        if not isinstance(entries, list) or len(entries) > MAX_SEEDS:
            raise ValueError(f"bootnodes must be an array of at most {MAX_SEEDS} endpoints")
        return list(dict.fromkeys(normalize_enode(entry) for entry in entries))
    except (OSError, ValueError, UnicodeError) as error:
        raise ValueError(f"INVALID_BOOTNODES_CONFIG: {path}: {error}") from error


def resolve_bootnodes(bundle_dir: Path, bundle_id: str, explicit: str | None) -> str:
    """Apply release defaults only when the operator did not specify a list."""
    seeds = load_bootnodes(bundle_dir, bundle_id) if explicit is None else parse_seeds(explicit)
    return ",".join(seeds)
