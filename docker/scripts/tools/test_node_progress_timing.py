"""Test ETA bounds, time origins and resets without wall-clock sleeps."""

import copy
import unittest

from node_progress_timing import ProgressTiming, service_elapsed


class ProgressTimingTests(unittest.TestCase):
    def report(self, current=10, total=100, **updates):
        return {"release_id": "test", "overall_state": "SYNCING", "components": [
            {"id": "bitcoin", "state": "SYNCING", "current": current, "total": total,
             "unit": "blocks", **updates}]}

    def sample(self, timer, now, **updates):
        return timer.apply(self.report(**updates), now)["components"][0]["timing"]

    def test_uses_recent_deltas_not_progress_since_genesis(self):
        timer = ProgressTiming()
        self.assertEqual(self.sample(timer, 0, current=80)["eta_state"], "sampling")
        self.sample(timer, 15, current=82)
        timing = self.sample(timer, 30, current=84)
        self.assertEqual(timing["eta_secs"], 120)
        self.assertEqual(timing["elapsed_secs"], 30)
        self.assertEqual(timing["elapsed_source"], "observation")

    def test_bitcoin_uses_verification_work_instead_of_block_height(self):
        timer = ProgressTiming()
        for now, verification, blocks in [(0, 0.5, 800), (15, 0.6, 810), (30, 0.7, 820)]:
            timing = self.sample(timer, now, current=blocks, total=1000, verification_progress=verification)
        self.assertAlmostEqual(timing["eta_secs"], 45, delta=1)
        self.assertEqual(timing["eta_basis"], "verification")

    def test_moving_target_uses_net_catchup_rate(self):
        timer = ProgressTiming()
        self.sample(timer, 0, current=10, total=100)
        self.sample(timer, 15, current=20, total=105)
        self.assertEqual(self.sample(timer, 30, current=30, total=110)["eta_secs"], 240)
        self.assertEqual(self.sample(timer, 45, current=40, total=200)["eta_state"], "not-catching-up")

    def test_stall_timeout_ready_and_missing_progress_never_guess_completion(self):
        timer = ProgressTiming()
        self.sample(timer, 0)
        self.sample(timer, 15, current=20)
        self.sample(timer, 30, current=30)
        timing = self.sample(timer, 90, current=30)
        self.assertEqual(timing["eta_state"], "stalled")
        self.assertIsNone(timing["eta_secs"])
        timing = self.sample(timer, 95, current=30, state="STARTING")
        self.assertEqual(timing["eta_state"], "unavailable")
        self.assertIsNone(timing["eta_secs"])
        self.assertEqual(self.sample(timer, 100, current=40)["eta_state"], "sampling")
        self.assertEqual(self.sample(timer, 110, current=100)["eta_state"], "waiting-readiness")
        report = timer.apply(self.report(current=100, state="READY"), 115)
        self.assertNotIn("timing", report["components"][0])

    def test_regression_restart_phase_change_or_long_gap_resets_history(self):
        for updates, now in [({"current": 5}, 45), ({"total": 90}, 45),
                             ({"service_started_at": "new-process"}, 45),
                             ({"progress_phase": "new-stage"}, 45), ({}, 400)]:
            with self.subTest(updates=updates, now=now):
                timer = ProgressTiming()
                self.sample(timer, 0)
                self.sample(timer, 15, current=20)
                self.sample(timer, 30, current=30)
                self.assertEqual(self.sample(timer, now, **{"current": 40, **updates})["eta_state"], "sampling")

    def test_waiting_and_failed_components_drop_history(self):
        for state in ("WAITING", "FAILED", "BLOCKED", "SKIPPED"):
            timer = ProgressTiming()
            self.sample(timer, 0)
            timer.apply(self.report(state=state), 15)
            self.assertEqual(timer.samples, {})

    def test_native_snapshot_timing_is_preserved_and_reports_are_not_mutated(self):
        report = self.report(stage_elapsed_secs=100, stage_eta_secs=200, state="IMPORTING")
        original = copy.deepcopy(report)
        result = ProgressTiming().apply(report, 10)
        self.assertEqual(result["components"], report["components"])
        self.assertEqual(report, original)
        self.assertEqual(result["overall_state"], "SYNCING")

    def test_sample_storage_is_bounded_and_elapsed_uses_process_start(self):
        timer = ProgressTiming()
        for now in range(1000):
            timing = self.sample(timer, now, current=now, total=2000, service_elapsed_secs=10000 + now)
        self.assertLessEqual(len(timer.samples["bitcoin"].points), 128)
        self.assertEqual(timing["elapsed_secs"], 10999)
        self.assertEqual(timing["elapsed_source"], "process")

    def test_invalid_measurements_do_not_produce_eta(self):
        for value in (None, True, float("nan"), float("inf"), -1):
            with self.subTest(value=value):
                timing = self.sample(ProgressTiming(), 0, current=value, verification_progress=value)
                self.assertIsNone(timing["eta_secs"])

    def test_process_elapsed_handles_nanoseconds_unknown_and_future_starts(self):
        self.assertEqual(service_elapsed("2026-09-08T04:00:00.123456789Z", "2026-09-10T04:00:00+00:00"), 172799)
        for started in (None, "bad", "0001-01-01T00:00:00Z", "2026-09-11T00:00:00Z", "2026-09-09T00:00:00"):
            self.assertIsNone(service_elapsed(started, "2026-09-10T00:00:00Z"))


if __name__ == "__main__":
    unittest.main()
