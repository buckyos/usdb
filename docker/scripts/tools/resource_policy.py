#!/usr/bin/env python3
"""Deterministic, capped whole-node memory budgets for bootstrap and steady use."""

from __future__ import annotations

import re
from dataclasses import dataclass, replace
from pathlib import Path

MIB = 1024**2
GIB = 1024**3
# A nominal 32 GB machine must expose at least 32 decimal GB to the OS/controller.
MIN_HOST_MEMORY_BYTES = 32_000_000_000
# Retain the conservative 16 GiB deployment cap across the Core 31.1 upgrade.
MAX_BITCOIN_DBCACHE_MIB = 16384
PHASES = ("bitcoin", "overlap", "steady")
# Absence of these keys keeps released configurations on their original formula.
POLICY_KEYS = ("USDB_STORAGE_PROFILE", "USDB_RESOURCE_MEMORY_PERCENT")
STORAGE_PROFILES = ("balanced", "slow-disk")
DEFAULT_MEMORY_PERCENT = "90"
CAP_DEFAULTS = {
    "USDB_EXTERNAL_MEMORY_BUDGET": "0",
    "USDB_BH_MEMORY_CAP": "64g",
    "USDB_BTC_IBD_MEMORY_CAP": "32g",
    "USDB_BTC_OVERLAP_MEMORY_CAP": "16g",
    "USDB_BTC_STEADY_MEMORY_CAP": "8g",
    "USDB_ORD_MEMORY_CAP": "16g",
}
SERVICE_MEMORY_KEYS = {
    "btc-node": "BTC_MEMORY_LIMIT",
    "ord-server": "ORD_MEMORY_LIMIT",
    "btc-snapshot-bootstrap": "BTC_BOOTSTRAP_MEMORY_LIMIT",
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
    "BTC_BOOTSTRAP_MEMORY_LIMIT": "512m",
    "ORD_MEMORY_LIMIT": "4g",
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


def resource_cap_defaults(env: dict[str, str]) -> dict[str, str]:
    """Native foreground readiness can overlap two active Core chainstates."""
    defaults = {**CAP_DEFAULTS, **({"USDB_BTC_STEADY_MEMORY_CAP": "16g"}
                             if env.get("SNAPSHOT_MODE") == "assumeutxo" else {})}
    if env.get("USDB_STORAGE_PROFILE") == "slow-disk":
        defaults.update(USDB_BTC_IBD_MEMORY_CAP="64g", USDB_BTC_OVERLAP_MEMORY_CAP="32g",
                        USDB_BTC_STEADY_MEMORY_CAP="32g")
    if env.get("USDB_ORD_RESOURCE_POLICY") == "adaptive-v1":
        defaults["USDB_ORD_MEMORY_CAP"] = "32g"
    return defaults


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
    ord_cache_bytes: int | None = None
    ord_memory_cap: int | None = None
    storage_profile: str | None = None
    memory_percent: int | None = None
    ord_deferred: bool = False
    ord_steady_cache_bytes: int | None = None

    @property
    def total_bytes(self) -> int:
        """Budget Bitcoin plus its private monitor during IBD; reserve other services after handoff."""
        return self.reserve_bytes + self.external_services_bytes + sum(
            amount for key, amount in self.limits.items()
            if self.phase != "bitcoin" or key in {"BTC_MEMORY_LIMIT", "BTC_BOOTSTRAP_MEMORY_LIMIT", "CONTROL_PLANE_MEMORY_LIMIT", "ORD_MEMORY_LIMIT"}
        )

    def environment(self) -> dict[str, str]:
        """Render all managed settings from this single budget, in exact bytes."""
        bitcoin = self.limits["BTC_MEMORY_LIMIT"]
        return {
            **({"USDB_EXTERNAL_MEMORY_BUDGET": str(self.external_services_bytes)} if self.external_services_bytes else {}),
            **({"USDB_STORAGE_PROFILE": self.storage_profile,
                "USDB_RESOURCE_MEMORY_PERCENT": str(self.memory_percent),
                "ORD_STARTUP_DEFERRED": str(int(self.ord_deferred))} if self.storage_profile else {}),
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
            **({"ORD_INDEX_CACHE_BYTES": str(self.ord_cache_bytes),
                "USDB_ORD_MEMORY_CAP": str(self.ord_memory_cap)} if self.ord_cache_bytes is not None else {}),
            **({"USDB_ORD_RESOURCE_POLICY": "adaptive-v1",
                "ORD_STEADY_INDEX_CACHE_BYTES": str(self.ord_steady_cache_bytes)}
               if self.ord_steady_cache_bytes is not None else {}),
        }


def build_resource_plan(host_memory: int, phase: str, env: dict[str, str]) -> ResourcePlan:
    """Reserve a bounded Ord catch-up ceiling without spending other services' minima."""
    strategy = env.get("USDB_ORD_RESOURCE_POLICY", "fixed")
    if strategy not in {"fixed", "adaptive-v1"}:
        raise ValueError("USDB_ORD_RESOURCE_POLICY must be fixed or adaptive-v1")
    if strategy == "adaptive-v1":
        env = {"USDB_ORD_MEMORY_CAP": "32g", **env}
    plan = _base_resource_plan(host_memory, phase, env)
    if strategy != "adaptive-v1" or "ORD_MEMORY_LIMIT" not in plan.limits:
        return plan
    limits = dict(plan.limits)
    cap = memory_bytes(env.get("USDB_ORD_MEMORY_CAP", "32g"), "USDB_ORD_MEMORY_CAP")
    if phase == "steady":
        current = limits["ORD_MEMORY_LIMIT"]
        desired = min(cap, (host_memory - plan.external_services_bytes) // 2) // MIB * MIB
        # BH has completed bootstrap. Donate only its excess above 4 GiB and
        # genuinely unallocated pool space; Core and all other ceilings stay put.
        unused = max(0, host_memory - plan.total_bytes) // MIB * MIB
        donated = min(max(0, desired - current - unused), max(0, limits["BH_MEMORY_LIMIT"] - 4 * GIB))
        limits["BH_MEMORY_LIMIT"] -= donated
        limits["ORD_MEMORY_LIMIT"] = max(current, min(desired, current + unused + donated))
    # The redb cache is not total Ord RSS. Leave room for the UTXO batch,
    # address tables and charged filesystem pages, especially during commits.
    cache = min(8 * GIB, limits["ORD_MEMORY_LIMIT"] // 4)
    bh_cache = limits["BH_MEMORY_LIMIT"] * 5 // 8
    return replace(plan, limits=limits, ord_cache_bytes=cache,
                   ord_memory_cap=cap,
                   ord_steady_cache_bytes=min(GIB, cache),
                   utxo_cache_bytes=bh_cache // 4, balance_cache_bytes=bh_cache - bh_cache // 4)


def _base_resource_plan(host_memory: int, phase: str, env: dict[str, str]) -> ResourcePlan:
    """Scale the 64 GiB plan proportionally and cap each heavy service independently."""
    if host_memory < MIN_HOST_MEMORY_BYTES:
        raise ValueError("automatic resources require at least 32 GB of effective host memory")
    if phase not in PHASES:
        raise ValueError(f"invalid USDB_RESOURCE_PHASE: {phase}")
    if "USDB_STORAGE_PROFILE" in env:
        return _profile_plan(host_memory, phase, env)
    if "USDB_RESOURCE_MEMORY_PERCENT" in env:
        raise ValueError("memory percentage requires a storage profile; use --storage-profile balanced or slow-disk")
    caps = {key: memory_bytes(env.get(key, default), key) for key, default in resource_cap_defaults(env).items()
            if key != "USDB_EXTERNAL_MEMORY_BUDGET"}
    external = external_services_budget(env)
    reserve = max(4 * GIB, host_memory * 10 // 64)
    if external and host_memory - external < MIN_HOST_MEMORY_BYTES:
        raise ValueError("external services must leave at least 32 GB for the node and system")
    ord_memory = 0
    ord_cache = None
    ord_cap = None
    if env.get("USDB_MINTING_ENABLED") == "1":
        if "USDB_ORD_MEMORY_CAP" in env:
            # Opt in at setup or an explicit policy recalculation. Merely loading
            # an older node.env must preserve its fixed Ord budget and cache.
            ord_cap = caps["USDB_ORD_MEMORY_CAP"]
            ord_memory = min(max(4 * GIB, (host_memory - external) // 4), ord_cap) // MIB * MIB
            if ord_memory < 2 * GIB:
                raise ValueError("Ord memory cap must allow at least 2 GiB")
            ord_cache = ord_memory // 2
        else:
            ord_memory = memory_bytes(env.get("ORD_MEMORY_LIMIT", "4g"), "ORD_MEMORY_LIMIT")
    available = host_memory - reserve - external - ord_memory
    if available <= 0:
        raise ValueError("optional Ord memory budget leaves no memory for node services")

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
    if ord_memory:
        limits["ORD_MEMORY_LIMIT"] = ord_memory
    if env.get("SNAPSHOT_MODE") == "assumeutxo":
        # Include HTTPS buffers and bounded file writeback, not only Python RSS.
        limits["BTC_BOOTSTRAP_MEMORY_LIMIT"] = 512 * MIB
    # Keep the previous dbcache allowance: the IBD boost is headroom for file
    # cache and other allocations, not an equal increase in application cache.
    bitcoin_cache_limit = limits["BTC_MEMORY_LIMIT"]
    if phase == "bitcoin":
        limits["BTC_MEMORY_LIMIT"] = share(4, caps[cap_key], 5)
    elif phase == "overlap" and env.get("SNAPSHOT_MODE") == "assumeutxo":
        # Core still validates history while BH imports/replays/verifies. Transfer
        # headroom without increasing dbcache or starving BH's bootstrap work.
        # An explicit BH cap below 8 GiB is preserved and donates no memory.
        desired = share(24, caps[cap_key])
        extra = min(desired - bitcoin_cache_limit, max(0, limits["BH_MEMORY_LIMIT"] - 8 * GIB))
        limits["BTC_MEMORY_LIMIT"] += extra
        limits["BH_MEMORY_LIMIT"] -= extra
    elif phase == "steady" and env.get("SNAPSHOT_MODE") == "assumeutxo":
        # Foreground readiness does not end Core's background validation. Keep
        # file-cache headroom after chain startup without growing dbcache or the
        # whole-node budget. Retain this allocation after validation, too, so a
        # read-only probe never triggers a disruptive resource demotion.
        desired = share(32, caps[cap_key])
        extra = min(desired - bitcoin_cache_limit, max(0, limits["BH_MEMORY_LIMIT"] - 4 * GIB))
        limits["BTC_MEMORY_LIMIT"] += extra
        limits["BH_MEMORY_LIMIT"] -= extra
    if limits["BTC_MEMORY_LIMIT"] < 2 * GIB or limits["BH_MEMORY_LIMIT"] < 4 * GIB:
        raise ValueError("resource caps must allow at least 2 GiB for Bitcoin and 4 GiB for balance-history")
    cache = limits["BH_MEMORY_LIMIT"] * 5 // 8
    # Cache sizes use the allocation before any file-cache headroom transfer.
    dbcache = min(MAX_BITCOIN_DBCACHE_MIB,
                  bitcoin_cache_limit * (5 if phase == "bitcoin" else 4) // 8 // MIB)
    plan = ResourcePlan(
        host_memory, phase, reserve, limits,
        dbcache, cache // 4, cache - cache // 4, external, ord_cache, ord_cap,
    )
    excess = plan.total_bytes - host_memory
    if 0 < excess <= limits.get("BTC_BOOTSTRAP_MEMORY_LIMIT", 0):
        # On small hosts with Ord/external reservations, fund preparation from
        # Core's headroom; never take BH's minimum or the system reserve.
        reduction = (excess + MIB - 1) // MIB * MIB
        minimum = max(2 * GIB, (dbcache + 512) * MIB)
        if limits["BTC_MEMORY_LIMIT"] - reduction >= minimum:
            limits["BTC_MEMORY_LIMIT"] -= reduction
    if plan.total_bytes > host_memory:
        raise ValueError(f"{phase} resource budget exceeds effective host memory")
    return plan


def _profile_plan(host_memory: int, phase: str, env: dict[str, str]) -> ResourcePlan:
    """Split a bounded node pool; disk profiles change shares, never host safety.

    MemAvailable includes reclaimable cache and is not an allocation boundary.
    Persisted host/cgroup capacity and explicit external reservations make this
    calculation reproducible across restarts and safe to validate offline.
    """
    profile = env["USDB_STORAGE_PROFILE"]
    if profile not in STORAGE_PROFILES:
        raise ValueError("USDB_STORAGE_PROFILE must be balanced or slow-disk")
    percentage = env.get("USDB_RESOURCE_MEMORY_PERCENT", DEFAULT_MEMORY_PERCENT)
    if not percentage.isascii() or not percentage.isdigit() or not 80 <= int(percentage) <= 90:
        raise ValueError("USDB_RESOURCE_MEMORY_PERCENT must be an integer between 80 and 90")
    external = external_services_budget(env)
    usable = host_memory - external
    if usable < MIN_HOST_MEMORY_BYTES:
        raise ValueError("external services must leave at least 32 GB for the node and system")
    pool = min(usable * int(percentage) // 100, usable - 4 * GIB) // MIB * MIB
    reserve = usable - pool
    caps = {key: memory_bytes(env.get(key, default), key)
            for key, default in resource_cap_defaults(env).items() if key != "USDB_EXTERNAL_MEMORY_BUDGET"}
    slow = profile == "slow-disk"
    def rounded(value):
        return value // MIB * MIB

    limits = {key: rounded(min(usable * numerator // 64, cap)) for key, numerator, cap in (
        ("USDB_INDEXER_MEMORY_LIMIT", 4, 4 * GIB), ("USDB_CHAIN_MEMORY_LIMIT", 5, 5 * GIB),
        ("CONTROL_PLANE_MEMORY_LIMIT", 1, GIB), ("BH_SCRIPT_REGISTRY_MEMORY_LIMIT", 2, 2 * GIB),
        ("USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT", 1, GIB))}
    if env.get("SNAPSHOT_MODE") == "assumeutxo":
        limits["BTC_BOOTSTRAP_MEMORY_LIMIT"] = 512 * MIB
    ord_cap = ord_cache = None
    deferred = env.get("USDB_MINTING_ENABLED") == "1" and phase != "steady"
    if env.get("USDB_MINTING_ENABLED") == "1":
        ord_cap = caps["USDB_ORD_MEMORY_CAP"]
        if ord_cap < 2 * GIB:
            raise ValueError("Ord memory cap must allow at least 2 GiB")
        limits["ORD_MEMORY_LIMIT"] = (512 * MIB if deferred else
            rounded(min(max(4 * GIB, usable // (8 if slow else 4)), ord_cap)))
        ord_cache = limits["ORD_MEMORY_LIMIT"] // 2
    auxiliary = sum(value for key, value in limits.items() if phase != "bitcoin" or key in {
        "CONTROL_PLANE_MEMORY_LIMIT", "BTC_BOOTSTRAP_MEMORY_LIMIT", "ORD_MEMORY_LIMIT"})
    remaining = pool - auxiliary
    btc_cap = caps[{"bitcoin": "USDB_BTC_IBD_MEMORY_CAP", "overlap": "USDB_BTC_OVERLAP_MEMORY_CAP",
                    "steady": "USDB_BTC_STEADY_MEMORY_CAP"}[phase]]
    bh_floor = (8 if phase != "steady" else 4) * GIB
    if caps["USDB_BH_MEMORY_CAP"] < bh_floor:
        raise ValueError("profile resources require a BH cap of at least 8 GiB for bootstrap")
    if phase == "bitcoin":
        bitcoin = rounded(min(remaining, btc_cap))
        # BH is stopped in this phase; this is its future minimum, not concurrent RAM.
        balance = bh_floor
    else:
        if remaining < bh_floor + 2 * GIB:
            raise ValueError("resource pool cannot fit Bitcoin and BH minimum budgets; reduce Ord/external budgets")
        bh_percent = (25 if phase == "steady" else 40) if slow else 60
        balance = rounded(min(caps["USDB_BH_MEMORY_CAP"], max(bh_floor, remaining * bh_percent // 100)))
        bitcoin = rounded(min(btc_cap, remaining - balance))
        # A Core cap may leave space for BH, but never spend above its cap.
        balance = rounded(min(caps["USDB_BH_MEMORY_CAP"], remaining - bitcoin))
    if bitcoin < 2 * GIB:
        raise ValueError("resource caps must allow at least 2 GiB for Bitcoin")
    limits.update(BTC_MEMORY_LIMIT=bitcoin, BH_MEMORY_LIMIT=balance)
    # Increasing Core's cgroup budget chiefly reserves room for charged file
    # cache. Application cache has a separate, conservative host-scaled bound.
    cache_share = {"bitcoin": 20, "overlap": 8, "steady": 4}[phase]
    dbcache = min(MAX_BITCOIN_DBCACHE_MIB, usable * cache_share // 64 // MIB, bitcoin // 2 // MIB)
    bh_cache = balance * 5 // 8
    plan = ResourcePlan(host_memory, phase, reserve, limits, dbcache, bh_cache // 4,
                        bh_cache - bh_cache // 4, external, ord_cache, ord_cap,
                        profile, int(percentage), deferred)
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
    if env.get("ORD_STARTUP_DEFERRED", "0") not in {"0", "1"}:
        raise ValueError("ORD_STARTUP_DEFERRED must be 0 or 1")
    if env.get("ORD_STARTUP_DEFERRED") == "1" and (resource_mode(env) != "auto" or not env.get("USDB_STORAGE_PROFILE")):
        raise ValueError("deferred Ord requires automatic profile resources")
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
        if env.get("USDB_MINTING_ENABLED") != "1":
            keys.discard("ORD_MEMORY_LIMIT")
        if env.get("SNAPSHOT_MODE") != "assumeutxo":
            keys.discard("BTC_BOOTSTRAP_MEMORY_LIMIT")
        total = sum(memory_bytes(settings.get(key, "0"), key) for key in keys)
        reserve = max(4 * GIB, host_memory * 10 // 64)
        total += external_services_budget(env)
        if total + reserve > host_memory:
            raise ValueError(f"manual service limits plus system reserve exceed effective host memory: {total + reserve} > {host_memory}")
