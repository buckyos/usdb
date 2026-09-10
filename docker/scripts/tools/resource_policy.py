#!/usr/bin/env python3
"""Deterministic, capped whole-node memory budgets for bootstrap and steady use."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

MIB = 1024**2
GIB = 1024**3
# A nominal 32 GB machine must expose at least 32 decimal GB to the OS/controller.
MIN_HOST_MEMORY_BYTES = 32_000_000_000
# The release pins Bitcoin Core 28.1; src/txdb.h caps -dbcache at 16384 MiB.
MAX_BITCOIN_DBCACHE_MIB = 16384
PHASES = ("bitcoin", "overlap", "steady")
CAP_DEFAULTS = {
    "USDB_EXTERNAL_MEMORY_BUDGET": "0",
    "USDB_BH_MEMORY_CAP": "64g",
    "USDB_BTC_IBD_MEMORY_CAP": "32g",
    "USDB_BTC_OVERLAP_MEMORY_CAP": "16g",
    "USDB_BTC_STEADY_MEMORY_CAP": "8g",
}
SERVICE_MEMORY_KEYS = {
    "btc-node": "BTC_MEMORY_LIMIT",
    "balance-history": "BH_MEMORY_LIMIT",
    "snapshot-loader": "BH_MEMORY_LIMIT",
    "script-registry-installer": "BH_SCRIPT_REGISTRY_MEMORY_LIMIT",
    "usdb-indexer": "USDB_INDEXER_MEMORY_LIMIT",
    "usdb-chain": "USDB_CHAIN_MEMORY_LIMIT",
    "usdb-chain-init": "USDB_CHAIN_MEMORY_LIMIT",
    "paired-checkpoint-recovery": "USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT",
    "usdb-control-plane": "CONTROL_PLANE_MEMORY_LIMIT",
}
MANUAL_DEFAULTS = {
    "BH_MEMORY_LIMIT": "20g",
    "BH_SYNC_UTXO_MAX_CACHE_BYTES": str(4 * GIB),
    "BH_SYNC_BALANCE_MAX_CACHE_BYTES": str(8 * GIB),
    "BH_SYNC_MAX_MEMORY_PERCENT": "75",
    "BH_SCRIPT_REGISTRY_MEMORY_LIMIT": "2g",
    "USDB_INDEXER_MEMORY_LIMIT": "4g",
    "USDB_CHAIN_MEMORY_LIMIT": "5g",
    "CONTROL_PLANE_MEMORY_LIMIT": "1g",
    "USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT": "1g",
}


def memory_bytes(value: str, label: str) -> int:
    """Parse a positive Docker byte quantity without accepting unlimited budgets."""
    match = re.fullmatch(r"([0-9]+)([bBkKmMgG]?)", value)
    if match is None:
        raise ValueError(f"{label} must be positive bytes or an integer with k/m/g suffix")
    amount = int(match[1]) * {"": 1, "b": 1, "k": 1024, "m": MIB, "g": GIB}[match[2].lower()]
    if amount <= 0:
        raise ValueError(f"{label} must be greater than zero")
    return amount


def effective_memory_bytes(
    meminfo: Path = Path("/proc/meminfo"),
    proc_cgroup: Path = Path("/proc/self/cgroup"),
    cgroup_root: Path = Path("/sys/fs/cgroup"),
) -> int:
    """Read physical memory and any finite enclosing cgroup v1/v2 ceiling."""
    physical = None
    for line in meminfo.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"MemTotal:\s+([0-9]+) kB", line)
        if match:
            physical = int(match[1]) * 1024
            break
    if not physical:
        raise ValueError(f"{meminfo} does not contain a valid MemTotal")
    limits = [physical]
    for line in proc_cgroup.read_text(encoding="utf-8").splitlines():
        hierarchy, controllers, relative = line.split(":", 2)
        if hierarchy == "0" and not controllers:
            root, filename = cgroup_root, "memory.max"
        elif "memory" in controllers.split(","):
            root, filename = cgroup_root / "memory", "memory.limit_in_bytes"
        else:
            continue
        # A cgroup namespace can expose only the hierarchy root. Never traverse '..'.
        parts = Path(relative.lstrip("/")).parts
        directory = root.joinpath(*parts) if ".." not in parts else root
        while True:
            path = directory / filename
            if path.is_file():
                value = path.read_text(encoding="utf-8").strip()
                if value != "max":
                    if not value.isdigit():
                        raise ValueError(f"invalid memory ceiling in {path}")
                    limits.append(int(value))
            if directory == root:
                break
            directory = directory.parent
    return min(limits)


def resource_mode(env: dict[str, str]) -> str:
    """Older node configurations retain operator-controlled resource settings."""
    mode = env.get("USDB_RESOURCE_MODE", "manual")
    if mode not in {"auto", "manual"}:
        raise ValueError("USDB_RESOURCE_MODE must be auto or manual")
    return mode


@dataclass(frozen=True)
class ResourcePlan:
    host_memory_bytes: int
    phase: str
    reserve_bytes: int
    limits: dict[str, int]
    dbcache_mib: int
    utxo_cache_bytes: int
    balance_cache_bytes: int
    external_services_bytes: int = 0

    @property
    def total_bytes(self) -> int:
        """Budget Bitcoin alone during IBD; reserve downstream services after handoff."""
        return self.reserve_bytes + self.external_services_bytes + sum(
            amount for key, amount in self.limits.items()
            if self.phase != "bitcoin" or key == "BTC_MEMORY_LIMIT"
        )

    def environment(self) -> dict[str, str]:
        """Render all managed settings from this single budget, in exact bytes."""
        bitcoin = self.limits["BTC_MEMORY_LIMIT"]
        return {
            **({"USDB_EXTERNAL_MEMORY_BUDGET": str(self.external_services_bytes)} if self.external_services_bytes else {}),
            "USDB_RESOURCE_MODE": "auto",
            "USDB_RESOURCE_HOST_MEMORY_BYTES": str(self.host_memory_bytes),
            "USDB_RESOURCE_PHASE": self.phase,
            "BTC_RESOURCE_PROFILE": f"managed-{self.phase}",
            **{key: str(value) for key, value in self.limits.items()},
            # Swap is a small emergency allowance, never part of the RAM budget.
            "BTC_MEMORY_SWAP_LIMIT": str(bitcoin + min(bitcoin // 8, 2 * GIB)),
            "BH_MEMORY_SWAP_LIMIT": str(self.limits["BH_MEMORY_LIMIT"] + 2 * GIB),
            "BH_SYNC_UTXO_MAX_CACHE_BYTES": str(self.utxo_cache_bytes),
            "BH_SYNC_BALANCE_MAX_CACHE_BYTES": str(self.balance_cache_bytes),
            "BH_SYNC_MAX_MEMORY_PERCENT": "80",
            "BTC_DBCACHE_MB": str(self.dbcache_mib),
        }


def build_resource_plan(host_memory: int, phase: str, env: dict[str, str]) -> ResourcePlan:
    """Scale the 64 GiB plan proportionally and cap each heavy service independently."""
    if host_memory < MIN_HOST_MEMORY_BYTES:
        raise ValueError("automatic resources require at least 32 GB of effective host memory")
    if phase not in PHASES:
        raise ValueError(f"invalid USDB_RESOURCE_PHASE: {phase}")
    caps = {key: memory_bytes(env.get(key, default), key) for key, default in CAP_DEFAULTS.items()
            if key != "USDB_EXTERNAL_MEMORY_BUDGET"}
    external = external_services_budget(env)
    reserve = max(4 * GIB, host_memory * 10 // 64)
    if external and host_memory - external < MIN_HOST_MEMORY_BYTES:
        raise ValueError("external services must leave at least 32 GB for the node and system")
    available = host_memory - reserve - external

    def share(numerator: int, cap: int, denominator: int = 64) -> int:
        # Preserve the physical-host system reserve; reduce service shares only.
        return min(host_memory * numerator // denominator * available // (host_memory - reserve), cap) // MIB * MIB

    btc_share, cap_key = {
        "bitcoin": (32, "USDB_BTC_IBD_MEMORY_CAP"),
        "overlap": (16, "USDB_BTC_OVERLAP_MEMORY_CAP"),
        "steady": (8, "USDB_BTC_STEADY_MEMORY_CAP"),
    }[phase]
    limits = {
        "BTC_MEMORY_LIMIT": share(btc_share, caps[cap_key]),
        "BH_MEMORY_LIMIT": share(32 if phase == "steady" else 24, caps["USDB_BH_MEMORY_CAP"]),
        "USDB_INDEXER_MEMORY_LIMIT": share(4, 4 * GIB),
        "USDB_CHAIN_MEMORY_LIMIT": share(5, 5 * GIB),
        "CONTROL_PLANE_MEMORY_LIMIT": share(1, GIB),
        "BH_SCRIPT_REGISTRY_MEMORY_LIMIT": share(2, 2 * GIB),
        "USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT": share(1, GIB),
    }
    # Keep the previous dbcache allowance: the IBD boost is headroom for file
    # cache and other allocations, not an equal increase in application cache.
    bitcoin_cache_limit = limits["BTC_MEMORY_LIMIT"]
    if phase == "bitcoin":
        limits["BTC_MEMORY_LIMIT"] = share(4, caps[cap_key], 5)
    if limits["BTC_MEMORY_LIMIT"] < 2 * GIB or limits["BH_MEMORY_LIMIT"] < 4 * GIB:
        raise ValueError("resource caps must allow at least 2 GiB for Bitcoin and 4 GiB for balance-history")
    cache = limits["BH_MEMORY_LIMIT"] * 5 // 8
    # IBD retains its legacy cache size; later phases use half their container limit.
    dbcache = min(MAX_BITCOIN_DBCACHE_MIB,
                  bitcoin_cache_limit * (5 if phase == "bitcoin" else 4) // 8 // MIB)
    plan = ResourcePlan(
        host_memory, phase, reserve, limits,
        dbcache, cache // 4, cache - cache // 4, external,
    )
    if plan.total_bytes > host_memory:
        raise ValueError(f"{phase} resource budget exceeds effective host memory")
    return plan


def external_services_budget(env: dict[str, str]) -> int:
    """Reserve opt-in colocated services outside all node phase allocations."""
    value = env.get("USDB_EXTERNAL_MEMORY_BUDGET", "0")
    return 0 if value == "0" else memory_bytes(value, "USDB_EXTERNAL_MEMORY_BUDGET")


def validate_cache_budget(env: dict[str, str]) -> None:
    """Match the Rust startup guard before expensive snapshot installation."""
    settings = {**MANUAL_DEFAULTS, **env}
    limit = memory_bytes(settings["BH_MEMORY_LIMIT"], "BH_MEMORY_LIMIT")
    cache = 0
    for key in ("BH_SYNC_UTXO_MAX_CACHE_BYTES", "BH_SYNC_BALANCE_MAX_CACHE_BYTES"):
        value = settings[key]
        if not value.isdigit() or int(value) < MIB:
            raise ValueError(f"{key} must be at least 1 MiB in decimal bytes")
        cache += int(value)
    threshold = settings["BH_SYNC_MAX_MEMORY_PERCENT"]
    if not threshold.isdigit() or not 20 <= int(threshold) <= 95:
        raise ValueError("BH_SYNC_MAX_MEMORY_PERCENT must be between 20 and 95")
    maximum = limit * (int(threshold) - 10) // 100
    if cache > maximum:
        raise ValueError(f"balance-history cache budget {cache} exceeds {maximum} bytes; leave 10 percentage points below the memory-pressure threshold")


def validate_resource_environment(env: dict[str, str], host_memory: int | None = None) -> None:
    """Validate persisted budgets without guessing host RAM inside a service container."""
    if host_memory is not None and host_memory < MIN_HOST_MEMORY_BYTES:
        raise ValueError("USDB node requires at least 32 GB of effective host memory")
    validate_cache_budget(env)
    if resource_mode(env) == "auto":
        recorded = env.get("USDB_RESOURCE_HOST_MEMORY_BYTES", "")
        if not recorded.isdigit():
            raise ValueError("automatic resources require USDB_RESOURCE_HOST_MEMORY_BYTES; run setup or set-resource-policy")
        memory = int(recorded)
        if host_memory is not None and memory > host_memory:
            raise ValueError("effective host memory decreased; stop the node and recalculate its resource policy")
        phase = env.get("USDB_RESOURCE_PHASE", "")
        # Validate every reachable phase, not only today's allocation.
        for candidate in PHASES:
            build_resource_plan(memory, candidate, env)
        expected = build_resource_plan(memory, phase, env).environment()
        for key, value in expected.items():
            if env.get(key) != value:
                raise ValueError(f"{key} does not match the automatic resource plan; stop the node and run "
                                 "set-resource-policy --mode auto to recalculate, or use manual mode for custom allocations")
    elif host_memory is not None:
        settings = {**MANUAL_DEFAULTS, **env}
        keys = set(SERVICE_MEMORY_KEYS.values())
        total = sum(memory_bytes(settings.get(key, "0"), key) for key in keys)
        reserve = max(4 * GIB, host_memory * 10 // 64)
        total += external_services_budget(env)
        if total + reserve > host_memory:
            raise ValueError(f"manual service limits plus system reserve exceed effective host memory: {total + reserve} > {host_memory}")
