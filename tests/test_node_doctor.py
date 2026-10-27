#!/usr/bin/env python3
"""Exercise grouped doctor output with real release validation and isolated host probes."""

from contextlib import contextmanager, redirect_stderr, redirect_stdout
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import node_doctor as presentation
import resource_policy as policy
import usdb_node as node
from common.native_node import native_kit
from common.controller_status import autostart_observation
from common.node_identity import container_identity, provision_identity


class DoctorTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-doctor-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        self.memory = mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB)
        self.memory.start()
        self.addCleanup(self.memory.stop)
        with mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(self.layout, data_root=self.root / "data", role="full", miner_address="",
                                miner_threads=1, bootnodes="", nat="none", bitcoin_rpc_user=None,
                                bitcoin_p2p="private", resource_management="auto")
        provision_identity(self.layout, node)
        self.controller = mock.Mock(return_value=autostart_observation())

    def helper(self, layout, helper, args, **kwargs):
        self.assertTrue(kwargs.get("capture_output"))
        self.assertFalse(kwargs.get("output_to_stderr"))
        if helper == "prepare_usdb_host.sh":
            return subprocess.CompletedProcess([], 0, "INFO distribution: ubuntu 26.04\nPASS Docker daemon: active\n", "")
        self.assertEqual(helper, "run_testnet_runtime.sh")
        self.assertEqual(args, ["validate-node"])
        return subprocess.CompletedProcess([], 0, "Node configuration is valid.\n", "")

    def command(self, helper=None):
        output, errors = io.StringIO(), io.StringIO()
        args = ["usdb-node", "--kit-root", str(self.layout.kit_root), "--node-env", str(self.layout.node_env), "doctor"]
        with mock.patch.object(sys, "argv", args), mock.patch.object(node, "run_helper", side_effect=helper or self.helper), \
                mock.patch("node_controller_status.inspect_controller", self.controller), \
                redirect_stdout(output), redirect_stderr(errors):
            result = node.main()
        self.assertEqual(errors.getvalue(), "")
        self.assertNotIn("\x1b", output.getvalue())
        return result, output.getvalue()

    def test_native_first_start_report_preserves_config_and_explains_boundaries(self):
        original = self.layout.node_env.read_bytes()
        with mock.patch.object(node, "install_snapshot_artifact") as install:
            result, output = self.command()
        install.assert_not_called()
        self.assertEqual(result, 0)
        self.assertEqual(self.layout.node_env.read_bytes(), original)
        compact = " ".join(output.split())
        self.assertIn("Chain ID: 202608250 | Role: full", compact)
        self.assertIn("Result: PASSED WITH NOTES", compact)
        self.assertLess(output.index("Needs attention"), output.index("\nHost and Docker"))
        self.assertIn("AssumeUTXO", output)
        self.assertIn("up pulls required images automatically", compact)
        self.assertIn("External firewall mode", compact)
        self.assertIn("local checks do not prove public reachability", compact)
        self.assertIn("does not mean services are running or synchronized", compact)
        self.assertNotIn("Not checked", output)

    def test_missing_identity_blocks_doctor_status_and_up_without_generation(self):
        import node_identity
        key = node_identity.key_path(self.layout, node)
        key.unlink()
        result, output = self.command()
        self.assertEqual(result, 1)
        self.assertIn('P2P_IDENTITY_REQUIRED', output)
        with mock.patch('node_resource_lifecycle.prepare_for_up'), \
                mock.patch.object(node, 'run_helper') as helper:
            report, code = node.up_node(self.layout, dry_run=False, allow_activation=False,
                sync_timeout_secs=1, pull=True, json_output=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['status']['overall_state'], 'BLOCKED')
        self.assertIn('identity restore', report['status']['checks']['data']['summary'])
        helper.assert_not_called()
        self.assertFalse(key.exists())

    def test_autostart_is_advisory_and_never_installs_or_changes_services(self):
        cases = (
            ({}, "ENABLED", "PASS"),
            ({"autostart": "disabled", "install_command": "usdb-node controller install --sync-timeout-secs 456 --skip-pull"}, "DISABLED", "WARN"),
            ({"configuration_state": "missing"}, "MISSING", "WARN"),
            ({"configuration_state": "custom"}, "REVIEW_REQUIRED", "WARN"),
            ({"observation_available": False}, "UNAVAILABLE", "WARN"),
        )
        original = self.layout.node_env.read_bytes()
        for updates, state, severity in cases:
            with self.subTest(state=state), mock.patch.object(node, "install_controller_unit") as install, \
                    mock.patch.object(node, "_privileged_command") as systemctl:
                self.controller.return_value = autostart_observation(**updates)
                code, output = self.command()
                compact = " ".join(output.split())
                self.assertEqual(code, 0)
                self.assertIn(f"{severity} Autostart {state}", compact)
                if state == "DISABLED":
                    self.assertIn("--kit-root " + str(self.layout.kit_root), compact)
                    self.assertIn("--node-env " + str(self.layout.node_env), compact)
                    self.assertIn("controller install --sync-timeout-secs 456 --skip-pull", compact)
                    self.assertIn("otherwise leave it disabled", compact)
                if state == "MISSING":
                    self.assertIn("up --foreground", compact)
                if state in {"REVIEW_REQUIRED", "UNAVAILABLE"}:
                    self.assertIn("controller status", compact)
                install.assert_not_called()
                systemctl.assert_not_called()
                self.assertEqual(original, self.layout.node_env.read_bytes())

    def test_uncached_identity_helper_is_pending_and_other_preflight_checks_continue(self):
        import node_identity
        key = node_identity.key_path(self.layout, node)
        original = key.read_bytes(), self.layout.node_env.read_bytes()
        with container_identity(self.layout, node) as image:
            result, output = self.command()
            self.assertFalse(image.cached)
            self.assertTrue(all(command[1] in {'run', 'image'} for command in image.commands))
        self.assertEqual(result, 0)
        compact = ' '.join(output.split())
        self.assertIn('PENDING P2P identity', compact)
        self.assertIn('Result: PASSED WITH NOTES', compact)
        self.assertNotIn('FAIL', output)
        self.assertNotIn('Not checked', output)
        self.assertNotIn('P2P identity', output.split('Needs attention\n')[1].split('\nRelease\n')[0])
        self.assertIn('Runtime configuration', output)
        self.assertNotIn('usdb-node pull', output)
        self.assertEqual(original, (key.read_bytes(), self.layout.node_env.read_bytes()))

    def test_cached_identity_helper_validates_and_real_identity_errors_still_fail(self):
        with container_identity(self.layout, node) as image:
            image.cached = True
            for state, code, message in (('READY', 0, 'Preflight passed'),
                                         ('MISSING', 1, 'P2P_IDENTITY_REQUIRED'),
                                         ('INVALID', 1, 'P2P_IDENTITY_INVALID')):
                with self.subTest(state=state):
                    image.report['state'] = state
                    result, output = self.command()
                    self.assertEqual(result, code)
                    self.assertIn(message, output)
                    self.assertNotIn('PENDING P2P identity', output)

    @contextmanager
    def startup(self, image, *, pull_fails=False):
        """Run real preflight/startup ordering with only external operations modeled."""
        import assumeutxo_node
        import node_image_progress
        original_status = node.collect_node_status
        started = []

        def status(layout):
            if started:
                return dict(overall_state='READY', checks={})
            return original_status(layout)

        def helper(layout, name, args, **kwargs):
            image.events.append(' '.join(args))
            if args == ['pull', 'usdb-chain']:
                if pull_fails:
                    raise subprocess.CalledProcessError(1, ['docker', 'pull'], stderr='registry unavailable')
                image.cached = True
            else:
                self.assertIn(args, (['check', '--docker-user', node.default_docker_user()],
                                     ['validate-node'], ['pull', 'balance-history'], ['pull'], ['up-console']))
            return subprocess.CompletedProcess([], 0, '', '')

        with mock.patch('node_resource_lifecycle.prepare_for_up'), \
                mock.patch.object(node, '_runtime_lifecycle_status', return_value={'state': 'stopped'}), \
                mock.patch.object(node, 'collect_node_status', side_effect=status), \
                mock.patch.object(node, 'run_helper', side_effect=helper), \
                mock.patch.object(node, '_start_optional_ord') as optional, \
                mock.patch.object(node_image_progress, 'image_cached', side_effect=lambda ref:
                                  image.cached and ref == self.layout.images['USDB_CHAIN_IMAGE']), \
                mock.patch.object(assumeutxo_node, 'start_native_node', side_effect=lambda *a, **k: started.append(True)) as start, \
                redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            yield start, optional

    def up(self, *, dry_run=False, pull=True):
        return node.up_node(self.layout, dry_run=dry_run, allow_activation=False,
                            sync_timeout_secs=1, pull=pull, json_output=True)

    def test_up_resolves_pending_image_then_checks_identity_before_starting(self):
        import node_identity
        key = node_identity.key_path(self.layout, node)
        original = key.read_bytes()
        with container_identity(self.layout, node) as image, self.startup(image) as (start, optional):
            preview, code = self.up(dry_run=True)
            self.assertEqual(code, 0)
            self.assertEqual(preview['status']['overall_state'], 'PREPARATION_REQUIRED')
            self.assertEqual(preview['status']['checks']['data']['state'], 'pending')
            self.assertEqual(preview['planned_actions'], ['up'])
            self.assertEqual(image.events, [])
            result, code = self.up()
            self.assertEqual((code, result['outcome']), (0, 'ready'))
            self.assertEqual(result['completed_actions'], ['up'])
            self.assertLess(image.events.index('pull usdb-chain'), image.events.index('identity-READY'))
            self.assertLess(image.events.index('identity-READY'), image.events.index('up-console'))
            start.assert_called_once()
            optional.assert_called_once()
        self.assertEqual(key.read_bytes(), original)

    def test_up_does_not_start_services_when_post_pull_identity_is_missing_or_invalid(self):
        for state, message in (('MISSING', 'P2P_IDENTITY_REQUIRED'), ('INVALID', 'P2P_IDENTITY_INVALID')):
            with self.subTest(state=state), container_identity(self.layout, node) as image, \
                    self.startup(image) as (start, optional):
                image.report['state'] = state
                with self.assertRaisesRegex(ValueError, message):
                    self.up()
                self.assertTrue(image.cached)
                self.assertNotIn('up-console', image.events)
                start.assert_not_called()
                optional.assert_not_called()

    def test_background_up_accepts_pending_identity_without_pulling_or_starting_containers(self):
        with container_identity(self.layout, node) as image, self.startup(image) as (start, optional), \
                mock.patch.object(node, 'ensure_background_services', return_value={}), \
                mock.patch.object(node, '_require_controller_unit'), \
                mock.patch.object(node, 'start_controller_unit', return_value='test-controller.service') as controller:
            result, code = node.submit_up_to_controller(self.layout, dry_run=False, allow_activation=False)
            self.assertEqual((code, result['outcome']), (0, 'controller_started'))
            self.assertEqual(result['status']['overall_state'], 'PREPARATION_REQUIRED')
            controller.assert_called_once()
            self.assertFalse(image.cached)
            self.assertEqual(image.events, [])
            start.assert_not_called()
            optional.assert_not_called()

    def test_pending_identity_does_not_hide_unhealthy_runtime(self):
        for runtime, overall in (('degraded', 'DEGRADED'), ('unavailable', 'BLOCKED')):
            with self.subTest(runtime=runtime), container_identity(self.layout, node), \
                    mock.patch.object(node, '_runtime_lifecycle_status', return_value={'state': runtime}):
                report = node.collect_node_status(self.layout)
                self.assertEqual(report['overall_state'], overall)
                self.assertIsNone(node._up_action(report, False))

    def test_skip_pull_and_pull_failure_leave_identity_unverified_and_services_stopped(self):
        for pull in (False, True):
            with self.subTest(pull=pull), container_identity(self.layout, node) as image, \
                    self.startup(image, pull_fails=True) as (start, optional):
                error = subprocess.CalledProcessError if pull else ValueError
                with self.assertRaises(error) as raised:
                    self.up(pull=pull)
                if not pull:
                    self.assertIn('without --skip-pull', str(raised.exception))
                    self.assertFalse(any(event.startswith('pull') for event in image.events))
                self.assertFalse(image.cached)
                self.assertNotIn('up-console', image.events)
                start.assert_not_called()
                optional.assert_not_called()

    def test_host_session_block_preserves_both_streams_and_stops_later_checks(self):
        calls = []

        def helper(layout, helper, args, **kwargs):
            calls.append(helper)
            raise subprocess.CalledProcessError(20, ["/private/helper", "check"],
                output="PASS Docker account: lyx is registered in the docker group\n",
                stderr="WAIT Docker access: the current terminal has not loaded the docker group\n"
                       "ACTION REQUIRED [DOCKER_SESSION_REFRESH_REQUIRED]: refresh your login session.\n")

        result, output = self.command(helper)
        compact = " ".join(output.split())
        self.assertEqual(result, 1)
        self.assertEqual(calls, ["prepare_usdb_host.sh"])
        self.assertIn("Result: ACTION REQUIRED", output)
        self.assertIn("PASS Docker account", compact)
        self.assertIn("WAIT Docker access", compact)
        self.assertIn("DOCKER_SESSION_REFRESH_REQUIRED", output)
        self.assertIn("newgrp docker", compact)
        self.assertIn("usdb-node host check && usdb-node doctor", compact)
        self.assertIn("Not checked", output)
        self.assertNotIn("Preflight passed", output)
        self.assertNotIn("/private/helper", output)

    def test_missing_dependency_remains_a_failure_with_actionable_detail(self):
        def helper(*args, **kwargs):
            raise subprocess.CalledProcessError(1, ["helper"], output="PASS kernel: Linux\n",
                                                stderr="FAIL jq: jq is missing\n")
        result, output = self.command(helper)
        self.assertEqual(result, 1)
        self.assertIn("FAIL jq jq is missing", " ".join(output.split()))
        self.assertIn("HOST_PREREQUISITES_FAILED", output)
        self.assertNotIn("newgrp docker", output)

    def test_runtime_helper_failure_is_retained_not_reported_as_pass(self):
        def helper(layout, name, args, **kwargs):
            if name == "prepare_usdb_host.sh":
                return self.helper(layout, name, args, **kwargs)
            raise subprocess.CalledProcessError(1, ["/private/runtime"], stderr="ERROR bindings: unsafe API binding\n")
        result, output = self.command(helper)
        self.assertEqual(result, 1)
        self.assertIn("unsafe API binding", output)
        self.assertIn("Script registry", output.split("Not checked\n", 1)[1])
        self.assertNotIn("Preflight passed", output)
        self.assertNotIn("/private/runtime", output)

    def test_invalid_credentials_still_block_and_are_not_dumped(self):
        env = node.read_env(self.layout.node_env)
        secret = "doctor-secret-that-must-not-be-displayed"
        self.layout.node_env.write_text(node.render_env(self.layout.node_env.read_text(), {"BTC_RPC_PASSWORD": secret}))
        result, output = self.command()
        self.assertEqual(result, 1)
        self.assertIn("rpcauth does not match", output)
        self.assertNotIn(secret, output)
        self.assertNotIn(env["BTC_RPC_PASSWORD"], output)

    def test_managed_firewall_result_and_errors_appear_in_the_report(self):
        self.layout.node_env.write_text(node.render_env(self.layout.node_env.read_text(), {"USDB_FIREWALL_MODE": "managed"}))
        for fail in (False, True):
            with self.subTest(fail=fail):
                def helper(layout, name, args, **kwargs):
                    if name != "prepare_usdb_firewall.sh":
                        return self.helper(layout, name, args, **kwargs)
                    self.assertEqual(args[0], "check")
                    self.assertTrue(kwargs.get("capture_output"))
                    if fail:
                        raise subprocess.CalledProcessError(1, ["helper"], stderr="ERROR: ufw is missing\n")
                    return subprocess.CompletedProcess([], 0, "PASS UFW status: active\n", "")
                result, output = self.command(helper)
                self.assertEqual(result, 1 if fail else 0)
                self.assertNotIn("External firewall", output)
                if fail:
                    self.assertIn("- Startup after reboot", output.split("Not checked\n", 1)[1])
                    self.assertIn("ufw is missing", output.split("Needs attention\n", 1)[1].split("\nRelease\n", 1)[0])
                    self.assertNotIn("Preflight passed", output)
                else:
                    self.assertNotIn("Not checked", output)
                    self.assertIn("Result: PASSED\n", output)
                    self.assertIn("PASS UFW status active", " ".join(output.split()))

    def test_missing_config_has_setup_guidance_and_unobserved_host(self):
        self.layout.node_env.unlink()
        with mock.patch.object(node, "run_host_action") as host:
            result, output = self.command()
        self.assertEqual(result, 1)
        host.assert_not_called()
        self.assertIn("usdb-node setup", output)
        self.assertIn("Host and Docker", output.split("Not checked\n", 1)[1])

    def test_bad_release_is_also_presented_without_starting_host_checks(self):
        self.layout.manifest_path.write_text("invalid manifest")
        with mock.patch.object(node, "run_host_action") as host:
            result, output = self.command()
        host.assert_not_called()
        self.assertEqual(result, 1)
        self.assertIn("USDB Doctor | release unavailable", output)
        self.assertIn("checksum", output.lower())
        self.assertIn("Not checked", output)

    def test_packaged_cli_can_render_release_failure_in_isolation(self):
        # Exercise the shipped dependency list without the checkout on PYTHONPATH.
        self.layout.manifest_path.write_text("invalid manifest")
        result = subprocess.run([sys.executable, str(self.layout.kit_root / "docker/scripts/tools/usdb_node.py"),
                                 "--kit-root", str(self.layout.kit_root), "doctor"],
                                cwd=self.root, env={**os.environ, "PYTHONPATH": ""}, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Result: ACTION REQUIRED", result.stdout)
        self.assertEqual(result.stderr, "")


class DoctorRenderingTests(unittest.TestCase):
    def test_narrow_terminal_preserves_long_error_and_recovery_text(self):
        report = presentation.DoctorReport()
        error = "P2P_IPV6_HOST_CHANGED: pinned IPv6 address is unavailable; use peers configure --ip-family dual --advertise-ipv6 auto"
        with self.assertRaises(ValueError):
            report.check("network", lambda: (_ for _ in ()).throw(ValueError(error)))
        for width in (40, 80, 120):
            for unicode in (False, True):
                with self.subTest(width=width, unicode=unicode):
                    output = report.render(width=width, unicode=unicode)
                    self.assertTrue(all(len(line) <= width for line in output.splitlines()))
                    self.assertIn(error, " ".join(output.split()))
                    self.assertNotIn("\x1b", output)

    def test_color_requires_tty_and_respects_no_color_and_dumb_terminal(self):
        report = presentation.DoctorReport()
        report.check("release", lambda: None)
        for tty, term, no_color, colored in (
            (False, "xterm", False, False), (True, "xterm", False, True),
            (True, "xterm", True, False), (True, "dumb", False, False),
        ):
            with self.subTest(tty=tty, term=term, no_color=no_color):
                output = io.StringIO()
                environment = {"TERM": term, **({"NO_COLOR": ""} if no_color else {})}
                with mock.patch.object(output, "isatty", return_value=tty), mock.patch.dict(os.environ, environment, clear=True):
                    report.print(output)
                self.assertEqual("\x1b" in output.getvalue(), colored)

    def test_captured_signature_notice_is_nested_and_not_a_problem(self):
        report = presentation.DoctorReport()
        report.check("release", lambda: print("Artifact signature verified: type=bitcoin-assumeutxo, signer=test", file=sys.stderr))
        output = report.render()
        self.assertLess(output.index("\nRelease\n"), output.index("Artifact signature verified"))
        self.assertNotIn("Artifact signature", output.split("Needs attention\n", 1)[1].split("\nRelease\n", 1)[0])

    def test_known_secret_values_and_escape_sequences_are_not_rendered(self):
        report = presentation.DoctorReport()
        report.secrets = ["private-rpc-password"]
        report.add("data", "FAIL", "credentials", "\x1b[31mprivate-rpc-password\x1b[0m is invalid")
        report.sections["data"].state = "FAIL"
        report.failed = True
        output = report.render()
        self.assertIn("[redacted]", output)
        self.assertNotIn("private-rpc-password", output)
        self.assertNotIn("\x1b", output)


if __name__ == "__main__":
    unittest.main()
