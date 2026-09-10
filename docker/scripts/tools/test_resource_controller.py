#!/usr/bin/env python3
"""Exercise resource transitions against an observed-container model and crash points."""

import copy
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

import resource_policy as POLICY
import usdb_node as NODE


class ResourceControllerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.layout = SimpleNamespace(node_env=Path(self.temp.name) / "node.env", bundle_id="test",
                                      network_identity={"btc_index_origin_height": 100})
        self.events = []
        self.containers = {}
        self.crash = None
        self.tick = 0
        self.memory = 64 * POLICY.GIB
        self.write_phase("bitcoin")
        self.containers["btc-node"] = self.container("btc-node")
        for patcher in [mock.patch.object(NODE, "effective_memory_bytes", side_effect=lambda: self.memory),
                        mock.patch.object(NODE, "_resource_containers", side_effect=lambda layout: copy.deepcopy(self.containers)),
                        mock.patch.object(NODE, "run_helper", side_effect=self.helper),
                        mock.patch.object(NODE, "_print_startup_phase")]:
            patcher.start()
            self.addCleanup(patcher.stop)

    def write_phase(self, phase):
        env = {**POLICY.build_resource_plan(self.memory, phase, {}).environment(),
               "USDB_BITCOIN_IMAGE": "bitcoin@sha256:1", "USDB_SERVICES_IMAGE": "services@sha256:2"}
        self.layout.node_env.write_text("".join(f"{key}={value}\n" for key, value in env.items()))

    def container(self, service):
        env = NODE.read_env(self.layout.node_env)
        return {"state": "running", "exit_code": 0,
                "memory": int(env[POLICY.SERVICE_MEMORY_KEYS[service]]),
                "swap": int(env["BTC_MEMORY_SWAP_LIMIT"] if service == "btc-node" else env["BH_MEMORY_SWAP_LIMIT"]),
                "image": env["USDB_BITCOIN_IMAGE"] if service == "btc-node" else env["USDB_SERVICES_IMAGE"],
                "environment": env}

    def helper(self, layout, helper, arguments, **kwargs):
        action = arguments[0]
        self.events.append((action, NODE.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"]))
        if action == "quiesce-data":
            for name in ("balance-history", "usdb-indexer", "usdb-chain", "usdb-control-plane"):
                if name in self.containers:
                    self.containers[name]["state"] = "exited"
        elif action == "down":
            self.containers.pop("btc-node", None)
        elif action == "start":
            self.containers["btc-node"] = self.container("btc-node")
        elif action == "managed-start-snapshot":
            self.containers["snapshot-loader"] = self.container("snapshot-loader")
        elif action == "managed-start-data":
            self.assertEqual(self.containers["snapshot-loader"]["state"], "exited")
            self.containers["balance-history"] = self.container("balance-history")
        elif action == "up-indexer":
            self.containers["usdb-indexer"] = self.container("usdb-indexer")
        elif action == "up-chain":
            for name in ("usdb-chain", "usdb-control-plane"):
                self.containers[name] = self.container(name)
        elif action != "wait-data-origin":
            raise AssertionError(f"unexpected helper action: {action}")
        if self.crash == action:
            self.crash = None
            raise RuntimeError(f"simulated interruption after {action}")
        return subprocess.CompletedProcess(arguments, 0, "", "")

    def transition(self, target):
        NODE._transition_resources(self.layout, target, output_to_stderr=False)

    def test_releases_bitcoin_before_new_allocation_and_records_observed_success(self):
        self.transition("overlap")
        self.assertEqual(self.events, [("quiesce-data", "bitcoin"), ("down", "bitcoin"), ("start", "overlap")])
        self.assertEqual(self.containers["btc-node"]["memory"], 16 * POLICY.GIB)
        state = NODE._read_resource_state(self.layout)
        self.assertFalse(state["pending"])
        self.assertEqual(state["phase"], "overlap")

    def test_interruption_after_stop_preserves_old_config_and_resumes(self):
        self.crash = "down"
        with self.assertRaisesRegex(RuntimeError, "after down"):
            self.transition("overlap")
        self.assertEqual(NODE.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"], "bitcoin")
        self.assertTrue(NODE._read_resource_state(self.layout)["pending"])
        self.transition("overlap")
        self.assertEqual(sum(action == "start" for action, _ in self.events), 1)
        self.assertFalse(NODE._read_resource_state(self.layout)["pending"])

    def test_interruption_after_start_does_not_restart_adopted_container_again(self):
        self.crash = "start"
        with self.assertRaises(RuntimeError):
            self.transition("overlap")
        before = list(self.events)
        self.transition("overlap")
        self.assertEqual(self.events, before)
        self.assertFalse(NODE._read_resource_state(self.layout)["pending"])

    def test_failed_quiesce_never_changes_config_or_starts_new_container(self):
        self.crash = "quiesce-data"
        before = self.layout.node_env.read_bytes()
        with self.assertRaises(RuntimeError):
            self.transition("overlap")
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertEqual(self.events, [("quiesce-data", "bitcoin")])

    def test_stale_docker_limits_cannot_commit_a_transition(self):
        original = self.helper

        def stale(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[2][0] == "start":
                self.containers["btc-node"]["memory"] = 32 * POLICY.GIB
            return result

        with mock.patch.object(NODE, "run_helper", side_effect=stale), self.assertRaisesRegex(ValueError, "did not adopt"):
            self.transition("overlap")
        self.assertTrue(NODE._read_resource_state(self.layout)["pending"])

    def test_steady_transition_preserves_import_and_quiesces_consumers(self):
        self.write_phase("overlap")
        self.containers = {name: self.container(name) for name in ("btc-node", "balance-history", "usdb-indexer")}
        self.transition("steady")
        self.assertEqual(self.containers["balance-history"]["state"], "exited")
        self.assertEqual(self.containers["btc-node"]["memory"], 8 * POLICY.GIB)
        self.assertEqual(NODE._read_resource_state(self.layout)["recover_services"], ["balance-history", "usdb-indexer"])
        self.write_phase("overlap")
        self.containers = {name: self.container(name) for name in ("btc-node", "snapshot-loader")}
        self.transition("steady")
        self.assertEqual(self.containers["snapshot-loader"]["state"], "running")
        self.assertEqual(self.containers["snapshot-loader"]["memory"], 24 * POLICY.GIB)

    def test_actual_overlap_including_both_loader_and_indexer_is_checked(self):
        self.write_phase("overlap")
        env = NODE.read_env(self.layout.node_env)
        containers = {name: self.container(name) for name in ("btc-node", "snapshot-loader", "balance-history")}
        with self.assertRaisesRegex(ValueError, "concurrent"):
            NODE._check_running_resource_budget(env, containers)

    def test_bitcoin_boost_rejects_active_downstream_before_any_mutation(self):
        original = self.layout.node_env.read_bytes()
        for service in POLICY.SERVICE_MEMORY_KEYS:
            if service == "btc-node":
                continue
            for state in ("running", "restarting", "paused"):
                with self.subTest(service=service, state=state):
                    self.containers = {"btc-node": self.container("btc-node"),
                                       service: {**self.container(service), "state": state}}
                    with self.assertRaisesRegex(ValueError, "exclusive Bitcoin memory phase"):
                        self.transition("bitcoin")
                    self.assertEqual(self.events, [])
                    self.assertEqual(self.layout.node_env.read_bytes(), original)
                    self.assertFalse(NODE._resource_state_path(self.layout).exists())

    def test_bitcoin_boost_allows_completed_downstream_containers(self):
        self.containers["snapshot-loader"] = {**self.container("snapshot-loader"), "state": "exited"}
        self.transition("bitcoin")
        self.assertEqual(self.events, [])
        self.assertFalse(NODE._read_resource_state(self.layout)["pending"])

    def test_progress_waits_for_final_handoff_without_hiding_real_failures(self):
        self.layout.release_id = "test-release"
        services = {name: {"state": "running"} for name in NODE.CORE_RUNTIME_SERVICES}

        def component(name):
            return NODE._component_progress(name, "READY", "ready")

        with (
            mock.patch.object(NODE, "_mining_status", return_value={"state": "DISABLED", "applied": True}),
            mock.patch.object(NODE, "controller_observed_state", return_value="active"),
            mock.patch.object(NODE, "_collect_compose_services", return_value=services),
            mock.patch.object(NODE, "_snapshot_lifecycle_status", return_value={}),
            mock.patch.object(NODE, "_snapshot_component", side_effect=lambda *args: component("snapshot")),
            mock.patch.object(NODE, "_script_registry_component", side_effect=lambda *args: component("script_registry")),
            mock.patch.object(NODE, "_failed_container_component", side_effect=lambda *args: component("bitcoin")),
            mock.patch.object(NODE, "_balance_history_component", side_effect=lambda *args: component("balance_history")),
            mock.patch.object(NODE, "_indexed_service_component", side_effect=lambda *args: component("usdb_indexer")),
            mock.patch.object(NODE, "_chain_component", side_effect=lambda *args: component("usdb_chain")) as chain,
            mock.patch.object(NODE, "_read_service_readiness", return_value=(None, None)),
            mock.patch.object(NODE, "_bitcoin_data_start_anchor", side_effect=ValueError("not needed for ready services")),
        ):
            for phase, pending, recovering, expected in (
                ("overlap", False, [], "STARTING"),
                ("steady", True, [], "STARTING"),
                ("steady", False, ["usdb-chain"], "STARTING"),
                ("steady", False, [], "READY"),
            ):
                with self.subTest(phase=phase, pending=pending, recovering=recovering):
                    self.write_phase(phase)
                    self.containers = {name: self.container(name) for name in ("btc-node", "balance-history")}
                    NODE._write_resource_state(self.layout, {
                        "schema_version": "usdb-resource-state:v1", "bundle_id": "test",
                        "phase": phase, "pending": pending, "recover_services": recovering,
                    })
                    report = NODE.collect_node_progress(self.layout)
                    self.assertEqual(report["overall_state"], expected)
            self.containers["btc-node"]["memory"] = 16 * POLICY.GIB
            report = NODE.collect_node_progress(self.layout)
            self.assertEqual(report["overall_state"], "STARTING")
            self.assertFalse(report["resources"]["runtime_adopted"])
            self.write_phase("overlap")
            chain.side_effect = lambda *args: NODE._component_progress("usdb_chain", "FAILED", "process failed")
            self.assertEqual(NODE.collect_node_progress(self.layout)["overall_state"], "FAILED")

    def test_chain_restart_intent_survives_until_each_service_has_started(self):
        self.write_phase("steady")
        NODE._resource_prepare_restart(self.layout, "usdb-chain", "usdb-control-plane")
        state = NODE._read_resource_state(self.layout)
        self.assertEqual(state["recover_services"], ["usdb-chain", "usdb-control-plane"])
        self.assertFalse(state["pending"])
        NODE._resource_service_started(self.layout, "usdb-chain")
        self.assertEqual(NODE._read_resource_state(self.layout)["recover_services"], ["usdb-control-plane"])

    def test_cannot_change_release_mid_transition_or_move_phase_backwards(self):
        self.crash = "down"
        with self.assertRaises(RuntimeError):
            self.transition("overlap")
        self.layout.node_env.write_text(self.layout.node_env.read_text().replace("bitcoin@sha256:1", "bitcoin@sha256:3"))
        with self.assertRaisesRegex(ValueError, "changed during"):
            self.transition("overlap")
        self.write_phase("steady")
        with self.assertRaisesRegex(ValueError, "backwards"):
            self.transition("overlap")

    def run_startup_model(self, already_synced=False):
        def advance(_seconds):
            self.tick += 1
            if self.tick >= 6 and "snapshot-loader" in self.containers:
                self.containers["snapshot-loader"]["state"] = "exited"
        def readiness(layout, helper, arguments, service):
            return ({"service": service, "consensus_ready": True}, None) if service in self.containers else (None, "waiting")
        with mock.patch.object(NODE, "_bitcoin_startup_progress", side_effect=lambda *a, **k: {"ready": already_synced or self.tick >= 2}), \
             mock.patch.object(NODE, "_bitcoin_data_start_anchor", return_value=NODE.BitcoinDataStartAnchor(100, 110, 10, None)), \
             mock.patch.object(NODE, "_managed_data_ready", return_value=True), \
             mock.patch.object(NODE, "_read_service_readiness", side_effect=readiness), \
             mock.patch.object(NODE, "_runtime_lifecycle_status", return_value={"state": "ready"}), \
             mock.patch.object(NODE.time, "sleep", side_effect=advance):
            NODE._start_managed_node(self.layout, sync_timeout_secs=60, output_to_stderr=False, progress_monitor=mock.Mock())

    def test_long_snapshot_import_does_not_block_bitcoin_demotion(self):
        self.run_startup_model()
        starts = [(action, phase) for action, phase in self.events if action in {"start", "managed-start-snapshot", "managed-start-data"}]
        self.assertEqual(starts, [("start", "overlap"), ("managed-start-snapshot", "overlap"),
                                  ("start", "steady"), ("managed-start-data", "steady")])
        self.assertEqual(self.containers["balance-history"]["memory"], 32 * POLICY.GIB)

    def test_already_synced_bitcoin_skips_overlap_after_stable_observations(self):
        self.run_startup_model(already_synced=True)
        self.assertNotIn(("start", "overlap"), self.events)
        self.assertIn(("start", "steady"), self.events)

    def test_boosted_small_host_releases_memory_before_each_downstream_stage(self):
        self.memory = 32 * POLICY.GIB
        self.write_phase("bitcoin")
        self.containers = {"btc-node": self.container("btc-node")}
        self.assertEqual(self.containers["btc-node"]["memory"], 26214 * POLICY.MIB)
        original = self.helper
        observed_allocations = []

        def checked(*args, **kwargs):
            result = original(*args, **kwargs)
            env = NODE.read_env(self.layout.node_env)
            NODE._check_running_resource_budget(env, self.containers)
            if args[2][0] == "start":
                observed_allocations.append(self.containers["btc-node"]["memory"])
            return result

        with mock.patch.object(NODE, "run_helper", side_effect=checked):
            self.run_startup_model()
        self.assertEqual(observed_allocations, [8 * POLICY.GIB, 4 * POLICY.GIB])
        self.assertEqual(NODE.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"], "steady")


if __name__ == "__main__":
    unittest.main()
