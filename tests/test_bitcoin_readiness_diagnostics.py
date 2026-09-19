"""Exercise RPC failure classification and bounded mining probes without node mutations."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import ssl
import subprocess
import sys
import unittest
from unittest import mock
import urllib.error

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import assumeutxo_node as NATIVE
import bitcoin_assumeutxo as BOOT
import usdb_mining as MINING
import usdb_node as NODE
from common.mining import ADDRESS, MiningFixture
from common.bitcoin_readiness import READY, failed_rpc


class RpcDiagnosticsTests(unittest.TestCase):
    def test_transport_and_protocol_failures_have_safe_retry_policy(self):
        secret = "credential-secret https://private.invalid/rpc"
        cases = [
            (TimeoutError(secret), "timeout", True, None),
            (urllib.error.URLError(TimeoutError(secret)), "timeout", True, None),
            (ConnectionRefusedError(secret), "connection", True, None),
            (urllib.error.URLError(ssl.SSLError(secret)), "tls", False, None),
            (urllib.error.HTTPError(secret, 401, secret, {}, None), "authentication", False, 401),
            (urllib.error.HTTPError(secret, 503, secret, {}, None), "service_unavailable", True, 503),
            (urllib.error.HTTPError(secret, 302, secret, {}, None), "http_error", False, 302),
            (b"not JSON: credential-secret", "invalid_response", False, None),
            (b'{"id":"wrong","result":true}', "invalid_response", False, None),
            (b'{"id":"usdb-assumeutxo"}', "invalid_response", False, None),
            (b'{"id":"usdb-assumeutxo","error":{"code":-28,"message":"credential-secret"}}', "warmup", True, -28),
            (b'{"id":"usdb-assumeutxo","error":{"code":-32601,"message":"credential-secret"}}', "rpc_error", False, -32601),
            (b'{"id":"usdb-assumeutxo","error":{"code":"credential-secret"}}', "invalid_response", False, None),
        ]
        for response, kind, retryable, code in cases:
            with self.subTest(kind=kind, code=code), mock.patch.object(BOOT.urllib.request, "build_opener") as opener:
                if isinstance(response, Exception):
                    opener.return_value.open.side_effect = response
                else:
                    opener.return_value.open.return_value = io.BytesIO(response)
                rpc = BOOT.Rpc("http://private.invalid/rpc", user="user", password="credential-secret")
                with self.assertRaises(BOOT.RpcFailure) as raised:
                    rpc.call("getchainstates")
                self.assertEqual(raised.exception.diagnostic(), dict(method="getchainstates", kind=kind, code=code, retryable=retryable))
                self.assertNotIn("credential-secret", str(raised.exception) + json.dumps(raised.exception.diagnostic()))
                self.assertNotIn("private.invalid", str(raised.exception))

    def test_http_500_preserves_structured_warmup_and_cookie_failure_is_not_transport(self):
        body = io.BytesIO(b'{"id":"usdb-assumeutxo","error":{"code":-28,"message":"secret"}}')
        with mock.patch.object(BOOT.urllib.request, "build_opener") as opener:
            opener.return_value.open.side_effect = urllib.error.HTTPError("http://private.invalid", 500, "secret", {}, body)
            with self.assertRaises(BOOT.RpcFailure) as raised:
                BOOT.Rpc("http://private.invalid", user="u", password="secret").call("getblockchaininfo")
            self.assertEqual(raised.exception.kind, "warmup")
            self.assertTrue(raised.exception.retryable)
        with mock.patch.object(Path, "read_text", side_effect=PermissionError("secret")), \
                mock.patch.object(BOOT.urllib.request, "build_opener") as opener:
            with self.assertRaises(BOOT.RpcFailure) as raised:
                BOOT.Rpc("http://private.invalid", cookie=Path("/private/cookie")).call("getblockchaininfo")
            self.assertEqual(raised.exception.kind, "authentication")
            opener.assert_not_called()

    def test_status_cli_exposes_additive_metadata_without_raw_error_details(self):
        output = io.StringIO()
        with mock.patch.object(sys, "argv", ["bitcoin_assumeutxo.py", "status"]), \
                mock.patch.dict(os.environ, {"BTC_RPC_USER": "u", "BTC_RPC_PASSWORD": "secret"}, clear=True), \
                mock.patch.object(BOOT, "core_status", side_effect=BOOT.RpcFailure("getchainstates", kind="timeout")), \
                redirect_stdout(output):
            self.assertEqual(BOOT.main(), 1)
        report = json.loads(output.getvalue())
        self.assertEqual(report, failed_rpc())
        self.assertNotIn("secret", output.getvalue())


class MiningProbeTests(unittest.TestCase):
    def test_transient_failures_recover_from_fresh_probe_without_waiting_for_history(self):
        for failed in (failed_rpc(), failed_rpc("connection"), failed_rpc("warmup", -28),
                       {k: v for k, v in failed_rpc().items() if k != "rpc_failure"}):
            with self.subTest(failed=failed), MiningFixture() as fixture, redirect_stderr(io.StringIO()) as output:
                fixture.update_env(SNAPSHOT_MODE="assumeutxo")
                with mock.patch.object(NATIVE, "core_progress", side_effect=[failed, failed, READY]) as probe:
                    plan = MINING.preflight(fixture.layout, ADDRESS, first_node=True)
                self.assertEqual(plan["state"], "READY")
                self.assertEqual(probe.call_count, 3)
                self.assertIn("retrying readiness probe", output.getvalue())
                self.assertEqual(fixture.calls, [])

    def test_exhausted_rpc_timeouts_do_not_change_role_or_create_operation(self):
        with MiningFixture() as fixture, redirect_stderr(io.StringIO()):
            fixture.update_env(SNAPSHOT_MODE="assumeutxo")
            before = fixture.layout.node_env.read_bytes()
            with mock.patch.object(NATIVE, "core_progress", return_value=failed_rpc()) as probe, \
                    self.assertRaisesRegex(ValueError, "BITCOIN_RPC_TIMEOUT:.*method=getchainstates.*attempts=3") as raised:
                fixture.enable()
            self.assertIn("Readiness is unknown", str(raised.exception))
            self.assertEqual(probe.call_count, 3)
            self.assertEqual(fixture.layout.node_env.read_bytes(), before)
            self.assertFalse(MINING.state_path(fixture.layout).exists())
            self.assertEqual(fixture.calls, [])

    def test_permanent_rpc_failures_and_real_unreadiness_never_retry(self):
        cases = [(failed_rpc("authentication", 401), "BITCOIN_RPC_AUTH_FAILED"),
                 (failed_rpc("invalid_response"), "BITCOIN_RPC_INVALID_RESPONSE"),
                 (failed_rpc("rpc_error", -32601), "BITCOIN_RPC_ERROR"),
                 (failed_rpc("tls"), "BITCOIN_RPC_ERROR"),
                 ({**READY, "tip_ready": False, "active_height": 964000, "headers": 965000}, "BITCOIN_NOT_READY")]
        for report, prefix in cases:
            with self.subTest(prefix=prefix), MiningFixture() as fixture:
                fixture.update_env(SNAPSHOT_MODE="assumeutxo")
                with mock.patch.object(NATIVE, "core_progress", return_value=report) as probe, \
                        self.assertRaisesRegex(ValueError, prefix) as raised:
                    MINING.preflight(fixture.layout, ADDRESS, first_node=True)
                self.assertEqual(probe.call_count, 1)
                if prefix == "BITCOIN_NOT_READY":
                    self.assertIn("active_height=964000, headers=965000", str(raised.exception))
                self.assertEqual(fixture.calls, [])

    def test_helper_failures_are_not_reported_as_bitcoin_rpc_failure(self):
        for body in ("", "secret diagnostic, not JSON", '{"schema_version":"wrong"}'):
            with self.subTest(body=body), MiningFixture() as fixture:
                fixture.update_env(SNAPSHOT_MODE="assumeutxo")
                result = subprocess.CompletedProcess([], 1, body, "private URL and password")
                with mock.patch.object(NODE, "run_helper", return_value=result) as helper, \
                        self.assertRaisesRegex(ValueError, "BITCOIN_PROBE_FAILED") as raised:
                    MINING.preflight(fixture.layout, ADDRESS, first_node=True)
                self.assertEqual(helper.call_count, 1)
                self.assertNotIn("secret", str(raised.exception))
                self.assertNotIn("password", str(raised.exception))
        with MiningFixture() as fixture, mock.patch.object(NATIVE, "core_progress", side_effect=OSError("secret")), \
                self.assertRaisesRegex(ValueError, "BITCOIN_PROBE_FAILED") as raised:
            MINING.bitcoin_check(fixture.layout, {"SNAPSHOT_MODE": "assumeutxo"})
        self.assertNotIn("secret", str(raised.exception))

    def test_deadline_limits_all_attempts_and_helper_timeouts(self):
        for duration, expected_calls, expected_timeouts in ((10, 3, [45, 33, 21]), (44, 1, [45])):
            with self.subTest(duration=duration), MiningFixture() as fixture, redirect_stderr(io.StringIO()):
                now = [0.0]
                def fail(*args, **kwargs):
                    now[0] += duration
                    raise subprocess.TimeoutExpired("secret", kwargs["command_timeout_secs"])
                with mock.patch.object(MINING.time, "monotonic", side_effect=lambda: now[0]), \
                        mock.patch.object(MINING.time, "sleep", side_effect=lambda delay: now.__setitem__(0, now[0] + delay)), \
                        mock.patch.object(NATIVE, "core_progress", side_effect=fail) as probe, \
                        self.assertRaisesRegex(ValueError, "BITCOIN_PROBE_TIMEOUT"):
                    MINING.bitcoin_check(fixture.layout, {"SNAPSHOT_MODE": "assumeutxo"})
                self.assertEqual(probe.call_count, expected_calls)
                self.assertEqual([call.kwargs["command_timeout_secs"] for call in probe.call_args_list], expected_timeouts)
                self.assertLessEqual(now[0], MINING.BITCOIN_PROBE_BUDGET_SECS)

    def test_entire_miner_switch_accepts_unfinished_history_and_rechecks_each_stage(self):
        with MiningFixture() as fixture:
            fixture.update_env(SNAPSHOT_MODE="assumeutxo")
            observations = []
            def helper(layout, script, arguments, **kwargs):
                if script == "run_testnet_bitcoin.sh" and arguments == ["progress"]:
                    observations.append(dict(READY))
                    return subprocess.CompletedProcess([], 0, json.dumps(READY), "")
                return fixture.helper(layout, script, arguments, **kwargs)
            with mock.patch.object(NODE, "run_helper", side_effect=helper):
                fixture.enable()
                self.assertEqual(fixture.run(), 0)
            self.assertEqual(MINING.read_state(fixture.layout)["phase"], "APPLIED")
            self.assertGreaterEqual(len(observations), 4)
            self.assertTrue(all(report["history_validated"] is False for report in observations))
            self.assertEqual(NODE.read_env(fixture.layout.node_env)["USDB_NODE_ROLE"], "miner")


if __name__ == "__main__":
    unittest.main()
