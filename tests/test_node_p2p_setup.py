#!/usr/bin/env python3
"""Verify operator-visible P2P checks before setup writes or host preparation exits."""
from contextlib import redirect_stdout
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE
import usdb_p2p as P2P
from common.enode import V6 as SEED6
from common.p2p import V4, V6
from common.p2p_setup import SetupFixture

REAL_CONFIGURE = NODE.configure_node


class P2PSetupTests(unittest.TestCase):
    def test_fallback_and_ipv6_seed_warning_precede_confirmation_and_can_cancel(self):
        with SetupFixture() as f:
            f.host["ipv6_ra_interfaces"] = [{"interface": "enp170s0", "accept_ra": "0"}]

            def review(text):
                self.assertIn("family=ipv4 (requested=auto)", text)
                self.assertIn("WARNING P2P_IPV4_FALLBACK", text)
                self.assertIn("P2P_IPV6_SEED_UNREACHABLE", text)
                self.assertIn("sudo sysctl -w net.ipv6.conf.enp170s0.accept_ra=2", text)
                self.assertIn("/etc/sysctl.d/", text)
                f.configure.assert_not_called()

            with self.assertRaisesRegex(ValueError, "setup cancelled"):
                f.run(seeds=SEED6, confirm="n", before_confirm=review)
            f.configure.assert_not_called()
            self.assertFalse(f.layout.node_env.exists())

    def test_healthy_dual_preserves_cli_endpoints_and_previews_written_values(self):
        with SetupFixture() as f:
            options = dict(requested="dual", advertise_ipv4=V4, advertise_ipv6=V6,
                           advertise_port=41303, discovery_port=41304)
            f.run(family="", seeds=SEED6, options=options)
            text = f.output.getvalue()
            self.assertIn("family=dual (requested=dual)", text)
            self.assertIn(f"Advertised IPv6: {V6}; TCP 41303, UDP 41304", text)
            self.assertNotIn("WARNING", text)
            passed = f.configure.call_args.kwargs
            self.assertEqual(passed["p2p_options"], options)
            self.assertEqual(passed["expected_p2p"], P2P.select(**options)[0])

    def test_explicit_dual_failure_never_writes_or_silently_falls_back(self):
        with SetupFixture() as f:
            f.host["ipv6_ra_interfaces"] = [{"interface": "eth0", "accept_ra": "1"}]
            with self.assertRaisesRegex(ValueError, "P2P_IPV6_RA_REQUIRED"):
                f.run(family="dual")
            f.configure.assert_not_called()
            self.assertIn("net.ipv6.conf.eth0.accept_ra=2", f.output.getvalue())

    def test_explicit_ipv4_remains_usable_without_ipv6(self):
        with SetupFixture() as f:
            f.host.update(ipv6=[], ipv6_default_route=False)
            f.run(family="ipv4")
            self.assertIn("family=ipv4 (requested=ipv4)", f.output.getvalue())
            self.assertNotIn("P2P_IPV4_FALLBACK", f.output.getvalue())
            f.engine.assert_not_called()
            f.configure.assert_called_once()

    def test_host_preview_distinguishes_missing_address_route_engine_and_probe(self):
        for issue, expected in (("address", "no usable stable IPv6 address"),
                                ("route", "no IPv6 default route"),
                                ("engine", "docker version"),
                                ("probe", "host inspection failed")):
            with self.subTest(issue=issue), SetupFixture() as f:
                if issue == "address":
                    f.host["ipv6"] = []
                elif issue == "route":
                    f.host["ipv6_default_route"] = False
                elif issue == "engine":
                    f.engine.side_effect = ValueError("P2P_IPV6_ENGINE_REQUIRED: old Docker")
                else:
                    f.host["errors"] = ["ip inspection unavailable"]
                report = P2P.host_preflight_report()
                self.assertIn("family=ipv4", report)
                self.assertIn(expected, report)
                self.assertIn("existing node configuration is unchanged", report)
                self.assertIn("peers configure --ip-family dual", report)

    def test_host_prepare_reports_optional_ipv6_failure_without_installing_packages(self):
        with SetupFixture() as f, mock.patch.object(NODE, "run_host_action", return_value=subprocess.CompletedProcess([], 0)) as action:
            f.host["ipv6_ra_interfaces"] = [{"interface": "eth0", "accept_ra": "0"}]
            NODE.prepare_host(f.layout, docker_user="operator", output=f.output,
                              input_fn=lambda _: self.fail("unexpected package installation prompt"))
            self.assertEqual(action.call_count, 1)
            self.assertIn("P2P_IPV4_FALLBACK", f.output.getvalue())

    def test_host_command_includes_read_only_p2p_preview(self):
        with SetupFixture() as f, mock.patch.object(NODE, "run_host_action"), redirect_stdout(f.output):
            args = NODE.build_parser().parse_args(["host", "check"])
            NODE._execute_command(f.layout, args)
            self.assertIn("family=dual", f.output.getvalue())
            f.configure.assert_not_called()

    def test_host_prepare_rechecks_p2p_after_package_install(self):
        with SetupFixture() as f, \
                mock.patch.object(NODE, "run_host_action", side_effect=[subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0)]) as action, \
                mock.patch.object(NODE.sys, "stdin", mock.Mock(isatty=lambda: True)), \
                mock.patch.object(NODE.sys, "stdout", mock.Mock(isatty=lambda: True)):
            f.host["ipv6_ra_interfaces"] = [{"interface": "eth0", "accept_ra": "1"}]
            NODE.prepare_host(f.layout, docker_user="operator", input_fn=lambda _: "yes", output=f.output)
            self.assertEqual([c.args[1] for c in action.call_args_list], ["check", "install"])
            self.assertIn("P2P_IPV6_RA_REQUIRED", f.output.getvalue())
            f.configure.assert_not_called()

    def test_changed_auto_selection_after_review_is_rejected_before_writing(self):
        for change in ({"ipv6_default_route": False}, {"ipv6": []}):
            with self.subTest(change=change), SetupFixture() as f:
                reviewed, _ = P2P.select("auto")
                f.host.update(change)
                # Exercise the real writer's guard before it touches any dataset or credentials.
                with self.assertRaisesRegex(ValueError, "P2P_HOST_CHANGED"):
                    REAL_CONFIGURE(f.layout, data_root=f.root / "data", role="full", miner_address="",
                                   miner_threads=1, bootnodes="", nat="", bitcoin_rpc_user=None,
                                   bitcoin_p2p="private", p2p_options={"requested": "auto"}, expected_p2p=reviewed)
                self.assertFalse((f.root / "data").exists())
                self.assertFalse(f.layout.node_env.exists())


if __name__ == "__main__":
    unittest.main()
