"""Sixteen-GB resume, truthful completion and interrupted runtime resource handoffs."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import assumeutxo_node
import bitcoin_assumeutxo
import node_resource_lifecycle as lifecycle
import node_resource_metrics as metrics
import resource_policy as policy
import usdb_minting
import usdb_node as node
import test_resource_runner as runner_fixture
from common.runtime_resources import RuntimeResources


class RuntimeBudgetTests(unittest.TestCase):
    def test_core_catchup_uses_spare_capacity_without_growing_caches_or_other_services(self):
        for memory in (16_000_000_000, 21999050752, 32 * policy.GIB, 64 * policy.GIB):
            for active in ("0", "1"):
                for percent in ("80", "90"):
                    for profile in policy.STORAGE_PROFILES:
                        with self.subTest(memory=memory, ord=active, percent=percent, profile=profile):
                            env = dict(USDB_RESOURCE_POLICY_VERSION="2", SNAPSHOT_MODE="assumeutxo",
                                       USDB_STORAGE_PROFILE=profile, USDB_RESOURCE_MEMORY_PERCENT=percent,
                                       USDB_MINTING_ENABLED=active)
                            if memory == 16_000_000_000 and active == "1" and percent == "80":
                                for budget in ("compact", "core-catchup", "runtime-ready"):
                                    with self.assertRaisesRegex(ValueError, "minima"):
                                        policy.build_resource_plan(memory, "catchup", dict(env, USDB_RESOURCE_RUNTIME_BUDGET=budget))
                                continue
                            compact = policy.build_resource_plan(memory, "steady", env)
                            catchup = policy.build_resource_plan(memory, "catchup", dict(env, USDB_RESOURCE_RUNTIME_BUDGET="core-catchup"))
                            ready = policy.build_resource_plan(memory, "catchup", dict(env, USDB_RESOURCE_RUNTIME_BUDGET="runtime-ready"))
                            self.assertLessEqual(catchup.total_bytes, memory)
                            self.assertGreaterEqual(catchup.limits["BTC_MEMORY_LIMIT"], compact.limits["BTC_MEMORY_LIMIT"])
                            self.assertLessEqual(catchup.limits["BTC_MEMORY_LIMIT"], 8 * policy.GIB)
                            self.assertEqual(catchup.dbcache_mib, compact.dbcache_mib)
                            self.assertEqual(catchup.reserve_bytes, compact.reserve_bytes)
                            for key in compact.limits:
                                if key != "BTC_MEMORY_LIMIT":
                                    self.assertEqual(catchup.limits[key], compact.limits[key])
                            self.assertEqual(ready.limits, compact.limits)
                            for plan in (catchup, ready):
                                policy.validate_resource_environment({**env, **plan.environment()}, memory)
                                self.assertEqual(metrics.configuration(plan.environment())["runtime_budget"], plan.runtime_budget)

    def test_node3_budget_preserves_external_reserve_and_explicit_core_cap(self):
        env = dict(USDB_RESOURCE_POLICY_VERSION="2", USDB_RESOURCE_RUNTIME_BUDGET="core-catchup",
                   SNAPSHOT_MODE="assumeutxo", USDB_STORAGE_PROFILE="slow-disk", USDB_MINTING_ENABLED="0")
        host = 21999050752
        plan = policy.build_resource_plan(host, "catchup", env)
        self.assertEqual(plan.limits["BTC_MEMORY_LIMIT"], 8 * policy.GIB)
        self.assertEqual(plan.dbcache_mib, 655)
        for extra in ({"USDB_BTC_STEADY_MEMORY_CAP": "3g"}, {"USDB_EXTERNAL_MEMORY_BUDGET": "3g"}):
            capped = policy.build_resource_plan(host, "catchup", dict(env, **extra))
            self.assertLess(capped.limits["BTC_MEMORY_LIMIT"], plan.limits["BTC_MEMORY_LIMIT"])
            self.assertLessEqual(capped.total_bytes, host)
            if "USDB_EXTERNAL_MEMORY_BUDGET" in extra:
                self.assertEqual(capped.external_services_bytes, 3 * policy.GIB)
            else:
                self.assertEqual(capped.limits["BTC_MEMORY_LIMIT"], 3 * policy.GIB)

    def test_runtime_matrix_including_nominal_sixteen_gb_and_ord(self):
        for memory in (16_000_000_000, 16 * policy.GIB, 20 * policy.GIB, 32 * policy.GIB, 64 * policy.GIB):
            for profile in policy.STORAGE_PROFILES:
                for ord_enabled in ("0", "1"):
                    for phase in ("catchup", "steady"):
                        with self.subTest(memory=memory, profile=profile, ord=ord_enabled, phase=phase):
                            env = dict(USDB_RESOURCE_POLICY_VERSION="2", USDB_RESOURCE_PHASE=phase,
                                       USDB_STORAGE_PROFILE=profile, USDB_MINTING_ENABLED=ord_enabled,
                                       USDB_ORD_RESOURCE_POLICY="adaptive-v1", SNAPSHOT_MODE="assumeutxo")
                            plan = policy.build_resource_plan(memory, phase, env)
                            policy.validate_resource_environment({**env, **plan.environment()}, memory)
                            self.assertLessEqual(plan.total_bytes, memory)
                            self.assertGreaterEqual(plan.limits["USDB_CHAIN_MEMORY_LIMIT"], 2 * policy.GIB)
                            self.assertGreaterEqual(plan.reserve_bytes, 2 * policy.GIB)
                            self.assertFalse(plan.ord_deferred)

    def test_small_runtime_is_not_permission_for_fresh_bootstrap(self):
        env = dict(USDB_RESOURCE_POLICY_VERSION="2", SNAPSHOT_MODE="assumeutxo")
        for phase in ("bitcoin", "overlap"):
            with self.assertRaisesRegex(ValueError, "32 GB"):
                policy.build_resource_plan(16 * policy.GIB, phase, env)
        with self.assertRaisesRegex(ValueError, "16 GB"):
            policy.build_resource_plan(15_000_000_000, "steady", env)
        with self.assertRaisesRegex(ValueError, "reserve"):
            policy.build_resource_plan(16 * policy.GIB, "steady", dict(env, USDB_EXTERNAL_MEMORY_BUDGET="8g"))

    def test_steady_releases_capacity_and_resume_keeps_compact_limits(self):
        env = dict(USDB_RESOURCE_POLICY_VERSION="2", SNAPSHOT_MODE="assumeutxo",
                   USDB_STORAGE_PROFILE="slow-disk", USDB_MINTING_ENABLED="1", USDB_ORD_RESOURCE_POLICY="adaptive-v1")
        catchup = policy.build_resource_plan(64 * policy.GIB, "catchup", env)
        steady = policy.build_resource_plan(64 * policy.GIB, "steady", env)
        resumed = policy.build_resource_plan(64 * policy.GIB, "catchup", {**env, **steady.environment()})
        self.assertLess(steady.total_bytes, catchup.total_bytes)
        self.assertEqual(resumed.limits, steady.limits)
        self.assertEqual(resumed.dbcache_mib, steady.dbcache_mib)
        self.assertEqual(policy.resource_phases({**env, **steady.environment()}), ("catchup", "steady"))

    def test_explicit_caps_and_cache_guards_survive_runtime_rebudget(self):
        env = dict(USDB_RESOURCE_POLICY_VERSION="2", USDB_RESOURCE_PHASE="steady")
        for key in ("USDB_BH_MEMORY_CAP", "USDB_BTC_STEADY_MEMORY_CAP"):
            with self.assertRaisesRegex(ValueError, "minimum"):
                policy.build_resource_plan(32 * policy.GIB, "steady", dict(env, **{key: "1g"}))
        plan = policy.build_resource_plan(16 * policy.GIB, "steady", env)
        invalid = dict(env, **plan.environment())
        invalid["BH_SYNC_BALANCE_MAX_CACHE_BYTES"] = str(8 * policy.GIB)
        with self.assertRaisesRegex(ValueError, "cache budget"):
            policy.validate_resource_environment(invalid, 16 * policy.GIB)

    def test_core_completion_releases_bitcoin_without_reducing_ord_budget(self):
        env = dict(USDB_RESOURCE_POLICY_VERSION="2", SNAPSHOT_MODE="assumeutxo",
                   USDB_STORAGE_PROFILE="slow-disk", USDB_MINTING_ENABLED="1", USDB_ORD_RESOURCE_POLICY="adaptive-v1")
        before = policy.build_resource_plan(64 * policy.GIB, "catchup", env)
        after = policy.build_resource_plan(64 * policy.GIB, "catchup", dict(env, USDB_RESOURCE_RUNTIME_BUDGET="core-ready"))
        self.assertLess(after.limits["BTC_MEMORY_LIMIT"], before.limits["BTC_MEMORY_LIMIT"])
        for key in before.limits:
            if key != "BTC_MEMORY_LIMIT":
                self.assertEqual(after.limits[key], before.limits[key])
        self.assertLess(after.dbcache_mib, before.dbcache_mib)
        self.assertEqual(after.environment()["USDB_RESOURCE_RUNTIME_BUDGET"], "core-ready")


class RuntimeHandoffTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_stopped_legacy_node_can_resume_after_shrink_without_trusting_steady(self):
        f = RuntimeResources(self.root)
        env = dict(node.read_env(f.layout.node_env), USDB_RESOURCE_POLICY_VERSION="1")
        env.update(policy.build_resource_plan(32 * policy.GIB, "steady", env).environment())
        f.layout.node_env.write_text(node.upsert_env("", env))
        for item in f.containers.values():
            item["state"] = "exited"
        f.memory = 16_000_000_000
        with f.patches():
            self.assertTrue(lifecycle.prepare_for_up(f.layout, node))
            updated = node.read_env(f.layout.node_env)
            self.assertEqual(updated["USDB_RESOURCE_PHASE"], "catchup")
            self.assertEqual(updated["USDB_RESOURCE_HOST_MEMORY_BYTES"], str(f.memory))
            self.assertEqual(updated["USDB_RESOURCE_RUNTIME_BUDGET"], "core-catchup")
            policy.validate_resource_environment(updated, f.memory)
            self.assertFalse(f.events)

    def test_r19_compact_catchup_migrates_running_node_with_recoverable_stops(self):
        f = RuntimeResources(self.root, memory=21999050752, runtime_budget="compact")
        before = node.read_env(f.layout.node_env)
        with f.patches():
            self.assertTrue(lifecycle.prepare_for_up(f.layout, node))
            after = node.read_env(f.layout.node_env)
            self.assertEqual(after["USDB_RESOURCE_RUNTIME_BUDGET"], "core-catchup")
            self.assertEqual(after["BTC_DBCACHE_MB"], before["BTC_DBCACHE_MB"])
            self.assertEqual(int(after["BTC_MEMORY_LIMIT"]), 8 * policy.GIB)
            self.assertEqual(f.events[0][0], "quiesce-services")
            self.assertNotIn("usdb-control-plane", f.events[0][1])
            self.assertEqual(sum(event[0] == "start" for event in f.events), 1)
            self.assertIn("balance-history", node._read_resource_state(f.layout)["recover_services"])
            self.assertFalse(lifecycle.prepare_for_up(f.layout, node))

    def test_stopped_compact_node_promotes_but_runtime_ready_does_not_repeat_promotion(self):
        f = RuntimeResources(self.root, memory=21999050752, runtime_budget="compact")
        for item in f.containers.values():
            item["state"] = "exited"
        with f.patches():
            self.assertTrue(lifecycle.prepare_for_up(f.layout, node))
            self.assertFalse(f.events)
            self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_RUNTIME_BUDGET"], "core-catchup")
            lifecycle.apply_plan(f.layout, node, "catchup", runtime_budget="runtime-ready")
            self.assertFalse(lifecycle.prepare_for_up(f.layout, node))
            for item in f.containers.values():
                item["state"] = "exited"
            self.assertTrue(lifecycle.prepare_for_up(f.layout, node))
            self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_RUNTIME_BUDGET"], "core-catchup")

    def test_core_catchup_release_restarts_core_dependents_and_preserves_ord(self):
        f = RuntimeResources(self.root, memory=21999050752, ord_enabled=True, runtime_budget="core-catchup")
        with f.patches():
            lifecycle.apply_plan(f.layout, node, "catchup", runtime_budget="runtime-ready")
            self.assertNotIn("ord-server", f.events[0][1])
            self.assertNotIn("usdb-control-plane", f.events[0][1])
            env = node.read_env(f.layout.node_env)
            self.assertEqual(env["USDB_RESOURCE_RUNTIME_BUDGET"], "runtime-ready")
            self.assertEqual(int(env["BTC_MEMORY_LIMIT"]), 2749366272)

    def test_steady_large_lag_reacquires_core_catchup_budget(self):
        f = RuntimeResources(self.root, memory=21999050752, phase="steady")
        with f.patches():
            lifecycle.apply_plan(f.layout, node, "catchup")
            env = node.read_env(f.layout.node_env)
            self.assertEqual(env["USDB_RESOURCE_RUNTIME_BUDGET"], "core-catchup")
            self.assertEqual(int(env["BTC_MEMORY_LIMIT"]), 8 * policy.GIB)

    def test_running_shrink_and_pending_transition_are_not_rewritten(self):
        f = RuntimeResources(self.root, phase="steady")
        before = f.layout.node_env.read_bytes()
        f.memory = 16 * policy.GIB
        with f.patches(), self.assertRaisesRegex(ValueError, "containers are active"):
            lifecycle.prepare_for_up(f.layout, node)
        self.assertEqual(before, f.layout.node_env.read_bytes())
        for item in f.containers.values():
            item["state"] = "exited"
        node._write_resource_state(f.layout, dict(schema_version="usdb-resource-state:v1",
            bundle_id=f.layout.bundle_id, phase="steady", pending=True, recover_services=[]))
        with f.patches(), self.assertRaisesRegex(ValueError, "pending resource transition"):
            lifecycle.prepare_for_up(f.layout, node)
        self.assertEqual(before, f.layout.node_env.read_bytes())

    def test_small_catchup_to_steady_does_not_restart_unchanged_services(self):
        f = RuntimeResources(self.root, memory=16_000_000_000, ord_enabled=True)
        with f.patches():
            lifecycle.apply_plan(f.layout, node, "steady")
            self.assertEqual(f.events, [])
            self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_PHASE"], "steady")
            self.assertFalse(node._read_resource_state(f.layout)["pending"])

    def test_interrupted_stop_and_start_resume_from_actual_containers(self):
        for failure in ("quiesce-services", "start"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                f = RuntimeResources(Path(directory), memory=64 * policy.GIB)
                with f.patches():
                    f.crash = failure
                    with self.assertRaises(RuntimeError):
                        lifecycle.apply_plan(f.layout, node, "steady")
                    self.assertTrue(node._read_resource_state(f.layout)["pending"])
                    lifecycle.apply_plan(f.layout, node, "steady")
                    self.assertFalse(node._read_resource_state(f.layout)["pending"])
                    starts = sum(event[0] == "start" for event in f.events)
                    self.assertEqual(starts, 1)

    def test_up_preserves_pending_steady_plan_when_host_capacity_is_unchanged(self):
        f = RuntimeResources(self.root, memory=64 * policy.GIB)
        with f.patches():
            f.crash = "start"
            with self.assertRaises(RuntimeError):
                lifecycle.apply_plan(f.layout, node, "steady")
            for item in f.containers.values():
                item["state"] = "exited"
            before = f.layout.node_env.read_bytes()
            self.assertFalse(lifecycle.prepare_for_up(f.layout, node))
            self.assertEqual(before, f.layout.node_env.read_bytes())
            self.assertTrue(node._read_resource_state(f.layout)["pending"])
            lifecycle.apply_plan(f.layout, node, "steady")
            self.assertFalse(node._read_resource_state(f.layout)["pending"])

    def test_active_preparation_blocks_demotion_without_stopping_anything(self):
        f = RuntimeResources(self.root)
        f.containers["btc-snapshot-bootstrap"] = f.container("btc-snapshot-bootstrap")
        with f.patches(), self.assertRaisesRegex(ValueError, "RESOURCE_JOB_ACTIVE"):
            lifecycle.apply_plan(f.layout, node, "steady")
        self.assertFalse(f.events)

    def test_partial_release_preserves_ord_and_resumes_after_env_was_written(self):
        f = RuntimeResources(self.root, memory=64 * policy.GIB, ord_enabled=True)
        before = dict(f.containers["ord-server"])
        with f.patches():
            f.crash = "start"
            with self.assertRaises(RuntimeError):
                lifecycle.apply_plan(f.layout, node, "catchup", runtime_budget="core-ready")
            lifecycle.apply_plan(f.layout, node, "catchup")
            self.assertEqual(f.containers["ord-server"], before)
            self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_RUNTIME_BUDGET"], "core-ready")
            self.assertEqual(sum(event[0] == "start" for event in f.events), 1)
            self.assertNotIn("ord-server", f.events[0][1])

    def test_native_chain_starts_during_catchup_without_waiting_for_history_or_ord(self):
        f = RuntimeResources(self.root)
        f.containers.pop("usdb-chain")
        f.core["history_validated"] = False
        with f.patches(), mock.patch.object(node, "_runtime_lifecycle_status", return_value={"state": "ready"}):
            assumeutxo_node.start_native_node(f.layout, sync_timeout_secs=10, output_to_stderr=False,
                                              progress_monitor=mock.Mock())
        self.assertIn("usdb-chain", f.containers)
        self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_PHASE"], "catchup")

    def test_native_bootstrap_waits_for_active_registry_before_transition(self):
        f = RuntimeResources(self.root, phase="overlap")
        f.containers["btc-snapshot-bootstrap"] = {**f.container("btc-snapshot-bootstrap"), "state": "exited"}
        f.containers["script-registry-installer"] = f.container("script-registry-installer")
        waiting = []
        def finish_job(_seconds):
            if not waiting:
                self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_PHASE"], "overlap")
                self.assertFalse(any(event[0] == "quiesce-services" for event in f.events))
                f.containers["script-registry-installer"]["state"] = "exited"
                waiting.append(True)
        with f.patches(), mock.patch.object(assumeutxo_node.time, "sleep", side_effect=finish_job), \
             mock.patch.object(node, "_runtime_lifecycle_status", return_value={"state": "ready"}):
            assumeutxo_node.start_native_node(f.layout, sync_timeout_secs=10, output_to_stderr=False,
                                              progress_monitor=mock.Mock())
        self.assertEqual(waiting, [True])
        self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_PHASE"], "catchup")

    def test_unknown_or_old_ord_report_cannot_establish_completion(self):
        f = RuntimeResources(self.root, ord_enabled=True)
        ready = dict(state="READY", backend_ready=True, ord_gap=0, observed_at_ms=1760000000000)
        with f.patches(), mock.patch.object(usdb_minting, "progress", return_value=ready):
            self.assertFalse(lifecycle.observations(f.layout, node)["known"])
        ready["observed_at_ms"] = 1792022400001
        with f.patches(), mock.patch.object(usdb_minting, "progress", return_value=ready):
            self.assertTrue(lifecycle.observations(f.layout, node)["complete"])
            f.core["history_validated"] = False
            self.assertFalse(lifecycle.observations(f.layout, node)["complete"])

    def test_controller_continues_after_ready_and_releases_lock_between_samples(self):
        f = RuntimeResources(self.root, memory=16 * policy.GIB)
        now = [0]
        def sleep(seconds):
            self.assertFalse((f.layout.node_env.parent / ".usdb-node-operation.lock").read_text()
                             if (f.layout.node_env.parent / ".usdb-node-operation.lock").exists() else "")
            now[0] += seconds
            if now[0] > 75:
                for item in f.containers.values(): item["state"] = "exited"
        with f.patches(), mock.patch.object(lifecycle.time, "monotonic", side_effect=lambda: now[0]), \
             mock.patch.object(node, "start_node") as start, \
             mock.patch("usdb_mining.pending", return_value=False), mock.patch("usdb_peers.pending", return_value=False):
            self.assertEqual(lifecycle.run(f.layout, node, sleep=sleep), 0)
        self.assertEqual(node.read_env(f.layout.node_env)["USDB_RESOURCE_PHASE"], "steady")
        self.assertEqual(start.call_count, 1)

    def test_controller_recovers_pending_transition_with_all_services_stopped(self):
        f = RuntimeResources(self.root, memory=64 * policy.GIB)
        with f.patches():
            f.crash = "quiesce-services"
            with self.assertRaises(RuntimeError):
                lifecycle.apply_plan(f.layout, node, "steady")
            for item in f.containers.values():
                item["state"] = "exited"
            def finish(_seconds):
                state = node._read_resource_state(f.layout)
                self.assertFalse(state["pending"])
                state["recover_services"] = []
                node._write_resource_state(f.layout, state)
                for item in f.containers.values():
                    item["state"] = "exited"
            def start_services(*_args, **_kwargs):
                for name in lifecycle.ORDER:
                    if name in f.containers:
                        f.containers[name] = f.container(name)
            with mock.patch.object(node, "start_node", side_effect=start_services) as start, \
                 mock.patch("usdb_mining.pending", return_value=False), mock.patch("usdb_peers.pending", return_value=False):
                self.assertEqual(lifecycle.run(f.layout, node, sleep=finish), 0)
                start.assert_called_once()

    def test_enabled_txindex_blocks_steady_but_disabled_index_does_not(self):
        f = RuntimeResources(self.root)
        f.core["indexes_ready"] = False
        with f.patches():
            self.assertTrue(lifecycle.observations(f.layout, node)["complete"])
            f.layout.node_env.write_text(node.upsert_env(f.layout.node_env.read_text(), {"BTC_TXINDEX": "1"}))
            self.assertFalse(lifecycle.observations(f.layout, node)["complete"])
            f.core["indexes_ready"] = True
            self.assertTrue(lifecycle.observations(f.layout, node)["complete"])

    def test_chain_still_syncing_does_not_authorize_steady(self):
        f = RuntimeResources(self.root)
        with f.patches(), mock.patch.object(node, "_chain_component", return_value=dict(state="SYNCING")):
            sample = lifecycle.observations(f.layout, node)
            self.assertTrue(sample["known"])
            self.assertTrue(sample["core_complete"])
            self.assertFalse(sample["complete"])


class CompletionWindowTests(unittest.TestCase):
    def test_core_catchup_release_waits_for_core_completion_and_fresh_window(self):
        state = lifecycle.Stability()
        sample = dict(known=True, complete=False, core_complete=False, identity=1)
        for now in range(0, 90, 15):
            self.assertIsNone(state.target("catchup", sample, now, "core-catchup"))
        sample["core_complete"] = True
        for now in range(90, 150, 15):
            self.assertIsNone(state.target("catchup", sample, now, "core-catchup"))
        self.assertEqual(state.target("catchup", sample, 150, "core-catchup"), "runtime-ready")
        self.assertIsNone(state.target("catchup", sample, 165, "runtime-ready"))
        sample.update(core_complete=False, lag=1000)
        for now in range(180, 480, 15):
            self.assertIsNone(state.target("catchup", sample, now, "runtime-ready"))
        self.assertEqual(state.target("catchup", sample, 480, "runtime-ready"), "core-catchup")

    def test_core_release_uses_a_window_and_is_not_repeated(self):
        state = lifecycle.Stability()
        sample = dict(known=True, complete=False, core_complete=True, identity=1)
        for now in range(0, 60, 15):
            self.assertIsNone(state.target("catchup", sample, now))
        self.assertEqual(state.target("catchup", sample, 60), "core-ready")
        self.assertIsNone(state.target("catchup", sample, 75, "core-ready"))
        self.assertIsNone(state.target("catchup", sample, 90, "compact"))

    def test_completion_requires_fresh_continuous_observations(self):
        state = lifecycle.Stability()
        good = dict(known=True, complete=True, identity="boot1")
        for now in (0, 15, 30, 45): self.assertIsNone(state.target("catchup", good, now))
        self.assertEqual(state.target("catchup", good, 60), "steady")
        self.assertIsNone(state.target("catchup", dict(known=False), 75))
        self.assertIsNone(state.target("catchup", good, 90))
        self.assertIsNone(state.target("catchup", good, 200))

    def test_new_blocks_do_not_return_to_bootstrap_and_large_lag_has_hysteresis(self):
        state = lifecycle.Stability()
        for now in range(0, 301, 15):
            self.assertIsNone(state.target("steady", dict(known=True, complete=False, lag=1, identity=1), now))
        large = dict(known=True, complete=False, lag=1000, identity=1)
        for now in range(315, 615, 15): self.assertIsNone(state.target("steady", large, now))
        self.assertEqual(state.target("steady", large, 615), "catchup")


class RuntimeProbeAndRunnerTests(unittest.TestCase):
    def test_index_observation_failure_does_not_revoke_foreground_readiness(self):
        core = dict(bootstrap_ready=True, active_height=100, headers=100,
                    history_validated=True, chainstates=[dict(bestblockhash="a" * 64)])
        snapshot = mock.Mock(base_height=100)
        rpc = mock.Mock()
        def call(method, *_args):
            if method == "getblockheader": return {"time": 1000}
            if method == "getnetworkinfo": return {"connections": 3}
            if method == "getindexinfo": raise bitcoin_assumeutxo.RpcFailure(method, kind="timeout")
            self.fail("unexpected RPC: " + method)
        rpc.call.side_effect = call
        with mock.patch.object(bitcoin_assumeutxo, "core_status", return_value=core), \
             mock.patch.object(bitcoin_assumeutxo.time, "time", return_value=1000):
            report = bitcoin_assumeutxo.tip_status(rpc, snapshot, {"BTC_TXINDEX": "1"})
        self.assertTrue(report["tip_ready"])
        self.assertFalse(report["indexes_ready"])
        self.assertEqual(report["index_observation_error"]["kind"], "timeout")

    def test_targeted_shell_shutdown_preserves_unaffected_services(self):
        fixture = runner_fixture.ResourceRunnerTests()
        result, states, calls = fixture.run_quiesce(
            {name: {"state": "running"} for name in ("usdb-chain", "balance-history", "ord-server")},
            action="quiesce-services", arguments=("usdb-chain", "balance-history"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[-1] for call in calls if call[0] == "kill"], ["usdb-chain", "balance-history"])
        self.assertEqual(states["ord-server"], {"state": "running"})
        self.assertFalse(any("SIGKILL" in " ".join(call) for call in calls))
        result, _, calls = fixture.run_quiesce({}, action="quiesce-services", arguments=("btc-node",))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(calls)


if __name__ == "__main__":
    unittest.main()
