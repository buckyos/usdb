"""UIP-0018 policy propagation, retained authorization and historical work display."""
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import node_mining_policy as POLICY
import node_progress_render as RENDER
import usdb_mining as MINING
import usdb_node as NODE
from common.mining import ADDRESS, PASS_ID, MiningFixture, RuntimeScriptFixture
from common.node_progress import ready_miner_progress


class MiningPolicyTests(unittest.TestCase):
    def test_defaults_explicit_zero_and_invalid_input(self):
        self.assertEqual(POLICY.configured_gap({}), 2)
        for value in ("0", "1", "2", "4294967295"):
            self.assertEqual(POLICY.configured_gap({POLICY.KEY: value}), int(value))
        for value in ("", "-1", "1.5", "4294967296", "01", " 2", "+2", True, 2):
            with self.subTest(value=value), self.assertRaises(ValueError):
                POLICY.parse_gap(value)
        args = NODE.build_parser().parse_args(["mining", "configure", "--anchor-gap", "0"])
        self.assertEqual(args.anchor_gap, "0")

    def test_saved_policy_reconciles_only_chain_without_new_authorization(self):
        with MiningFixture() as f:
            f.enable()
            self.assertEqual(f.run(), 0)
            before = MINING.read_state(f.layout)
            env_before = NODE.read_env(f.layout.node_env)
            f.calls.clear()
            saved = POLICY.configure(f.layout, "0", MINING)
            self.assertEqual(saved["configured_gap"], 0)
            self.assertEqual(f.calls, [])
            self.assertEqual(MINING.read_state(f.layout), before)
            self.assertEqual(NODE.read_env(f.layout.node_env), {**env_before, POLICY.KEY: "0"})
            report = MINING.observe(f.layout)
            self.assertTrue(report["drift"])
            self.assertEqual(report["anchor_policy"]["runtime_gap"], 2)
            MINING.validate_start(f.layout)  # Existing APPLIED authorization survives.
            self.assertEqual(MINING.reconcile_config(f.layout), 0)
            self.assertEqual([call[1][0] for call in f.calls], ["stop-chain", "recreate-chain"])
            report = MINING.observe(f.layout)
            self.assertEqual(report["state"], "ACTIVE")
            self.assertEqual(report["anchor_policy"]["runtime_gap"], 0)
            self.assertEqual(MINING.read_state(f.layout)["binding"], before["binding"])

    def test_pending_operations_and_bad_values_do_not_mutate_config(self):
        with MiningFixture() as f:
            before = f.layout.node_env.read_bytes()
            for value in ("-1", "", "4294967296"):
                with self.assertRaises(ValueError): POLICY.configure(f.layout, value, MINING)
            f.enable()
            with self.assertRaisesRegex(ValueError, "pending"):
                POLICY.configure(f.layout, "0", MINING)
            self.assertEqual(f.layout.node_env.read_bytes(), before)

    def test_runtime_missing_duplicate_and_mismatched_arguments(self):
        with MiningFixture() as f:
            env = NODE.read_env(f.layout.node_env)
            del f.runtime["argv"][-2:]
            self.assertIsNone(POLICY.policy_view(env, f.runtime, MINING)["runtime_gap"])
            self.assertFalse(MINING.runtime_matches(f.layout, env, f.runtime))
            f.runtime["argv"] += ["--miner.usdb-anchor-gap=0"]
            self.assertEqual(POLICY.policy_view(env, f.runtime, MINING)["runtime_state"], "differs")
            f.runtime["argv"] += ["--miner.usdb-anchor-gap=2"]
            with self.assertRaisesRegex(ValueError, "duplicate"):
                MINING.runtime_matches(f.layout, env, f.runtime)

    def test_old_pass_work_remains_active_while_latest_candidate_changes(self):
        with MiningFixture() as f:
            f.enable(); f.run()
            f.candidate["pass"]["pass_id"] = "fa" * 32 + "i0"
            f.pass_snapshot = {"inscription_id": f.candidate["pass"]["pass_id"], "mint_block_height": 100,
                               "resolved_height": 100, "state": "active", "usdb_main": ADDRESS}
            report = MINING.observe(f.layout)
            self.assertEqual(report["state"], "ACTIVE")
            self.assertEqual(report["anchor_work"]["work"]["build"]["pass_id"], PASS_ID)
            self.assertIn("indexer height 102", report["candidate_hint"])
            text = "\n".join(POLICY.display_lines(report))
            self.assertIn("L=100 -> H=98", text)
            self.assertIn("Work Pass: " + PASS_ID, text)
            self.assertIn("not a block-finding ETA", text)
            progress = ready_miner_progress()
            progress["mining"] = report
            rendered = RENDER.render_node_progress(progress, width=160)
            self.assertIn("Latest Candidate Pass: " + f.candidate["pass"]["pass_id"], rendered)
            self.assertIn("Work Pass: " + PASS_ID, rendered)
            self.assertIn("configured 2", rendered)
            # Latest candidate probes cannot veto an independently valid historical work.
            f.fail_rpc = "resolve_miner_candidate"
            report = MINING.observe(f.layout)
            self.assertEqual(report["state"], "ACTIVE")
            self.assertIn("latest_eligibility_error", report)

    def test_hard_readiness_barriers_still_hide_old_work(self):
        for service, blocker in (("bh_ready", "RollbackInProgress"), ("ready", "ReorgRecoveryPending")):
            with self.subTest(blocker=blocker), MiningFixture() as f:
                f.enable(); f.run()
                getattr(f, service).update(consensus_ready=False, blockers=[blocker])
                report = MINING.observe(f.layout)
                self.assertEqual(report["state"], "WAITING")
                self.assertIn(blocker, report["detail"])

    def test_unknown_stale_malformed_and_unmatched_work_are_not_active(self):
        for scenario in ("missing", "stale", "unmatched", "malformed", "gap", "address"):
            with self.subTest(scenario=scenario), MiningFixture() as f:
                f.enable(); f.run()
                diagnostic = f.rpc(f.layout, "miner_getUSDBWorkStatus")
                if scenario == "missing": f.fail_rpc = "miner_getUSDBWorkStatus"
                if scenario == "stale": diagnostic["work_current"] = False
                if scenario == "unmatched": diagnostic["work"]["seal_hash"] = "0x" + "ab" * 32
                if scenario == "malformed": diagnostic["work"] = []
                if scenario == "gap": diagnostic["work"]["build"]["selection"]["actual_gap"] = 0
                if scenario == "address": diagnostic["work"]["build"]["miner_address"] = "0x" + "ab" * 20
                f.work_diagnostic = diagnostic
                report = MINING.observe(f.layout)
                self.assertNotEqual(report["state"], "ACTIVE")
                if scenario != "stale" and scenario != "unmatched":
                    self.assertFalse(report["anchor_work"]["available"])

    def test_waiting_new_mint_and_parent_height_show_go_failure(self):
        with MiningFixture() as f:
            f.enable(); f.run()
            diagnostic = f.rpc(f.layout, "miner_getUSDBWorkStatus")
            diagnostic.pop("work")
            diagnostic["work_current"] = False
            diagnostic["attempt"].update(stage="candidate", failure="candidate_unavailable")
            diagnostic["attempt"].pop("pass_id")
            f.work_diagnostic = diagnostic
            report = MINING.observe(f.layout)
            self.assertEqual(report["state"], "WAITING")
            self.assertIn("candidate_unavailable", "\n".join(POLICY.display_lines(report)))
            diagnostic["attempt"].update(stage="select_height", failure="parent_state_unavailable", required_height=101)
            diagnostic["attempt"].pop("selection")
            self.assertIn("required height=101", "\n".join(POLICY.display_lines(MINING.observe(f.layout))))

    def test_unknown_snapshot_and_overflow_do_not_predict_mint_visibility(self):
        with MiningFixture() as f:
            f.enable(); f.run()
            f.candidate["pass"]["pass_id"] = "fa" * 32 + "i0"
            report = MINING.observe(f.layout)
            self.assertNotIn("indexer height", report["candidate_hint"])
            report["eligibility"]["candidate"]["external_state"]["btc_height"] = POLICY.MAX_HEIGHT
            f.pass_snapshot = {"inscription_id": f.candidate["pass"]["pass_id"], "mint_block_height": POLICY.MAX_HEIGHT,
                               "resolved_height": POLICY.MAX_HEIGHT, "state": "active", "usdb_main": ADDRESS}
            POLICY.candidate_hint(f.layout, report, MINING)
            self.assertNotIn("indexer height", report["candidate_hint"])

    @unittest.skipUnless((ROOT.parent / "go-ethereum/scripts/usdb/docker/usdb_runtime_node.sh").is_file(), "requires paired Go checkout")
    def test_real_entrypoint_passes_default_zero_and_rejects_bad_values(self):
        with RuntimeScriptFixture() as f:
            for env, expected in (({}, "2"), ({POLICY.KEY: "0"}, "0"), ({POLICY.KEY: "2"}, "2")):
                for role in ("full", "miner"):
                    result = f.run(**env, USDB_NODE_ROLE=role, USDB_MINER_ADDRESS=ADDRESS)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    argv = json.loads(f.argv_file.read_text())
                    self.assertEqual(argv[argv.index("--miner.usdb-anchor-gap")+1], expected)
            for value in ("-1", "", "1.5", "4294967296", "01", "999999999999999999999"):
                self.assertNotEqual(f.run(**{POLICY.KEY: value}).returncode, 0)


if __name__ == "__main__":
    unittest.main()
