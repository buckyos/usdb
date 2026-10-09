"""Warmup is observable progress, not readiness or a blanket failure exemption."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_monitor_rules as rules
from control_plane_monitor import project
from common.node_monitor import BASE, MonitorFixture, core_startup_report, report


class CoreStartupMonitorTests(unittest.TestCase):
    def codes(self, fixture):
        return [a["code"] for a in fixture.store.alerts() if a["state"] == "firing"]

    def tick(self, fixture, seconds, **options):
        at = BASE + seconds * 1000
        value = core_startup_report(at, **options)
        rules.evaluate(fixture.store, project(value, at), at, fixture.settings)
        return value

    def test_three_minute_snapshot_warmup_then_rpc_recovery_has_no_failure(self):
        with MonitorFixture() as f:
            f.settings.update(rules.DEFAULTS)
            for elapsed in range(0, 211, 30):
                self.tick(f, elapsed)
            self.assertNotIn("SERVICE_UNAVAILABLE", self.codes(f))
            self.assertNotIn("STARTUP_STALLED", self.codes(f))
            f.tick(240)
            self.assertNotIn("SERVICE_UNAVAILABLE", self.codes(f))

    def test_repeated_warmup_stalls_and_monitor_restart_does_not_reset_age(self):
        with MonitorFixture() as f:
            self.tick(f, 0)
            self.tick(f, 2)
            self.tick(f, 3)
            self.assertIn("STARTUP_STALLED", self.codes(f))
            self.assertNotIn("SERVICE_UNAVAILABLE", self.codes(f))
            f.store.begin(BASE + 4000, "r2")
            self.tick(f, 4)
            self.tick(f, 8)
            self.assertTrue(any(a["severity"] == "critical" for a in f.store.alerts()))
            f.tick(9)
            f.tick(11)
            self.assertFalse(f.store.alerts())

    def test_actual_counter_or_phase_progress_prevents_stall(self):
        with MonitorFixture() as f:
            for elapsed in range(12):
                self.tick(f, elapsed, progressed=BASE + (elapsed // 2) * 2000)
            self.assertNotIn("STARTUP_STALLED", self.codes(f))

    def test_restart_rejects_old_stage_and_rpc_ready_run_cannot_reenter_exception(self):
        with MonitorFixture() as f:
            # RPC is already usable before the foreground tip is ready.
            f.tick(0, ready=False, phase="SYNCING")
            f.store.begin(BASE + 500, "r2")
            self.tick(f, 1)
            self.tick(f, 3)
            self.assertIn("SERVICE_UNAVAILABLE", self.codes(f))
            self.assertNotIn("STARTUP_STALLED", self.codes(f))
        with MonitorFixture() as f:
            for elapsed in (0, 2):
                value = core_startup_report(BASE + elapsed * 1000)
                value["observations"]["services"]["bitcoin"]["runtime"]["started_at"] = "2026-10-09T07:32:57Z"
                rules.evaluate(f.store, value, BASE + elapsed * 1000, f.settings)
            self.assertIn("SERVICE_UNAVAILABLE", self.codes(f))

    def test_new_core_run_gets_its_own_bounded_startup_window(self):
        with MonitorFixture() as f:
            f.tick(0)
            self.tick(f, 1, started=BASE + 1000, progressed=BASE + 1000)
            self.tick(f, 3, started=BASE + 1000, progressed=BASE + 1000)
            self.assertNotIn("SERVICE_UNAVAILABLE", self.codes(f))
            self.assertNotIn("STARTUP_STALLED", self.codes(f))
            self.tick(f, 4, started=BASE + 1000, progressed=BASE + 1000)
            self.assertIn("STARTUP_STALLED", self.codes(f))

    def test_startup_stall_requires_continuous_positive_evidence_to_recover(self):
        with MonitorFixture() as f:
            self.tick(f, 0)
            self.tick(f, 3)
            self.tick(f, 4, progressed=BASE + 4000)
            self.assertIn("STARTUP_STALLED", self.codes(f))
            f.tick(5, available=False)
            self.tick(f, 6, progressed=BASE + 6000)
            self.tick(f, 7, progressed=BASE + 6000)
            self.assertIn("STARTUP_STALLED", self.codes(f))
            self.tick(f, 8, progressed=BASE + 6000)
            self.assertNotIn("STARTUP_STALLED", self.codes(f))

    def test_crash_oom_and_absent_warmup_evidence_keep_failure_alerts(self):
        for mutation in (dict(state="exited", exit_code=1), dict(oom_killed=True), dict(state="restarting"), {}):
            with self.subTest(mutation=mutation), MonitorFixture() as f:
                for elapsed in (0, 2):
                    value = core_startup_report(BASE + elapsed * 1000)
                    value["observations"]["services"]["bitcoin"]["runtime"].update(mutation)
                    if not mutation:
                        value["components"][0].pop("startup_progress")
                    rules.evaluate(f.store, value, BASE + elapsed * 1000, f.settings)
                self.assertIn("SERVICE_UNAVAILABLE", self.codes(f))
                self.assertNotIn("STARTUP_STALLED", self.codes(f))

    def test_warmup_does_not_clear_existing_health_alert(self):
        with MonitorFixture() as f:
            for elapsed in (0, 2):
                value = report(BASE + elapsed * 1000)
                value["components"][0]["state"] = "STARTING"
                value["observations"]["services"]["bitcoin"].update(probe_status="unavailable", readiness={})
                rules.evaluate(f.store, value, BASE + elapsed * 1000, f.settings)
            self.assertIn("SERVICE_UNAVAILABLE", self.codes(f))
            self.tick(f, 3, progressed=BASE + 3000)
            self.tick(f, 6, progressed=BASE + 6000)
            self.assertTrue(any(a["code"] == "SERVICE_UNAVAILABLE" for a in f.store.alerts()))


if __name__ == "__main__":
    unittest.main()
