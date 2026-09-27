#!/usr/bin/env python3
"""Accept peer diagnostics without changing persistent configuration or services."""
import argparse
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_peer_check as CHECK
import usdb_peers as PEERS
from common.enode import V4, DNS
from common.peer_check import PeerCheckFixture


class PeerCheckTests(unittest.TestCase):
    def test_success_runs_isolated_probe_in_chain_network_without_state_changes(self):
        with PeerCheckFixture() as f:
            before = f.layout.node_env.read_bytes()
            report = CHECK.check(f.layout, V4)
            self.assertTrue(report["usable"])
            self.assertEqual(report["scope"]["kind"], "chain-container")
            argv, kwargs = f.probes[0]
            self.assertEqual(argv[argv.index("--network") + 1], "container:" + f.runtime["id"])
            self.assertIn("--pull=never", argv)
            self.assertIn("--read-only", argv)
            self.assertNotIn("--mount", argv)
            self.assertNotIn("--volume", argv)
            self.assertNotIn("fixture-secret", kwargs["input"])
            self.assertEqual(f.layout.node_env.read_bytes(), before)
            self.assertFalse(PEERS.state_path(f.layout).exists())
            self.assertEqual(f.calls, [])
            self.assertIn("peers add", report["next_command"])

    def test_stopped_chain_is_explicitly_a_host_network_observation(self):
        with PeerCheckFixture() as f:
            f.runtime["state"] = "exited"
            report = CHECK.check(f.layout, V4)
            self.assertTrue(report["usable"])
            self.assertEqual(report["scope"]["kind"], "host")
            self.assertIn("repeat with the chain running", " ".join(report["warnings"]))
            self.assertIn("host", f.probes[0][0])

    def test_invalid_input_and_timeout_never_invoke_docker(self):
        with PeerCheckFixture() as f:
            report = CHECK.check(f.layout, "enode://bad@localhost:31303")
            self.assertFalse(report["usable"])
            self.assertEqual(report["syntax"]["reason"], "INVALID_ENODE")
            for timeout in (0, 121, -1, 1.5, float("nan"), True):
                with self.assertRaises(ValueError): CHECK.check(f.layout, V4, timeout_secs=timeout)
            self.assertEqual(f.probes, [])

    def test_partial_transport_or_handshake_is_never_usable(self):
        for stage in CHECK.STAGES:
            with self.subTest(stage=stage), PeerCheckFixture() as f:
                def mutate(report):
                    report["endpoints"][0][stage] = {"state": "FAIL", "reason": "DISCOVERY_FAILED", "detail": "timeout"}
                f.mutate = mutate
                report = CHECK.check(f.layout, V4)
                self.assertFalse(report["usable"])
                self.assertEqual(report["state"], "PARTIAL")
                self.assertNotIn("next_command", report)

    def test_dns_failure_preserves_a_distinct_stage_and_guidance(self):
        with PeerCheckFixture() as f:
            def mutate(report):
                report["dns"] = {"state": "FAIL", "reason": "DNS_LOOKUP_FAILED", "detail": "no such host"}
                report["endpoints"] = []
            f.mutate = mutate
            report = CHECK.check(f.layout, DNS)
            self.assertEqual(report["state"], "FAIL")
            self.assertIn("DNS resolver", " ".join(report["guidance"]))

    def test_one_complete_address_can_pass_while_another_family_fails(self):
        with PeerCheckFixture() as f:
            def mutate(report):
                report["endpoints"].append({"ip": "2001:db8::1", "family": "ipv6", "tcp_port": 31303, "udp_port": 31303,
                    **{stage: {"state": "FAIL", "reason": "TCP_CONNECT_FAILED"} for stage in CHECK.STAGES}})
            f.mutate = mutate
            report = CHECK.check(f.layout, DNS)
            self.assertTrue(report["usable"])
            self.assertFalse(report["endpoints"][1]["usable"])

    def test_missing_or_wrong_helper_report_is_incomplete(self):
        for mutation in (lambda r: r.pop("endpoints"), lambda r: r.update(genesis_hash="0x" + "cc"*32),
                         lambda r: r["endpoints"][0].pop("eth"), lambda r: r.update(schema_version="old")):
            with PeerCheckFixture() as f:
                f.mutate = mutation
                report = CHECK.check(f.layout, V4)
                self.assertFalse(report["usable"])
                self.assertEqual(report["state"], "INCOMPLETE")

    def test_old_image_does_not_fall_back_to_claiming_tcp_success(self):
        with PeerCheckFixture() as f:
            f.runner.side_effect = None
            f.runner.return_value = SimpleNamespace(returncode=1, stdout="", stderr="invalid command: usdb-peer-check")
            report = CHECK.check(f.layout, V4)
            self.assertEqual(report["state"], "INCOMPLETE")
            self.assertIn("node kit and chain image", " ".join(report["guidance"]))

    def test_timeout_and_interrupt_remove_only_this_temporary_probe(self):
        for error in (subprocess.TimeoutExpired("docker", 45), KeyboardInterrupt()):
            with self.subTest(error=type(error)), PeerCheckFixture() as f:
                f.runner.side_effect = [error, SimpleNamespace(returncode=0)]
                if isinstance(error, KeyboardInterrupt):
                    with self.assertRaises(KeyboardInterrupt): CHECK.check(f.layout, V4)
                else:
                    self.assertEqual(CHECK.check(f.layout, V4)["state"], "INCOMPLETE")
                first, cleanup = f.runner.call_args_list
                name = first.args[0][first.args[0].index("--name") + 1]
                self.assertTrue(name.startswith("usdb-peer-check-"))
                self.assertEqual(cleanup.args[0], ["docker", "rm", "--force", name])

    def test_wrong_genesis_artifact_never_invokes_probe(self):
        with PeerCheckFixture() as f:
            (f.layout.bundle_dir / "genesis.json").write_text("{}")
            report = CHECK.check(f.layout, V4)
            self.assertEqual(report["state"], "INCOMPLETE")
            self.assertIn("checksum mismatch", report["detail"])
            self.assertEqual(f.probes, [])

    def test_cli_json_exit_codes_and_no_implicit_add(self):
        parser = argparse.ArgumentParser()
        PEERS.add_parser(parser.add_subparsers(dest="command", required=True))
        for state, expected in (("PASS", 0), ("PARTIAL", 1), ("INCOMPLETE", 2)):
            with PeerCheckFixture() as f, io.StringIO() as out:
                args = parser.parse_args(["peers", "check", V4, "--json", "--timeout-secs", "10"])
                report = {"state": state, "usable": state == "PASS"}
                with mock.patch.object(CHECK, "check", return_value=report) as check, redirect_stdout(out):
                    self.assertEqual(PEERS.execute(f.layout, args), expected)
                self.assertEqual(json.loads(out.getvalue()), report)
                check.assert_called_once_with(f.layout, V4, timeout_secs=10)
                self.assertFalse(PEERS.state_path(f.layout).exists())


if __name__ == "__main__": unittest.main()
