"""Native release fixtures and an observed-container model; never use host node data."""

import copy
import hashlib
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import assumeutxo_deployment as deployment
import prepare_release_node_kit as builder
import release_manifest as release
import resource_policy as policy
import usdb_node as node

ROOT = Path(__file__).resolve().parents[2]
ORIGIN = "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551"


def native_kit(root, *, manifest=None, trusted=None):
    bundle = deployment.prepare_bundle(ROOT / "docker/networks/testnet-v0", root / "bundle", ORIGIN, "", manifest, trusted)
    revisions = dict(go_ethereum="a" * 40, usdb="b" * 40, source_dao="c" * 40)
    lock = root / "ci-revisions.json"
    lock.write_text(json.dumps(dict(schema_version="usdb-ci-revisions:v2", toolchains={},
        coordinator=dict(repository="buckyos/go-ethereum", directory="go-ethereum"),
        dependencies={key: dict(repository=release.REPOSITORIES[key], directory=release.REPOSITORY_DIRECTORIES[key], revision=revisions[key])
                      for key in ("usdb", "source_dao")})))
    value = release.create_manifest(bundle_dir=bundle, release_id="usdb-testnet-v0-r999", created_at_utc="2026-09-13T12:00:00Z",
        compatibility_lock_path=lock, revisions=revisions,
        image_references={key: f"ghcr.io/buckyos/{name}@sha256:" + str(i) * 64
                          for i, (key, name) in enumerate((("sourcedao_tools", "sourcedao-bootstrap-tools"),
                              ("usdb_services", "usdb-services"), ("usdb_chain", "usdb-chain"), ("bitcoin_core", "usdb-bitcoin-core")), 1)},
        qualification_level="fast", qualification_evidence=[dict(suite="fast", repository=release.REPOSITORIES[key],
            workflow=".github/workflows/usdb-fast.yml" if key == "source_dao" else ".github/workflows/usdb-release-build.yml",
            revision=revisions[key], run_id=i, run_attempt=1) for i, key in enumerate(revisions, 101)])
    path = root / "usdb-release-manifest.json"
    path.write_text(json.dumps(value))
    checksum = path.with_suffix(".json.sha256")
    checksum.write_text(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.name + "\n")
    output = builder.build_node_kit(repository_root=ROOT, bundle_dir=bundle, manifest_path=path,
                                  manifest_checksum_path=checksum, output_dir=root / "kit")
    return node.load_release_layout(output, node_env=root / "node.env")


class NativeRuntime:
    """Model Docker observations, including graceful resource stops and interrupted starts."""
    def __init__(self, root):
        self.layout = SimpleNamespace(node_env=root / "node.env", bundle_id="native-test",
                                      network_identity={"btc_index_origin_height": 963800})
        self.memory = 64 * policy.GIB
        env = dict(SNAPSHOT_MODE="assumeutxo", USDB_BITCOIN_IMAGE="bitcoin@sha256:1", USDB_SERVICES_IMAGE="services@sha256:2")
        env.update(policy.build_resource_plan(self.memory, "bitcoin", env).environment())
        self.layout.node_env.write_text(node.upsert_env("", env))
        self.containers, self.events = {}, []
        self.core = dict(schema_version="usdb-bitcoin-assumeutxo:v1", bootstrap_ready=True, tip_ready=False,
                         history_validated=False, active_height=935000, background_height=12)
        self.ready = False
        self.crash = None
        self.tick = 0
        self.advance = lambda: None

    def observed(self, _layout):
        return copy.deepcopy(self.containers)

    def container(self, service):
        env = node.read_env(self.layout.node_env)
        return dict(state="running", exit_code=0, memory=int(env[policy.SERVICE_MEMORY_KEYS[service]]),
                    swap=int(env["BTC_MEMORY_SWAP_LIMIT"] if service == "btc-node" else env["BH_MEMORY_SWAP_LIMIT"]),
                    image=env["USDB_BITCOIN_IMAGE"] if service == "btc-node" else env["USDB_SERVICES_IMAGE"], environment=env)

    def helper(self, _layout, _helper, args, **_kwargs):
        action = args[0]
        self.events.append((action, node.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"]))
        if action == "quiesce-data":
            for name in ("balance-history", "usdb-indexer", "usdb-chain", "usdb-control-plane"):
                if name in self.containers:
                    self.containers[name]["state"] = "exited"
        elif action == "down":
            self.containers.pop("btc-node", None)
            self.containers.pop("btc-snapshot-bootstrap", None)
        elif action == "start":
            self.containers["btc-node"] = self.container("btc-node")
        elif action == "bootstrap-start":
            self.containers["btc-snapshot-bootstrap"] = {**self.container("btc-snapshot-bootstrap"), "state": "exited"}
        elif action == "native-start-data":
            assert self.core["bootstrap_ready"]
            assert node.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"] != "bitcoin"
            for name in ("balance-history", "usdb-indexer"):
                self.containers[name] = self.container(name)
        elif action == "up-chain":
            assert self.core["tip_ready"] and self.ready
            for name in ("usdb-chain", "usdb-control-plane"):
                self.containers[name] = self.container(name)
        elif action == "progress":
            return subprocess.CompletedProcess(args, 0 if self.core["tip_ready"] else 1, json.dumps(self.core), "")
        else:
            raise AssertionError("Unexpected native helper action: " + action)
        if self.crash == action:
            self.crash = None
            raise RuntimeError("Interrupted after " + action)
        return subprocess.CompletedProcess(args, 0, "", "")

    def readiness(self, _layout, _helper, _args, service):
        if self.containers.get(service, {}).get("state") != "running":
            return None, "not started"
        return dict(service=service, consensus_ready=self.ready), ""

    def sleep(self, seconds):
        self.tick += seconds
        self.advance()
