"""Probe a candidate Seed using a disposable client in the selected network scope."""
from __future__ import annotations

import hashlib
import ipaddress
import json
import shlex
import subprocess
import uuid
from urllib.parse import urlsplit

import usdb_node as node
import usdb_mining as mining
from peer_sources import normalize_enode

SCHEMA = "usdb-peer-check:v1"
STAGES = ("tcp", "discovery", "identity", "hello", "eth")
LABELS = {"tcp": "TCP", "discovery": "UDP discovery", "identity": "RLPx identity",
          "hello": "P2P protocol", "eth": "USDB network"}
GUIDANCE = {
    "DNS_LOOKUP_FAILED": "Check the hostname and DNS resolver; retry after DNS recovers.",
    "TCP_CONNECT_FAILED": "Check the target TCP port, address-family routing, firewall and node process.",
    "DISCOVERY_FAILED": "Check the discovery UDP port, NAT mapping and firewall; TCP alone is not a Seed discovery check.",
    "RLPX_IDENTITY_FAILED": "Confirm the enode public key belongs to this endpoint and that it serves Geth P2P.",
    "HELLO_IDENTITY_MISMATCH": "Obtain the correct enode from the node operator; the peer identity does not match.",
    "HELLO_FAILED": "Inspect the P2P handshake error and the target node's logs/capacity.",
    "PEER_DISCONNECTED": "Inspect the disconnect reason; the target may be full or rejecting new peers.",
    "NO_SHARED_ETH_PROTOCOL": "Use a Seed running a compatible USDB client release.",
    "ETH_HANDSHAKE_FAILED": "Read the handshake error; check network ID/genesis/fork compatibility, timeouts and peer capacity.",
    "PROBE_TIMEOUT": "Check routing or retry with a larger --timeout-secs budget.",
}


def _blank(endpoint: str) -> dict:
    return {"schema_version": SCHEMA, "enode": endpoint, "state": "INCOMPLETE", "usable": False,
            "checked_at": node.datetime.now(node.timezone.utc).isoformat(),
            "syntax": {"state": "SKIPPED"}, "dns": {"state": "SKIPPED"},
            "endpoints": [], "warnings": [], "guidance": [], "scope": {"kind": "unavailable"}}


def _request(layout, endpoint: str, timeout: int) -> dict:
    """Supply the release's verified genesis, without mounting any node data."""
    network = node._load_json(layout.bundle_dir / "network.json")
    root = layout.bundle_dir.resolve()
    genesis_path = (root / network["artifacts"]["genesis"]["path"]).resolve()
    if not genesis_path.is_relative_to(root) or genesis_path.stat().st_size > 1024 * 1024:
        raise ValueError("PEER_CHECK_NETWORK_INVALID: invalid genesis artifact path or size")
    raw = genesis_path.read_bytes()
    identity = layout.network_identity
    if hashlib.sha256(raw).hexdigest() != identity["genesis_sha256"]:
        raise ValueError("PEER_CHECK_NETWORK_INVALID: genesis artifact checksum mismatch")
    return {"enode": endpoint, "network_id": int(identity["network_id"]),
            "chain_id": int(identity["chain_id"]), "genesis_hash": identity["genesis_block_hash"],
            "genesis": json.loads(raw), "timeout_secs": timeout}


def _scope(layout) -> tuple[dict, str]:
    """Share a running chain's namespace; label host probes when no chain is up."""
    runtime = mining.inspect_chain(layout, processes=False) if layout.node_env.exists() else {}
    if runtime.get("state") == "running":
        container = runtime.get("id", "")
        if not container or any(c not in "0123456789abcdef" for c in container) or not 12 <= len(container) <= 64:
            raise ValueError("PEER_CHECK_SCOPE_UNAVAILABLE: invalid chain container identity")
        return {"kind": "chain-container", "container_id": container}, f"container:{container}"
    return {"kind": "host", "reason": "chain is not running"}, "host"


def _validate_report(value, request):
    """Reject old/incomplete helpers rather than turning missing checks green."""
    if (not isinstance(value, dict) or value.get("schema_version") != SCHEMA
            or value.get("enode") != request["enode"]
            or value.get("network_id") != request["network_id"]
            or value.get("genesis_hash") != request["genesis_hash"]
            or not isinstance(value.get("checked_at"), str)
            or not isinstance(value.get("warnings"), list)
            or not all(isinstance(warning, str) for warning in value["warnings"])
            or not isinstance(value.get("endpoints"), list) or len(value["endpoints"]) > 16):
        raise ValueError("PEER_CHECK_REPORT_INVALID: unexpected helper identity or schema")
    parsed = urlsplit(request["enode"])
    try:
        literal = ipaddress.ip_address(parsed.hostname)
    except ValueError:
        literal = None
    udp_port = int(parsed.query.partition("=")[2]) if parsed.query else parsed.port
    def valid_check(check):
        return (isinstance(check, dict) and check.get("state") in {"PASS", "FAIL", "SKIPPED"}
                and all(isinstance(check.get(key, ""), str) for key in ("reason", "detail")))
    if not all(valid_check(value.get(stage)) for stage in ("syntax", "dns")):
        raise ValueError("PEER_CHECK_REPORT_INVALID: missing format or DNS check")
    for endpoint in value["endpoints"]:
        if not isinstance(endpoint, dict) or not all(valid_check(endpoint.get(stage)) for stage in STAGES):
            raise ValueError("PEER_CHECK_REPORT_INVALID: missing endpoint checks")
        address = ipaddress.ip_address(endpoint.get("ip", ""))
        if (address.is_unspecified or address.is_multicast or address.is_link_local
                or (literal is not None and address != literal)
                or endpoint.get("family") != f"ipv{address.version}"
                or endpoint.get("tcp_port") != parsed.port or endpoint.get("udp_port") != udp_port):
            raise ValueError("PEER_CHECK_REPORT_INVALID: endpoint address/port mismatch")
        endpoint["usable"] = all(endpoint[stage]["state"] == "PASS" for stage in STAGES)
    ready = value["syntax"]["state"] == "PASS" and value["dns"]["state"] == "PASS"
    value["usable"] = ready and any(endpoint["usable"] for endpoint in value["endpoints"])
    value["state"] = "PASS" if value["usable"] else "PARTIAL" if any(
        endpoint[stage]["state"] == "PASS" for endpoint in value["endpoints"] for stage in ("tcp", "discovery")) else "FAIL"
    return value


def check(layout, enode: str, *, timeout_secs: int = 30) -> dict:
    """Run a bounded observation, without changing Seeds, services or node identity."""
    if type(timeout_secs) is not int or not 1 <= timeout_secs <= 120:
        raise ValueError("timeout must be an integer between 1 and 120 seconds")
    report = _blank(enode)
    try:
        endpoint = normalize_enode(enode)
    except ValueError as error:
        report.update(state="FAIL", syntax={"state": "FAIL", "reason": "INVALID_ENODE", "detail": str(error)},
                      guidance=["Obtain a complete enode://PUBLIC_KEY@HOST:PORT URL from the node operator."])
        return report
    report.update(enode=endpoint, syntax={"state": "PASS"})
    name = "usdb-peer-check-" + uuid.uuid4().hex
    try:
        request = _request(layout, endpoint, timeout_secs)
        report.update(network_id=request["network_id"], genesis_hash=request["genesis_hash"])
        scope, network = _scope(layout)
        report["scope"] = scope
        command = ["docker", "run", "--rm", "--interactive", "--pull=never", "--name", name,
                   "--network", network, "--read-only", "--user=65534:65534", "--cap-drop=ALL",
                   "--security-opt=no-new-privileges", "--pids-limit=64", "--memory=512m", "--cpus=1",
                   "--env=GOMAXPROCS=2",
                   "--entrypoint=geth", layout.images["USDB_CHAIN_IMAGE"], "usdb-peer-check"]
        try:
            result = subprocess.run(command, input=json.dumps(request), capture_output=True, text=True,
                                    timeout=timeout_secs + 15, check=False)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            # Killing the Docker client alone could leave its temporary probe running.
            subprocess.run(["docker", "rm", "--force", name], capture_output=True, text=True, timeout=10, check=False)
            raise
        if result.returncode:
            raise ValueError("PEER_CHECK_HELPER_FAILED: " + (result.stderr.strip()[-1000:] or "helper exited without a report"))
        value = _validate_report(json.loads(result.stdout), request)
        report.update(value, scope=scope)
        if scope["kind"] == "host":
            report["warnings"].append("Host network observation only; repeat with the chain running to verify its container network.")
        reasons = [report["dns"].get("reason")]
        reasons.extend(endpoint[stage].get("reason") for endpoint in report["endpoints"] for stage in STAGES)
        report["guidance"] = list(dict.fromkeys(GUIDANCE[reason] for reason in reasons if reason in GUIDANCE))
        if report["usable"]:
            report["next_command"] = "usdb-node peers add " + shlex.quote(endpoint)
        return report
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        report.update(state="INCOMPLETE", usable=False, reason="PEER_CHECK_UNAVAILABLE", detail=str(error),
                      guidance=["Check Docker access and the cached chain image. Both the node kit and chain image must include peers check support."])
        return report


def render(report: dict) -> str:
    """Present every layer separately and keep a partial probe visibly incomplete."""
    def clean(value):
        return "".join(c if c.isprintable() else " " for c in str(value))[:1200]
    def line(label, value):
        detail = ": ".join(clean(value[key]) for key in ("reason", "detail") if value.get(key))
        return f"  {label}: {value['state']}" + (" | " + detail if detail else "")
    lines = [f"Peer check | {report['state']} | {report['checked_at']}",
             f"Target: {clean(report['enode'])}", f"Probe network: {report['scope']['kind']}",
             line("Enode format", report["syntax"]), line("DNS", report["dns"])]
    if "network_id" in report:
        lines.insert(3, f"Expected network ID: {report['network_id']} | genesis: {report['genesis_hash']}")
    for endpoint in report["endpoints"]:
        lines.append(f"{endpoint['family']} {endpoint['ip']} | TCP/{endpoint['tcp_port']} UDP/{endpoint['udp_port']}")
        lines.extend(line(LABELS[stage], endpoint[stage]) for stage in STAGES)
    if report.get("detail"): lines.append(clean(report["detail"]))
    lines.extend("Warning: " + clean(value) for value in report["warnings"])
    lines.extend("Next: " + clean(value) for value in report["guidance"])
    if report.get("next_command"): lines.append("To add this Seed: " + report["next_command"])
    lines.append("One-time observation; no Seed was added. Success does not prove chain synchronization or mining eligibility.")
    return "\n".join(lines)
