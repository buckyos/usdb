"""Offline configuration visibility must not leak credentials or mutate a node."""

import argparse
from contextlib import ExitStack, redirect_stdout, redirect_stderr
import io
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_node as node
from common.bootnodes import write_bootnodes
from common.enode import V4, V6, DNS
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
            stack.enter_context(mock.patch.object(socket, "getaddrinfo", side_effect=AssertionError("DNS probe")))
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

    def test_chain_settings_are_grouped_and_seed_lists_remain_distinct(self):
        env = dict(USDB_NODE_ROLE="full", USDB_CHAIN_DATA_HOST_DIR="/data/chain", USDB_CHAIN_GCMODE="archive",
                   USDB_CHAIN_TRACING="1", USDB_CHAIN_MEMORY_LIMIT="4g", USDB_P2P_IP_FAMILY="dual",
                   USDB_P2P_ADVERTISE_IPV6="auto", USDB_HTTP_BIND_ADDRESS="127.0.0.1", USDB_HTTP_BIND_PORT="8545",
                   USDB_WS_BIND_PORT="8546", USDB_MINER_THREADS="1", USDB_MINTING_ENABLED="1", ORD_COMMIT_INTERVAL="100",
                   USDB_BOOTNODES=",".join([V4, V6]))
        self.layout.node_env.write_text(node.upsert_env("", env))
        write_bootnodes(self.layout.bundle_dir, [DNS])
        code, out, err = self.command("--json")
        self.assertEqual((code, err), (0, ""))
        report = json.loads(out)
        groups = {group["name"]: {row["key"] for row in group["settings"]} for group in report["groups"]}
        chain_keys = set(env) - {"USDB_MINER_THREADS", "USDB_MINTING_ENABLED", "ORD_COMMIT_INTERVAL"}
        self.assertTrue(chain_keys.issubset(groups["USDB chain"]))
        self.assertTrue({"USDB_CHAIN_ID", "USDB_NETWORK_ID", "USDB_NETWORK_BUNDLE_ID"}.issubset(groups["USDB chain"]))
        self.assertEqual(groups["Mining"], {"USDB_MINER_THREADS"})
        self.assertEqual(groups["Inscriptions and optional minting (Ord)"], {"USDB_MINTING_ENABLED", "ORD_COMMIT_INTERVAL"})
        for name, keys in groups.items():
            if name != "USDB chain":
                self.assertFalse(chain_keys & keys)
        saved = report["peer_sources"]
        self.assertEqual(saved["configured"]["endpoints"], [V4, V6])
        self.assertEqual(saved["release_defaults"]["endpoints"], [DNS])
        self.assertFalse(saved["matches_release_defaults"])
        self.assertFalse(saved["runtime_observed"])
        code, out, err = self.command()
        self.assertEqual((code, err), (0, ""))
        chain = out.split("\nUSDB chain\n", 1)[1].split("\nMining\n", 1)[0]
        self.assertIn("Configured seeds: 2", chain)
        self.assertIn("Release default seeds: 1 — differs from saved endpoints", chain)
        for endpoint in (V4, V6, DNS):
            self.assertIn("\n    " + endpoint + "\n", chain)
            self.assertEqual(chain.count(endpoint), 1)
        self.assertIn("USDB_CHAIN_GCMODE", chain)
        self.assertIn("4.0 GiB (saved: 4g)", chain)
        self.assertIn("usdb-node peers status", chain)
        self.assertIn("usdb-node peers enode", chain)

    def test_explicit_empty_seeds_are_not_replaced_with_release_defaults(self):
        self.layout.node_env.write_text("USDB_NODE_ROLE=full\nUSDB_BOOTNODES=\n")
        write_bootnodes(self.layout.bundle_dir, [DNS])
        code, out, err = self.command()
        self.assertEqual((code, err), (0, ""))
        self.assertIn("Configured seeds: 0 — no seed saved", out)
        self.assertIn("Release default seeds: 1 — not applied to saved configuration", out)
        code, out, err = self.command("--json")
        seeds = json.loads(out)["peer_sources"]
        self.assertEqual(seeds["configured"]["state"], "EMPTY")
        self.assertEqual(seeds["configured"]["count"], 0)
        self.assertEqual(seeds["configured"]["endpoints"], [])
        self.assertFalse(seeds["matches_release_defaults"])

    def test_seed_settings_before_setup_missing_and_matching_are_distinguishable(self):
        write_bootnodes(self.layout.bundle_dir, [DNS, V6])
        for content, state, matches in ((None, "UNCONFIGURED", None), ("USDB_NODE_ROLE=full\n", "MISSING", None),
                                      (f"USDB_BOOTNODES={V6},{DNS}\n", "SAVED", True)):
            with self.subTest(state=state):
                if content is not None:
                    self.layout.node_env.write_text(content)
                code, out, err = self.command("--json")
                self.assertEqual((code, err), (0, ""))
                seeds = json.loads(out)["peer_sources"]
                self.assertEqual(seeds["configured"]["state"], state)
                self.assertIs(seeds["matches_release_defaults"], matches)
                self.assertEqual(seeds["release_defaults"]["endpoints"], [DNS, V6])
        code, out, err = self.command()
        self.assertIn("matches saved endpoints", out)

    def test_missing_or_invalid_default_catalog_does_not_hide_saved_settings(self):
        self.layout.node_env.write_text(f"USDB_BOOTNODES={DNS}\n")
        path = write_bootnodes(self.layout.bundle_dir, [])
        path.unlink()
        code, out, err = self.command("--json")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out)["peer_sources"]["release_defaults"]["state"], "MISSING")
        for invalid in ("private-secret-invalid-json", '{"private-secret-key":1,"private-secret-key":2}'):
            path.write_text(invalid)
            for options in ((), ("--json",)):
                code, out, err = self.command(*options)
                self.assertEqual((code, err), (0, ""))
                self.assertIn(DNS, out)
                self.assertIn("UNAVAILABLE", out)
                self.assertNotIn("private-secret", out + err)
        path.unlink()
        path.symlink_to(self.root / "missing-catalog")
        code, out, err = self.command("--json")
        self.assertEqual(json.loads(out)["peer_sources"]["release_defaults"]["state"], "UNAVAILABLE")

    def test_invalid_saved_seed_urls_never_leak_credentials(self):
        self.layout.node_env.write_text("USDB_BOOTNODES=https://private-user:private-pass@host.invalid/?token=private-token\n")
        write_bootnodes(self.layout.bundle_dir, [DNS])
        for options in ((), ("--json",)):
            code, out, err = self.command(*options)
            self.assertEqual((code, err), (0, ""))
            for secret in ("private-user", "private-pass", "private-token"):
                self.assertNotIn(secret, out + err)
            if options:
                seeds = json.loads(out)["peer_sources"]
                self.assertEqual(seeds["configured"]["state"], "INVALID")
                self.assertIsNone(seeds["configured"]["count"])
                self.assertIsNone(seeds["matches_release_defaults"])

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
