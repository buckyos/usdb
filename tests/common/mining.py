"""Controllable RPC and Docker boundaries for durable mining acceptance tests."""
from contextlib import ExitStack, contextmanager
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from unittest import mock

import usdb_node as NODE
import usdb_mining as MINING
from common.enode import V4 as SEED

ADDRESS = "0x4ddc71108239dbb30aa288b93ab1d18539ec863a"
PASS_ID = "1f" * 32 + "i0"


class MiningFixture:
    """Run the real state machine against private files and fake service boundaries."""
    def __enter__(self):
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory(prefix="usdb-mining-test-")))
        data = self.root / "chain"
        (data / "geth/chaindata").mkdir(parents=True)
        (data / "geth/chaindata/CURRENT").write_text("MANIFEST-1\n")
        (data / "geth/nodekey").write_text("test-node-key")
        (data / NODE.DATASET_IDENTITY_FILE).write_text('{"dataset":"usdb-chain"}\n')
        self.layout = SimpleNamespace(node_env=self.root / "node.env", release_id="test-r1", bundle_id="test-v0",
                                      kit_root=self.root, bundle_dir=self.root / "bundle",
                                      network_identity={"chain_id": 123, "network_id": 123,
                                                        "genesis_block_hash": "0x" + "ab" * 32,
                                                        "btc_activation_registry_id": "cd" * 32},
                                      images={"USDB_CHAIN_IMAGE": "chain@sha256:" + "11" * 32})
        self.env = {"USDB_CHAIN_DATA_HOST_DIR": str(data), "USDB_NODE_ROLE": "full",
                    "USDB_MINER_ADDRESS": "", "USDB_MINER_THREADS": "1", "USDB_BOOTNODES": "",
                    "USDB_RESOURCE_MODE": "manual", "BTC_RPC_PASSWORD": "fixture-secret-do-not-journal"}
        self.layout.node_env.write_text("".join(f"{k}={v}\n" for k, v in self.env.items()))
        self.chain = {"height": 0, "syncing": False, "peers": 0, "node_id": "ee" * 32,
                      "enode": "enode://" + "ee" * 64 + "@172.18.0.2:31303",
                      "network": self.layout.network_identity}
        self.ready = {"service": "usdb-indexer", "consensus_ready": True, "synced_block_height": 100,
                      "upstream_snapshot_id": "01" * 32, "system_state_id": "02" * 32,
                      "local_state_commit": "03" * 32, "upstream_reorg_epoch": 0}
        self.candidate = {"view_version": MINING.VIEW, "selection_rule": MINING.RULE, "matching_candidate_count": 2,
                          "external_state": {"btc_height": 100, "snapshot_id": "01" * 32,
                                             "system_state_id": "02" * 32, "local_state_commit": "03" * 32,
                                             "stable_block_hash": "04" * 32, "stable_lag": 10,
                                             "active_version_set_id": "05" * 32, "active_version_set": {"v": 1},
                                             "balance_history_api_version": "1.0.0", "balance_history_semantics_version": "v1",
                                             "activation_registry_id": "cd" * 32},
                          "pass": {"pass_id": PASS_ID, "state": "active", "pass_kind": "standard",
                                   "usdb_main": ADDRESS, "raw_energy": "0", "collab_contribution": "0",
                                   "effective_energy": "0", "owner_btc_addr": None, "level": 0,
                                   "difficulty_factor_bps": 10000},
                          "miner_aggregate": {"total_miner_btc_sats": "18894", "active_miner_owner_count": 1}}
        self.calls = []
        self.rpc_calls = []
        self.fail_rpc = None
        self.fail_helper = None
        self.after_helper = None
        self.after_rpc = None
        self.work_available = True
        self.mining_override = None
        self.container_number = 0
        self.adopt()
        observations = {"_validate_node_config": None, "_validate_node_release_images": None,
                        "_require_controller_unit": self.root / "controller.service",
                        "start_controller_unit": "usdb-controller", "stop_controller_unit": None,
                        "_bitcoin_startup_progress": {"ready": True},
                        "_read_service_readiness": ({"consensus_ready": True}, None)}
        for name, value in observations.items():
            self.stack.enter_context(mock.patch.object(NODE, name, return_value=value))
        self.stack.enter_context(mock.patch.object(NODE, "run_helper", side_effect=self.helper))
        self.stack.enter_context(mock.patch.object(MINING, "inspect_chain", side_effect=lambda *a, **kw: deepcopy(self.runtime)))
        self.stack.enter_context(mock.patch.object(MINING, "chain_view", side_effect=lambda *a: deepcopy(self.chain)))
        self.stack.enter_context(mock.patch.object(MINING, "rpc", side_effect=self.rpc))
        self.stack.enter_context(mock.patch.object(MINING.time, "sleep", return_value=None))
        return self

    def __exit__(self, *args):
        return self.stack.__exit__(*args)

    def update_env(self, **updates):
        MINING.node._atomic_write_private(self.layout.node_env,
            NODE.upsert_env(self.layout.node_env.read_text(), updates))

    def adopt(self):
        env = NODE.read_env(self.layout.node_env)
        args = ["geth", "--networkid", "123", "--bootnodes", env.get("USDB_BOOTNODES", ""), "--discovery.dns", ""]
        if env["USDB_NODE_ROLE"] == "miner":
            args += ["--mine", "--miner.threads", env["USDB_MINER_THREADS"], "--miner.etherbase", env["USDB_MINER_ADDRESS"]]
        self.runtime = {"id": f"{self.container_number:064x}", "image": self.layout.images["USDB_CHAIN_IMAGE"],
                        "state": "running", "exit_code": 0, "memory": 5 * 1024**3,
                        "environment": {**MINING.role_config(env), "USDB_BOOTNODES": env.get("USDB_BOOTNODES", "")},
                        "argv": args}

    def helper(self, layout, helper, arguments, **kwargs):
        self.calls.append((helper, tuple(arguments)))
        action = arguments[0]
        if self.fail_helper == action:
            raise ValueError("injected OCI failure")
        if action == "stop-chain":
            self.runtime.update(state="exited", argv=[])
        elif action == "recreate-chain":
            if self.runtime["state"] == "running":
                raise AssertionError("new writer created before the old writer stopped")
            self.container_number += 1
            self.adopt()
        else:
            raise AssertionError(f"unexpected helper action: {arguments}")
        if self.after_helper:
            self.after_helper(action)
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    def rpc(self, layout, method, params=None, **kwargs):
        self.rpc_calls.append((method, deepcopy(params)))
        if self.fail_rpc == method:
            raise ValueError("injected RPC unavailable")
        values = {"get_readiness": self.ready, "resolve_miner_candidate": self.candidate,
                  "eth_mining": self.runtime["environment"]["USDB_NODE_ROLE"] == "miner" if self.mining_override is None else self.mining_override,
                  "eth_coinbase": self.runtime["environment"].get("USDB_MINER_ADDRESS"),
                  "admin_peers": [{"protocols": {"eth": {"version": 66, "head": "0x" + "11" * 32}}}],
                  "eth_getWork": ["0x" + "01" * 32, "0x" + "02" * 32, "0x" + "03" * 32]}
        if method == "eth_getWork" and not self.work_available:
            raise ValueError("mining work not ready")
        if method not in values:
            raise AssertionError(f"unexpected RPC {method}")
        value = deepcopy(values[method])
        if self.after_rpc:
            self.after_rpc(method)
        return value

    def enable(self):
        return MINING.submit(self.layout, address=ADDRESS, first_node=True, yes=True)

    def run(self):
        with NODE.node_operation_lock(self.layout, "mining"):
            return MINING.run_operation(self.layout, wait_secs=0.02)

    @contextmanager
    def protected_files(self, *, key_only=False):
        """Deny host reads and execute the actual stdin probe at the Docker boundary."""
        import subprocess
        import sys
        data = Path(self.env["USDB_CHAIN_DATA_HOST_DIR"])
        original_stat, original_open, original_run = Path.stat, Path.open, subprocess.run
        probes = []

        def host_stat(path, *args, **kwargs):
            if not key_only and any(path.is_relative_to(data / name) for name in ("geth", "recovery")):
                raise PermissionError(13, "Permission denied", str(path))
            return original_stat(path, *args, **kwargs)

        def host_open(path, *args, **kwargs):
            if path == data / "geth/nodekey":
                raise PermissionError(13, "Permission denied", str(path))
            return original_open(path, *args, **kwargs)

        def docker_probe(command, **kwargs):
            assert command[:2] == ["docker", "run"], command
            for flag in ("--rm", "--pull=never", "--network=none", "--read-only", "--user=0:0",
                         "--cap-drop=ALL", "--cap-add=DAC_READ_SEARCH", "--security-opt=no-new-privileges"):
                assert flag in command, command
            assert command[command.index("--mount") + 1] == f"type=bind,src={data},dst=/chain,readonly"
            assert command[-5] == self.layout.images["USDB_CHAIN_IMAGE"]
            assert command[-3] in ("binding", "guard") and command[-2] == "/chain"
            result = original_run([sys.executable, "-", command[-3], str(data), command[-1]], **kwargs)
            probes.append((command[-3], self.runtime["state"], result.stdout))
            return result

        with mock.patch.object(Path, "stat", host_stat), mock.patch.object(Path, "open", host_open), \
                mock.patch.object(MINING.subprocess, "run", side_effect=docker_probe):
            yield probes


class RuntimeScriptFixture:
    """Execute the real runtime shell with a geth argv recorder in a temporary data root."""
    def __enter__(self):
        import hashlib
        import os
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory(prefix="usdb-mining-runtime-")))
        self.script = Path(__file__).resolve().parents[3] / "go-ethereum/scripts/usdb/docker/usdb_runtime_node.sh"
        self.data = self.root / "data"
        (self.data / "geth/chaindata").mkdir(parents=True)
        (self.data / "geth/chaindata/CURRENT").write_text("MANIFEST-1")
        (self.data / "bootstrap").mkdir()
        genesis = self.root / "genesis.json"
        genesis.write_text("{}")
        marker = {"genesis_sha256": hashlib.sha256(b"{}").hexdigest(), "chain_id": 123, "network_id": 123, "done": True}
        (self.data / "bootstrap/usdb-init.done.json").write_text(json.dumps(marker, indent=2))
        validator = self.root / "validator"
        validator.write_text("#!/bin/bash\nexit 0\n")
        validator.chmod(0o755)
        geth = self.root / "geth"
        geth.write_text("#!/usr/bin/env python3\nimport json,os,sys\nopen(os.environ['ARGV_FILE'],'w').write(json.dumps(sys.argv))\n")
        geth.chmod(0o755)
        self.argv_file = self.root / "argv.json"
        self.env = {**os.environ, "USDB_CHAIN_DATA_DIR": str(self.data), "USDB_GENESIS_FILE": str(genesis),
                    "USDB_CHAIN_ID": "123", "USDB_NETWORK_ID": "123", "USDB_GENESIS_VALIDATOR": str(validator),
                    "USDB_INDEXER_RPC_URL": "http://127.0.0.1:1", "USDB_GETH_BIN": str(geth),
                    "ARGV_FILE": str(self.argv_file), "USDB_DEEP_REORG_GUARD_ENABLED": "0"}
        for key in ("USDB_CHAIN_EXTRA_ARGS", "USDB_BOOTNODES", "USDB_MINER_THREADS", "USDB_NODE_ROLE"):
            self.env.pop(key, None)
        return self

    def __exit__(self, *args):
        return self.stack.__exit__(*args)

    def run(self, **env):
        import subprocess
        return subprocess.run(["bash", str(self.script)], env={**self.env, **env}, capture_output=True, text=True, timeout=10)


class RuntimeHelperFixture:
    """Execute the real Compose helper against a Docker command recorder."""
    def __enter__(self):
        import os
        import shutil
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory(prefix="usdb-mining-helper-")))
        source = Path(__file__).resolve().parents[2] / "docker/scripts/tools/run_testnet_runtime.sh"
        self.script = self.root / "docker/scripts/tools/run_testnet_runtime.sh"
        self.script.parent.mkdir(parents=True)
        shutil.copy2(source, self.script)
        (self.script.parent / "validate_network_bundle.py").write_text("# Isolated bundle validation boundary.\n")
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.node_env = self.root / "node.env"
        self.node_env.write_text("USDB_NODE_ROLE=full\n")
        self.state = self.root / "docker-state"
        self.state.write_text("running")
        self.calls = self.root / "docker-calls.jsonl"
        binary = self.root / "docker-bin"
        binary.mkdir()
        docker = binary / "docker"
        docker.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
state = pathlib.Path(os.environ['DOCKER_STATE'])
with open(os.environ['DOCKER_CALLS'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'compose':
    if 'ps' in args:
        print('a' * 64)
    elif 'up' in args:
        assert state.read_text() == 'exited', 'old writer is still running'
        assert '--no-deps' in args and '--force-recreate' in args
        assert args[-1] == 'usdb-chain'
        state.write_text('running')
elif args[0] == 'inspect':
    print(state.read_text())
elif args[0] == 'kill':
    assert '--signal=SIGTERM' in args
    state.write_text('exited')
elif args[0] != 'update':
    raise SystemExit('unexpected Docker command')
''')
        docker.chmod(0o755)
        self.env = {**os.environ, "PATH": str(binary) + os.pathsep + os.environ.get("PATH", ""),
                    "USDB_TESTNET_BUNDLE_DIR": str(self.bundle), "USDB_TESTNET_NODE_ENV": str(self.node_env),
                    "DOCKER_STATE": str(self.state), "DOCKER_CALLS": str(self.calls)}
        return self

    def __exit__(self, *args):
        return self.stack.__exit__(*args)

    def run(self, action):
        import subprocess
        return subprocess.run(["bash", str(self.script), action], env=self.env, capture_output=True, text=True, timeout=10)
