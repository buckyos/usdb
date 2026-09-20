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
    parser.add_argument("--advertise-ipv6", default="auto",
                        help="auto follows current non-temporary global IPv6 addresses (default); an explicit IP pins the address")
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
            if not (af == 6 and value == "auto"):
                usable_ip(value, af)
            if family not in {f"ipv{af}", "dual"}:
                raise ValueError(f"P2P_FAMILY_CONFLICT: cannot advertise IPv{af} in {family} mode")
    if family in {"ipv6", "dual"} and not env.get("USDB_P2P_ADVERTISE_IPV6"):
        raise ValueError("P2P_IPV6_HOST_REQUIRED: configure auto or a fixed IPv6 address with peers configure")
    for key in KEYS[-2:]:
        value = env.get(key, "31303")
        if not re.fullmatch(r"[0-9]+", value) or not 1 <= int(value) <= 65535:
            raise ValueError(f"{key} must be a port between 1 and 65535")


def host_capabilities():
    """Read assigned addresses and routes; never guess a NAT address via a third party."""
    report = {"ipv4": [], "ipv6": [], "ipv6_default_route": False, "ipv6_ra_interfaces": [], "errors": []}
    try:
        interfaces = command_json(["ip", "-j", "address", "show", "up"])
        ipv6_by_interface = {}
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
                if af == 6:
                    ipv6_by_interface.setdefault(interface.get("ifname"), []).append(address)
        routes = command_json(["ip", "-j", "-6", "route", "show", "default"])
        usable_routes = sorted((route for route in routes if route.get("dev")
            and route.get("type", "unicast") == "unicast" and "linkdown" not in route.get("flags", [])),
            key=lambda route: route.get("metric", 0))
        report["ipv6_default_route"] = bool(usable_routes)
        report["ipv6_uplink_addresses"] = list(dict.fromkeys(address for route in usable_routes
            for address in sorted(ipv6_by_interface.get(route["dev"], []))))
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


def automatic_ipv6(host):
    """Select a usable public address on the default uplink when interface data is available."""
    candidates = host["ipv6"]
    if "ipv6_uplink_addresses" in host:
        candidates = host["ipv6_uplink_addresses"]
    return next((value for value in candidates if ipaddress.ip_address(value).is_global), "")


def select(requested, *, advertise_ipv4="", advertise_ipv6="auto", advertise_port=31303,
           discovery_port=None, host=None):
    """Persist the address family and selection policy, not a transient automatic IPv6."""
    if requested not in FAMILIES:
        raise ValueError("invalid P2P address family")
    host = host if host is not None else host_capabilities()
    ipv4 = usable_ip(advertise_ipv4, 4) if advertise_ipv4 else next(
        (value for value in host["ipv4"] if ipaddress.ip_address(value).is_global), "")
    pinned_ipv6 = advertise_ipv6 not in {"", "auto"}
    ipv6 = usable_ip(advertise_ipv6, 6) if pinned_ipv6 else automatic_ipv6(host)
    reason = "explicit address family"
    family = requested
    if requested == "auto":
        family = "dual" if ipv6 and host["ipv6_default_route"] else "ipv4"
        reason = "stable IPv6 address and default route found"
        if host.get("errors"):
            family, reason = "ipv4", "host inspection failed: " + "; ".join(host["errors"])
        elif not ipv6:
            reason = "no usable stable IPv6 address"
        elif not host["ipv6_default_route"]:
            reason = "no IPv6 default route"
    if family in {"ipv6", "dual"}:
        try:
            if not ipv6 or ipv6 not in host["ipv6"] or not host["ipv6_default_route"]:
                raise ValueError("P2P_IPV6_HOST_REQUIRED: need an assigned stable IPv6 address and default route")
            check_router_advertisements(host)
            engine_capabilities()
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            if requested != "auto" or pinned_ipv6:
                raise
            family, reason = "ipv4", f"IPv6 unavailable: {error}"
    if family == "ipv4":
        if pinned_ipv6:
            raise ValueError("P2P_FAMILY_CONFLICT: IPv6 advertisement requires ipv6 or dual")
        ipv6 = ""
    if family == "ipv6":
        if advertise_ipv4:
            raise ValueError("P2P_FAMILY_CONFLICT: IPv4 advertisement requires ipv4 or dual")
        ipv4 = ""
    updates = {"USDB_P2P_REQUESTED_FAMILY": requested, "USDB_P2P_IP_FAMILY": family,
               "USDB_P2P_ADVERTISE_IPV4": ipv4,
               "USDB_P2P_ADVERTISE_IPV6": ("auto" if ipv6 and not pinned_ipv6 else ipv6),
               "USDB_P2P_ADVERTISE_PORT": str(advertise_port),
               "USDB_P2P_ADVERTISE_DISCOVERY_PORT": str(discovery_port if discovery_port is not None else advertise_port)}
    validate(updates)
    return updates, reason


def diagnostic_guidance(reason):
    """Explain host repairs without changing sysctls, routes or firewall policy."""
    lines = ["Check: ip -6 address show scope global", "Check: ip -6 route show default"]
    ra = re.search(r"net\.ipv6\.conf\.([A-Za-z0-9_.:-]+)\.accept_ra=2", reason)
    if ra:
        key = f"net.ipv6.conf.{ra[1]}.accept_ra"
        lines += [f"Check: sysctl {key}", f"Administrator action: sudo sysctl -w {key}=2",
                  "Persist this setting for the actual uplink in /etc/sysctl.d/ or its network manager; recheck the IPv6 address and default route."]
    if "Docker" in reason or "docker" in reason or "P2P_IPV6_ENGINE_REQUIRED" in reason:
        lines.append("Check: docker version; docker compose version; docker info (rootful Linux, Engine >= 28.0.0, Compose >= 2.33.1).")
    lines.append("Handbook: https://github.com/buckyos/usdb/blob/master/doc/handbook/node/peers.md")
    return "\n".join(lines)


def selection_report(updates, reason, *, bootnodes=""):
    """Render the resolved transport and actionable warnings before configuration consent."""
    family, requested = updates["USDB_P2P_IP_FAMILY"], updates["USDB_P2P_REQUESTED_FAMILY"]
    lines = [f"USDB P2P: public TCP/UDP 31303; family={family} (requested={requested})",
             f"  Selection: {reason}"]
    for af in (4, 6):
        address = updates[f"USDB_P2P_ADVERTISE_IPV{af}"]
        if address:
            if af == 6 and address == "auto":
                current = automatic_ipv6(host_capabilities()) or "unavailable; recheck before startup"
                address = f"{current} (auto; follows address changes without recreating containers)"
            lines.append(f"  Advertised IPv{af}: {address}; TCP {updates['USDB_P2P_ADVERTISE_PORT']}, UDP {updates['USDB_P2P_ADVERTISE_DISCOVERY_PORT']}")
    if requested == "auto" and family == "ipv4":
        lines += ["WARNING P2P_IPV4_FALLBACK: automatic selection will use IPv4 only; IPv6 peers require fixing the checks above.",
                  diagnostic_guidance(reason)]
    seed_hosts = [urlsplit(seed.strip()).hostname or "" for seed in bootnodes.split(",") if seed.strip()]
    if family == "ipv4" and any(":" in host for host in seed_hosts):
        lines.append("WARNING P2P_IPV6_SEED_UNREACHABLE: the configured IPv6 seed endpoints cannot be reached by an IPv4-only chain container; fix IPv6 and select dual/ipv6, or obtain a reachable IPv4 seed.")
    lines += ["  Address family is saved once; it will not automatically change when IPv6 becomes available.",
              "  Local checks do not verify public TCP/UDP reachability or peer connections."]
    return "\n".join(lines)


def host_preflight_report():
    """Preview automatic P2P selection separately from mandatory package readiness."""
    updates, reason = select("auto")
    return "\n".join([
        "P2P host check (read-only preview; existing node configuration is unchanged):",
        selection_report(updates, reason),
        "New node requiring IPv6: usdb-node setup --p2p-ip-family dual --advertise-ipv6 auto",
        "Existing node after host repair: usdb-node peers configure --ip-family dual --advertise-ipv6 auto",
    ])


def check_host(env, *, host=None):
    """Recheck transport requirements and resolve auto without rewriting operator settings."""
    validate(env)
    family = env.get("USDB_P2P_IP_FAMILY", "ipv4")
    if family == "ipv4":
        return
    host = host if host is not None else host_capabilities()
    configured = env.get("USDB_P2P_ADVERTISE_IPV6")
    address = automatic_ipv6(host) if configured == "auto" else configured
    if not host["ipv6_default_route"] or address not in host["ipv6"]:
        if configured == "auto":
            raise ValueError("P2P_IPV6_HOST_REQUIRED: automatic IPv6 needs a usable public address and default route; "
                             "check ip -6 address / ip -6 route; no fixed address needs updating")
        raise ValueError("P2P_IPV6_HOST_CHANGED: pinned IPv6 address/route is unavailable; restore it or use "
                         "peers configure --ip-family " + family + " --advertise-ipv6 auto")
    check_router_advertisements(host)
    engine_capabilities()
    return address


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


def endpoint_report(layout, *, chain=None):
    """Inspect endpoint candidates, optionally reusing a network-validated chain observation."""
    import usdb_node as node
    import usdb_mining as mining
    import usdb_peers as peers
    env = node.read_env(layout.node_env)
    validate(env)
    host = host_capabilities()
    report = {"family": env.get("USDB_P2P_IP_FAMILY", "ipv4"), "host": host, "endpoints": [],
              "reachability": "unverified", "state": "WAITING"}
    report["ipv6_address_mode"] = "auto" if env.get("USDB_P2P_ADVERTISE_IPV6") == "auto" else "fixed"
    operation = peers.read_state(layout)
    pending_transport = operation.get("transport_updates") and operation.get("phase") != "APPLIED"
    if pending_transport:
        report["desired_family"] = operation["transport_updates"]["USDB_P2P_IP_FAMILY"]
        report["operation"] = {key: operation[key] for key in ("operation_id", "phase", "error") if key in operation}
    report["guidance"] = "Verify these candidates from another host; local port configuration does not prove public TCP/UDP reachability."
    try:
        resolved_ipv6 = check_host(env, host=host)
        report["resolved_ipv6"] = resolved_ipv6
        runtime = container_view(layout)
        report["container"] = runtime
        report["state"] = "CONFIGURED" if transport_ready(env, runtime) else "WAITING"
        if pending_transport:
            report.update(state="BLOCKED" if operation.get("error") else "WAITING",
                          error=operation.get("error", f"P2P_OPERATION_PENDING: phase={operation['phase']}; observe peers status"))
            return report
        if chain is None:
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
        if af == 6 and address == "auto":
            address, source = resolved_ipv6, "host-auto"
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
