"""Installer advice selects a single path without touching configurations or services."""

import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_install_guidance as guidance
import node_upgrade as upgrade
from common.install_guidance import GuidanceFixture, copy_tools


class InstallGuidanceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.f = GuidanceFixture(Path(temporary.name))
        self.home = mock.patch.object(Path, "home", return_value=self.f.home)
        self.home.start()
        self.addCleanup(self.home.stop)

    def render(self):
        before = {p: p.read_bytes() for p in self.f.root.rglob("*") if p.is_file()}
        with mock.patch.object(subprocess, "run", side_effect=AssertionError("service probe or mutation")):
            output = guidance.render(self.f.kit, self.f.bin)
        self.assertEqual(before, {p: p.read_bytes() for p in self.f.root.rglob("*") if p.is_file()})
        self.assertNotIn("private-secret-fixture", output)
        return output

    def test_fresh_install_has_only_initial_setup_and_retained_data_advice(self):
        output = self.render()
        self.assertIn("no standard node configuration found", output)
        self.assertIn("usdb-node prepare-host", output)
        self.assertIn("usdb-node setup", output)
        self.assertIn("Retained data may still exist", output)
        self.assertNotIn("activate-release", output)
        self.assertNotIn("--resume", output)
        self.assertFalse((self.f.home / ".config").exists())

    def test_target_config_takes_precedence_over_other_network_versions(self):
        self.f.config(self.f.bundle)
        self.f.config("usdb-testnet-v99")
        output = self.render()
        self.assertIn("existing target network configuration", output)
        self.assertIn("usdb-node upgrade-plan", output)
        self.assertNotIn("usdb-node prepare-host", output)
        self.assertNotIn("usdb-testnet-v99", output)
        self.assertNotIn("--execute", output)

    def test_numeric_default_prefers_same_family_and_lists_overrides(self):
        self.f.config("usdb-testnet-v2")
        latest = self.f.config("usdb-testnet-v10")
        self.f.config("usdb-mainnet-v99")
        output = self.render()
        self.assertIn(f"Default: usdb-testnet-v10 | {latest}", output)
        self.assertIn("Alternative: usdb-testnet-v2", output)
        self.assertIn("Alternative: usdb-mainnet-v99", output)
        self.assertIn("--node-env", output)
        self.assertIn("old kit's stop command", output)
        self.assertNotIn("usdb-node prepare-host", output)
        self.assertNotIn("network_reset", output)

    def test_pending_old_network_overrides_newer_default_and_uses_saved_kit(self):
        self.f.config("usdb-testnet-v10")
        config, backup, kit = self.f.record("usdb-testnet-v2")
        output = self.render()
        self.assertIn("unfinished upgrade requires review", output)
        self.assertIn(shlex.quote(str(backup)), output)
        self.assertIn(str(kit / "docker/scripts/tools/usdb_node.py"), output)
        self.assertIn(f"--node-env {config}", output)
        self.assertNotIn("usdb-node upgrade-plan", output)
        self.assertNotIn("--execute", output)
        self.assertNotIn("Default: usdb-testnet-v10", output)

    def test_pending_marker_survives_missing_node_env(self):
        config, _, _ = self.f.record(self.f.bundle)
        config.unlink()
        self.assertIn("unfinished upgrade requires review", self.render())

    def test_registered_staged_operation_without_marker_is_detected(self):
        self.f.record(self.f.bundle, marker=False, register=True, phase="staged")
        output = self.render()
        self.assertIn("phase=staged", output)
        self.assertIn("--resume", output)

    def test_completed_registration_does_not_offer_resume(self):
        self.f.record(self.f.bundle, marker=False, register=True, phase="applied")
        output = self.render()
        self.assertIn("existing target network configuration", output)
        self.assertNotIn("--resume", output)

    def test_missing_recovery_kit_requires_restore_instead_of_new_launcher(self):
        _, _, kit = self.f.record(self.f.bundle)
        (kit / "release/usdb-release-manifest.json").unlink()
        output = self.render()
        self.assertIn("Restore the exact saved target kit", output)
        self.assertNotIn("--resume", output)

    def test_broken_record_blocks_fresh_setup_advice(self):
        _, backup, _ = self.f.record(self.f.bundle)
        (backup / "upgrade.json").write_text("private-broken-json")
        with self.assertRaisesRegex(ValueError, "Cannot validate saved upgrade") as error:
            self.render()
        self.assertNotIn("private-broken-json", str(error.exception))

    def test_unsafe_or_malformed_configuration_is_not_treated_as_unconfigured(self):
        path = self.f.config(self.f.bundle)
        for text in ("PRIVATE-secret-invalid-key=x\n", "USDB_DATA_ROOT=x\n" + "X" * (1024*1024)):
            path.write_text(text)
            with self.assertRaisesRegex(ValueError, "Cannot inspect network configuration") as error:
                self.render()
            self.assertNotIn("PRIVATE-secret", str(error.exception))
        path.unlink()
        path.symlink_to(self.f.root / "missing")
        with self.assertRaisesRegex(ValueError, "Cannot inspect network configuration"):
            self.render()

    def test_configuration_values_are_never_executed(self):
        path = self.f.config(self.f.bundle)
        sentinel = self.f.root / "executed"
        path.write_text(f'USDB_DATA_ROOT=/unmounted/$(touch {sentinel})\n')
        self.render()
        self.assertFalse(sentinel.exists())

    def test_packaged_entrypoint_is_offline_and_redacts_failure(self):
        copy_tools(self.f.kit)
        path = self.f.config(self.f.bundle)
        path.write_text("SECRET-invalid-key=private-value\n")
        result = subprocess.run([sys.executable, "-B", str(self.f.kit / "docker/scripts/tools/node_install_guidance.py"),
                                 "--kit-root", str(self.f.kit), "--bin-dir", str(self.f.bin)],
                                env={**os.environ, "HOME": str(self.f.home)}, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("Deployment advice unavailable", result.stdout)
        self.assertIn(str(path), result.stdout)
        self.assertNotIn("SECRET", result.stdout + result.stderr)
        self.assertNotIn("private-value", result.stdout + result.stderr)
        self.assertNotIn("Traceback", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
