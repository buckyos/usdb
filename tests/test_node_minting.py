#!/usr/bin/env python3
"""Optional Ord deployment, readiness, resource, and lifecycle boundaries."""

import copy
import io
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from contextlib import redirect_stderr
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import control_plane_monitor as monitor
import ord_runtime as runtime
import resource_policy as policy
import usdb_minting as minting
import usdb_node as node
import usdb_p2p as p2p
import assumeutxo_node as native
from common.minting import Child, Loop, core_observation, disk_space
from common.native_node import native_kit
from common.p2p import HOST, V4


class MintingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.env = dict(USDB_DATA_ROOT=str(self.root), **minting.environment(self.root, True))
        disk_patch = mock.patch.object(minting, "disk_usage", return_value=disk_space())
        disk_patch.start()
        self.addCleanup(disk_patch.stop)

    def test_new_index_requires_300_gib_before_creating_any_files(self):
        for free in (50 * policy.GIB, minting.MIN_NEW_INDEX_FREE_BYTES - 1):
            with self.subTest(free=free), mock.patch.object(minting, "disk_usage", return_value=disk_space(free)):
                with self.assertRaisesRegex(ValueError, "300.0 GiB required"):
                    minting.prepare(self.env)
            self.assertFalse(minting.data_path(self.root).exists())
        with mock.patch.object(minting, "disk_usage", return_value=disk_space(minting.MIN_NEW_INDEX_FREE_BYTES)):
            minting.prepare(self.env)
        self.assertTrue((minting.data_path(self.root) / "identity.json").is_file())

    def test_capacity_checks_target_filesystem_and_does_not_count_old_index(self):
        old = minting.data_path(self.root, minting.LEGACY_VERSION)
        old.mkdir(parents=True)
        (old / "index.redb").write_bytes(b"keep old index")
        with mock.patch.object(minting, "disk_usage", return_value=disk_space(299 * policy.GIB)) as usage:
            with self.assertRaisesRegex(ValueError, "1.0 GiB short"):
                minting.prepare(self.env)
        usage.assert_called_once_with(old.parent)
        self.assertEqual((old / "index.redb").read_bytes(), b"keep old index")
        self.assertFalse(minting.data_path(self.root).exists())

    def test_existing_index_reuses_storage_while_empty_marker_still_needs_capacity(self):
        minting.prepare(self.env)
        root = minting.data_path(self.root)
        with mock.patch.object(minting, "disk_usage", return_value=disk_space(100 * policy.GIB)):
            for empty_index in (False, True):
                if empty_index:
                    (root / "index.redb").touch()
                with self.assertRaisesRegex(ValueError, "300.0 GiB required"):
                    minting.prepare(self.env)
            (root / "index.redb").write_bytes(b"existing index")
            minting.prepare(self.env)
            self.assertFalse(minting.check_disk_capacity(self.env)["new_index"])
            self.assertEqual((root / "index.redb").read_bytes(), b"existing index")
        self.assertEqual(self.env["ORD_MIN_FREE_BYTES"], str(50 * policy.GIB))

    def test_disabled_ord_does_not_inspect_or_reserve_disk(self):
        with mock.patch.object(minting, "disk_usage", side_effect=AssertionError("disabled capacity check")):
            minting.prepare({**self.env, "USDB_MINTING_ENABLED": "0"})
        self.assertFalse(minting.data_path(self.root).exists())

    def test_startup_reports_disk_failure_without_starting_ord_or_blocking_core(self):
        layout = SimpleNamespace(node_env=self.root / "node.env")
        layout.node_env.write_text(node.upsert_env("", self.env))
        output = io.StringIO()
        with mock.patch.object(minting, "disk_usage", return_value=disk_space(299 * policy.GIB)), \
                mock.patch.object(node, "run_helper") as run, redirect_stderr(output):
            node._start_optional_ord(layout, output_to_stderr=False)
        run.assert_not_called()
        self.assertIn("300.0 GiB required", output.getvalue())
        self.assertIn("Core node startup continues", output.getvalue())
        self.assertFalse(minting.data_path(self.root).exists())

    def test_set_minting_rejects_insufficient_disk_without_saving_configuration(self):
        layout = SimpleNamespace(node_env=self.root / "node.env")
        env = dict(self.env, USDB_MINTING_ENABLED="0", BTC_TXINDEX="0", SNAPSHOT_MODE="assumeutxo")
        env.update(policy.build_resource_plan(64 * policy.GIB, "steady", env).environment())
        original = node.upsert_env("", env)
        layout.node_env.write_text(original)
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_collect_compose_services", return_value={}), \
                mock.patch.object(minting, "disk_usage", return_value=disk_space(299 * policy.GIB)):
            with self.assertRaisesRegex(ValueError, "300.0 GiB required"):
                minting.configure(layout, True, node)
        self.assertEqual(layout.node_env.read_text(), original)
        self.assertFalse(minting.data_path(self.root).exists())

    def test_fresh_configuration_adds_ord_capacity_to_the_base_node_requirement(self):
        layout = native_kit(self.root)
        data = self.root / "data"
        required = node.MIN_DATA_ROOT_BYTES + minting.MIN_NEW_INDEX_FREE_BYTES
        arguments = dict(data_root=data, role="full", miner_address="", miner_threads=1,
                         bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private",
                         resource_management="auto", minting=True)
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB):
            for total, free in ((3 * 1024**4, required - 1), (required - 1, required - 1)):
                with self.subTest(total=total, free=free), \
                        mock.patch.object(node, "_data_root_capacity", return_value=node.DataRootCapacity(self.root, total, free)):
                    with self.assertRaisesRegex(ValueError, "300.0 GiB for Ord"):
                        node.configure_node(layout, **arguments)
                self.assertFalse(layout.node_env.exists())
                self.assertFalse(data.exists())
            with mock.patch.object(node, "_data_root_capacity", return_value=node.DataRootCapacity(self.root, required, required)):
                node.configure_node(layout, **arguments)
        self.assertEqual(node.read_env(layout.node_env)["USDB_MINTING_ENABLED"], "1")

    def test_history_and_index_are_independent_readiness_gates(self):
        for complete, indexed, synced, expected in [
            (False, 40, True, "WAITING_HISTORY"), (True, 99, True, "WAITING_TXINDEX"),
            (True, None, False, "WAITING_TXINDEX"), (True, 100, False, "WAITING_TXINDEX"),
            (True, 100, True, "STARTING"),
        ]:
            info, chains, indexes = core_observation(complete=complete, indexed=indexed, synced=synced)
            report = runtime.prerequisites(info, chains, indexes, now=1001)
            self.assertEqual(report["state"], expected)
        info["headers"] = 101
        self.assertEqual(runtime.prerequisites(info, chains, indexes, now=1001)["state"], "WAITING_CORE")
        info["headers"] = 100
        self.assertEqual(runtime.prerequisites(info, chains, indexes, now=10000)["state"], "WAITING_CORE")
        info["pruned"] = True
        self.assertEqual(runtime.prerequisites(info, chains, indexes, now=1001)["state"], "BLOCKED_CONFIG")

    def test_ord_requires_genesis_count_and_canonical_anchor_including_reorg_race(self):
        core = runtime.prerequisites(*core_observation(), now=1001)
        for count, ord_hash, core_hash, state in [
            (100, "a", "a", "INDEXING"), (101, "a", "a", "READY"),
            (101, "b", "a", "INDEXING"), (101, "a", "b", "INDEXING"),
            (102, "a", "a", "READY"), (0, "a", "a", "INDEXING"),
        ]:
            fetch = lambda url: count if url.endswith("/blockcount") else ord_hash * 64
            report = runtime.observe_ord(core, fetch, lambda *_: core_hash * 64)
            self.assertEqual(report["state"], state)

    def test_stale_and_invalid_observations_do_not_enable_capability(self):
        minting.prepare(self.env)
        path = minting.data_path(self.root) / "progress.json"
        report = dict(schema_version=runtime.SCHEMA, observed_at_ms=1000, state="READY", canonical=True,
                      txindex_synced=True, history_validated=True, password="SECRET")
        path.write_text(json.dumps(report))
        current = minting.progress(self.env, now_ms=1001)
        self.assertTrue(current["backend_ready"])
        self.assertFalse(current["transactions_enabled"])
        self.assertNotIn("SECRET", json.dumps(current))
        self.assertFalse(minting.progress(self.env, now_ms=62000)["backend_ready"])
        self.assertFalse(minting.progress(self.env, now_ms=999)["backend_ready"])
        report["canonical"] = False
        path.write_text(json.dumps(report))
        self.assertFalse(minting.progress(self.env, now_ms=1001)["backend_ready"])
        self.assertEqual(minting.progress({})["state"], "DISABLED")

    def test_dataset_preserved_on_disable_and_unknown_data_rejected(self):
        minting.prepare(self.env)
        index = minting.data_path(self.root) / "index.redb"
        index.write_bytes(b"preserve")
        minting.prepare(dict(self.env, USDB_MINTING_ENABLED="0"))
        self.assertEqual(index.read_bytes(), b"preserve")
        minting.prepare(self.env)
        (index.parent / "identity.json").unlink()
        with self.assertRaisesRegex(ValueError, "unmarked"):
            minting.prepare(self.env)
        self.assertEqual(index.read_bytes(), b"preserve")

    def test_all_resource_phases_reserve_ord_without_overcommitting(self):
        for memory in (32_000_000_000, 32 * policy.GIB, 64 * policy.GIB, 128 * policy.GIB):
            for phase in policy.PHASES:
                env = dict(self.env, SNAPSHOT_MODE="assumeutxo")
                plan = policy.build_resource_plan(memory, phase, env)
                env.update(plan.environment())
                self.assertLessEqual(plan.total_bytes, memory)
                self.assertEqual(plan.limits["ORD_MEMORY_LIMIT"], 4 * policy.GIB)
                policy.validate_resource_environment(env, memory)
                node._check_running_resource_budget(env, {"ord-server": dict(state="running", memory=4 * policy.GIB)})
        baseline = policy.build_resource_plan(64 * policy.GIB, "steady", {})
        self.assertNotIn("ORD_MEMORY_LIMIT", baseline.limits)
        with self.assertRaises(ValueError):
            policy.build_resource_plan(32 * policy.GIB, "steady", dict(self.env, ORD_MEMORY_LIMIT="32g"))
        with self.assertRaisesRegex(ValueError, "half"):
            minting.validate(dict(self.env, ORD_INDEX_CACHE_BYTES=str(3 * policy.GIB)))

    def test_monitor_projection_keeps_optional_readiness_separate(self):
        report = monitor.project(dict(overall_state="READY", components=[],
            minting=dict(enabled=True, state="WAITING_HISTORY", history_height=40, password="SECRET")), 1000)
        self.assertEqual(report["overall_state"], "READY")
        self.assertEqual(report["minting"]["history_height"], 40)
        self.assertNotIn("SECRET", json.dumps(report))

    def test_supervisor_failure_is_not_reported_as_an_ordinary_stop(self):
        core = runtime.prerequisites(*core_observation(), now=1001)
        child = Child()
        child.code = 1
        reports = []
        with mock.patch.dict(runtime.os.environ, dict(ORD_DATA_DIR=str(self.root), BTC_RPC_USER="user", BTC_RPC_PASSWORD="secret")), \
                mock.patch.object(runtime.threading, "Event", return_value=Loop(2)), \
                mock.patch.object(runtime.signal, "signal"), \
                mock.patch.object(runtime, "observe_core", return_value=core), \
                mock.patch.object(runtime, "observe_ord", side_effect=OSError("not running")), \
                mock.patch.object(runtime.shutil, "disk_usage", return_value=SimpleNamespace(free=100 * policy.GIB)), \
                mock.patch.object(runtime.subprocess, "Popen", return_value=child), \
                mock.patch.object(runtime, "publish", side_effect=lambda _, report: reports.append(copy.deepcopy(report))):
            self.assertEqual(runtime.supervise(), 1)
        self.assertEqual(reports[-1]["state"], "FAILED")

    def test_ready_node_can_retry_only_the_optional_service(self):
        layout = SimpleNamespace(node_env=self.root / "node.env")
        env = dict(self.env, SNAPSHOT_MODE="assumeutxo")
        env.update(policy.build_resource_plan(64 * policy.GIB, "steady", env).environment())
        layout.node_env.write_text(node.upsert_env("", env))
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_resource_containers", return_value={}), \
                mock.patch.object(node, "run_helper") as run:
            node._start_optional_ord(layout, output_to_stderr=False)
        run.assert_called_once_with(layout, "run_testnet_runtime.sh", ["up-ord"], output_to_stderr=False)

    def test_disabling_legacy_ord_preserves_legacy_txindex_requirement(self):
        layout = SimpleNamespace(node_env=self.root / "node.env")
        env = dict(self.env, SNAPSHOT_MODE="none")
        env.update(policy.build_resource_plan(64 * policy.GIB, "steady", env).environment())
        layout.node_env.write_text(node.upsert_env("", env))
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_collect_compose_services", return_value={}), \
                mock.patch.object(node, "_validate_node_config"):
            minting.configure(layout, False, node)
        disabled = node.read_env(layout.node_env)
        self.assertEqual(disabled["USDB_MINTING_ENABLED"], "0")
        self.assertEqual(disabled["BTC_TXINDEX"], "1")

    def test_supervisor_does_not_start_before_gates_and_pauses_for_disk(self):
        core = runtime.prerequisites(*core_observation(), now=1001)
        child = Child()
        reports = []
        with mock.patch.dict(runtime.os.environ, dict(ORD_DATA_DIR=str(self.root), BTC_RPC_USER="user", BTC_RPC_PASSWORD="SECRET")), \
                mock.patch.object(runtime.threading, "Event", return_value=Loop(3)), \
                mock.patch.object(runtime.signal, "signal"), \
                mock.patch.object(runtime, "observe_core", side_effect=[dict(core, state="WAITING_TXINDEX"), core, core]), \
                mock.patch.object(runtime, "observe_ord", return_value=dict(core, state="READY", canonical=True)), \
                mock.patch.object(runtime.shutil, "disk_usage", side_effect=[SimpleNamespace(free=value * policy.GIB) for value in (100, 100, 1)]), \
                mock.patch.object(runtime.subprocess, "Popen", return_value=child) as start, \
                mock.patch.object(runtime, "publish", side_effect=lambda _, report: reports.append(copy.deepcopy(report))):
            self.assertEqual(runtime.supervise(), 0)
        start.assert_called_once()
        self.assertNotIn("SECRET", str(start.call_args.args))
        self.assertEqual(child.signals, [signal.SIGINT])
        self.assertEqual(list(dict.fromkeys(value["state"] for value in reports)), ["WAITING_TXINDEX", "READY", "BLOCKED_DISK", "STOPPED"])

    def test_temporary_upstream_lag_revokes_readiness_without_restarting_ord(self):
        core = runtime.prerequisites(*core_observation(), now=1001)
        child = Child()
        reports = []
        with mock.patch.dict(runtime.os.environ, dict(ORD_DATA_DIR=str(self.root), BTC_RPC_USER="user", BTC_RPC_PASSWORD="secret")), \
                mock.patch.object(runtime.threading, "Event", return_value=Loop(3)), \
                mock.patch.object(runtime.signal, "signal"), \
                mock.patch.object(runtime, "observe_core", side_effect=[core, dict(core, state="WAITING_TXINDEX"), core]), \
                mock.patch.object(runtime, "observe_ord", return_value=dict(core, state="READY", canonical=True)), \
                mock.patch.object(runtime, "ord_heights", side_effect=lambda report: report), \
                mock.patch.object(runtime.shutil, "disk_usage", return_value=SimpleNamespace(free=100 * policy.GIB)), \
                mock.patch.object(runtime.subprocess, "Popen", return_value=child) as start, \
                mock.patch.object(runtime, "publish", side_effect=lambda _, report: reports.append(copy.deepcopy(report))):
            self.assertEqual(runtime.supervise(), 0)
        start.assert_called_once()
        self.assertEqual(child.signals, [signal.SIGINT])
        self.assertEqual([report["state"] for report in reports], ["READY", "WAITING_TXINDEX", "READY", "STOPPED"])

    def test_native_setup_persists_txindex_before_start_and_toggle_retains_data(self):
        layout = native_kit(self.root)
        data = self.root / "data"
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_validate_data_root_capacity"), \
                mock.patch.object(node, "_collect_compose_services", return_value={}):
            node.configure_node(layout, data_root=data, role="full", miner_address="", miner_threads=1,
                bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private", resource_management="auto", minting=True)
            env = node.read_env(layout.node_env)
            self.assertEqual(env["BTC_TXINDEX"], "1")
            self.assertEqual(env["USDB_MINTING_ENABLED"], "1")
            self.assertTrue((layout.kit_root / "docker/compose.runtime-ord.yml").is_file())
            index = minting.data_path(data) / "index.redb"
            index.write_bytes(b"preserve")
            minting.configure(layout, False, node)
            self.assertEqual(node.read_env(layout.node_env)["BTC_TXINDEX"], "0")
            self.assertEqual(index.read_bytes(), b"preserve")
            original = layout.node_env.read_bytes()
            with mock.patch.object(node, "_validate_node_config", side_effect=ValueError("validation failure")):
                with self.assertRaisesRegex(ValueError, "validation failure"):
                    minting.configure(layout, True, node)

            self.assertEqual(layout.node_env.read_bytes(), original)
            with mock.patch.object(node, "_collect_compose_services", return_value={"btc-node": dict(state="running")}):
                with self.assertRaisesRegex(ValueError, "stop the node"):
                    minting.configure(layout, True, node)

    def test_optional_start_failure_does_not_block_native_startup(self):
        layout = SimpleNamespace(node_env=self.root / "node.env")
        layout.node_env.write_text(node.upsert_env("", dict(self.env, SNAPSHOT_MODE="assumeutxo")))
        calls = []

        def run(_layout, _helper, args, **_kwargs):
            calls.append(args[0])
            if args[0] == "up-ord":
                raise ValueError("optional startup failure")

        with mock.patch.object(node, "doctor"), mock.patch.object(monitor, "prepare"), \
                mock.patch.object(node, "validate_resource_environment"), \
                mock.patch.object(node, "run_helper", side_effect=run), \
                mock.patch.object(native, "start_native_node") as start:
            node._start_node(layout, sync_timeout_secs=10, pull=False,
                             output_to_stderr=False, progress_monitor=mock.Mock())
        self.assertEqual(calls, ["up-console", "up-ord"])
        start.assert_called_once()

    def test_interactive_setup_explicitly_enables_minting(self):
        layout = native_kit(self.root)
        prompts = []

        def answer(prompt):
            prompts.append(prompt)
            if prompt.startswith("Host data root"):
                return str(self.root / "data")
            return "y" if prompt.startswith("Enable local minting backend") else ""

        args = node.build_parser().parse_args(["setup", "--p2p-ip-family", "ipv4", "--advertise-ipv4", V4])
        capacity = node.DataRootCapacity(self.root, 3 * 1024**4, 3 * 1024**4)
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_host_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_validate_data_root_capacity", return_value=capacity) as check_capacity, \
                mock.patch.object(p2p, "host_capabilities", return_value=HOST):
            node.setup_node(layout, input_fn=answer, output=io.StringIO(), resource_management="auto", p2p_options=p2p.options(args))
        self.assertTrue(any("Enable local minting backend (txindex + private Ord) [y/N]" in item for item in prompts))
        check_capacity.assert_any_call(self.root / "data", extra_bytes=minting.MIN_NEW_INDEX_FREE_BYTES)
        env = node.read_env(layout.node_env)
        self.assertEqual(env["BTC_TXINDEX"], "1")
        self.assertEqual(env["USDB_MINTING_ENABLED"], "1")

    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required")
    def test_rendered_compose_is_private_bounded_and_enables_core_txindex(self):
        layout = native_kit(self.root)
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
                mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(layout, data_root=self.root / "data", role="full", miner_address="", miner_threads=1,
                bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private", resource_management="auto", minting=True)
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("USDB_", "BTC_", "BH_", "ORD_", "COMPOSE_"))}
        environment.update(USDB_NETWORK_ARTIFACTS_DIR=str(layout.bundle_dir / "artifacts"),
                           BH_SNAPSHOT_TRUST_HOST_DIR=str(layout.bundle_dir / "trust"))
        docker = layout.kit_root / "docker"
        for core in (False, True):
            paths = ([docker / "compose.bitcoin.yml", docker / "compose.bitcoin-assumeutxo.yml"] if core else
                     [docker / "compose.runtime.yml", layout.bundle_dir / "compose.network.yml",
                      docker / "compose.runtime-assumeutxo.yml", docker / "compose.runtime-ord.yml"])
            command = ["docker", "compose", "--project-name", "minting-compose-test", "--env-file", str(layout.bundle_dir / "network.env"), "--env-file", str(layout.node_env)]
            for path in paths:
                command.extend(["-f", str(path)])
            result = subprocess.run([*command, "config", "--format", "json"], env=environment, capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            services = json.loads(result.stdout)["services"]
            if core:
                self.assertEqual(str(services["btc-node"]["environment"]["BTC_TXINDEX"]), "1")
            else:
                ord_service = services["ord-server"]
                self.assertFalse(ord_service.get("ports"))
                self.assertEqual(int(ord_service["mem_limit"]), 4 * policy.GIB)
                self.assertEqual(float(ord_service["cpus"]), 2)
                self.assertEqual(ord_service["entrypoint"][-1], "/opt/usdb/docker/scripts/tools/ord_runtime.py")
                self.assertNotIn("ord-server", services["usdb-chain"].get("depends_on", {}))


if __name__ == "__main__":
    unittest.main()
