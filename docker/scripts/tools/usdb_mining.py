"""Operate mining through a durable, chain-only controller transaction.

All reports and journals deliberately exclude node.env credentials. Observations
used for display are never reused as authorization to start mining.
"""
from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import time
import uuid
from typing import Any
from urllib.parse import urlsplit

import usdb_node as node

SCHEMA = "usdb-node-mining:v1"
VIEW = "uip-0006-usdb-economic-state-view:v1"
RULE = "uip-0006:effective-energy-desc-pass-id-asc:v1"
ROLE_KEYS = ("USDB_NODE_ROLE", "USDB_MINER_ADDRESS", "USDB_MINER_THREADS")
TERMINAL = {"APPLIED", "FAILED", "CANCELLED"}
ISSUED_SLOT = "0xdd1651483272028cad87b8ab291a694a9deb1d7f6b60efe175f823c406233da2"
HASH = re.compile(r"(?:0x)?[0-9a-f]{64}")
PASS = re.compile(r"[0-9a-f]{64}i[0-9]+")
# Extra arguments may tune logging but cannot override identity, discovery,
# recovery, or sealing settings assembled by the managed runtime.
RESERVED = ("--mine", "--miner.", "--datadir", "--config", "--networkid", "--usdb",
            "--mainnet", "--testnet", "--goerli", "--sepolia", "--ropsten", "--rinkeby",
            "--kiln", "--dev", "--bootnodes", "--discovery", "--nodiscover", "--nodekey",
            "--nat", "--port", "--maxpeers", "--ethash.usdb-indexer", "--http", "--syncmode", "--fakepow", "--override")


class OwnershipError(ValueError):
    """The operation no longer owns the observed configuration or database."""


def state_path(layout: node.ReleaseLayout) -> Path:
    """Return the bundle-scoped private mining journal path."""
    return layout.node_env.parent / "node.mining.json"


def read_state(layout: node.ReleaseLayout) -> dict[str, Any]:
    """Read and validate the private operation journal without changing it."""
    path = state_path(layout)
    if not path.exists():
        return {}
    value = node._load_json(path)
    if value.get("schema_version") != SCHEMA:
        raise ValueError("MINING_JOURNAL_INVALID: unsupported operation schema")
    if (value.get("phase") not in TERMINAL | {"QUEUED", "STOPPING", "STOPPED", "CONFIGURED", "STARTING", "ROLLING_BACK"}
            or not isinstance(value.get("target"), dict) or not isinstance(value.get("original"), dict)
            or not isinstance(value.get("binding"), dict) or not value.get("operation_id")):
        raise ValueError("MINING_JOURNAL_INVALID: incomplete operation record")
    for key in ("target", "original"):
        role = value[key]
        if set(role) != set(ROLE_KEYS) or any(not isinstance(v, str) for v in role.values()):
            raise ValueError("MINING_JOURNAL_INVALID: invalid role configuration")
        if key == "target":
            node._require_role(role["USDB_NODE_ROLE"], role["USDB_MINER_ADDRESS"], int(role["USDB_MINER_THREADS"]))
    return value


def write_state(layout: node.ReleaseLayout, value):
    """Fsync both the journal and its directory before touching a container."""
    value.update(schema_version=SCHEMA, updated_at=node.datetime.now(node.timezone.utc).isoformat())
    node._atomic_write_private(state_path(layout), json.dumps(value, indent=2, sort_keys=True) + "\n")
    descriptor = os.open(state_path(layout).parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def pending(layout: node.ReleaseLayout) -> bool:
    """Report durable unfinished work independently of systemd process state."""
    state = read_state(layout)
    return bool(state and state.get("phase") not in TERMINAL)


def role_config(env: dict[str, str]) -> dict[str, str]:
    """Select only the three non-secret role settings for a journal backup."""
    return {"USDB_NODE_ROLE": env.get("USDB_NODE_ROLE", "full"),
            "USDB_MINER_ADDRESS": env.get("USDB_MINER_ADDRESS", ""),
            "USDB_MINER_THREADS": env.get("USDB_MINER_THREADS", "1")}


def fingerprint(env: dict[str, str]) -> str:
    # A digest detects external config changes without storing credentials.
    return hashlib.sha256(json.dumps({k: v for k, v in env.items() if k not in ROLE_KEYS},
                                    sort_keys=True).encode()).hexdigest()


def authorization_config(env: dict[str, str]) -> dict[str, str]:
    """Keep an applied identity valid across compatible image/resource upgrades."""
    return {key: env.get(key, "") for key in ("USDB_BOOTNODES", "USDB_NAT", "USDB_CHAIN_EXTRA_ARGS")}


def binding(layout: node.ReleaseLayout, env):
    """Bind acknowledgements to this generation and this actual chain database."""
    root = Path(env["USDB_CHAIN_DATA_HOST_DIR"]).resolve()
    data = root / "geth/chaindata"
    key = root / "geth/nodekey"
    if not (data / "CURRENT").is_file() or not key.is_file():
        raise ValueError("CHAIN_NOT_INITIALIZED: start the full node before enabling mining")
    info = data.stat()
    return {"network": layout.network_identity, "data_path": str(root),
            "data_device": info.st_dev, "data_inode": info.st_ino,
            "dataset_sha256": node._sha256(root / node.DATASET_IDENTITY_FILE),
            "node_key_sha256": node._sha256(key)}


def rpc(layout: node.ReleaseLayout, method, params=None, *, indexer=False):
    env = node.read_env(layout.node_env)
    if indexer:
        url = node._host_rpc_url(env, "USDB_INDEXER_BIND_ADDRESS", "USDB_INDEXER_BIND_PORT", 28020)
    else:
        url = node._host_rpc_url(env, "USDB_HTTP_BIND_ADDRESS", "USDB_HTTP_BIND_PORT", 8545)
    return node._json_rpc_batch(url, ((method, params or []),), timeout_secs=8)[method]


def address_check(layout: node.ReleaseLayout, address: str) -> str:
    """Validate address syntax and optional EIP-55 checksum, then normalize it."""
    if not isinstance(address, str) or not node.ADDRESS_RE.fullmatch(address) or int(address[2:], 16) == 0:
        raise ValueError("INVALID_ADDRESS: expected a nonzero 0x-prefixed 20-byte address")
    body = address[2:]
    if body != body.lower() and body != body.upper():
        # web3_sha3 is Keccak-256, whereas hashlib.sha3_256 is a different hash.
        digest = rpc(layout, "web3_sha3", ["0x" + body.lower().encode("ascii").hex()])
        if not isinstance(digest, str) or not re.fullmatch(r"0x[0-9a-f]{64}", digest):
            raise ValueError("INVALID_ADDRESS: checksum RPC returned an invalid digest")
        expected = "".join(c.upper() if int(digest[i + 2], 16) >= 8 else c
                           for i, c in enumerate(body.lower()))
        if body != expected:
            raise ValueError("INVALID_ADDRESS: mixed-case address has an invalid EIP-55 checksum")
    return address.lower()


def inspect_chain(layout: node.ReleaseLayout, *, processes=True):
    result = node.run_helper(layout, "run_testnet_runtime.sh", ["ps", "--all", "--quiet", "usdb-chain"],
                             capture_output=True, command_timeout_secs=15)
    ids = result.stdout.split()
    if not ids:
        return {"state": "absent", "environment": {}, "argv": []}
    if len(ids) != 1 or not re.fullmatch(r"[0-9a-f]{12,64}", ids[0]):
        raise ValueError("CHAIN_CONTAINER_CONFLICT: expected exactly one chain container")
    result = subprocess.run(["docker", "inspect", ids[0]], check=True, capture_output=True, text=True, timeout=15)
    item, = json.loads(result.stdout)
    config = item["Config"]
    env = dict(v.split("=", 1) for v in config.get("Env", []) if "=" in v)
    host = item["HostConfig"]
    report = {"id": ids[0], "state": item["State"]["Status"],
              "exit_code": item["State"]["ExitCode"], "error": item["State"].get("Error", ""),
              "image": config["Image"], "memory": host.get("Memory", 0),
              "nano_cpus": host.get("NanoCpus", 0), "cpu_quota": host.get("CpuQuota", 0),
              "cpu_period": host.get("CpuPeriod", 0), "cpuset": host.get("CpusetCpus", ""),
              "environment": {k: env.get(k, "") for k in (*ROLE_KEYS, "USDB_BOOTNODES", "USDB_CHAIN_EXTRA_ARGS")},
              "argv": []}
    if processes and report["state"] == "running":
        script = ("import glob,json,os; "
                  "p=[open(f,'rb').read().decode().split('\\0')[:-1] for f in glob.glob('/proc/[0-9]*/cmdline') "
                  "if os.path.exists(f)]; "
                  "print(json.dumps([a for a in p if a and os.path.basename(a[0])=='geth']))")
        result = subprocess.run(["docker", "exec", ids[0], "python3", "-c", script],
                                check=True, capture_output=True, text=True, timeout=15)
        commands = json.loads(result.stdout)
        if len(commands) > 1:
            raise ValueError("CHAIN_PROCESS_CONFLICT: multiple geth processes")
        report["argv"] = commands[0] if commands else []
    return report


def flag_values(argv):
    values = {}
    for index, arg in enumerate(argv):
        if not arg.startswith("--"):
            continue
        key, separator, value = arg.partition("=")
        if key in values:
            raise ValueError(f"RUNTIME_ARGUMENT_CONFLICT: duplicate {key}")
        values[key] = value if separator else (argv[index + 1] if index + 1 < len(argv)
                                               and not argv[index + 1].startswith("--") else True)
    return values


def runtime_matches(layout: node.ReleaseLayout, env, runtime):
    if runtime.get("state") != "running" or runtime.get("image") != layout.images["USDB_CHAIN_IMAGE"]:
        return False
    configured = role_config(env)
    if any(runtime["environment"].get(k) != v for k, v in configured.items()):
        return False
    if runtime["environment"].get("USDB_BOOTNODES", "") != env.get("USDB_BOOTNODES", ""):
        return False
    flags = flag_values(runtime.get("argv", []))
    if not flags:
        return False
    miner = configured["USDB_NODE_ROLE"] == "miner"
    if ("--mine" in flags and flags["--mine"] not in {"false", "0"}) != miner:
        return False
    return not miner or (str(flags.get("--miner.threads")) == configured["USDB_MINER_THREADS"]
                         and str(flags.get("--miner.etherbase", "")).lower() == configured["USDB_MINER_ADDRESS"].lower())


def check_resources(layout: node.ReleaseLayout, env, runtime, threads):
    if type(threads) is not int or threads < 1:
        raise ValueError("INVALID_THREADS: CPU PoW worker count must be positive; default is 1")
    cpus = len(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else os.cpu_count() or 1
    if runtime.get("nano_cpus", 0) > 0:
        cpus = min(cpus, runtime["nano_cpus"] / 10**9)
    if runtime.get("cpu_quota", 0) > 0 and runtime.get("cpu_period", 0) > 0:
        cpus = min(cpus, runtime["cpu_quota"] / runtime["cpu_period"])
    if runtime.get("cpuset"):
        assigned = set()
        for part in runtime["cpuset"].split(","):
            ends = [int(n) for n in part.split("-")]
            assigned.update(range(ends[0], ends[-1] + 1))
        cpus = min(cpus, len(assigned))
    if threads > cpus:
        raise ValueError(f"INVALID_THREADS: requested {threads} CPU workers exceeds available quota {cpus}")
    if node.resource_mode(env) == "auto":
        node.validate_resource_environment(env, node.effective_memory_bytes())
        state = node._read_resource_state(layout)
        if state.get("pending") or state.get("recover_services") or env.get("USDB_RESOURCE_PHASE") != "steady":
            raise ValueError("RESOURCE_TRANSITION_PENDING: wait for the steady resource plan")
        node._check_running_resource_budget(env, node._resource_containers(layout))
    if runtime.get("memory", 0) <= 0:
        raise ValueError("RESOURCE_LIMIT_REQUIRED: chain container must have a bounded memory budget")
    return cpus


def guard_check(env, epoch=None):
    root = Path(env["USDB_CHAIN_DATA_HOST_DIR"]) / "recovery/deep-btc-reorg"
    if (root / "halted.json").exists():
        raise ValueError("DEEP_REORG_HALTED: preserve the recovery incident; mining cannot resume this generation")
    if epoch is not None and (root / "baseline.json").exists():
        baseline = node._load_json(root / "baseline.json")
        if baseline.get("upstream_reorg_epoch") != epoch:
            raise ValueError("DEEP_REORG_EPOCH_CHANGED: current upstream epoch differs from the chain baseline")


def upstream_candidate(layout: node.ReleaseLayout, address, expect_pass=None):
    env = node.read_env(layout.node_env)
    guard_check(env)
    bitcoin = node._bitcoin_startup_progress(layout, command_timeout_secs=20)
    if not bitcoin or bitcoin.get("ready") is not True:
        raise ValueError("BITCOIN_NOT_READY: full Bitcoin readiness is required")
    bh, error = node._read_service_readiness(layout, "run_testnet_runtime.sh", ["data-status"], "balance-history")
    if error or not bh or bh.get("consensus_ready") is not True:
        raise ValueError(f"BALANCE_HISTORY_NOT_READY: {error or (bh or {}).get('blockers')}")
    # Retry a moving tip using a wholly fresh selector. A changed reorg epoch
    # fails instead of authorizing the operation against a different history.
    first_epoch = None
    for _ in range(3):
        ready = rpc(layout, "get_readiness", indexer=True)
        if not isinstance(ready, dict) or ready.get("service") != "usdb-indexer" or ready.get("consensus_ready") is not True:
            raise ValueError(f"INDEXER_NOT_READY: {(ready or {}).get('blockers') if isinstance(ready, dict) else ready}")
        height, epoch = ready.get("synced_block_height"), ready.get("upstream_reorg_epoch")
        if type(height) is not int or height < 0 or type(epoch) is not int or epoch < 0:
            raise ValueError("INVALID_READINESS: missing committed height or reorg epoch")
        if first_epoch is None:
            first_epoch = epoch
        if epoch != first_epoch:
            raise ValueError("REORG_DURING_CHECK: repeat the check against the new history")
        guard_check(env, epoch)
        for key in ("upstream_snapshot_id", "system_state_id", "local_state_commit"):
            if not isinstance(ready.get(key), str) or not HASH.fullmatch(ready[key]):
                raise ValueError(f"INVALID_READINESS: missing {key}")
        selector = {"requested_height": height, "expected_state": {
            "snapshot_id": ready["upstream_snapshot_id"], "system_state_id": ready["system_state_id"]}}
        candidate = rpc(layout, "resolve_miner_candidate", [{"view_version": VIEW, "usdb_main": address,
                         "block_height": height, "context": selector}], indexer=True)
        if not isinstance(candidate, dict) or not isinstance(candidate.get("external_state"), dict):
            raise ValueError("INVALID_CANDIDATE: missing external state")
        external = candidate.get("external_state", {})
        expected = {"btc_height": height, "snapshot_id": ready["upstream_snapshot_id"],
                    "system_state_id": ready["system_state_id"], "local_state_commit": ready["local_state_commit"],
                    "activation_registry_id": layout.network_identity["btc_activation_registry_id"]}
        if any(external.get(k) != v for k, v in expected.items()):
            raise ValueError("CANDIDATE_IDENTITY_MISMATCH: response does not match the pinned state")
        if candidate.get("view_version") != VIEW or candidate.get("selection_rule") != RULE:
            raise ValueError("CANDIDATE_CONTRACT_MISMATCH: unsupported selection contract")
        for key in ("stable_block_hash", "active_version_set_id"):
            if not isinstance(external.get(key), str) or not HASH.fullmatch(external[key]):
                raise ValueError(f"CANDIDATE_IDENTITY_MISMATCH: invalid {key}")
        if (type(external.get("stable_lag")) is not int or external["stable_lag"] < 0
                or not isinstance(external.get("active_version_set"), dict)
                or not external.get("balance_history_api_version") or not external.get("balance_history_semantics_version")):
            raise ValueError("CANDIDATE_IDENTITY_MISMATCH: incomplete protocol identity")
        profile = candidate.get("pass", {})
        if (profile.get("state") != "active" or profile.get("pass_kind") != "standard"
                or str(profile.get("usdb_main", "")).lower() != address
                or not PASS.fullmatch(str(profile.get("pass_id", "")))):
            raise ValueError("INELIGIBLE_CANDIDATE: expected an Active Standard pass for this address")
        if expect_pass and profile["pass_id"] != expect_pass:
            raise ValueError(f"UNEXPECTED_PASS: selected {profile['pass_id']}; expected {expect_pass}")
        for key in ("raw_energy", "collab_contribution", "effective_energy"):
            if not re.fullmatch(r"0|[1-9][0-9]*", str(profile.get(key, ""))):
                raise ValueError(f"INVALID_CANDIDATE: invalid {key}")
        if type(candidate.get("matching_candidate_count")) is not int or candidate["matching_candidate_count"] < 1:
            raise ValueError("INVALID_CANDIDATE: missing matching candidate count")
        if (type(profile.get("level")) is not int or not 0 <= profile["level"] <= 255
                or type(profile.get("difficulty_factor_bps")) is not int
                or not 0 < profile["difficulty_factor_bps"] <= 10000):
            raise ValueError("INVALID_CANDIDATE: invalid level or difficulty factor")
        aggregate = candidate.get("miner_aggregate")
        if (not isinstance(aggregate, dict)
                or not re.fullmatch(r"0|[1-9][0-9]*", str(aggregate.get("total_miner_btc_sats", "")))):
            raise ValueError("INVALID_CANDIDATE: missing BTC balance aggregate")
        after = rpc(layout, "get_readiness", indexer=True)
        if not isinstance(after, dict):
            raise ValueError("INVALID_READINESS: expected a readiness object")
        if after.get("upstream_reorg_epoch") != epoch:
            raise ValueError("REORG_DURING_CHECK: upstream reorg epoch changed")
        keys = ("synced_block_height", "upstream_snapshot_id", "local_state_commit", "system_state_id")
        if after.get("consensus_ready") is True and all(after.get(k) == ready[k] for k in keys):
            return {"candidate": candidate, "upstream_reorg_epoch": epoch}
    raise ValueError("UPSTREAM_MOVING: no stable observation; retry the check")


def chain_view(layout: node.ReleaseLayout):
    calls = (
        ("eth_chainId", []), ("net_version", []), ("eth_getBlockByNumber", ["0x0", False]),
        ("eth_blockNumber", []), ("eth_syncing", []), ("net_peerCount", []), ("admin_nodeInfo", []))
    env = node.read_env(layout.node_env)
    url = node._host_rpc_url(env, "USDB_HTTP_BIND_ADDRESS", "USDB_HTTP_BIND_PORT", 8545)
    values = node._json_rpc_batch(url, calls, timeout_secs=8)
    identity = layout.network_identity
    if (node._hex_quantity(values["eth_chainId"], "chain ID") != int(identity["chain_id"])
            or str(values["net_version"]) != str(identity["network_id"])
            or (values["eth_getBlockByNumber"] or {}).get("hash") != identity["genesis_block_hash"]):
        raise ValueError("CHAIN_IDENTITY_MISMATCH: RPC does not serve this release network")
    info = values["admin_nodeInfo"]
    if not isinstance(info, dict) or not re.fullmatch(r"[0-9a-f]{64}", str(info.get("id", ""))):
        raise ValueError("NODE_IDENTITY_UNAVAILABLE: admin_nodeInfo has no valid node ID")
    return {"height": node._hex_quantity(values["eth_blockNumber"], "chain height"),
            "syncing": values["eth_syncing"], "peers": node._hex_quantity(values["net_peerCount"], "peer count"),
            "node_id": info["id"], "enode": info.get("enode"), "network": identity}


def parse_seeds(value):
    seeds = []
    for entry in value.split(","):
        if not entry.strip():
            continue
        parsed = urlsplit(entry.strip())
        try:
            valid = (parsed.scheme == "enode" and re.fullmatch(r"[0-9a-fA-F]{128}", parsed.username or "")
                     and parsed.hostname and parsed.port and not parsed.password and not parsed.fragment
                     and not parsed.path and (not parsed.query or re.fullmatch(r"discport=[0-9]+", parsed.query)))
        except ValueError:
            valid = False
        if not valid:
            raise ValueError("INVALID_PEER_SOURCE: USDB_BOOTNODES must contain comma-separated enode URLs")
        seeds.append(entry.strip())
    return seeds


def peer_check(layout: node.ReleaseLayout, env, chain, data_binding, first_node):
    seeds = parse_seeds(env.get("USDB_BOOTNODES", ""))
    record = read_state(layout).get("first_node")
    remembered = (record and record.get("binding") == data_binding and record.get("node_id") == chain["node_id"])
    if chain["syncing"] is not False:
        raise ValueError("CHAIN_SYNCING: wait for chain synchronization before enabling mining")
    if first_node and seeds:
        raise ValueError("FIRST_NODE_CONFLICT: configured peers select joining an existing network")
    if seeds:
        if chain["peers"] < 1:
            raise ValueError("PEERS_UNREACHABLE: check the seed, NAT and 31303/TCP+UDP; never fall back to first-node")
        peers = rpc(layout, "admin_peers")
        # eth status handshakes reject wrong genesis/fork identities. Also require
        # an actual eth peer on the intended network, rather than a raw discovery count.
        protocols = [p.get("protocols", {}).get("eth") for p in peers if isinstance(p, dict)]
        if not any(isinstance(p, dict) and type(p.get("version")) is int and p["version"] >= 66
                   for p in protocols):
            raise ValueError("PEERS_NOT_CONFIRMED: no established eth peer on the configured network")
        return {"mode": "join", "seed_count": len(seeds)}
    if remembered:
        return {"mode": "first-node", "record": record}
    if not first_node:
        raise ValueError("PEER_SOURCE_REQUIRED: configure USDB_BOOTNODES or explicitly use --first-node")
    if chain["height"] != 0 or chain["peers"] != 0:
        raise ValueError("FIRST_NODE_CONFLICT: first declaration requires genesis and no connected peers")
    return {"mode": "first-node", "record": {"binding": data_binding, "node_id": chain["node_id"]}}


def preflight(layout: node.ReleaseLayout, address: str, *, threads: int = 1, first_node: bool = False,
              expect_pass: str | None = None) -> dict[str, Any]:
    """Build a read-only enable plan from fresh chain, peer and upstream state."""
    address = address_check(layout, address)
    env = node.read_env(layout.node_env)
    node._validate_node_config(layout, require_runtime=True, require_bitcoin_runtime=True)
    node._validate_node_release_images(layout)
    if any(arg.startswith(RESERVED) for arg in shlex.split(env.get("USDB_CHAIN_EXTRA_ARGS", ""))):
        raise ValueError("CONFLICTING_EXTRA_ARGS: remove identity, discovery or mining overrides")
    runtime = inspect_chain(layout)
    cpus = check_resources(layout, env, runtime, threads)
    if runtime["state"] != "running" or not runtime["argv"]:
        raise ValueError("CHAIN_NOT_RUNNING: start the full node before enabling mining")
    data_binding = binding(layout, env)
    chain = chain_view(layout)
    peer = peer_check(layout, env, chain, data_binding, first_node)
    upstream = upstream_candidate(layout, address, expect_pass)
    return {"schema_version": SCHEMA, "state": "READY", "release_id": layout.release_id,
            "address": address, "threads": threads, "available_cpus": cpus,
            "binding": data_binding, "chain": chain, "peer": peer, **upstream,
            "bootstrap": bootstrap_status(layout, chain),
            "economics": economics_status(layout, chain, upstream["candidate"]),
            "config_fingerprint": fingerprint(env), "expect_pass": expect_pass,
            "impact": "Recreate only usdb-chain; preserve data and upstream processes"}


def _write_role(layout: node.ReleaseLayout, target):
    original = layout.node_env.read_text()
    node._atomic_write_private(layout.node_env, node.upsert_env(original, target))
    descriptor = os.open(layout.node_env.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def make_operation(layout: node.ReleaseLayout, target, plan=None):
    env = node.read_env(layout.node_env)
    old = read_state(layout)
    first_node = old.get("first_node")
    if not first_node and old.get("phase") in {"STARTING", "ROLLING_BACK", "FAILED"}:
        # The process may have sealed blocks before its RPC became observable.
        # Keep the acknowledged identity when cancelling that interrupted start.
        first_node = old.get("plan", {}).get("peer", {}).get("record")
    operation = {"operation_id": uuid.uuid4().hex, "release_id": layout.release_id,
                 "phase": "QUEUED", "original": role_config(env), "target": target,
                 "config_fingerprint": fingerprint(env), "binding": binding(layout, env),
                 "authorization_config": authorization_config(env),
                 "first_node": first_node, "previous_operation_id": old.get("operation_id")}
    if plan:
        operation["plan"] = plan
    return operation


def _same_target(operation, target):
    return operation.get("target") == target and operation.get("phase") not in {"FAILED", "CANCELLED"}


def submit(layout: node.ReleaseLayout, *, address=None, threads=1, first_node=False, expect_pass=None, disable=False,
           yes=False, json_output=False):
    target = {"USDB_NODE_ROLE": "full" if disable else "miner",
              "USDB_MINER_ADDRESS": "" if disable else address_check(layout, address),
              "USDB_MINER_THREADS": "1" if disable else str(threads)}
    node._require_controller_unit(layout)
    existing = read_state(layout)
    if not disable and existing and existing.get("phase") not in TERMINAL:
        if not _same_target(existing, target):
            raise ValueError("MINING_OPERATION_BUSY: another target is pending; use mining disable to cancel")
        # Attaching is read-only and must work while the controller owns the lock.
        node.start_controller_unit(layout)
        return {**existing, "outcome": "controller_submitted"}
    if disable and not yes:
        if not sys.stdin.isatty():
            raise ValueError("CONFIRMATION_REQUIRED: non-interactive disable requires --yes")
        print("Stop local mining and apply full role to chain; preserve data and upstream processes.", file=sys.stderr)
        if input("Apply mining disable? [y/N] ").strip().lower() not in {"y", "yes"}:
            raise ValueError("MINING_CANCELLED: no configuration or container changes")
        yes = True
    # Stop only orchestration before cancelling an enable. Docker data services
    # stay alive; the durable disable task then owns the same operation lock.
    if disable:
        node.stop_controller_unit(layout)
    with node.node_operation_lock(layout, "mining-disable" if disable else "mining-enable"):
        operation = read_state(layout)
        if operation and operation.get("phase") not in TERMINAL and not disable:
            if not _same_target(operation, target):
                raise ValueError("MINING_OPERATION_BUSY: another target is pending; use mining disable to cancel")
            result = operation
        else:
            plan = None if disable else preflight(layout, target["USDB_MINER_ADDRESS"], threads=threads,
                                                  first_node=first_node, expect_pass=expect_pass)
            if not yes:
                if not sys.stdin.isatty():
                    raise ValueError("CONFIRMATION_REQUIRED: non-interactive enable/disable requires --yes")
                print_report(plan or {"state": "DISABLE", "address": target["USDB_MINER_ADDRESS"],
                                      "impact": "Stop local mining; recreate only usdb-chain as full"}, output=sys.stderr)
                if input("Apply this mining operation? [y/N] ").strip().lower() not in {"y", "yes"}:
                    raise ValueError("MINING_CANCELLED: no configuration or container changes")
            env = node.read_env(layout.node_env)
            runtime = inspect_chain(layout)
            observed_mining = None
            observed_coinbase = None
            if runtime_matches(layout, env, runtime):
                try:
                    observed_mining = rpc(layout, "eth_mining")
                    if not disable:
                        observed_coinbase = rpc(layout, "eth_coinbase")
                except (OSError, ValueError):
                    pass
            if (role_config(env) == target and _same_target(operation, target)
                    and operation.get("phase") == "APPLIED"
                    and observed_mining is (not disable)
                    and (disable or str(observed_coinbase).lower() == target["USDB_MINER_ADDRESS"])):
                return {**operation, "outcome": "already_applied"}
            operation = make_operation(layout, target, plan)
            if disable and pending(layout):
                operation["cancelled_operation_id"] = read_state(layout).get("operation_id")
            write_state(layout, operation)
            result = operation
    # A disconnect between the durable write and this submission is recoverable
    # with mining enable/disable or up; systemd also resumes it after host restart.
    node.start_controller_unit(layout)
    return {**result, "outcome": "controller_submitted"}


def validate_start(layout: node.ReleaseLayout):
    """Gate every host-helper path that can create a configured miner."""
    env = node.read_env(layout.node_env)
    if env.get("USDB_NODE_ROLE", "full") != "miner":
        return
    operation = read_state(layout)
    if (operation.get("phase") not in {"STARTING", "APPLIED"}
            or operation.get("target") != role_config(env)
            or operation.get("binding") != binding(layout, env)
            or operation.get("authorization_config") != authorization_config(env)
            or (operation.get("phase") != "APPLIED" and operation.get("config_fingerprint") != fingerprint(env))):
        raise ValueError("MINING_AUTHORIZATION_REQUIRED: use usdb-node mining enable before starting this miner configuration")
    node._validate_node_config(layout, require_runtime=True, require_bitcoin_runtime=True)
    node._validate_node_release_images(layout)
    fresh = upstream_candidate(layout, env["USDB_MINER_ADDRESS"],
                               operation.get("plan", {}).get("expect_pass") if operation.get("phase") == "STARTING" else None)
    prior_epoch = operation.get("plan", {}).get("upstream_reorg_epoch")
    if prior_epoch != fresh["upstream_reorg_epoch"]:
        raise ValueError("REORG_AFTER_PLAN: mining authorization belongs to a different upstream epoch")


def _phase(layout: node.ReleaseLayout, operation, phase, **fields):
    operation.update(phase=phase, **fields)
    write_state(layout, operation)
    print(f"Mining operation {operation['operation_id']}: phase={phase}", file=sys.stderr, flush=True)


def _assert_operation(layout: node.ReleaseLayout, operation):
    env = node.read_env(layout.node_env)
    if operation.get("release_id") != layout.release_id:
        raise OwnershipError("MINING_RELEASE_CHANGED: finish or disable the pending operation using its original release")
    if operation.get("binding") != binding(layout, env):
        raise OwnershipError("MINING_DATA_CHANGED: pending operation does not own this chain database")
    if operation.get("config_fingerprint") != fingerprint(env):
        raise OwnershipError("MINING_CONFIG_CHANGED: configuration changed outside the pending operation")
    if role_config(env) not in (operation["original"], operation["target"], operation.get("rollback_target")):
        raise OwnershipError("MINING_ROLE_CHANGED: configuration no longer matches the operation")
    return env


def _recreate(layout: node.ReleaseLayout):
    node.run_helper(layout, "run_testnet_runtime.sh", ["recreate-chain"], output_to_stderr=True)


def _stop(layout: node.ReleaseLayout):
    node.run_helper(layout, "run_testnet_runtime.sh", ["stop-chain"], output_to_stderr=True)


def _rollback(layout: node.ReleaseLayout, operation, error):
    """Never restore an unvalidated miner or erase blocks mined before failure."""
    _phase(layout, operation, "ROLLING_BACK", error=str(error))
    target = {"USDB_NODE_ROLE": "full", "USDB_MINER_ADDRESS": "", "USDB_MINER_THREADS": "1"}
    operation["rollback_target"] = target
    write_state(layout, operation)
    _stop(layout)
    _write_role(layout, target)
    try:
        guard_check(node.read_env(layout.node_env))
    except ValueError as guard_error:
        _phase(layout, operation, "FAILED", rollback="full_configured_chain_halted", rollback_detail=str(guard_error))
        return
    _recreate(layout)
    # Full recovery must be observed, rather than inferred from a config write.
    for _ in range(12):
        try:
            env = node.read_env(layout.node_env)
            runtime = inspect_chain(layout)
            chain_view(layout)
            if runtime_matches(layout, env, runtime) and rpc(layout, "eth_mining") is False:
                _phase(layout, operation, "FAILED", rollback="full_running")
                return
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
        time.sleep(5)
    _phase(layout, operation, "FAILED", rollback="unverified", rollback_detail="full RPC did not become ready; inspect chain logs")


def run_operation(layout: node.ReleaseLayout, *, wait_secs: float = 120) -> int:
    """Resume one transaction while the caller owns the shared operation lock.

    Each side effect has a durable predecessor. In STARTING, an existing target
    container is observed, never recreated just because DAG/RPC is warming up.
    """
    operation = read_state(layout)
    if not operation or operation.get("phase") in TERMINAL:
        return 0
    try:
        env = _assert_operation(layout, operation)
    except (OSError, ValueError) as error:
        _phase(layout, operation, "FAILED", error=str(error), rollback="refused_identity_or_config_change")
        return node.CONTROLLER_MANUAL_EXIT_CODE
    try:
        target = operation["target"]
        enabling = target["USDB_NODE_ROLE"] == "miner"
        if operation["phase"] == "ROLLING_BACK":
            _rollback(layout, operation, operation.get("error", "interrupted operation"))
            return node.CONTROLLER_MANUAL_EXIT_CODE
        if operation["phase"] == "QUEUED":
            if enabling:
                plan = operation["plan"]
                fresh = preflight(layout, target["USDB_MINER_ADDRESS"], threads=int(target["USDB_MINER_THREADS"]),
                                  first_node=plan["peer"]["mode"] == "first-node", expect_pass=plan.get("expect_pass"))
                if fresh["upstream_reorg_epoch"] != plan["upstream_reorg_epoch"]:
                    raise ValueError("REORG_AFTER_PLAN: repeat enable against a freshly reviewed state")
                operation["plan"] = fresh
            _phase(layout, operation, "STOPPING")
        if operation["phase"] == "STOPPING":
            _stop(layout)
            _phase(layout, operation, "STOPPED")
        if operation["phase"] == "STOPPED":
            _assert_operation(layout, operation)
            if enabling:
                fresh = upstream_candidate(layout, target["USDB_MINER_ADDRESS"], operation["plan"].get("expect_pass"))
                if fresh["upstream_reorg_epoch"] != operation["plan"]["upstream_reorg_epoch"]:
                    raise ValueError("REORG_AFTER_STOP: chain remains subject to the deep-reorg gate")
            _write_role(layout, target)
            _phase(layout, operation, "CONFIGURED")
        if operation["phase"] == "CONFIGURED":
            if not enabling:
                try:
                    guard_check(env)
                except ValueError as error:
                    _phase(layout, operation, "APPLIED", result="disabled_chain_halted", detail=str(error))
                    return 0
            _phase(layout, operation, "STARTING")
        if operation["phase"] == "STARTING":
            runtime = inspect_chain(layout, processes=False)
            target_container = (runtime.get("image") == layout.images["USDB_CHAIN_IMAGE"]
                                and all(runtime.get("environment", {}).get(k) == v for k, v in target.items()))
            if not target_container or runtime["state"] in {"absent", "exited", "created"} and runtime.get("exit_code", 0) == 0:
                if enabling:
                    validate_start(layout)
                _recreate(layout)
            elif runtime["state"] in {"dead", "restarting"} or runtime.get("exit_code", 0) != 0 or runtime.get("error"):
                raise ValueError(f"CHAIN_START_FAILED: state={runtime['state']} exit={runtime.get('exit_code')} {runtime.get('error')}")
            deadline = time.monotonic() + wait_secs
            while time.monotonic() < deadline:
                runtime = inspect_chain(layout)
                try:
                    guard_check(node.read_env(layout.node_env))
                except ValueError as error:
                    if enabling:
                        raise
                    _phase(layout, operation, "APPLIED", result="disabled_chain_halted", detail=str(error))
                    return 0
                if runtime.get("error") or runtime["state"] in {"dead", "exited", "restarting"}:
                    raise ValueError(f"CHAIN_START_FAILED: state={runtime['state']} exit={runtime.get('exit_code')} {runtime.get('error')}")
                if runtime.get("argv") and not runtime_matches(layout, node.read_env(layout.node_env), runtime):
                    raise ValueError("CHAIN_ARGUMENT_MISMATCH: running geth did not adopt the requested role, image or parameters")
                if runtime_matches(layout, node.read_env(layout.node_env), runtime):
                    try:
                        chain = chain_view(layout)
                        mining = rpc(layout, "eth_mining")
                        coinbase = rpc(layout, "eth_coinbase") if enabling else None
                    except (OSError, ValueError) as error:
                        if str(error).startswith(("CHAIN_IDENTITY_MISMATCH", "NODE_IDENTITY_UNAVAILABLE")):
                            raise
                        time.sleep(5)
                        continue
                    if mining is enabling and (not enabling or str(coinbase).lower() == target["USDB_MINER_ADDRESS"]):
                        if enabling and operation["plan"]["peer"]["mode"] == "first-node":
                            record = operation["plan"]["peer"]["record"]
                            if record["node_id"] != chain["node_id"]:
                                raise ValueError("NODE_IDENTITY_CHANGED: runtime did not preserve the node key")
                            operation["first_node"] = record
                        _phase(layout, operation, "APPLIED", result="miner_configured" if enabling else "disabled")
                        return 0
                time.sleep(5)
            _phase(layout, operation, "STARTING", detail="Waiting for runtime RPC/mining configuration; inspect mining status and chain logs")
            return 1  # systemd retries observation, not container recreation.
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        if isinstance(error, OwnershipError):
            _phase(layout, operation, "FAILED", error=str(error), rollback="refused_identity_or_config_change")
        elif operation.get("phase") == "QUEUED":
            _phase(layout, operation, "FAILED", error=str(error), rollback="not_needed")
        else:
            try:
                _rollback(layout, operation, error)
            except (OSError, ValueError, subprocess.SubprocessError) as rollback_error:
                _phase(layout, operation, "FAILED", error=str(error), rollback="failed", rollback_detail=str(rollback_error))
        return node.CONTROLLER_MANUAL_EXIT_CODE
    return 0


def reconcile_config(layout: node.ReleaseLayout):
    """Apply legacy non-miner role drift; miners require an explicit enable record."""
    env = node.read_env(layout.node_env)
    if env.get("USDB_NODE_ROLE", "full") == "miner":
        validate_start(layout)
    operation = make_operation(layout, role_config(env))
    if env.get("USDB_NODE_ROLE", "full") == "miner":
        old = read_state(layout)
        operation["plan"] = old["plan"]
    write_state(layout, operation)
    return run_operation(layout)


def local_seal(layout: node.ReleaseLayout, runtime):
    """Accept only full JSON log hashes confirmed at their canonical height."""
    if not runtime.get("id"):
        return None
    result = subprocess.run(["docker", "logs", "--tail", "200", runtime["id"]],
                            capture_output=True, text=True, timeout=8, check=True)
    for line in reversed((result.stdout + result.stderr).splitlines()):
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if value.get("msg") != "Successfully sealed new block":
            continue
        block_hash = value.get("hash")
        if not isinstance(block_hash, str) or not re.fullmatch(r"0x[0-9a-f]{64}", block_hash):
            continue
        block = rpc(layout, "eth_getBlockByHash", [block_hash, False])
        if not isinstance(block, dict) or "number" not in block:
            continue
        canonical = rpc(layout, "eth_getBlockByNumber", [block["number"], False])
        return {"hash": block_hash, "height": node._hex_quantity(block["number"], "seal height"),
                "canonical": isinstance(canonical, dict) and canonical.get("hash") == block_hash}
    return None


def bootstrap_status(layout: node.ReleaseLayout, chain):
    result = {"bootstrap_finalized": None}
    try:
        genesis = node._load_json(layout.bundle_dir / "artifacts/usdb-genesis.json")
        config = genesis["config"]
        gate = config.get("dividendFeeSplitBlock")
        result.update(fee_split_block=gate, blocks_remaining=max(0, int(gate) - chain["height"]) if gate is not None else None)
        selector = rpc(layout, "web3_sha3", ["0x" + b"bootstrapFinalized()".hex()])[:10]
        value = rpc(layout, "eth_call", [{"to": config["dividendAddress"], "data": selector}, "latest"])
        if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", value) or int(value, 16) not in {0, 1}:
            raise ValueError("bootstrapFinalized() returned an invalid ABI boolean")
        result["bootstrap_finalized"] = bool(int(value, 16))
    except (ValueError, KeyError, TypeError, OSError) as error:
        result["observation_error"] = str(error)
    return result


def economics_status(layout: node.ReleaseLayout, chain, candidate):
    """Display a conditional first-block estimate without adding eligibility gates."""
    total_sats = int(candidate["miner_aggregate"]["total_miner_btc_sats"])
    result = {"total_miner_btc_sats": str(total_sats), "unit_sats": 100000,
              "energy_note": "Zero energy is eligible; new energy growth uses stable owner balance in whole UNIT_SATS units"}
    try:
        genesis = node._load_json(layout.bundle_dir / "artifacts/usdb-genesis.json")
        versions = genesis["config"]["usdb"]["activations"][0]["versions"]
        if chain["height"] == 0 and versions.get("pricePolicyVersion") == 1 and versions.get("coinbaseEmissionPolicyVersion") == 1:
            value = rpc(layout, "eth_getStorageAt", ["0x" + "0" * 36 + "1000", ISSUED_SLOT, "0x0"])
            if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]{64}", value):
                raise ValueError("Invalid cumulative issuance storage")
            issued = int(value, 16)
            target = total_sats * 100000 * 10**18 // 100000000
            result.update(issued_usdb_atoms=str(issued), target_usdb_atoms=str(target),
                          first_block_emission_atoms=str(max(0, target - issued) // 157680),
                          estimate_assumption="First block, fixed price v1, K=1; unchanged BTC state; fees excluded")
    except (OSError, ValueError, KeyError, IndexError, TypeError) as error:
        result["estimate_unavailable"] = str(error)
    return result


def observe(layout: node.ReleaseLayout, *, details: bool = False) -> dict[str, Any]:
    """Report intent and runtime separately; full-role DISABLED is healthy."""
    report = {"schema_version": SCHEMA, "observed_at": node.datetime.now(node.timezone.utc).isoformat(),
              "state": "WAITING", "applied": False}
    try:
        env = node.read_env(layout.node_env)
        report["configured"] = role_config(env)
        operation = read_state(layout)
        report["operation"] = {key: operation.get(key) for key in
                               ("operation_id", "phase", "target", "error", "rollback", "rollback_detail", "detail")}
        runtime = inspect_chain(layout)
        report["runtime"] = {key: runtime.get(key) for key in ("id", "state", "image", "environment", "error", "exit_code")}
        try:
            guard_check(env)
        except ValueError as error:
            report.update(state="BLOCKED", detail=str(error))
            return report
        if operation and operation.get("phase") not in TERMINAL:
            report.update(state="SWITCHING", detail=f"operation phase={operation['phase']}")
            return report
        if operation.get("phase") == "FAILED":
            report.update(state="FAILED", detail=operation.get("error", "Mining operation failed"))
            return report
        if not runtime_matches(layout, env, runtime):
            report.update(state="BLOCKED", drift=True, detail="Configured role/image/parameters are not applied; use mining enable/disable or up")
            return report
        chain = chain_view(layout)
        mining = rpc(layout, "eth_mining")
        report.update(chain=chain, mining=mining)
        miner = env.get("USDB_NODE_ROLE", "full") == "miner"
        if miner:
            coinbase = rpc(layout, "eth_coinbase")
            if str(coinbase).lower() != env.get("USDB_MINER_ADDRESS", "").lower():
                report.update(state="BLOCKED", drift=True, detail="Runtime coinbase does not match the configured mining address")
                return report
            report["applied"] = mining is True
            try:
                report["eligibility"] = upstream_candidate(layout, env["USDB_MINER_ADDRESS"])
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                report.update(state="BLOCKED" if str(error).startswith("DEEP_REORG") else "WAITING", detail=str(error),
                              observation_unavailable="unavailable" in str(error).lower() or "timed out" in str(error).lower())
                return report
            try:
                work = rpc(layout, "eth_getWork")
                if not isinstance(work, list) or len(work) < 3 or not all(re.fullmatch(r"0x[0-9a-fA-F]{64}", str(v)) for v in work[:3]):
                    raise ValueError("No valid current work")
                report.update(state="ACTIVE" if mining is True else "WARMING_UP", work_available=True)
            except (OSError, ValueError) as error:
                report.update(state="WARMING_UP", work_available=False, detail=f"Waiting for DAG/work: {error}")
        else:
            report.update(state="DISABLED" if mining is False else "BLOCKED", applied=mining is False)
            if mining is not False:
                report.update(drift=True, detail="Runtime is mining despite the configured non-miner role; use mining disable")
        if details:
            report["bootstrap"] = bootstrap_status(layout, chain)
            try:
                report["last_local_seal"] = local_seal(layout, runtime)
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                report["last_local_seal"] = None
                report["local_seal_observation_error"] = str(error)
            enode = chain.get("enode")
            try:
                address = ipaddress.ip_address(urlsplit(enode or "").hostname or "")
                report["shareable_enode"] = enode if address.is_global else None
            except ValueError:
                report["shareable_enode"] = None
            report["peer_guidance"] = "Verify external enode address and 31303/TCP+UDP mapping before sharing; preserve the node key"
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        identity_error = str(error).startswith(("CHAIN_IDENTITY_MISMATCH", "NODE_IDENTITY_UNAVAILABLE", "MINING_JOURNAL_INVALID"))
        report.update(state="BLOCKED" if identity_error else "WAITING", detail=str(error), observation_unavailable=not identity_error)
    return report


def print_report(report, *, output=sys.stdout):
    print(f"USDB mining | state={report.get('state', report.get('phase', 'UNKNOWN'))}", file=output)
    if "address" in report:
        print(f"Address: {report['address']} | CPU workers: {report.get('threads', 1)}", file=output)
    candidate = report.get("candidate", {})
    if candidate:
        profile = candidate["pass"]
        print(f"Pass: {profile['pass_id']} | candidates={candidate['matching_candidate_count']} | "
              f"state={profile['state']}/{profile['pass_kind']} | energy={profile['effective_energy']} | "
              f"level={profile.get('level')} | difficulty={profile.get('difficulty_factor_bps')} bps", file=output)
        print(f"BTC height: {candidate['external_state']['btc_height']} | reorg epoch={report['upstream_reorg_epoch']}", file=output)
    if "chain" in report:
        chain = report["chain"]
        print(f"Chain: {chain['network']['chain_id']} | genesis={chain['network']['genesis_block_hash']} | "
              f"height={chain['height']} | peers={chain['peers']}", file=output)
    if "configured" in report:
        print(f"Configured: {json.dumps(report['configured'], sort_keys=True)} | applied={report['applied']}", file=output)
    for key in ("operation", "bootstrap", "economics", "last_local_seal", "shareable_enode", "peer_guidance", "impact", "detail"):
        if report.get(key) is not None:
            print(f"{key}: {json.dumps(report[key], sort_keys=True) if isinstance(report[key], dict) else report[key]}", file=output)


def add_parser(subparsers):
    parser = subparsers.add_parser("mining", help="Check, enable, disable or observe managed CPU mining")
    actions = parser.add_subparsers(dest="mining_action", required=True, metavar="{check,enable,disable,status}")
    for name in ("check", "enable"):
        command = actions.add_parser(name)
        command.add_argument("--address", required=True)
        command.add_argument("--threads", type=int, default=1, help="CPU PoW workers (default: 1; must be positive)")
        command.add_argument("--first-node", action="store_true", help="Explicitly acknowledge genesis cold start without peers")
        command.add_argument("--expect-pass", help="Assert the protocol-selected pass ID without overriding selection")
        command.add_argument("--json", action="store_true")
        if name == "enable":
            command.add_argument("--yes", action="store_true")
    disable = actions.add_parser("disable", help="Persistently stop mining even when upstream RPC is unavailable")
    disable.add_argument("--yes", action="store_true")
    disable.add_argument("--json", action="store_true")
    status = actions.add_parser("status")
    status.add_argument("--watch", action="store_true")
    status.add_argument("--json", action="store_true")
    status.add_argument("--refresh-secs", type=float, default=5)
    actions.add_parser("validate-start")


def execute(layout: node.ReleaseLayout, args: argparse.Namespace) -> int:
    """Dispatch the operator CLI and optionally attach a display to systemd work."""
    if args.mining_action == "validate-start":
        validate_start(layout)
        return 0
    if args.mining_action == "status":
        if not math.isfinite(args.refresh_secs) or args.refresh_secs <= 0:
            raise ValueError("refresh interval must be positive and finite")
        cached = None
        cached_at = 0.0
        while True:
            report = observe(layout, details=True)
            if report.get("observation_unavailable") and cached and time.monotonic() - cached_at <= 60:
                report = {**cached, "state": "STALE", "observation_unavailable": True,
                          "detail": report["detail"], "last_observed_at": cached["observed_at"]}
            elif not report.get("observation_unavailable"):
                cached, cached_at = report, time.monotonic()
            print(json.dumps(report, indent=2, sort_keys=True)) if args.json else print_report(report)
            if not args.watch:
                return 0 if report["state"] in {"ACTIVE", "DISABLED", "WARMING_UP"} else 1
            try:
                time.sleep(args.refresh_secs)
            except KeyboardInterrupt:
                return 0
    if args.mining_action == "check":
        report = preflight(layout, args.address, threads=args.threads, first_node=args.first_node, expect_pass=args.expect_pass)
    else:
        report = submit(layout, address=getattr(args, "address", None), threads=getattr(args, "threads", 1),
                        first_node=getattr(args, "first_node", False), expect_pass=getattr(args, "expect_pass", None),
                        disable=args.mining_action == "disable", yes=args.yes, json_output=args.json)
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print_report(report.get("plan", report))
        if "operation_id" in report:
            print(f"Operation: {report['operation_id']} | phase={report['phase']} | {report['outcome']}")
            print("The systemd controller owns this operation. Use usdb-node mining status --watch to observe it.")
            if sys.stdout.isatty() and report["outcome"] != "already_applied":
                try:
                    while pending(layout):
                        print_report(observe(layout))
                        time.sleep(5)
                except KeyboardInterrupt:
                    print("Detached from progress; the controller continues the submitted operation.")
                    return 0
                final = read_state(layout)
                print_report(observe(layout))
                return 0 if final.get("phase") == "APPLIED" else 1
    return 0
