#!/usr/bin/env python3
"""Boundary tests for proportional plans and early runtime configuration checks."""

import tempfile
import unittest
from pathlib import Path

import resource_policy as POLICY


class ResourcePolicyTests(unittest.TestCase):
    def test_external_services_reserve_preserves_system_and_every_phase(self):
        for memory in (67_297_947_648, 64 * POLICY.GIB, 256 * POLICY.GIB):
            for phase in POLICY.PHASES:
                baseline = POLICY.build_resource_plan(memory, phase, {})
                plan = POLICY.build_resource_plan(memory, phase, {"USDB_EXTERNAL_MEMORY_BUDGET": "8g"})
                self.assertEqual(plan.reserve_bytes, baseline.reserve_bytes)
                self.assertEqual(plan.external_services_bytes, 8 * POLICY.GIB)
                self.assertLessEqual(plan.total_bytes, memory)
                POLICY.validate_resource_environment(plan.environment(), memory)
        with self.assertRaisesRegex(ValueError, "leave at least"):
            POLICY.build_resource_plan(32 * POLICY.GIB, "steady", {"USDB_EXTERNAL_MEMORY_BUDGET": "8g"})

    def test_small_standard_and_large_hosts_have_safe_all_phases(self):
        expected = {
            32: [(25.6, 12), (8, 12), (4, 16)],
            64: [(32, 24), (16, 24), (8, 32)],
            256: [(32, 64), (16, 64), (8, 64)],
        }
        for gib, phases in expected.items():
            for phase, (bitcoin, balance) in zip(POLICY.PHASES, phases):
                with self.subTest(gib=gib, phase=phase):
                    plan = POLICY.build_resource_plan(gib * POLICY.GIB, phase, {})
                    self.assertEqual(plan.limits["BTC_MEMORY_LIMIT"], int(bitcoin * 1024) * POLICY.MIB)
                    self.assertEqual(plan.limits["BH_MEMORY_LIMIT"], balance * POLICY.GIB)
                    self.assertLessEqual(plan.total_bytes, gib * POLICY.GIB)
                    POLICY.validate_resource_environment(plan.environment(), gib * POLICY.GIB)

    def test_real_memtotal_and_custom_caps_do_not_round_up_to_machine_class(self):
        memory = 67_297_947_648
        plan = POLICY.build_resource_plan(memory, "steady", {})
        self.assertLessEqual(plan.limits["BH_MEMORY_LIMIT"], memory // 2)
        self.assertLess(plan.limits["BTC_MEMORY_LIMIT"], 8 * POLICY.GIB)
        self.assertLessEqual(plan.total_bytes, memory)
        plan = POLICY.build_resource_plan(256 * POLICY.GIB, "steady", {"USDB_BH_MEMORY_CAP": "96g"})
        self.assertEqual(plan.limits["BH_MEMORY_LIMIT"], 96 * POLICY.GIB)

    def test_cache_plan_preserves_pressure_headroom_and_split(self):
        plan = POLICY.build_resource_plan(64 * POLICY.GIB, "steady", {})
        self.assertEqual(plan.utxo_cache_bytes, 5 * POLICY.GIB)
        self.assertEqual(plan.balance_cache_bytes, 15 * POLICY.GIB)
        self.assertEqual(plan.environment()["BH_SYNC_MAX_MEMORY_PERCENT"], "80")

    def test_native_steady_transfers_memory_without_growing_dbcache_or_total(self):
        for memory in (32_000_000_000, 32_495_595_520, 32 * POLICY.GIB,
                       64 * POLICY.GIB, 256 * POLICY.GIB):
            with self.subTest(memory=memory):
                native_env = {"SNAPSHOT_MODE": "assumeutxo"}
                legacy = POLICY.build_resource_plan(memory, "steady", {"USDB_BTC_STEADY_MEMORY_CAP": "16g"})
                native = POLICY.build_resource_plan(memory, "steady", native_env)
                gain = native.limits["BTC_MEMORY_LIMIT"] - legacy.limits["BTC_MEMORY_LIMIT"]
                self.assertGreaterEqual(gain, 0)
                if memory <= 64 * POLICY.GIB:
                    self.assertGreater(gain, 3 * POLICY.GIB)
                self.assertEqual(native.limits["BH_MEMORY_LIMIT"], legacy.limits["BH_MEMORY_LIMIT"] - gain)
                self.assertEqual(native.dbcache_mib, legacy.dbcache_mib)
                self.assertEqual(native.total_bytes, legacy.total_bytes + 128 * POLICY.MIB)
                self.assertLessEqual(native.total_bytes, memory)
                self.assertEqual(native.utxo_cache_bytes + native.balance_cache_bytes,
                                 native.limits["BH_MEMORY_LIMIT"] * 5 // 8)
                POLICY.validate_resource_environment({**native_env, **native.environment()}, memory)

    def test_native_headroom_respects_caps_external_services_and_optional_ord(self):
        for memory in (32 * POLICY.GIB, 64 * POLICY.GIB, 256 * POLICY.GIB):
            for cap in ("4g", "8g", "16g", "32g"):
                for bh_cap in ("4g", "64g"):
                    env = {"SNAPSHOT_MODE": "assumeutxo", "USDB_BTC_STEADY_MEMORY_CAP": cap,
                           "USDB_BH_MEMORY_CAP": bh_cap, "USDB_MINTING_ENABLED": "1"}
                    if memory >= 64 * POLICY.GIB:
                        env["USDB_EXTERNAL_MEMORY_BUDGET"] = "8g"
                    with self.subTest(memory=memory, cap=cap, bh_cap=bh_cap):
                        plan = POLICY.build_resource_plan(memory, "steady", env)
                        self.assertLessEqual(plan.total_bytes, memory)
                        self.assertLessEqual(plan.limits["BTC_MEMORY_LIMIT"], POLICY.memory_bytes(cap, "cap"))
                        self.assertGreaterEqual(plan.limits["BH_MEMORY_LIMIT"], 4 * POLICY.GIB)
                        POLICY.validate_resource_environment({**env, **plan.environment()}, memory)

    def test_native_old_steady_plan_requires_recalculation(self):
        memory = 32 * POLICY.GIB
        old = {"SNAPSHOT_MODE": "assumeutxo",
               **POLICY.build_resource_plan(memory, "steady", {}).environment(),
               "BTC_BOOTSTRAP_MEMORY_LIMIT": str(128 * POLICY.MIB)}
        with self.assertRaisesRegex(ValueError, "set-resource-policy --mode auto"):
            POLICY.validate_resource_environment(old, memory)
        updated = {**old, **POLICY.build_resource_plan(memory, "steady", old).environment()}
        POLICY.validate_resource_environment(updated, memory)
        self.assertEqual(updated["USDB_RESOURCE_PHASE"], "steady")

    def test_native_pre_steady_allocations_are_unchanged(self):
        for phase in ("bitcoin", "overlap"):
            legacy = POLICY.build_resource_plan(32 * POLICY.GIB, phase, {})
            native = POLICY.build_resource_plan(32 * POLICY.GIB, phase, {"SNAPSHOT_MODE": "assumeutxo"})
            self.assertEqual(native.dbcache_mib, legacy.dbcache_mib)
            self.assertEqual(native.utxo_cache_bytes, legacy.utxo_cache_bytes)
            for key, value in legacy.limits.items():
                self.assertEqual(native.limits[key], value)

    def test_native_default_and_explicit_steady_caps(self):
        memory = 32 * POLICY.GIB
        native = {"SNAPSHOT_MODE": "assumeutxo"}
        plan = POLICY.build_resource_plan(memory, "steady", native)
        self.assertEqual(plan.limits["BTC_MEMORY_LIMIT"], 16 * POLICY.GIB)
        self.assertEqual(plan.limits["BH_MEMORY_LIMIT"], 4 * POLICY.GIB)
        self.assertEqual(plan.dbcache_mib, 2048)
        constrained = POLICY.build_resource_plan(memory, "steady", {**native, "USDB_BTC_STEADY_MEMORY_CAP": "8g"})
        self.assertEqual(constrained.limits["BTC_MEMORY_LIMIT"], 8 * POLICY.GIB)
        self.assertEqual(constrained.limits["BH_MEMORY_LIMIT"], 12 * POLICY.GIB)

    def test_bitcoin_boost_keeps_cache_and_system_headroom(self):
        # The diagnosed node has about 30.6 GiB of effective RAM.
        memory = 32_866_566_144
        plan = POLICY.build_resource_plan(memory, "bitcoin", {})
        self.assertEqual(plan.limits["BTC_MEMORY_LIMIT"], 26_293_043_200)
        self.assertEqual(plan.dbcache_mib, 9795)
        self.assertEqual(plan.total_bytes, plan.reserve_bytes + plan.limits["BTC_MEMORY_LIMIT"] + plan.limits["CONTROL_PLANE_MEMORY_LIMIT"])
        self.assertLessEqual(plan.total_bytes, memory)
        self.assertGreater(plan.limits["BTC_MEMORY_LIMIT"] - plan.dbcache_mib * POLICY.MIB,
                           14 * POLICY.GIB)

    def test_bitcoin_boost_respects_custom_caps_and_external_reserve(self):
        for memory in (32_000_000_000, 32 * POLICY.GIB, 64 * POLICY.GIB, 256 * POLICY.GIB):
            for external in ("0", "8g"):
                if external != "0" and memory - 8 * POLICY.GIB < POLICY.MIN_HOST_MEMORY_BYTES:
                    continue
                for cap in ("8g", "128g"):
                    env = {"USDB_BTC_IBD_MEMORY_CAP": cap, "USDB_EXTERNAL_MEMORY_BUDGET": external}
                    with self.subTest(memory=memory, external=external, cap=cap):
                        plan = POLICY.build_resource_plan(memory, "bitcoin", env)
                        self.assertLessEqual(plan.limits["BTC_MEMORY_LIMIT"], memory * 4 // 5)
                        self.assertLessEqual(plan.limits["BTC_MEMORY_LIMIT"], POLICY.memory_bytes(cap, "cap"))
                        self.assertLessEqual(plan.total_bytes, memory)
                        POLICY.validate_resource_environment({**env, **plan.environment()}, memory)

    def test_old_bitcoin_plan_requires_explicit_recalculation(self):
        env = POLICY.build_resource_plan(32 * POLICY.GIB, "bitcoin", {}).environment()
        env["BTC_MEMORY_LIMIT"] = str(16 * POLICY.GIB)
        with self.assertRaisesRegex(ValueError, "set-resource-policy --mode auto"):
            POLICY.validate_resource_environment(env, 32 * POLICY.GIB)

    def test_bitcoin_cache_respects_the_pinned_core_limit_on_large_hosts(self):
        for phase in POLICY.PHASES:
            plan = POLICY.build_resource_plan(256 * POLICY.GIB, phase,
                                               {"USDB_BTC_IBD_MEMORY_CAP": "128g",
                                                "USDB_BTC_OVERLAP_MEMORY_CAP": "64g",
                                                "USDB_BTC_STEADY_MEMORY_CAP": "32g"})
            self.assertLessEqual(plan.dbcache_mib, 16384)

    def test_rejects_undersized_hosts_invalid_caps_and_phase(self):
        for memory, phase, caps in [(16 * POLICY.GIB, "bitcoin", {}),
                                     (64 * POLICY.GIB, "wrong", {}),
                                     (64 * POLICY.GIB, "steady", {"USDB_BH_MEMORY_CAP": "0"}),
                                     (64 * POLICY.GIB, "overlap", {"USDB_BTC_OVERLAP_MEMORY_CAP": "1g"})]:
            with self.subTest(memory=memory, phase=phase, caps=caps), self.assertRaises(ValueError):
                POLICY.build_resource_plan(memory, phase, caps)

    def test_legacy_startup_failure_is_caught_before_import(self):
        env = {"BH_MEMORY_LIMIT": "12g", "BH_SYNC_UTXO_MAX_CACHE_BYTES": str(2 * POLICY.GIB),
               "BH_SYNC_BALANCE_MAX_CACHE_BYTES": str(6 * POLICY.GIB), "BH_SYNC_MAX_MEMORY_PERCENT": "75"}
        with self.assertRaisesRegex(ValueError, "cache budget"):
            POLICY.validate_resource_environment(env)

    def test_plan_tampering_host_shrink_and_manual_oversubscription_are_rejected(self):
        env = POLICY.build_resource_plan(64 * POLICY.GIB, "overlap", {}).environment()
        for key in ("BTC_MEMORY_LIMIT", "BTC_DBCACHE_MB", "BH_SYNC_UTXO_MAX_CACHE_BYTES"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                POLICY.validate_resource_environment({**env, key: str(int(env[key]) + 1)})
        with self.assertRaisesRegex(ValueError, "memory decreased"):
            POLICY.validate_resource_environment(env, 32 * POLICY.GIB)
        with self.assertRaisesRegex(ValueError, "plus system reserve"):
            POLICY.validate_resource_environment({"BTC_MEMORY_LIMIT": "32g", "BH_MEMORY_LIMIT": "32g"}, 64 * POLICY.GIB)

    def test_effective_memory_uses_parent_limits_and_never_memavailable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            meminfo, proc, cgroup = root / "meminfo", root / "self-cgroup", root / "cgroup"
            meminfo.write_text(f"MemTotal:       {64 * POLICY.GIB // 1024} kB\nMemAvailable:   1024 kB\n")
            proc.write_text("0::/system.slice/controller\n")
            (cgroup / "system.slice/controller").mkdir(parents=True)
            (cgroup / "memory.max").write_text("max")
            (cgroup / "system.slice/memory.max").write_text(str(32 * POLICY.GIB))
            self.assertEqual(POLICY.effective_memory_bytes(meminfo, proc, cgroup), 32 * POLICY.GIB)
            proc.write_text("5:memory:/service\n")
            (cgroup / "memory/service").mkdir(parents=True)
            (cgroup / "memory/memory.limit_in_bytes").write_text(str(48 * POLICY.GIB))
            (cgroup / "memory/service/memory.limit_in_bytes").write_text(str(40 * POLICY.GIB))
            self.assertEqual(POLICY.effective_memory_bytes(meminfo, proc, cgroup), 40 * POLICY.GIB)
            (cgroup / "memory/service/memory.limit_in_bytes").write_text("0")
            self.assertEqual(POLICY.effective_memory_bytes(meminfo, proc, cgroup), 0)


if __name__ == "__main__":
    unittest.main()
