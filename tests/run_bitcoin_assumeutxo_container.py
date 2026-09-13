#!/usr/bin/env python3
"""Accept the built Core image and native entrypoint in isolated, network-disabled containers."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, help="Already-built local Core image")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = Path(tempfile.mkdtemp(prefix="usdb-p72-container-"))
    root.chmod(0o755)
    name = "usdb-p72-test-" + uuid.uuid4().hex[:12]
    report = dict(status="running", run=str(root), scope="Isolated Core mainnet genesis without peers; no mainnet snapshot import")
    print(f"P7.2 container acceptance: {root}", flush=True)

    def docker(*command, check=True):
        result = subprocess.run(["docker", *map(str, command)], text=True, capture_output=True, timeout=90)
        if check and result.returncode:
            raise RuntimeError(f"Docker acceptance command failed: {command[0]}: {result.stderr}")
        return result

    secret = root / "rpcauth"
    secret.write_text("test:" + "a" * 32 + "$" + "b" * 64)
    secret.chmod(0o644)
    fake = root / "fake-bitcoind"
    fake.write_text("#!/usr/bin/env python3\nimport json, sys\nprint(json.dumps([a for a in sys.argv[1:] if not a.startswith('-rpcauth=')]))\n")
    fake.chmod(0o755)
    base = ["--rm", "--network", "none", "--mount", f"type=bind,src={secret},dst=/run/secrets/bitcoin-rpcauth,readonly"]
    started = False
    try:
        report["image_id"] = docker("image", "inspect", args.image, "--format", "{{.Id}}").stdout.strip()
        version = docker("run", "--rm", "--network", "none", "--entrypoint", "/opt/bitcoin/bin/bitcoind", args.image, "--version").stdout
        assert "v31.1.0" in version, version
        report["version"] = version.splitlines()[0]
        compose_env = dict(os.environ, USDB_BITCOIN_IMAGE=args.image, BTC_NETWORK="bitcoin", BTC_TXINDEX="0",
                           BTC_RPC_USER="test", BTC_RPC_PASSWORD="fake-test-only", BTC_MIN_READY_HEIGHT="963800",
                           BTC_MAX_TIP_AGE_SECS="7200", BTC_MIN_CONNECTIONS="1", BTC_NODE_DATA_HOST_DIR=str(root / "bitcoin"),
                           BTC_RPCAUTH_HOST_FILE=str(secret), USDB_DOCKER_NETWORK="p72-render-only",
                           BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR=str(root / "artifacts"), BTC_ASSUMEUTXO_STATE_HOST_DIR=str(root / "bootstrap"))
        rendered = subprocess.run(["docker", "compose", "-f", str(repo / "docker/compose.bitcoin.yml"), "-f",
                                   str(repo / "docker/compose.bitcoin-assumeutxo.yml"), "config", "--format", "json"],
                                  env=compose_env, capture_output=True, text=True, check=True, timeout=15)
        services = json.loads(rendered.stdout)["services"]
        core, loader = services["btc-node"], services["btc-snapshot-bootstrap"]
        assert core["environment"]["SNAPSHOT_MODE"] == "assumeutxo" and core["environment"]["BTC_TXINDEX"] == "0"
        assert any(v["target"] == "/data/assumeutxo" and v["read_only"] for v in core["volumes"])
        assert all(v["target"] != "/data/bitcoin" for v in loader["volumes"])
        assert all(int(port["target"]) != 8332 for port in core.get("ports", []))
        assert loader["depends_on"]["btc-node"]["condition"] == "service_started"
        assert loader["restart"] == "no" and int(loader["mem_limit"]) == 128 * 1024 * 1024
        assert "status" in core["healthcheck"]["test"]
        report["compose_overlay"] = "pass"
        tool = "docker/scripts/tools/bitcoin_assumeutxo.py"
        digest = hashlib.sha256((repo / tool).read_bytes()).hexdigest()
        actual = docker("run", "--rm", "--network", "none", "--entrypoint", "sha256sum", args.image, f"/opt/usdb/{tool}").stdout.split()[0]
        assert digest == actual, "Image does not contain the current bootstrap tool"
        report["bootstrap_tool_sha256"] = digest
        invocations = []
        for mode, txindex in (("none", "1"), ("assumeutxo", "0")):
            result = docker("run", *base, "--mount", f"type=bind,src={fake},dst=/opt/bitcoin/bin/bitcoind,readonly",
                            "-e", f"SNAPSHOT_MODE={mode}", args.image)
            options = json.loads(result.stdout.splitlines()[-1])
            assert f"-txindex={txindex}" in options and "-prune=0" in options
            invocations.append(dict(mode=mode, txindex=txindex, prune=0))
        for value in ("-prune=550", "-notxindex=0", "-regtest", "-conf=/tmp/other.conf"):
            rejected = docker("run", *base, "--mount", f"type=bind,src={fake},dst=/opt/bitcoin/bin/bitcoind,readonly",
                              "-e", "SNAPSHOT_MODE=assumeutxo", "-e", f"BTC_EXTRA_ARGS={value}", args.image, check=False)
            assert rejected.returncode != 0 and "cannot override" in rejected.stderr
        report["entrypoints"] = invocations
        docker("run", "-d", "--name", name, "--network", "none", "--memory", "512m", "--memory-swap", "512m",
               "--mount", f"type=bind,src={secret},dst=/run/secrets/bitcoin-rpcauth,readonly",
               "-e", "SNAPSHOT_MODE=assumeutxo", "-e", "BTC_DBCACHE_MB=64",
               "-e", "BTC_COOKIE_FILE=/data/bitcoin/.cookie",
               "-e", "BTC_EXTRA_ARGS=-connect=0 -dnsseed=0 -listen=0 -discover=0", args.image)
        started = True
        deadline = time.monotonic() + 30
        while True:
            info = docker("exec", name, "bitcoin-cli", "-datadir=/data/bitcoin", "getblockchaininfo", check=False)
            if info.returncode == 0:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("Isolated Core RPC did not start")
            time.sleep(0.2)
        info = json.loads(info.stdout)
        assert info["blocks"] == 0 and info["chain"] == "main" and not info["pruned"]
        indexes = json.loads(docker("exec", name, "bitcoin-cli", "-datadir=/data/bitcoin", "getindexinfo").stdout)
        assert indexes == {}, indexes
        status = docker("exec", name, "python3", "/opt/usdb/docker/scripts/tools/bitcoin_assumeutxo.py", "status", check=False)
        native = json.loads(status.stdout)
        assert status.returncode == 1 and not native["bootstrap_ready"] and native["rpc_available"]
        assert not native["history_validated"] and native["active_height"] == 0
        report["genesis_status"] = native
        docker("stop", "--time", "60", name)
        assert docker("inspect", name, "--format", "{{.State.ExitCode}}").stdout.strip() == "0"
        report["status"] = "pass"
    finally:
        if started:
            (root / "core.log").write_text(docker("logs", name, check=False).stdout)
            docker("stop", "--time", "60", name, check=False)
            docker("rm", "--volumes", name, check=False)
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
