#!/usr/bin/env python3
"""Accept chain startup failures, restart display recovery, and executable gates."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_node as NODE  # noqa: E402
from common.node_progress import progress_fixture  # noqa: E402


class ChainStartupTests(unittest.TestCase):
    def services(self, **updates):
        return {**{name: {"state": "running", "exit_code": 0} for name in
                   ("btc-node", "balance-history", "usdb-indexer")}, **updates}

    def progress(self, services, *, pending=False):
        with progress_fixture(services, pending=pending) as layout:
            report = NODE.collect_node_progress(layout)
        chain = next(item for item in report["components"] if item["id"] == "usdb_chain")
        return report, chain

    def test_created_gate_exec_failure_survives_restart_intent_and_repeated_probes(self):
        for pending in (False, True):
            with self.subTest(pending=pending):
                services = self.services(**{
                    "usdb-chain-init": {"state": "exited", "exit_code": 0},
                    "paired-checkpoint-recovery": {"state": "created", "exit_code": 126,
                                                   "container_error": "exec: permission denied"},
                })
                for _ in range(2):
                    report, chain = self.progress(services, pending=pending)
                    self.assertEqual(report["overall_state"], "FAILED")
                    self.assertEqual(chain["state"], "FAILED")
                    self.assertIn("paired-checkpoint-recovery", chain["detail"])
                    self.assertIn("permission denied", chain["detail"])
                    self.assertNotIn("planned resource transition", chain["detail"])

    def test_each_failed_gate_and_control_plane_reports_its_name(self):
        for name in ("usdb-chain-init", "paired-checkpoint-recovery", "usdb-control-plane"):
            with self.subTest(name=name):
                services = self.services(**{name: {"state": "exited", "exit_code": 1}})
                if name == "usdb-control-plane":
                    services["usdb-chain"] = {"state": "running", "exit_code": 0}
                report, chain = self.progress(services)
                self.assertEqual(report["overall_state"], "FAILED")
                self.assertIn(name, chain["detail"])
                self.assertIn("exit_code=1", chain["detail"])

    def test_clean_planned_stop_is_starting_but_failed_chain_start_is_failed(self):
        for exit_code, expected in ((0, "STARTING"), (126, "FAILED")):
            with self.subTest(exit_code=exit_code):
                _, chain = self.progress(self.services(**{
                    "usdb-chain": {"state": "exited", "exit_code": exit_code},
                }), pending=True)
                self.assertEqual(chain["state"], expected)
                self.assertEqual("planned resource transition" in chain["detail"], exit_code == 0)

    def test_gate_running_and_successful_retry_clear_old_failure(self):
        services = self.services(**{
            "usdb-chain-init": {"state": "exited", "exit_code": 0},
            "paired-checkpoint-recovery": {"state": "running", "exit_code": 0},
        })
        _, chain = self.progress(services)
        self.assertEqual(chain["state"], "STARTING")
        self.assertIn("paired-checkpoint-recovery: running", chain["detail"])
        services["paired-checkpoint-recovery"] = {"state": "exited", "exit_code": 0}
        _, chain = self.progress(services)
        self.assertEqual(chain["state"], "STARTING")
        self.assertIn("managed service startup", chain["detail"])
        services["usdb-chain"] = {"state": "running", "exit_code": 0}
        _, chain = self.progress(services)
        self.assertEqual(chain["state"], "READY")

    def test_running_gate_does_not_hide_chain_exec_failure(self):
        _, chain = self.progress(self.services(**{
            "usdb-chain": {"state": "created", "exit_code": 126,
                           "container_error": "chain entrypoint permission denied"},
            "paired-checkpoint-recovery": {"state": "running", "exit_code": 0},
        }))
        self.assertEqual(chain["state"], "FAILED")
        self.assertIn("chain entrypoint permission denied", chain["detail"])

    def test_inspect_recovers_error_hidden_by_compose_created_status(self):
        identifier = "a" * 64
        compose = {"ID": identifier, "Service": "paired-checkpoint-recovery",
                   "State": "created", "ExitCode": 0}
        state = {"Status": "created", "ExitCode": 126, "Error": "permission denied"}
        with (
            mock.patch.object(NODE, "run_helper", side_effect=[
                subprocess.CompletedProcess([], 0, "[]"),
                subprocess.CompletedProcess([], 0, json.dumps([compose])),
            ]),
            mock.patch.object(NODE.subprocess, "run", return_value=
                              subprocess.CompletedProcess([], 0, json.dumps(state))) as inspect,
        ):
            services = NODE._collect_compose_services(object(), command_timeout_secs=3)
        self.assertEqual(inspect.call_args.args[0],
                         ["docker", "inspect", "--format", "{{json .State}}", identifier])
        self.assertEqual(inspect.call_args.kwargs["timeout"], 3)
        report, chain = self.progress(self.services(**services))
        self.assertEqual(report["overall_state"], "FAILED")
        self.assertIn("exit_code=126", chain["detail"])

    def test_failed_inspect_never_uses_a_successful_observation(self):
        compose = {"ID": "a" * 64, "Service": "usdb-chain-init", "State": "created"}
        for failure in (subprocess.CalledProcessError(1, "docker"),
                        subprocess.TimeoutExpired("docker", 3)):
            with (
                self.subTest(failure=type(failure).__name__),
                mock.patch.object(NODE, "run_helper", return_value=
                                  subprocess.CompletedProcess([], 0, json.dumps([compose]))),
                mock.patch.object(NODE.subprocess, "run", side_effect=failure),
            ):
                with self.assertRaises((ValueError, subprocess.TimeoutExpired)):
                    NODE._collect_compose_services(object(), command_timeout_secs=3)

    def test_executable_checkpoint_entrypoint_preserves_mode_and_verifier_gates(self):
        entrypoint = ROOT / "docker/scripts/entrypoints/verify_paired_checkpoint_recovery.sh"
        with tempfile.TemporaryDirectory(prefix="usdb-checkpoint-entrypoint-") as temporary:
            root = Path(temporary)
            verifier = root / "usdb-indexer-checkpoint-tool"
            verifier.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 23\n")
            verifier.chmod(0o755)
            env = {"PATH": f"{root}:/usr/bin:/bin", "SNAPSHOT_MODE": "balance-history"}
            result = subprocess.run([str(entrypoint)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("verification disabled", result.stdout)
            env["SNAPSHOT_MODE"] = "paired-checkpoint"
            result = subprocess.run([str(entrypoint)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn("requires checkpoint manifest", result.stderr)
            env.update(USDB_INDEXER_CHECKPOINT_MANIFEST="/test/checkpoint.json",
                       BH_SNAPSHOT_TRUSTED_KEYS_FILE="/test/trust.json")
            result = subprocess.run([str(entrypoint)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 23)
            self.assertIn("verify-recovery\n--checkpoint-manifest\n/test/checkpoint.json", result.stdout)


if __name__ == "__main__":
    unittest.main()
