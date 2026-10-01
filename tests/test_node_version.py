#!/usr/bin/env python3
"""Version queries must work offline, before setup and across release activation."""

from contextlib import ExitStack, redirect_stderr, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_node as node
from common.native_node import native_kit


class NodeVersionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-version-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)

    def command(self, *options):
        stdout, stderr = io.StringIO(), io.StringIO()
        argv = ["usdb-node", "--kit-root", str(self.layout.kit_root),
                "--node-env", str(self.layout.node_env), *options]
        with ExitStack() as stack:
            stack.enter_context(mock.patch.object(sys, "argv", argv))
            stack.enter_context(redirect_stdout(stdout))
            stack.enter_context(redirect_stderr(stderr))
            # Even broken services, snapshots and operation locks cannot block version.
            for name in ("load_release_layout", "validate_network_bundle", "run_helper",
                         "node_operation_lock", "_collect_compose_services"):
                stack.enter_context(mock.patch.object(node, name, side_effect=AssertionError(name)))
            stack.enter_context(mock.patch.object(node.subprocess, "run", side_effect=AssertionError("external process")))
            result = node.main()
        return result, stdout.getvalue(), stderr.getvalue()

    def test_all_spellings_work_before_setup_without_probes_or_writes(self):
        outputs = []
        for option in ("version", "--version", "-V"):
            with self.subTest(option=option):
                code, output, errors = self.command(option)
                self.assertEqual(code, 0, errors)
                self.assertEqual(errors, "")
                self.assertIn("USDB node | usdb-testnet-v0-r999", output)
                self.assertIn("Network: usdb-testnet-v0 | Chain ID: 202608250", output)
                self.assertIn("Release created: 2026-09-13T12:00:00Z", output)
                self.assertIn("USDB source revision: " + "b" * 40, output)
                self.assertIn("Configured images: UNCONFIGURED", output)
                self.assertIn("usdb-node setup", output)
                outputs.append(output)
        self.assertEqual(outputs, [outputs[0]] * 3)
        self.assertFalse(self.layout.node_env.exists())

    def test_json_reports_installed_identity_and_matching_config_without_runtime_claims(self):
        original = node.upsert_env("BTC_RPC_PASSWORD=private-value\n", self.layout.images)
        self.layout.node_env.write_text(original)
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        report = json.loads(output)
        self.assertEqual(report["schema_version"], "usdb-node-version:v1")
        self.assertEqual(report["kit_root"], str(self.layout.kit_root))
        self.assertEqual(report["configured_images"]["state"], "MATCHES_RELEASE")
        self.assertEqual(report["configured_images"]["next_actions"], [])
        self.assertEqual(report["images"], self.layout.images)
        self.assertFalse(report["runtime_observed"])
        self.assertNotIn("private-value", output)
        self.assertNotIn("active_release_id", report)
        self.assertEqual(self.layout.node_env.read_text(), original)

    def test_changed_image_requires_activation_but_reused_images_do_not_identify_old_release(self):
        self.layout.node_env.write_text(node.upsert_env("", self.layout.images))
        manifest_path = self.layout.manifest_path
        manifest = json.loads(manifest_path.read_text())
        manifest["release_id"] = "usdb-testnet-v0-r1000"
        content = json.dumps(manifest)
        manifest_path.write_text(content)
        manifest_path.with_suffix(".json.sha256").write_text(
            hashlib.sha256(content.encode()).hexdigest() + "  " + manifest_path.name + "\n")
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        self.assertEqual(json.loads(output)["release_id"], "usdb-testnet-v0-r1000")
        self.assertEqual(json.loads(output)["configured_images"]["state"], "MATCHES_RELEASE")
        previous = dict(self.layout.images, USDB_CHAIN_IMAGE="ghcr.io/buckyos/usdb-chain@sha256:" + "9" * 64)
        self.layout.node_env.write_text(node.upsert_env("", previous))
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        config = json.loads(output)["configured_images"]
        self.assertEqual(config["state"], "ACTIVATION_REQUIRED")
        self.assertEqual(config["mismatched_images"], ["USDB_CHAIN_IMAGE"])
        self.assertEqual(config["next_actions"], ["usdb-node down", "usdb-node activate-release", "usdb-node up"])
        self.assertEqual(node.read_env(self.layout.node_env), previous)

    def test_unreadable_or_invalid_config_does_not_hide_version_or_expose_secrets(self):
        self.layout.node_env.write_text("private-value!=not-a-valid-key\n")
        code, output, errors = self.command("version")
        self.assertEqual((code, errors), (0, ""))
        self.assertIn("Configured images: UNAVAILABLE", output)
        self.assertIn("usdb-testnet-v0-r999", output)
        self.assertNotIn("private-value", output)
        with mock.patch.object(node, "read_env", side_effect=PermissionError("private-value")):
            code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        self.assertEqual(json.loads(output)["configured_images"]["state"], "UNAVAILABLE")
        self.assertNotIn("private-value", output)

    def test_inaccessible_path_or_broken_config_link_is_unavailable_not_a_first_install(self):
        self.layout.node_env.symlink_to(self.root / "missing-config")
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        self.assertEqual(json.loads(output)["configured_images"]["state"], "UNAVAILABLE")
        self.layout.node_env.unlink()
        original_stat = Path.stat

        def inaccessible_config(path, *args, **kwargs):
            if path == self.layout.node_env:
                raise PermissionError("private-value")
            return original_stat(path, *args, **kwargs)

        with mock.patch.object(Path, "stat", inaccessible_config):
            code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        self.assertEqual(json.loads(output)["configured_images"]["state"], "UNAVAILABLE")
        self.assertNotIn("private-value", output)
        os.mkfifo(self.layout.node_env)
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (0, ""))
        self.assertEqual(json.loads(output)["configured_images"]["state"], "UNAVAILABLE")

    def test_changed_manifest_is_rejected_with_clean_text_and_json_errors(self):
        with self.layout.manifest_path.open("a") as output:
            output.write(" ")
        code, output, errors = self.command("--version")
        self.assertEqual(code, 1)
        self.assertEqual(output, "")
        self.assertIn("checksum mismatch", errors)
        code, output, errors = self.command("version", "--json")
        self.assertEqual((code, errors), (1, ""))
        report = json.loads(output)
        self.assertEqual(report["schema_version"], "usdb-node-version:v1")
        self.assertEqual(report["outcome"], "error")
        self.assertIn("checksum mismatch", report["error"])

    def test_missing_manifest_does_not_guess_a_release(self):
        self.layout.manifest_path.unlink()
        code, output, errors = self.command("-V")
        self.assertEqual(code, 1)
        self.assertEqual(output, "")
        self.assertIn("missing its manifest", errors)
        self.assertNotIn("Traceback", errors)

    def test_no_command_and_conflicting_version_flag_are_usage_errors(self):
        for options in ([], ["--version", "down"]):
            with self.subTest(options=options), self.assertRaises(SystemExit) as error:
                self.command(*options)
            self.assertEqual(error.exception.code, 2)

    def test_installed_launcher_resolves_its_kit_and_works_in_a_fresh_home(self):
        launcher = self.root / "usdb-node"
        launcher.symlink_to(self.layout.kit_root / "docker/scripts/tools/usdb_node.py")
        fresh_home = self.root / "home"
        fresh_home.mkdir()
        result = subprocess.run(
            [str(launcher), "version", "--json"], cwd=self.root,
            env={**os.environ, "HOME": str(fresh_home), "PYTHONDONTWRITEBYTECODE": "1"},
            capture_output=True, text=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        report = json.loads(result.stdout)
        self.assertEqual(report["release_id"], "usdb-testnet-v0-r999")
        self.assertEqual(report["node_env"], str(fresh_home / ".config/usdb/usdb-testnet-v0/node.env"))
        self.assertEqual(list(fresh_home.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
