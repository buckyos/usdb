"""Isolated Docker fixture using the production Compose P2P overlays."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.request
import uuid


def run(command, **kwargs):
    result = subprocess.run(command, capture_output=True, text=True, timeout=45, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{command[0:3]} failed ({result.returncode}): {result.stderr.strip()}")
    return result.stdout


def available_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class DockerP2PFixture:
    """Own only uniquely named test containers/networks and temporary bind mounts."""
    def __init__(self, root, binary, image, output, ipv4):
        self.root, self.binary, self.image, self.output = root, binary, image, output
        self.project = "usdb-p2p-acceptance-" + uuid.uuid4().hex[:10]
        self.shared = self.project + "-data"
        self.remote = self.project + "-remote"
        self.created_networks, self.containers = [], []
        self.http_port, self.ws_port = available_port(), available_port()
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("USDB_", "BH_", "BTC_", "CONTROL_PLANE_", "COMPOSE_"))}
        self.env.update(USDB_DOCKER_NETWORK=self.shared, USDB_CHAIN_IMAGE=image, USDB_SERVICES_IMAGE=image,
                        USDB_NETWORK_ARTIFACTS_DIR=str(output), BH_SNAPSHOT_TRUST_HOST_DIR=str(output),
                        USDB_P2P_BIND_PORT="31303", USDB_HTTP_BIND_PORT=str(self.http_port),
                        USDB_WS_BIND_PORT=str(self.ws_port))
        self.state = output / "state"
        self.state.mkdir()
        self.override = output / "fixture.yml"
        # Override process/data only. Ports and networks remain the production contract.
        self.override.write_text(f"""services:
  usdb-chain:
    user: "{os.getuid()}:{os.getgid()}"
    entrypoint: ["/probe", "-mode", "serve", "-advertise", {json.dumps(ipv4)}]
    volumes: !override
      - {json.dumps(str(binary) + ':/probe:ro')}
      - {json.dumps(str(self.state) + ':/state')}
    depends_on: !reset {{}}
    restart: "no"
    read_only: true
    cap_drop: [ALL]
    mem_limit: 128m
    memswap_limit: 128m
    cpus: 0.5
    stop_grace_period: 2s
    healthcheck:
      disable: true
    labels:
      org.usdb.p2p-acceptance: {self.project}
""")
        self.family = "ipv4"

    def docker(self, *args):
        return run(["docker", *args])

    def compose(self, *args):
        files = [self.root / "docker/compose.runtime.yml",
                 self.root / "docker/networks/testnet-v0/compose.network.yml"]
        if self.family != "ipv4":
            files.append(self.root / "docker/compose.p2p-dual.yml")
        if self.family == "ipv6":
            files.append(self.root / "docker/compose.p2p-ipv6.yml")
        files.append(self.override)
        command = ["docker", "compose", "--project-name", self.project,
                   "--env-file", str(self.root / "docker/networks/testnet-v0/network.env"),
                   "--env-file", str(self.root / "docker/networks/testnet-v0/node.env.example")]
        for path in files:
            command.extend(["-f", str(path)])
        return run([*command, *args], env={**self.env, "USDB_P2P_IP_FAMILY": self.family})

    def __enter__(self):
        # Fail before creating anything if the production P2P port is occupied.
        with ExitStack() as stack:
            for family in (socket.AF_INET, socket.AF_INET6):
                for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
                    sock = stack.enter_context(socket.socket(family, kind))
                    if family == socket.AF_INET6:
                        sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
                    sock.bind(("::" if family == socket.AF_INET6 else "0.0.0.0", 31303))
        try:
            self.docker("network", "create", "--label", f"org.usdb.p2p-acceptance={self.project}", self.shared)
            self.created_networks.append(self.shared)
            self.docker("network", "create", "--ipv6", "--label", f"org.usdb.p2p-acceptance={self.project}", self.remote)
            self.created_networks.append(self.remote)
            data = self.project + "-data-peer"
            self.containers.append(data)
            self.docker("run", "-d", "--name", data, "--label", f"org.usdb.p2p-acceptance={self.project}",
                        "--network", self.shared, "--network-alias", "data-peer", "--memory", "128m",
                        "--cpus", "0.5", "--read-only", "--tmpfs", "/state", "--cap-drop", "ALL",
                        "--security-opt", "no-new-privileges", "--mount", f"type=bind,src={self.binary},dst=/probe,readonly",
                        self.image, "/probe", "-mode", "serve", "-advertise", "127.0.0.1")
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def start(self, family):
        self.family = family
        config = json.loads(self.compose("config", "--format", "json"))
        chain = config["services"]["usdb-chain"]
        assert not chain.get("depends_on"), "fixture must not start upstream services"
        assert {v["source"] for v in chain["volumes"]} == {str(self.binary), str(self.state)}
        self.compose("up", "--detach", "--no-deps", "--force-recreate", "--pull", "never", "usdb-chain")
        item, = json.loads(self.docker("inspect", self.compose("ps", "--quiet", "usdb-chain").strip()))
        self.containers.append(item["Id"])
        if family != "ipv4":
            self.created_networks.append(self.shared + "-p2p-v6")
        deadline = time.monotonic() + 15
        client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        last_error = None
        while time.monotonic() < deadline:
            try:
                with client.open(f"http://127.0.0.1:{self.http_port}", timeout=1) as response:
                    info = json.load(response)
                return item, info
            except (OSError, ValueError) as error:
                last_error = error
                time.sleep(0.2)
        state = self.docker("inspect", "--format", "{{json .State}}", item["Id"])
        raise RuntimeError(f"fixture seed HTTP readiness timed out: {last_error}; container={state}")

    def probe(self, *args):
        name = self.project + "-probe-" + uuid.uuid4().hex[:6]
        self.containers.append(name)
        return json.loads(self.docker("run", "--rm", "--name", name,
            "--label", f"org.usdb.p2p-acceptance={self.project}", "--network", self.remote,
            "--memory", "128m", "--cpus", "0.5", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--mount", f"type=bind,src={self.binary},dst=/probe,readonly",
            self.image, "/probe", *args))

    def __exit__(self, *unused):
        # Capture logs and remove only this fixture's labelled objects, including
        # partially created Compose containers after a failed up.
        ids = self.docker("ps", "-aq", "--filter", f"label=org.usdb.p2p-acceptance={self.project}").split()
        for container in ids:
            logs = subprocess.run(["docker", "logs", container], capture_output=True, text=True, timeout=10)
            self.output.joinpath(f"container-{container}.log").write_text(logs.stdout + logs.stderr)
            self.docker("rm", "--force", container)
        networks = list(dict.fromkeys([self.shared + "-p2p-v6", *reversed(self.created_networks)]))
        for name in networks:
            result = subprocess.run(["docker", "network", "inspect", name], capture_output=True, text=True, timeout=10)
            if result.returncode:
                continue
            item, = json.loads(result.stdout)
            labels = item.get("Labels") or {}
            if labels.get("org.usdb.p2p-acceptance") == self.project or labels.get("com.docker.compose.project") == self.project:
                self.docker("network", "rm", name)
