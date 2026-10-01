#!/usr/bin/env python3
"""Disk-aware resource plans, opt-in migration and safe Ord budget handoff."""

import io
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import assumeutxo_node as native
import node_resource_metrics as metrics
import node_storage as storage
import ord_runtime as ord_runtime
import resource_policy as policy
import usdb_minting as minting
import usdb_node as node
import usdb_p2p as p2p
import node_monitor
from common.p2p import HOST, V4
from common.minting import Loop
from common.native_node import NativeRuntime, native_kit


class ProfileBudgetTests(unittest.TestCase):
    def test_host_phase_percentage_and_ord_matrix(self):
        for host in (32_000_000_000, 32574320640, 32 * policy.GIB,
                     64 * policy.GIB, 128 * policy.GIB, 256 * policy.GIB):
            for profile in policy.STORAGE_PROFILES:
                for percent in range(80, 91):
                    for active in ("0", "1"):
                        for phase in policy.PHASES:
                            with self.subTest(host=host, profile=profile, percent=percent, active=active, phase=phase):
                                env = dict(USDB_STORAGE_PROFILE=profile, USDB_RESOURCE_MEMORY_PERCENT=str(percent),
                                           USDB_MINTING_ENABLED=active, SNAPSHOT_MODE="assumeutxo")
                                plan = policy.build_resource_plan(host, phase, env)
                                pool = host - plan.reserve_bytes
                                self.assertLessEqual(plan.total_bytes, host)
                                self.assertLessEqual(pool, host * percent // 100)
                                self.assertGreaterEqual(plan.reserve_bytes, 4 * policy.GIB)
                                self.assertGreaterEqual(plan.limits["BH_MEMORY_LIMIT"], (4 if phase == "steady" else 8) * policy.GIB)
                                self.assertLessEqual(plan.dbcache_mib * policy.MIB, plan.limits["BTC_MEMORY_LIMIT"] // 2)
                                for key, cap in (("BH_MEMORY_LIMIT", "USDB_BH_MEMORY_CAP"),
                                    ("BTC_MEMORY_LIMIT", {"bitcoin": "USDB_BTC_IBD_MEMORY_CAP",
                                        "overlap": "USDB_BTC_OVERLAP_MEMORY_CAP", "steady": "USDB_BTC_STEADY_MEMORY_CAP"}[phase])):
                                    self.assertLessEqual(plan.limits[key], policy.memory_bytes(policy.resource_cap_defaults(env)[cap], cap))
                                policy.validate_resource_environment({**env, **plan.environment()}, host)

    def test_slow_disk_spends_more_on_core_without_growing_dbcache(self):
        for phase in policy.PHASES:
            env = dict(SNAPSHOT_MODE="assumeutxo", USDB_MINTING_ENABLED="1")
            normal = policy.build_resource_plan(32574320640, phase, dict(env, USDB_STORAGE_PROFILE="balanced"))
            slow = policy.build_resource_plan(32574320640, phase, dict(env, USDB_STORAGE_PROFILE="slow-disk"))
            self.assertGreaterEqual(slow.limits["BTC_MEMORY_LIMIT"], normal.limits["BTC_MEMORY_LIMIT"])
            self.assertEqual(slow.dbcache_mib, normal.dbcache_mib)
        self.assertEqual(slow.limits["ORD_MEMORY_LIMIT"], 4 * policy.GIB)
        self.assertFalse(slow.ord_deferred)

    def test_external_reservations_and_caps_leave_unused_memory(self):
        env = dict(USDB_STORAGE_PROFILE="slow-disk", USDB_EXTERNAL_MEMORY_BUDGET="16g",
                   USDB_RESOURCE_MEMORY_PERCENT="85", USDB_MINTING_ENABLED="1", SNAPSHOT_MODE="assumeutxo",
                   USDB_BH_MEMORY_CAP="8g", USDB_BTC_IBD_MEMORY_CAP="16g",
                   USDB_BTC_OVERLAP_MEMORY_CAP="8g", USDB_BTC_STEADY_MEMORY_CAP="8g", USDB_ORD_MEMORY_CAP="4g")
        for phase in policy.PHASES:
            plan = policy.build_resource_plan(128 * policy.GIB, phase, env)
            self.assertLess(plan.total_bytes, 100 * policy.GIB)
            self.assertEqual(plan.external_services_bytes, 16 * policy.GIB)
            self.assertLessEqual(128 * policy.GIB - plan.reserve_bytes - plan.external_services_bytes, 112 * policy.GIB * 85 // 100)
            policy.validate_resource_environment({**env, **plan.environment()}, 128 * policy.GIB)

    def test_invalid_policy_and_insufficient_phase_budgets_fail_closed(self):
        for overrides in (dict(USDB_STORAGE_PROFILE="auto"), dict(USDB_RESOURCE_MEMORY_PERCENT="91"),
                          dict(USDB_RESOURCE_MEMORY_PERCENT="0"), dict(USDB_RESOURCE_MEMORY_PERCENT="８５"),
                          dict(USDB_BH_MEMORY_CAP="4g"), dict(USDB_BTC_OVERLAP_MEMORY_CAP="1g"),
                          dict(USDB_EXTERNAL_MEMORY_BUDGET="8g"), dict(USDB_ORD_MEMORY_CAP="1g")):
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                policy.build_resource_plan(32 * policy.GIB, "overlap",
                    dict(USDB_STORAGE_PROFILE="slow-disk", USDB_MINTING_ENABLED="1", **overrides)
                    if "USDB_STORAGE_PROFILE" not in overrides else dict(overrides))
        with self.assertRaisesRegex(ValueError, "requires a storage profile"):
            policy.build_resource_plan(64 * policy.GIB, "steady", dict(USDB_RESOURCE_MEMORY_PERCENT="85"))

    def test_monitor_projection_records_strategy_without_secrets(self):
        env = policy.build_resource_plan(64 * policy.GIB, "overlap", dict(USDB_STORAGE_PROFILE="slow-disk",
            USDB_MINTING_ENABLED="1")).environment()
        report = metrics.configuration(dict(env, BTC_RPC_PASSWORD="secret"))
        self.assertEqual(report["storage_profile"], "slow-disk")
        self.assertEqual(report["memory_percent"], 90)
        self.assertTrue(report["ord_deferred"])
        self.assertNotIn("secret", str(report))


class StorageHintTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.sys = self.root / "sys"
        self.dev = self.sys / "dev/block"
        self.dev.mkdir(parents=True)
        self.target = self.dev / f"{os.major(self.root.stat().st_dev)}:{os.minor(self.root.stat().st_dev)}"

    def disk(self, name, flag):
        path = self.sys / "block" / name
        (path / "queue").mkdir(parents=True)
        (path / "queue/rotational").write_text(flag)
        return path

    def test_partition_and_nonexistent_child_follow_actual_filesystem(self):
        disk = self.disk("sda", "1")
        partition = disk / "sda1"
        partition.mkdir()
        (partition / "partition").write_text("1")
        self.target.symlink_to(partition)
        hint = storage.storage_hint(self.root / "new/data", self.sys)
        self.assertEqual(hint["recommended_profile"], "slow-disk")
        self.assertEqual(hint["devices"], ["sda"])

    def test_mapper_with_mixed_slaves_selects_slow_disk(self):
        mapper = self.sys / "block/dm-0/slaves"
        mapper.mkdir(parents=True)
        for name, flag in (("sda", "1"), ("nvme0n1", "0"), ("missing", "?")):
            (mapper / name).symlink_to(self.disk(name, flag))
        self.target.symlink_to(mapper.parent)
        self.assertEqual(storage.storage_hint(self.root, self.sys)["recommended_profile"], "slow-disk")

    def test_unknown_is_not_reported_as_fast_and_ssd_selects_balanced(self):
        self.assertEqual(storage.storage_hint(self.root, self.sys)["kind"], "unknown")
        self.target.symlink_to(self.disk("nvme0n1", "0"))
        hint = storage.storage_hint(self.root, self.sys)
        self.assertEqual(hint["kind"], "non-rotational")
        self.assertEqual(hint["recommended_profile"], "balanced")


class ProfileConfigurationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        for patcher in (mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB),
                        mock.patch.object(node, "_validate_data_root_capacity"),
                        mock.patch.object(node, "_collect_compose_services", return_value={}),
                        mock.patch.object(minting, "check_disk_capacity", return_value=dict(new_index=False))):
            patcher.start()
            self.addCleanup(patcher.stop)

    def configure(self, caps=None, active=False):
        node.configure_node(self.layout, data_root=self.root / "data", role="full", miner_address="", miner_threads=1,
                            bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private",
                            resource_management="auto", resource_caps=caps, minting=active)

    def test_auto_resolves_once_and_subsequent_recalculation_retains_profile(self):
        with mock.patch.object(storage, "storage_hint", return_value=dict(recommended_profile="slow-disk")):
            self.configure(dict(USDB_STORAGE_PROFILE="auto", USDB_RESOURCE_MEMORY_PERCENT="85"), active=True)
        before = node.read_env(self.layout.node_env)
        self.assertEqual(before["USDB_STORAGE_PROFILE"], "slow-disk")
        self.assertEqual(before["ORD_STARTUP_DEFERRED"], "1")
        minting.validate(before)
        with mock.patch.object(storage, "storage_hint", side_effect=AssertionError("unexpected disk re-detection")):
            node.set_resource_policy(self.layout, "auto", {})
        self.assertEqual(before, node.read_env(self.layout.node_env))

    def test_old_policy_is_retained_until_explicit_profile_selection(self):
        self.configure()
        before = node.read_env(self.layout.node_env)
        node.set_resource_policy(self.layout, "auto", {})
        self.assertEqual(before, node.read_env(self.layout.node_env))
        node.set_resource_policy(self.layout, "auto", dict(USDB_STORAGE_PROFILE="slow-disk", USDB_RESOURCE_MEMORY_PERCENT="80"))
        after = node.read_env(self.layout.node_env)
        self.assertEqual(after["USDB_RESOURCE_PHASE"], before["USDB_RESOURCE_PHASE"])
        self.assertEqual(after["BTC_RPC_PASSWORD"], before["BTC_RPC_PASSWORD"])
        self.assertEqual(after["USDB_BTC_IBD_MEMORY_CAP"], "64g")
        policy.validate_resource_environment(after, 64 * policy.GIB)

    def test_first_setup_detects_disk_and_reprompts_invalid_percentage(self):
        output = io.StringIO()
        answers = iter(["99", "85"])
        def answer(prompt):
            return next(answers) if prompt.startswith("Node memory budget percent") else ""
        args = node.build_parser().parse_args(["setup", "--p2p-ip-family", "ipv4", "--advertise-ipv4", V4])
        with mock.patch.object(storage, "storage_hint", return_value=dict(kind="rotational", devices=["sda"], recommended_profile="slow-disk")), \
             mock.patch.object(node, "_select_data_root", return_value=self.root / "data"), \
             mock.patch.object(node, "_host_memory_bytes", return_value=64 * policy.GIB), \
             mock.patch.object(p2p, "host_capabilities", return_value=HOST):
            node.setup_node(self.layout, resource_management="auto", input_fn=answer, output=output, p2p_options=p2p.options(args))
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["USDB_STORAGE_PROFILE"], "slow-disk")
        self.assertEqual(env["USDB_RESOURCE_MEMORY_PERCENT"], "85")
        self.assertIn("Enter an integer from 80 to 90", output.getvalue())
        self.assertIn("rotational", output.getvalue())

    def test_existing_setup_can_opt_in_then_enter_preserves_everything(self):
        self.configure()
        output = io.StringIO()
        def answer(prompt):
            if prompt.startswith("Storage resource profile"):
                return "slow-disk"
            if prompt.startswith("Node memory budget percent"):
                return "85"
            return ""
        with mock.patch.object(node_monitor, "is_running", return_value=False):
            node.setup_node(self.layout, input_fn=answer, output=output)
            before = self.layout.node_env.read_bytes()
            node.setup_node(self.layout, input_fn=lambda _: "", output=output)
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_STORAGE_PROFILE"], "slow-disk")
        self.assertIn("No configuration changes", output.getvalue())

    def test_disabling_ord_releases_waiting_budget_and_manual_exit_does_not_keep_deferred_flag(self):
        self.configure(dict(USDB_STORAGE_PROFILE="slow-disk"), active=True)
        minting.configure(self.layout, False, node)
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["ORD_STARTUP_DEFERRED"], "0")
        self.assertNotIn("ORD_MEMORY_LIMIT", policy.build_resource_plan(64 * policy.GIB, "overlap", env).limits)
        minting.configure(self.layout, True, node)
        self.assertEqual(node.read_env(self.layout.node_env)["ORD_STARTUP_DEFERRED"], "1")
        node.set_resource_policy(self.layout, "manual", {})
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["ORD_STARTUP_DEFERRED"], "0")
        minting.validate(env)

    def test_switching_profile_preserves_custom_service_caps_and_external_reserve(self):
        self.configure(dict(USDB_BTC_OVERLAP_MEMORY_CAP="12g", USDB_BH_MEMORY_CAP="12g", USDB_EXTERNAL_MEMORY_BUDGET="8g"))
        node.set_resource_policy(self.layout, "auto", dict(USDB_STORAGE_PROFILE="slow-disk"))
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["USDB_BTC_OVERLAP_MEMORY_CAP"], "12g")
        self.assertEqual(env["USDB_BH_MEMORY_CAP"], "12g")
        self.assertEqual(int(env["USDB_EXTERNAL_MEMORY_BUDGET"]), 8 * policy.GIB)
        self.assertEqual(env["USDB_BTC_IBD_MEMORY_CAP"], "64g")
        node.set_resource_policy(self.layout, "auto", dict(USDB_STORAGE_PROFILE="balanced", USDB_BTC_OVERLAP_MEMORY_CAP="10g"))
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_BTC_OVERLAP_MEMORY_CAP"], "10g")

    def test_cli_defaults_and_explicit_caps_remain_reviewable(self):
        args = node.build_parser().parse_args(["configure", "--storage-profile", "slow-disk", "--memory-percent", "85"])
        self.assertEqual(node._resource_caps_from_args(args)["USDB_RESOURCE_MEMORY_PERCENT"], "85")
        self.configure(dict(USDB_STORAGE_PROFILE="slow-disk"))
        node.set_resource_policy(self.layout, "auto", dict(USDB_BTC_OVERLAP_MEMORY_CAP="12g"))
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_BTC_OVERLAP_MEMORY_CAP"], "12g")
        before = self.layout.node_env.read_bytes()
        with mock.patch.object(node, "_collect_compose_services", return_value={"btc-node": dict(state="running")}):
            with self.assertRaisesRegex(ValueError, "stop the node"):
                node.set_resource_policy(self.layout, "auto", dict(USDB_STORAGE_PROFILE="balanced"))
        self.assertEqual(before, self.layout.node_env.read_bytes())


class ProfileHandoffTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.r = NativeRuntime(self.root)
        r = self.r
        env = dict(node.read_env(r.layout.node_env), USDB_STORAGE_PROFILE="slow-disk", USDB_MINTING_ENABLED="1")
        env.update(policy.build_resource_plan(r.memory, "bitcoin", env).environment())
        r.layout.node_env.write_text(node.upsert_env("", env))
        for patcher in (mock.patch.object(node, "effective_memory_bytes", return_value=r.memory),
                        mock.patch.object(node, "_resource_containers", side_effect=r.observed),
                        mock.patch.object(node, "run_helper", side_effect=r.helper),
                        mock.patch.object(node, "_read_service_readiness", side_effect=r.readiness),
                        mock.patch.object(node, "_runtime_lifecycle_status", return_value=dict(state="ready")),
                        mock.patch.object(node, "_print_startup_phase"), mock.patch.object(minting, "prepare"),
                        mock.patch.object(native.time, "monotonic", side_effect=lambda: r.tick),
                        mock.patch.object(native.time, "sleep", side_effect=r.sleep)):
            patcher.start()
            self.addCleanup(patcher.stop)
        r.advance = self.advance

    def advance(self):
        node._check_running_resource_budget(node.read_env(self.r.layout.node_env), self.r.containers)
        if "balance-history" in self.r.containers:
            self.r.ready, self.r.core["tip_ready"] = True, True

    def start(self):
        native.start_native_node(self.r.layout, sync_timeout_secs=60, output_to_stderr=False,
                                 progress_monitor=SimpleNamespace(set_phase=lambda _: None))

    def test_deferred_ord_graduates_only_after_steady_core_adoption(self):
        self.start()
        r = self.r
        self.assertFalse(r.core["history_validated"])
        self.assertIn(("up-chain", "steady"), r.events)
        self.assertEqual(r.containers["ord-server"]["environment"]["ORD_STARTUP_DEFERRED"], "0")
        self.assertEqual(r.containers["ord-server"]["memory"], 8 * policy.GIB)
        stop = r.events.index(("quiesce-ord", "overlap"))
        self.assertLess(stop, r.events.index(("up-ord", "steady")))
        self.assertLess(r.events.index(("start", "steady")), r.events.index(("up-ord", "steady")))
        self.assertFalse(node._read_resource_state(r.layout)["pending"])

    def test_interrupted_ord_handoff_recovers_without_starting_it_over_budget(self):
        self.r.crash = "quiesce-ord"
        with self.assertRaisesRegex(RuntimeError, "after quiesce-ord"):
            self.start()
        self.assertTrue(node._read_resource_state(self.r.layout)["pending"])
        self.assertEqual(self.r.containers["ord-server"]["state"], "exited")
        self.start()
        self.assertEqual(self.r.containers["ord-server"]["environment"]["ORD_STARTUP_DEFERRED"], "0")
        node._check_running_resource_budget(node.read_env(self.r.layout.node_env), self.r.containers)

    def test_small_ord_container_requires_live_defer_gate(self):
        env = node.read_env(self.r.layout.node_env)
        container = dict(state="running", memory=512 * policy.MIB, environment={})
        with self.assertRaisesRegex(ValueError, "waiting supervisor"):
            node._check_running_resource_budget(env, {"ord-server": container})

    def test_waiting_ord_never_probes_core_or_launches_indexer(self):
        with mock.patch.dict(os.environ, dict(ORD_STARTUP_DEFERRED="1", ORD_DATA_DIR=str(self.root),
                    BTC_RPC_USER="user", BTC_RPC_PASSWORD="password")), \
             mock.patch.object(ord_runtime.threading, "Event", return_value=Loop(2)), \
             mock.patch.object(ord_runtime.signal, "signal"), \
             mock.patch.object(ord_runtime, "observe_core") as core, \
             mock.patch.object(ord_runtime.subprocess, "Popen") as launch, \
             mock.patch.object(ord_runtime.shutil, "disk_usage", return_value=SimpleNamespace(free=100 * policy.GIB)), \
             mock.patch.object(ord_runtime, "publish") as publish, mock.patch("builtins.print"):
            self.assertEqual(ord_runtime.supervise(), 0)
        core.assert_not_called()
        launch.assert_not_called()
        self.assertEqual(publish.call_args_list[0].args[1]["state"], "WAITING_RESOURCES")


if __name__ == "__main__":
    unittest.main()
