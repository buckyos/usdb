#!/usr/bin/env python3
"""Opt-in acceptance against real Docker port publication and discovery/RLPx."""
import argparse
import ipaddress
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_p2p as P2P
from common.docker_p2p import DockerP2PFixture, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe-binary", type=Path, required=True, help="CGO_ENABLED=0 build of go-ethereum/tests/common/p2pprobe")
    parser.add_argument("--image", default="alpine:3.20", help="Locally available image with CA certificates; never pulled")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--local-only", action="store_true", help="Test Docker paths only; explicitly skip host startup qualification/public egress")
    args = parser.parse_args()
    binary = args.probe_binary.resolve(strict=True)
    output = args.output_dir or Path(tempfile.mkdtemp(prefix="usdb-p2p-docker-"))
    output.mkdir(parents=True, exist_ok=True)
    output = output.resolve()
    report = {"state": "RUNNING", "engine": P2P.engine_capabilities(), "cases": []}
    host = P2P.host_capabilities()
    assert host["ipv4"], f"host IPv4 inspection failed: {host}"
    if not args.local_only:
        assert host["ipv6_default_route"], f"host IPv6 default route is unavailable: {host}"
    ipv4 = host["ipv4"][0]
    ipv6 = next(value for value in host["ipv6"] if ipaddress.ip_address(value).is_global)
    image, = json.loads(run(["docker", "image", "inspect", args.image]))
    report["image"] = image["Id"]
    report["addresses"] = {"ipv4": ipv4, "ipv6": ipv6}
    report["auto"] = P2P.select("auto")[0]["USDB_P2P_IP_FAMILY"]
    report["host_startup_qualification"] = "skipped" if args.local_only else "required"
    if not args.local_only:
        assert report["auto"] == "dual", P2P.select("auto")[1]
    before = set(run(["docker", "ps", "-aq"]).split())
    before_networks = set(run(["docker", "network", "ls", "-q", "--no-trunc"]).split())
    first_key = None
    try:
        with DockerP2PFixture(ROOT, binary, image["Id"], output, ipv4) as fixture:
            report["project"] = fixture.project
            for family in ("ipv4", "dual", "ipv6", "ipv4"):
                print(f"Testing production Compose family={family}", flush=True)
                item, info = fixture.start(family)
                settings = item["NetworkSettings"]
                runtime = {"state": item["State"]["Status"], "ports": settings["Ports"], "networks": settings["Networks"]}
                assert P2P.transport_ready({"USDB_P2P_IP_FAMILY": family}, runtime), runtime
                assert all(binding["HostIp"] == "127.0.0.1" for port in ("8545/tcp", "8546/tcp")
                           for binding in settings["Ports"][port])
                key = info["node"]["enode"].split("@")[0]
                first_key = first_key or key
                assert first_key == key, "node identity changed during profile recreation"
                case = {"family": family, "state": "RUNNING", "node_id": info["node"]["id"], "ports": settings["Ports"],
                        "networks": settings["Networks"], "probes": []}
                report["cases"].append(case)
                for af, address in (("ipv4", ipv4), ("ipv6", ipv6)):
                    if family not in {af, "dual"}:
                        sock_family = socket.AF_INET if af == "ipv4" else socket.AF_INET6
                        with socket.socket(sock_family) as sock:
                            sock.settimeout(1)
                            assert sock.connect_ex((address, 31303)) != 0, f"unexpected {af} TCP exposure"
                        continue
                    target = f"{key}@{'[' + address + ']' if af == 'ipv6' else address}:31303"
                    result = fixture.probe("-mode", "probe", "-target", target)
                    assert result["udp_ping_verified"] and result["node_id"] == info["node"]["id"]
                    case["probes"].append(result)
                # Confirm Docker DNS and IPv4 data-service connectivity remain intact.
                case["data_service"] = json.loads(fixture.docker("exec", item["Id"], "/probe", "-mode", "http",
                                                                 "-target", "http://data-peer:8545"))
                if family == "dual" and not args.local_only:
                    case["outbound"] = {}
                    for af in ("4", "6"):
                        target = "https://www.debian.org/"
                        host_probe = subprocess.run([str(binary), "-mode", "http", "-target", target, "-family", af],
                                                    capture_output=True, text=True, timeout=15)
                        if host_probe.returncode:
                            case["outbound"][af] = {"state": "HOST_UNAVAILABLE", "error": host_probe.stderr.strip()}
                            continue
                        case["outbound"][af] = json.loads(fixture.docker("exec", item["Id"], "/probe", "-mode", "http",
                                                                                       "-target", target, "-family", af))
                case["state"] = "PASS"
                print(f"PASS family={family}; discovery/RLPx, data-service link and node identity", flush=True)
        assert set(run(["docker", "ps", "-aq"]).split()) == before, "container inventory changed"
        assert set(run(["docker", "network", "ls", "-q", "--no-trunc"]).split()) == before_networks, "network inventory changed"
        report["state"] = "PASS_LOCAL" if args.local_only else "PASS"
        if any(item.get("state") == "HOST_UNAVAILABLE" for case in report["cases"] for item in case.get("outbound", {}).values()):
            report["state"] = "PARTIAL"
    except BaseException as error:
        report.update(state="FAILED", error=str(error))
        raise
    finally:
        path = output / "report.json"
        path.write_text(json.dumps(report, indent=2) + "\n")
        print(f"Acceptance report: {path}", flush=True)


if __name__ == "__main__":
    main()
