"""Persist seed edits and apply them through recoverable, chain-only operations."""
from __future__ import annotations

from contextlib import contextmanager
import fcntl
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import urlsplit
import uuid

import usdb_node as node
import usdb_mining as mining
import usdb_p2p as p2p

SCHEMA = "usdb-node-peers:v1"
PHASES = {"QUEUED", "CONFIGURING", "STOPPING", "STARTING", "APPLIED"}
MAX_SEEDS = 64


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


def state_path(layout):
    return layout.node_env.parent / "node.peers.json"


@contextmanager
def intent_lock(layout):
    """Allow a short intent write while the controller holds the lifecycle lock."""
    path = layout.node_env.parent / ".usdb-node-peers.lock"
    with os.fdopen(os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600), "r+") as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def read_state(layout):
    path = state_path(layout)
    if not path.exists():
        return {}
    value = node._load_json(path)
    if (value.get("schema_version") != SCHEMA or value.get("phase") not in PHASES
            or not isinstance(value.get("target"), list) or not value.get("operation_id")
            or not isinstance(value.get("binding"), dict)):
        raise ValueError("PEER_JOURNAL_INVALID: incomplete operation")
    if (not all(isinstance(endpoint, str) for endpoint in value["target"])
            or parse_seeds(",".join(value["target"])) != value["target"]):
        raise ValueError("PEER_JOURNAL_INVALID: noncanonical seed list")
    updates = value.get("transport_updates", {})
    if (not isinstance(updates, dict) or (updates and set(updates) != set(p2p.KEYS))
            or any(not isinstance(entry, str) for entry in updates.values())):
        raise ValueError("PEER_JOURNAL_INVALID: invalid transport updates")
    if updates:
        p2p.validate(updates)
    if value["phase"] not in {"QUEUED", "APPLIED"} and not all(
            key in value for key in ("before", "after", "role", "mining_before", "mining_after", "restart")):
        raise ValueError("PEER_JOURNAL_INVALID: missing application checkpoint")
    return value


def write_state(layout, value):
    """Flush the transaction before and after each configuration/runtime mutation."""
    value.update(schema_version=SCHEMA, updated_at=node.datetime.now(node.timezone.utc).isoformat())
    node._atomic_write_private(state_path(layout), json.dumps(value, indent=2, sort_keys=True) + "\n")
    descriptor = os.open(state_path(layout).parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def pending(layout):
    value = read_state(layout)
    return bool(value and value["phase"] != "APPLIED")


def binding(layout, env):
    """Seeds can be configured before the chain database or node key exists."""
    return {"network": layout.network_identity,
            "data_path": str(Path(env["USDB_CHAIN_DATA_HOST_DIR"]).resolve())}


def remembered_first_node(layout, env, node_id=None):
    """Display the zero-peer exception only for the acknowledged chain identity."""
    record = mining.read_state(layout).get("first_node")
    if not record:
        return False
    if record.get("binding") != mining.binding(layout, env):
        return False
    if node_id is None:
        node_id = mining.rpc(layout, "admin_nodeInfo").get("id")
    return record.get("node_id") == node_id


def membership(layout, env, *, syncing, peer_count, node_id=None):
    """Separate process health from basic network membership, never mining approval."""
    seeds = mining.parse_seeds(env.get("USDB_BOOTNODES", ""))
    founder = not seeds and remembered_first_node(layout, env, node_id)
    if not seeds and not founder:
        return "WAITING", "SEED_REQUIRED", "configure a seed with usdb-node peers add ENODE"
    if peer_count == 0 and not founder:
        return "WAITING", "WAITING_FOR_PEERS", "waiting for a peer; check seed address and TCP/UDP 31303"
    if syncing is not False:
        return "SYNCING", "SYNCING", "synchronizing the USDB chain"
    if founder:
        return "READY", "FIRST_NODE", "acknowledged first node"
    return "READY", "CONNECTED", "connected; local chain reports no active synchronization"


def submit(layout, action, enode=None, *, transport_updates=None):
    """Queue edits without changing the active config under another operation."""
    endpoint = normalize_enode(enode) if enode is not None else None
    if transport_updates is not None:
        if set(transport_updates) != set(p2p.KEYS):
            raise ValueError("incomplete P2P transport configuration")
        p2p.validate(transport_updates)
    node._require_controller_unit(layout)
    with intent_lock(layout):
        env = node.read_env(layout.node_env)
        old = read_state(layout)
        if action == "apply":
            if not old or old["phase"] == "APPLIED":
                return {"outcome": "already_applied", **old}
            operation = old
            # An explicit retry may replace a failed target container, while
            # ordinary controller retries only observe an already started one.
            if operation["phase"] == "STARTING" and operation.get("error"):
                operation["phase"] = "STOPPING"
            operation.pop("error", None)
        else:
            replacing_transport = (action == "configure" and old.get("error")
                                   and old.get("phase") in {"STOPPING", "STARTING"})
            if replacing_transport and (old["binding"] != binding(layout, env) or mining.fingerprint(env) != old["after"]):
                raise ValueError("PEER_CONFIG_CHANGED: cannot replace a transport operation after external drift")
            if old and old["phase"] not in {"QUEUED", "APPLIED"} and not replacing_transport:
                raise ValueError("PEER_OPERATION_BUSY: use peers apply to finish the current change before editing again")
            seeds = old["target"][:] if old.get("phase") == "QUEUED" else parse_seeds(env.get("USDB_BOOTNODES", ""))
            transport = transport_updates if transport_updates is not None else (
                old.get("transport_updates", {}) if old.get("phase") == "QUEUED" else {})
            before = seeds[:]
            if action == "add" and endpoint not in seeds:
                seeds.append(endpoint)
            elif action == "remove" and endpoint in seeds:
                seeds.remove(endpoint)
            if len(seeds) > MAX_SEEDS:
                raise ValueError(f"INVALID_PEER_SOURCE: at most {MAX_SEEDS} seed endpoints are supported")
            if before == seeds and all(env.get(k, "") == v for k, v in transport.items()) and not pending(layout):
                return {"outcome": "unchanged", "target": seeds}
            if old.get("phase") == "QUEUED":
                if old["binding"] != binding(layout, env) or old["original_seeds"] != env.get("USDB_BOOTNODES", ""):
                    raise ValueError("PEER_CONFIG_CHANGED: configuration changed outside the pending peer operation")
            operation = {"operation_id": uuid.uuid4().hex, "phase": "QUEUED", "target": seeds,
                         "original_seeds": env.get("USDB_BOOTNODES", ""), "binding": binding(layout, env),
                         "resume_bootstrap": (old.get("phase") != "APPLIED" and old.get("resume_bootstrap", False)) or
                             node.controller_active_state(layout) in {"active", "activating"}}
            operation["transport_updates"] = transport
        write_state(layout, operation)
    # The controller consumes a peer-only request without implicitly bringing up
    # a stopped node. An already running bootstrap resumes after applying it.
    node.start_controller_unit(layout)
    return {**operation, "outcome": "controller_submitted"}


def request_bootstrap(layout):
    """An explicit up request also resumes bootstrap after a pending peer edit."""
    with intent_lock(layout):
        value = read_state(layout)
        if value and value["phase"] != "APPLIED":
            value["resume_bootstrap"] = True
            write_state(layout, value)


def pause_bootstrap(layout):
    """An explicit down preserves queued seeds but cancels their startup intent."""
    with intent_lock(layout):
        value = read_state(layout)
        if value and value["phase"] != "APPLIED":
            value["resume_bootstrap"] = False
            if value["phase"] != "QUEUED":
                # Finish a possibly partial env/authorization write, then stop.
                value.update(restart=False, phase="CONFIGURING")
            write_state(layout, value)


def defer_after_disable(layout):
    """Let an explicit mining disable take precedence without losing seed intent."""
    with intent_lock(layout):
        value = read_state(layout)
        if value and value["phase"] not in {"QUEUED", "APPLIED"}:
            env = node.read_env(layout.node_env)
            value.update(phase="QUEUED", original_seeds=env.get("USDB_BOOTNODES", ""))
            write_state(layout, value)


def _phase(layout, operation, phase):
    operation["phase"] = phase
    operation.pop("error", None)
    write_state(layout, operation)
    print(f"Peer operation {operation['operation_id']}: phase={phase}", file=sys.stderr, flush=True)


def run_operation(layout):
    """Apply one intent under the caller's node lock; recover partial writes safely."""
    with intent_lock(layout):
        operation = read_state(layout)
        if not operation or operation["phase"] == "APPLIED":
            return 0
        if mining.pending(layout):
            return 1
        try:
            env = node.read_env(layout.node_env)
            if operation["binding"] != binding(layout, env):
                raise ValueError("PEER_DATA_CHANGED: queued seeds belong to another network or data path")
            if operation["phase"] == "QUEUED":
                if env.get("USDB_BOOTNODES", "") != operation["original_seeds"]:
                    raise ValueError("PEER_CONFIG_CHANGED: seed configuration changed outside this operation")
                node._validate_node_config(layout, require_runtime=False, require_bitcoin_runtime=False)
                receipt = mining.read_state(layout)
                if env.get("USDB_NODE_ROLE") == "miner":
                    if (receipt.get("phase") != "APPLIED" or receipt.get("target") != mining.role_config(env)
                            or receipt.get("binding") != mining.binding(layout, env)
                            or receipt.get("authorization_config") != mining.authorization_config(env)):
                        raise ValueError("MINING_AUTHORIZATION_REQUIRED: peer edits cannot authorize a miner")
                    if not operation["target"] and not remembered_first_node(layout, env):
                        raise ValueError("LAST_MINER_SEED: disable mining before removing the last joiner seed")
                updated = {**env, **operation.get("transport_updates", {}), "USDB_BOOTNODES": ",".join(operation["target"])}
                if operation.get("transport_updates"):
                    p2p.check_host(updated)
                operation.update(before=mining.fingerprint(env), after=mining.fingerprint(updated),
                                 role=mining.role_config(env), mining_before=receipt)
                rebound = dict(receipt)
                if receipt:
                    rebound.update(config_fingerprint=mining.fingerprint(updated),
                                   authorization_config=mining.authorization_config(updated))
                operation["mining_after"] = rebound
                runtime = mining.inspect_chain(layout, processes=False)
                operation["restart"] = runtime["state"] not in {"absent", "exited", "dead"}
                _phase(layout, operation, "CONFIGURING")
            if (mining.fingerprint(env) not in {operation["before"], operation["after"]}
                    or mining.role_config(env) != operation["role"]):
                raise ValueError("PEER_CONFIG_CHANGED: configuration changed during peer application")
            if operation["phase"] == "CONFIGURING":
                receipt = mining.read_state(layout)
                # write_state adds timestamps; compare the authorization-bearing fields.
                comparable = lambda value: {k: v for k, v in value.items() if k != "updated_at"}
                if comparable(receipt) not in (comparable(operation["mining_before"]), comparable(operation["mining_after"])):
                    raise ValueError("MINING_OPERATION_CHANGED: refusing to overwrite a different mining receipt")
                node._atomic_write_private(layout.node_env, node.upsert_env(layout.node_env.read_text(),
                    {**operation.get("transport_updates", {}), "USDB_BOOTNODES": ",".join(operation["target"])}))
                if operation["mining_after"]:
                    mining.write_state(layout, operation["mining_after"])
                _phase(layout, operation, "STOPPING" if operation["restart"] else "APPLIED")
            if operation["phase"] == "STOPPING":
                mining.validate_start(layout)
                if operation.get("transport_updates"):
                    p2p.check_host(node.read_env(layout.node_env))
                    if node.configured_firewall_mode(layout) == "managed":
                        node.run_firewall_action(layout, "check", output_to_stderr=True)
                operation["stopped_container_id"] = mining.inspect_chain(layout, processes=False).get("id")
                mining._stop(layout)
                _phase(layout, operation, "STARTING")
            if operation["phase"] == "STARTING":
                env = node.read_env(layout.node_env)
                runtime = mining.inspect_chain(layout)
                flags = mining.flag_values(runtime.get("argv", []))
                matches = mining.runtime_matches(layout, env, runtime) and flags.get("--bootnodes") == env["USDB_BOOTNODES"]
                transport_matches = (not operation.get("transport_updates") or
                                     p2p.transport_ready(env, p2p.container_view(layout)))
                matches = matches and transport_matches
                if matches:
                    _phase(layout, operation, "APPLIED")
                    return 0
                target_container = (runtime.get("image") == layout.images["USDB_CHAIN_IMAGE"] and
                                    runtime.get("environment", {}).get("USDB_BOOTNODES", "") == env["USDB_BOOTNODES"] and
                                    all(runtime.get("environment", {}).get(k) == v for k, v in operation["role"].items()))
                if operation.get("transport_updates"):
                    target_container = target_container and runtime.get("environment", {}).get("USDB_P2P_IP_FAMILY", "ipv4") == env["USDB_P2P_IP_FAMILY"]
                old_container = runtime.get("id") == operation.get("stopped_container_id")
                if target_container and not old_container and (runtime["state"] in {"dead", "restarting"} or runtime.get("exit_code")):
                    raise ValueError(f"CHAIN_START_FAILED: {runtime['state']}; inspect chain logs")
                if target_container and runtime["state"] == "running" and flags:
                    if not transport_matches:
                        raise ValueError("P2P_TRANSPORT_MISMATCH: running container lacks the requested TCP/UDP bindings or IPv6 endpoint")
                    raise ValueError("CHAIN_ARGUMENT_MISMATCH: running geth does not match the requested seeds/role")
                if old_container or not target_container or runtime["state"] in {"absent", "exited"}:
                    mining.validate_start(layout)
                    mining._recreate(layout)
                return 1
            return 0
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            operation["error"] = str(error)
            write_state(layout, operation)
            print(f"Peer operation blocked: {error}", file=sys.stderr)
            return node.CONTROLLER_MANUAL_EXIT_CODE


def observe(layout, *, connected=False):
    """Show desired configuration separately from applied settings and live peers."""
    env = node.read_env(layout.node_env)
    operation = read_state(layout)
    applied = parse_seeds(env.get("USDB_BOOTNODES", ""))
    desired = operation["target"] if operation.get("phase") not in {None, "APPLIED"} else applied
    report = {"schema_version": SCHEMA, "configured": desired, "applied_config": applied,
              "operation": {k: operation[k] for k in ("operation_id", "phase", "error") if k in operation},
              "state": "CONFIGURED", "connected": []}
    if connected:
        try:
            chain = mining.chain_view(layout)
            peers = mining.rpc(layout, "admin_peers")
            if not isinstance(peers, list):
                raise ValueError("invalid admin_peers result")
            report["connected"] = [{k: peer.get(k) for k in ("id", "enode", "name", "network")}
                                   for peer in peers if isinstance(peer, dict)]
            state, reason, detail = membership(layout, env, syncing=chain["syncing"],
                                               peer_count=chain["peers"], node_id=chain["node_id"])
            report.update(state=state, reason=reason, detail=detail, height=chain["height"], peer_count=chain["peers"])
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            mismatch = str(error).startswith("CHAIN_IDENTITY_MISMATCH:")
            report.update(state="BLOCKED" if mismatch else "WAITING",
                          reason="CHAIN_IDENTITY_MISMATCH" if mismatch else "OBSERVATION_UNAVAILABLE", detail=str(error))
        report["membership"] = {key: report[key] for key in ("state", "reason", "detail")}
        if operation and operation["phase"] != "APPLIED" and report["state"] != "BLOCKED":
            report.update(state="BLOCKED" if operation.get("error") else "STARTING",
                          reason="PEER_OPERATION_BLOCKED" if operation.get("error") else "APPLYING_SEEDS",
                          detail=operation.get("error", f"phase={operation['phase']}; waiting for controller application"))
    return report


def add_parser(subparsers):
    parser = subparsers.add_parser("peers", help="Manage persistent seeds and inspect network membership")
    actions = parser.add_subparsers(dest="peers_action", required=True)
    descriptions = {"list": "List desired and applied seed configuration without RPC",
                    "add": "Persist a seed and submit controller application",
                    "remove": "Remove a seed endpoint (does not ban a peer)",
                    "status": "Observe application, membership and live peers",
                    "apply": "Retry an unfinished seed application",
                    "configure": "Select P2P address family and advertised addresses",
                    "enode": "Show IPv4/IPv6 enode candidates and actual container transport",
                    "network": "Inspect host and container P2P configuration"}
    for name, description in descriptions.items():
        command = actions.add_parser(name, help=description, description=description)
        if name in {"add", "remove"}:
            command.add_argument("enode", help="Complete enode URL; quote IPv6 addresses")
        command.add_argument("--json", action="store_true")
        if name == "status":
            command.add_argument("--watch", action="store_true")
            command.add_argument("--refresh-secs", type=float, default=5)
        if name == "configure":
            p2p.add_options(command)
        if name == "enode":
            command.add_argument("--family", choices=("all", "ipv4", "ipv6"), default="all")


def render_report(report, *, connected):
    """Keep desired settings, current settings, and live membership distinct."""
    lines = [f"Peers | {report['state']} | {report.get('reason', '')}",
             f"Operation: {json.dumps(report['operation'])}", "Configured seed endpoints:"]
    lines.extend(f"  {endpoint}" for endpoint in report["configured"])
    if not report["configured"]:
        lines.append("  (none) — use usdb-node peers add ENODE")
    if report["configured"] != report["applied_config"]:
        lines.append("Current node.env seed endpoints (application pending):")
        lines.extend(f"  {endpoint}" for endpoint in report["applied_config"] or ["(none)"])
    if connected:
        lines.append(f"Connected: {len(report['connected'])}; height={report.get('height', 'unknown')}")
        lines.extend(f"  {peer.get('enode') or peer.get('id')}" for peer in report["connected"])
        lines.append(report.get("detail", ""))
    return "\n".join(lines)


def execute(layout, args):
    """Submit durable changes or attach a read-only status display."""
    if args.peers_action == "configure":
        updates, reason = p2p.select(**p2p.options(args))
        report = submit(layout, "configure", transport_updates=updates)
        report["selection_reason"] = reason
        print(json.dumps(report, indent=2) if args.json else
              f"P2P family={updates['USDB_P2P_IP_FAMILY']}: {reason}; {report['outcome']}; observe peers network / peers status")
        return 0
    if args.peers_action in {"enode", "network"}:
        report = p2p.endpoint_report(layout)
        if getattr(args, "family", "all") != "all":
            report["endpoints"] = [entry for entry in report["endpoints"] if entry["family"] == args.family]
            if not report["endpoints"] and not report.get("error"):
                report.update(state="WAITING", error=f"P2P_ADDRESS_UNAVAILABLE: no {args.family} enode candidate")
        if args.json:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            print(f"P2P | {report['state']} | family={report['family']} | public reachability unverified")
            if report.get("desired_family"):
                print(f"  Pending family: {report['desired_family']}; phase={report['operation']['phase']}")
            for entry in report["endpoints"]:
                print(f"  {entry['family']}: {entry['enode']}")
            if not report["endpoints"]:
                print("  No advertised address available; use peers configure --advertise-ipv4/--advertise-ipv6")
            if report.get("error"):
                print(f"  {report['error']}")
            if args.peers_action == "network":
                print(json.dumps({key: report.get(key) for key in ("host", "container")}, indent=2))
            print(report["guidance"])
        return 0 if report["state"] == "CONFIGURED" else 1
    if args.peers_action in {"add", "remove", "apply"}:
        report = submit(layout, args.peers_action, getattr(args, "enode", None))
        print(json.dumps(report, indent=2) if args.json else
              f"Peers: {report['outcome']}; use usdb-node peers status --watch to observe application")
        return 0
    interval = getattr(args, "refresh_secs", 5)
    if not math.isfinite(interval) or interval <= 0:
        raise ValueError("refresh interval must be positive and finite")
    watching = getattr(args, "watch", False)
    display = node.TerminalProgressDisplay(sys.stdout, enabled=watching and not args.json)
    try:
        display.start()
        while True:
            report = observe(layout, connected=args.peers_action == "status")
            if args.json:
                print(json.dumps(report, indent=2, sort_keys=True), flush=True)
            else:
                display.render(render_report(report, connected=args.peers_action == "status"))
            if not watching:
                return 0
            time.sleep(interval)
    except KeyboardInterrupt:
        return 0
    finally:
        display.close()
