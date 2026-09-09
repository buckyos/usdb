"""Select P2P address families and inspect advertised endpoints without exposing RPC."""
from __future__ import annotations

import argparse
import ipaddress
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

FAMILIES = ("auto", "ipv4", "ipv6", "dual")
KEYS = ("USDB_P2P_IP_FAMILY", "USDB_P2P_REQUESTED_FAMILY", "USDB_P2P_ADVERTISE_IPV4",
        "USDB_P2P_ADVERTISE_IPV6", "USDB_P2P_ADVERTISE_PORT", "USDB_P2P_ADVERTISE_DISCOVERY_PORT")
MIN_ENGINE = (28, 0, 0)
MIN_COMPOSE = (2, 33, 1)


def add_options(parser, *, setup=False):
    """Use the same transport choices in setup and subsequent peer configuration."""
    parser.add_argument("--p2p-ip-family" if setup else "--ip-family", dest="p2p_ip_family",
                        choices=FAMILIES, default="auto")
    parser.add_argument("--advertise-ipv4", default="", help="Externally reachable IPv4 address, including a router's mapped address")
    parser.add_argument("--advertise-ipv6", default="", help="Stable IPv6 address assigned to this host")
    parser.add_argument("--advertise-port", type=int, default=31303, help="External TCP port, after any router forwarding")
    parser.add_argument("--advertise-discovery-port", type=int, help="External UDP port; defaults to advertised TCP port")


def options(args):
    return {"requested": args.p2p_ip_family, "advertise_ipv4": args.advertise_ipv4,
            "advertise_ipv6": args.advertise_ipv6, "advertise_port": args.advertise_port,
            "discovery_port": args.advertise_discovery_port}


def command_json(arguments):
    result = subprocess.run(arguments, check=True, capture_output=True, text=True, timeout=10)
    return json.loads(result.stdout)


def version(value):
    match = re.match(r"v?(\d+)\.(\d+)\.(\d+)", value)
    if not match:
        raise ValueError(f"P2P_VERSION_UNAVAILABLE: {value}")
    return tuple(int(v) for v in match.groups())


def usable_ip(value, family):
    """Accept unicast host addresses, including explicitly configured private labs."""
    address = ipaddress.ip_address(value)
    if (address.version != family or address.is_unspecified or address.is_multicast
            or address.is_loopback or address.is_link_local or "%" in value
            or getattr(address, "ipv4_mapped", None)):
        raise ValueError(f"P2P_INVALID_ADDRESS: expected an IPv{family} unicast host address")
    return str(address)


def validate(env):
    """Validate only configuration; no host or Docker IO in bundle validation."""
    family = env.get("USDB_P2P_IP_FAMILY", "ipv4")
    if family not in FAMILIES[1:]:
        raise ValueError("USDB_P2P_IP_FAMILY must be ipv4, ipv6 or dual (auto is resolved during configuration)")
    if env.get("USDB_P2P_REQUESTED_FAMILY", family) not in FAMILIES:
        raise ValueError("invalid USDB_P2P_REQUESTED_FAMILY")
    for af in (4, 6):
        value = env.get(f"USDB_P2P_ADVERTISE_IPV{af}", "")
        if value:
            usable_ip(value, af)
            if family not in {f"ipv{af}", "dual"}:
                raise ValueError(f"P2P_FAMILY_CONFLICT: cannot advertise IPv{af} in {family} mode")
    if family in {"ipv6", "dual"} and not env.get("USDB_P2P_ADVERTISE_IPV6"):
        raise ValueError("P2P_IPV6_HOST_REQUIRED: configure a stable IPv6 address with peers configure")
    for key in KEYS[-2:]:
        value = env.get(key, "31303")
        if not re.fullmatch(r"[0-9]+", value) or not 1 <= int(value) <= 65535:
            raise ValueError(f"{key} must be a port between 1 and 65535")


def host_capabilities():
    """Read assigned addresses and routes; never guess a NAT address via a third party."""
    report = {"ipv4": [], "ipv6": [], "ipv6_default_route": False, "ipv6_ra_interfaces": [], "errors": []}
    try:
        interfaces = command_json(["ip", "-j", "address", "show", "up"])
        for interface in interfaces:
            if interface.get("link_type") == "loopback" or interface.get("ifname", "").startswith(("docker", "br-", "veth")):
                continue
            for entry in interface.get("addr_info", []):
                af = {"inet": 4, "inet6": 6}.get(entry.get("family"))
                flags = entry.get("flags", [])
                if af is None or entry.get("scope") != "global" or any(
                        name in flags or entry.get(name) for name in ("temporary", "tentative", "dadfailed", "deprecated")):
                    continue
                if entry.get("preferred_life_time") == 0:
                    continue
                address = usable_ip(entry["local"], af)
                report[f"ipv{af}"].append(address)
        routes = command_json(["ip", "-j", "-6", "route", "show", "default"])
        report["ipv6_default_route"] = any(route.get("dev") and route.get("type", "unicast") == "unicast"
            and "linkdown" not in route.get("flags", []) for route in routes)
        for name in sorted({route["dev"] for route in routes if route.get("dev") and route.get("protocol") == "ra"}):
            accept_ra = Path(f"/proc/sys/net/ipv6/conf/{name}/accept_ra").read_text().strip()
            report["ipv6_ra_interfaces"].append({"interface": name, "accept_ra": accept_ra})
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report["ipv6_default_route"] = False
        report["errors"].append(f"host address/route inspection failed: {error}")
    for af in (4, 6):
        report[f"ipv{af}"] = sorted(set(report[f"ipv{af}"]))
    return report


def check_router_advertisements(host):
    """Docker enables forwarding; RA-derived default routes must survive that change."""
    for interface in host.get("ipv6_ra_interfaces", []):
        if interface["accept_ra"] != "2":
            raise ValueError(f"P2P_IPV6_RA_REQUIRED: set net.ipv6.conf.{interface['interface']}.accept_ra=2 "
                             "before enabling Docker IPv6 forwarding, or the RA default route will expire")


def engine_capabilities():
    """Require a Docker generation supporting NAT66 and per-network gateway priority."""
    server = command_json(["docker", "version", "--format", "{{json .Server}}"])
    compose = subprocess.run(["docker", "compose", "version", "--short"], check=True,
                             capture_output=True, text=True, timeout=10).stdout.strip()
    info = command_json(["docker", "info", "--format", "{{json .}}"])
    if version(server["Version"]) < MIN_ENGINE or version(compose) < MIN_COMPOSE:
        raise ValueError("P2P_IPV6_ENGINE_REQUIRED: IPv6 needs Docker Engine >= 28.0.0 and Compose >= 2.33.1")
    if info.get("OSType") != "linux" or any("rootless" in entry for entry in info.get("SecurityOptions", [])):
        raise ValueError("P2P_IPV6_ENGINE_REQUIRED: this profile requires rootful Linux Docker bridge networking")
    return {"engine": server["Version"], "compose": compose}


def select(requested, *, advertise_ipv4="", advertise_ipv6="", advertise_port=31303,
           discovery_port=None, host=None):
    """Resolve auto once and persist the decision; explicit IPv6 never falls back."""
    if requested not in FAMILIES:
        raise ValueError("invalid P2P address family")
    host = host if host is not None else host_capabilities()
    ipv4 = usable_ip(advertise_ipv4, 4) if advertise_ipv4 else next(
        (value for value in host["ipv4"] if ipaddress.ip_address(value).is_global), "")
    ipv6 = usable_ip(advertise_ipv6, 6) if advertise_ipv6 else next(
        (value for value in host["ipv6"] if ipaddress.ip_address(value).is_global), "")
    reason = "explicit address family"
    family = requested
    if requested == "auto":
        family = "dual" if ipv6 and host["ipv6_default_route"] else "ipv4"
        reason = "stable IPv6 address and default route found" if family == "dual" else "no usable IPv6 address/default route"
    if family in {"ipv6", "dual"}:
        try:
            if not ipv6 or ipv6 not in host["ipv6"] or not host["ipv6_default_route"]:
                raise ValueError("P2P_IPV6_HOST_REQUIRED: need an assigned stable IPv6 address and default route")
            check_router_advertisements(host)
            engine_capabilities()
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            if requested != "auto" or advertise_ipv6:
                raise
            family, reason = "ipv4", f"IPv6 unavailable: {error}"
    if family == "ipv4":
        if advertise_ipv6:
            raise ValueError("P2P_FAMILY_CONFLICT: IPv6 advertisement requires ipv6 or dual")
        ipv6 = ""
    if family == "ipv6":
        if advertise_ipv4:
            raise ValueError("P2P_FAMILY_CONFLICT: IPv4 advertisement requires ipv4 or dual")
        ipv4 = ""
    updates = {"USDB_P2P_REQUESTED_FAMILY": requested, "USDB_P2P_IP_FAMILY": family,
               "USDB_P2P_ADVERTISE_IPV4": ipv4, "USDB_P2P_ADVERTISE_IPV6": ipv6,
               "USDB_P2P_ADVERTISE_PORT": str(advertise_port),
               "USDB_P2P_ADVERTISE_DISCOVERY_PORT": str(discovery_port if discovery_port is not None else advertise_port)}
    validate(updates)
    return updates, reason


def check_host(env):
    """Recheck explicit transport requirements before creating/recreating chain."""
    validate(env)
    family = env.get("USDB_P2P_IP_FAMILY", "ipv4")
    if family == "ipv4":
        return
    host = host_capabilities()
    if not host["ipv6_default_route"] or env.get("USDB_P2P_ADVERTISE_IPV6") not in host["ipv6"]:
        raise ValueError("P2P_IPV6_HOST_CHANGED: configured IPv6 address/route is unavailable; rerun peers configure")
    check_router_advertisements(host)
    engine_capabilities()


def container_view(layout):
    """Inspect actual Docker network and published transport state, never infer it from env."""
    import usdb_mining as mining
    runtime = mining.inspect_chain(layout, processes=False)
    if not runtime.get("id"):
        return {"state": "not_started", "ports": {}, "networks": {}}
    item, = command_json(["docker", "inspect", runtime["id"]])
    settings = item.get("NetworkSettings", {})
    return {"state": runtime["state"], "ports": settings.get("Ports") or {},
            "networks": {name: {key: entry.get(key) for key in ("IPAddress", "GlobalIPv6Address", "IPv6Gateway")}
                         for name, entry in settings.get("Networks", {}).items()}}


def transport_ready(env, runtime):
    """Check both TCP and UDP publishing, plus the container IPv6 route endpoint."""
    family = env.get("USDB_P2P_IP_FAMILY", "ipv4")
    required = {"ipv4": {"0.0.0.0"}, "ipv6": {"::"}, "dual": {"0.0.0.0", "::"}}[family]
    if runtime.get("state") != "running":
        return False
    for protocol in ("tcp", "udp"):
        bindings = runtime["ports"].get(f"31303/{protocol}") or []
        actual = {binding.get("HostIp") for binding in bindings if binding.get("HostPort") == env.get("USDB_P2P_BIND_PORT", "31303")}
        if actual != required:
            return False
    if family != "ipv4" and not any(entry.get("GlobalIPv6Address") and entry.get("IPv6Gateway")
                                    for entry in runtime["networks"].values()):
        return False
    return True


def endpoint_report(layout):
    """Return shareable address candidates with runtime evidence and explicit reachability limits."""
    import usdb_node as node
    import usdb_mining as mining
    import usdb_peers as peers
    env = node.read_env(layout.node_env)
    validate(env)
    host = host_capabilities()
    report = {"family": env.get("USDB_P2P_IP_FAMILY", "ipv4"), "host": host, "endpoints": [],
              "reachability": "unverified", "state": "WAITING"}
    operation = peers.read_state(layout)
    pending_transport = operation.get("transport_updates") and operation.get("phase") != "APPLIED"
    if pending_transport:
        report["desired_family"] = operation["transport_updates"]["USDB_P2P_IP_FAMILY"]
        report["operation"] = {key: operation[key] for key in ("operation_id", "phase", "error") if key in operation}
    report["guidance"] = "Verify these candidates from another host; local port configuration does not prove public TCP/UDP reachability."
    try:
        check_host(env)
        runtime = container_view(layout)
        report["container"] = runtime
        report["state"] = "CONFIGURED" if transport_ready(env, runtime) else "WAITING"
        if pending_transport:
            report.update(state="BLOCKED" if operation.get("error") else "WAITING",
                          error=operation.get("error", f"P2P_OPERATION_PENDING: phase={operation['phase']}; observe peers status"))
            return report
        chain = mining.chain_view(layout)
        raw = chain.get("enode", "")
        key = peers.normalize_enode(raw).split("@")[0]
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        report["state"] = "BLOCKED" if str(error).startswith(("P2P_", "CHAIN_IDENTITY_MISMATCH:")) else "WAITING"
        return report
    port = env.get("USDB_P2P_ADVERTISE_PORT", "31303")
    udp = env.get("USDB_P2P_ADVERTISE_DISCOVERY_PORT", port)
    for af in (4, 6):
        if report["family"] not in {f"ipv{af}", "dual"}:
            continue
        address = env.get(f"USDB_P2P_ADVERTISE_IPV{af}", "")
        source = "configured"
        if not address and af == 4 and env.get("USDB_NAT", "").startswith("extip:"):
            try:
                address = usable_ip(env["USDB_NAT"][6:], 4)
                source = "nat"
            except ValueError:
                pass
        if not address:
            values = [value for value in host[f"ipv{af}"] if ipaddress.ip_address(value).is_global]
            if len(values) == 1:
                address, source = values[0], "host"
        if not address and af == 4:
            advertised = ipaddress.ip_address(urlsplit(raw).hostname)
            if advertised.version == 4 and advertised.is_global:
                address, source = str(advertised), "runtime"
        if not address:
            continue
        if af == 6 and address not in host["ipv6"]:
            report.setdefault("warnings", []).append("configured IPv6 address is no longer assigned")
            continue
        target = f"[{address}]" if af == 6 else address
        url = f"{key}@{target}:{port}" + (f"?discport={udp}" if udp != port else "")
        report["endpoints"].append({"family": f"ipv{af}", "enode": peers.normalize_enode(url),
                                    "source": source, "reachability": "unverified"})
    return report


def main():
    """Internal runtime helper: checks are read-only and stdout contains no secrets."""
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("check",))
    parser.add_argument("--node-env", type=Path, required=True)
    args = parser.parse_args()
    try:
        # Keep this module importable by the bundle validator without a cycle.
        from validate_network_bundle import read_env
        check_host(read_env(args.node_env))
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"P2P preflight failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
