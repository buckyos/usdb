"""Offline configuration visibility must not leak credentials or mutate a node."""

import argparse
from contextlib import ExitStack, redirect_stdout, redirect_stderr
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_node as node
from common.native_node import native_kit


class ConfigViewTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)

    def command(self, *options):
        out, err = io.StringIO(), io.StringIO()
        argv = ["usdb-node", "--kit-root", str(self.layout.kit_root), "--node-env", str(self.layout.node_env), "config", *options]
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with ExitStack() as stack:
            stack.enter_context(mock.patch.object(sys, "argv", argv))
            stack.enter_context(redirect_stdout(out))
            stack.enter_context(redirect_stderr(err))
            for name in ("load_release_layout", "validate_network_bundle", "run_helper", "node_operation_lock", "_collect_compose_services", "_atomic_write_private"):
                stack.enter_context(mock.patch.object(node, name, side_effect=AssertionError(name)))
            stack.enter_context(mock.patch.object(node.subprocess, "run", side_effect=AssertionError("external process")))
            code = node.main()
        after = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        return code, out.getvalue(), err.getvalue()

    def test_before_setup_is_read_only_and_explains_next_step(self):
        code, out, err = self.command()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("usdb-node setup", out)
        self.assertIn("Only bundled network settings", out)
        self.assertFalse(self.layout.node_env.exists())
        code, out, err = self.command("--json")
        report = json.loads(out)
        self.assertFalse(report["configured"])
        self.assertFalse(report["runtime_observed"])
        self.assertEqual(report["monitor"]["state"], "unconfigured")

    def test_all_saved_keys_have_source_and_secrets_are_hidden_in_both_formats(self):
        env = dict(self.layout.images, USDB_NODE_ROLE="full", USDB_DATA_ROOT="/data/usdb",
            USDB_MINER_THREADS="2", USDB_STORAGE_PROFILE="balanced", BTC_RPC_PASSWORD="rpc-secret",
            BTC_RPC_USER="rpc-user-secret", FUTURE_CREDENTIAL="future-secret",
            USDB_CHAIN_EXTRA_ARGS="--password extra-secret", EMPTY_CUSTOM="",
            BTC_RPC_URL="http://url-user:url-secret@localhost:8332/rpc?token=query-secret#fragment-secret")
        self.layout.node_env.write_text(node.upsert_env("", env))
        for options in ((), ("--json",)):
            code, out, err = self.command(*options)
            self.assertEqual((code, err), (0, ""))
            for secret in ("rpc-secret", "rpc-user-secret", "future-secret", "extra-secret", "url-user", "url-secret", "query-secret", "fragment-secret"):
                self.assertNotIn(secret, out)
            self.assertIn("localhost:8332/rpc", out)
            self.assertIn("[redacted]", out)
            if options:
                report = json.loads(out)
                rows = {r["key"]: r for group in report["groups"] for r in group["settings"] if r["source"] == "node.env"}
                self.assertEqual(set(rows), set(env))
                self.assertEqual(rows["USDB_DATA_ROOT"]["value"], "/data/usdb")
                self.assertEqual(rows["EMPTY_CUSTOM"]["value"], "")
                self.assertTrue(rows["FUTURE_CREDENTIAL"]["redacted"])
                self.assertEqual(report["monitor"]["state"], "defaults")

    def test_saved_monitor_thresholds_and_network_overrides_remain_distinguishable(self):
        self.layout.node_env.write_text("BTC_MIN_READY_HEIGHT=42\n")
        directory = self.root / "monitor"
        directory.mkdir(mode=0o700)
        config = directory / "config.json"
        config.write_text('{"interval_secs":60}')
        config.chmod(0o600)
        code, out, err = self.command("--json")
        report = json.loads(out)
        self.assertEqual(report["monitor"]["settings"]["interval_secs"], 60)
        self.assertEqual(report["monitor"]["state"], "saved_with_defaults")
        rows = [r for group in report["groups"] for r in group["settings"] if r["key"] == "BTC_MIN_READY_HEIGHT"]
        self.assertEqual({r["source"] for r in rows}, {"node.env", "network.env"})
        config.write_text('{"secret-unknown":"not-for-output"}')
        code, out, err = self.command("--json")
        self.assertNotIn("not-for-output", out)
        self.assertEqual(json.loads(out)["monitor"]["state"], "unavailable")

    def test_invalid_configuration_and_nonregular_files_fail_without_echoing_secrets(self):
        self.layout.node_env.write_text("private-secret!=bad-key\n")
        for options in ((), ("--json",)):
            code, out, err = self.command(*options)
            self.assertEqual(code, 1)
            self.assertNotIn("private-secret", out + err)
            self.assertIn("Cannot read node.env", out + err)
            if options:
                self.assertEqual(json.loads(out)["outcome"], "error")
                self.assertEqual(err, "")
        self.layout.node_env.unlink()
        os.mkfifo(self.layout.node_env)
        code, out, err = self.command("--json")
        self.assertEqual(code, 1)
        self.assertIn("Cannot read node.env", out)

    def test_terminal_control_characters_cannot_rewrite_display(self):
        self.layout.node_env.write_text("USDB_DATA_ROOT=/data/\x1b[2Jusdb\n")
        code, out, err = self.command()
        self.assertEqual(code, 0, err)
        self.assertNotIn("\x1b", out)
        self.assertIn("\\u001b", out)

    def test_public_options_have_help_and_configuration_entry_points_are_clear(self):
        parser = node.build_parser()
        def walk(current):
            for action in current._actions:
                if action.option_strings:
                    self.assertIsNotNone(action.help, f"{current.prog} {action.option_strings}")
                if isinstance(action, argparse._SubParsersAction):
                    for child in action.choices.values():
                        walk(child)
        walk(parser)
        commands = next(a for a in parser._actions if isinstance(a, argparse._SubParsersAction)).choices
        self.assertIn("Non-interactive", parser.format_help())
        self.assertIn("does not install the controller", commands["configure"].description)
        self.assertIn("usdb-node config", commands["setup"].description)
        self.assertEqual(parser.parse_args(["configure"]).role, "full")


if __name__ == "__main__":
    unittest.main()
