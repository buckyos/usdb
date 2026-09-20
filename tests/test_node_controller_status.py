"""Check actionable controller status independently of node runtime readiness."""

from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_node as NODE
from common.controller_status import ControllerStatusFixture


class ControllerStatusTests(unittest.TestCase):
    def test_standard_launcher_is_recognized_without_user_bin_in_path(self):
        with ControllerStatusFixture() as fixture:
            stable = fixture.root / ".local/bin/usdb-node"
            stable.parent.mkdir(parents=True)
            stable.symlink_to(fixture.launcher)
            fixture.launcher = stable
            fixture.write_unit()
            with mock.patch.dict(os.environ, {"USDB_NODE_LAUNCHER": ""}), \
                 mock.patch.object(Path, "home", return_value=fixture.root), \
                 mock.patch.object(NODE.shutil, "which", side_effect=lambda command:
                                   str(fixture.docker) if command == "docker" else None):
                report = fixture.report()
            self.assertEqual(report["checks"]["controller"]["configuration_state"], "current")
            self.assertEqual(report["next_actions"], [])

    def test_progress_distinguishes_historical_manual_exit_from_real_failure(self):
        for exit_status, expected in (("2", "idle"), ("1", "failed")):
            with self.subTest(exit_status=exit_status), ControllerStatusFixture() as fixture:
                fixture.properties.update(ActiveState="failed", Result="exit-code", ExecMainCode="1", ExecMainStatus=exit_status)
                base = dict(release_id="test", observed_at="now", overall_state="READY",
                            components=[], controller_state="failed")
                with mock.patch.object(NODE, "_collect_node_progress", return_value=base):
                    progress = NODE.collect_node_progress(fixture.layout)
                rendered = NODE.render_node_progress(progress)
                self.assertIn("controller=" + expected, rendered)
                self.assertIn("systemd=failed | last exit=" + exit_status, rendered)
                self.assertEqual(progress["overall_state"], "READY")
                self.assertEqual(progress["controller_state"], "failed")
                self.assertEqual(progress["controller"]["action_required"], exit_status == "1")

    def test_progress_keeps_controller_probe_failure_actionable(self):
        with ControllerStatusFixture() as fixture:
            fixture.run.side_effect = subprocess.TimeoutExpired("systemctl", 5)
            base = dict(release_id="test", observed_at="now", overall_state="READY", components=[])
            with mock.patch.object(NODE, "_collect_node_progress", return_value=base) as collect:
                progress = NODE.collect_node_progress(fixture.layout)
            collect.assert_called_once_with(fixture.layout, controller_state="unavailable")
            self.assertEqual(fixture.run.call_count, 1)
            rendered = NODE.render_node_progress(progress)
            self.assertIn("controller=unavailable", rendered)
            self.assertIn("Action: usdb-node controller status", rendered)
            self.assertEqual(progress["overall_state"], "READY")

    def test_completed_controller_is_visible_without_downgrading_ready_node(self):
        with ControllerStatusFixture() as fixture:
            report = fixture.report()
            item = report["checks"]["controller"]
            self.assertEqual(item["configuration_state"], "current")
            self.assertEqual(item["display_state"], "idle")
            self.assertEqual(item["autostart"], "enabled")
            self.assertEqual(report["overall_state"], "READY")
            self.assertEqual(report["next_actions"], [])
            output = io.StringIO()
            with redirect_stdout(output):
                NODE._print_node_status_report(report)
            self.assertIn("Controller     IDLE", output.getvalue())
            self.assertIn("automatic startup after reboot is enabled", output.getvalue())
            self.assertEqual(json.loads(json.dumps(report))["checks"]["controller"], item)
            self.assertEqual(fixture.run.call_args.kwargs["timeout"], 5)

    def test_same_launcher_retargets_release_without_requiring_unit_install(self):
        with ControllerStatusFixture() as fixture:
            before = fixture.unit.read_bytes()
            fixture.launcher.unlink()
            for release in ("r25", "r26"):
                target = fixture.root / release / "usdb-node"
                target.parent.mkdir()
                target.write_text("#!/bin/sh\n")
                target.chmod(0o755)
                fixture.launcher.symlink_to(target)
                fixture.layout.release_id = release
                self.assertEqual(fixture.report()["checks"]["controller"]["configuration_state"], "current")
                fixture.launcher.unlink()
            self.assertEqual(fixture.unit.read_bytes(), before)

    def test_old_template_or_pinned_launcher_requires_update_preserving_options(self):
        with ControllerStatusFixture() as fixture:
            fixture.write_unit(sync_timeout_secs=123, pull=False)
            original = fixture.unit.read_text()
            for old in (original.replace("RestartSec=30s", "RestartSec=1s"),
                        original.replace(str(fixture.launcher), str(fixture.root / "r25/usdb-node")),
                        original.replace(str(fixture.layout.node_env), str(fixture.root / "old/node.env"))):
                with self.subTest(unit=old):
                    fixture.unit.write_text(old)
                    report = fixture.report("STARTING")
                    item = report["checks"]["controller"]
                    self.assertEqual(item["configuration_state"], "update_required")
                    self.assertTrue(item["action_required"])
                    self.assertEqual(report["next_actions"][0], "usdb-node controller install --sync-timeout-secs 123 --skip-pull")
                    self.assertEqual(fixture.unit.read_text(), old)

    def test_custom_timeout_and_pull_are_not_template_drift(self):
        with ControllerStatusFixture() as fixture:
            fixture.write_unit(sync_timeout_secs=123, pull=False)
            report = fixture.report()
            self.assertEqual(report["checks"]["controller"]["configuration_state"], "current")
            self.assertEqual(report["next_actions"], [])

    def test_missing_unit_prompts_install_without_probing_or_mutating_systemd(self):
        with ControllerStatusFixture() as fixture:
            fixture.unit.unlink()
            for overall in ("STARTING", "READY_TO_START", "ACTIVATION_REQUIRED"):
                with self.subTest(overall=overall):
                    report = fixture.report(overall)
                    self.assertEqual(report["checks"]["controller"]["display_state"], "missing")
                    self.assertEqual(report["next_actions"][0], "usdb-node controller install")
                    self.assertEqual(report["overall_state"], overall)
            ready = fixture.report()
            self.assertEqual(ready["next_actions"], [])
            self.assertIn("foreground", " ".join(ready["operator_guidance"]))
            fixture.run.assert_not_called()

    def test_disabled_autostart_is_explicit_and_restoration_is_optional(self):
        with ControllerStatusFixture() as fixture:
            fixture.write_unit(sync_timeout_secs=456, pull=False)
            fixture.properties["UnitFileState"] = "disabled"
            for active in ("active", "inactive"):
                fixture.properties["ActiveState"] = active
                report = fixture.report()
                self.assertEqual(report["checks"]["controller"]["autostart"], "disabled")
                self.assertIn("is disabled", report["checks"]["controller"]["summary"])
                self.assertEqual(report["next_actions"], [])
                self.assertIn("If automatic startup", " ".join(report["operator_guidance"]))
                self.assertIn("--sync-timeout-secs 456 --skip-pull", " ".join(report["operator_guidance"]))

    def test_exit_two_for_peers_is_manual_action_not_reinstall_or_crash(self):
        with ControllerStatusFixture() as fixture:
            fixture.properties.update(ActiveState="failed", Result="exit-code", ExecMainCode="1", ExecMainStatus="2")
            report = fixture.report("AWAITING_PEERS")
            self.assertEqual(report["checks"]["controller"]["display_state"], "manual_action")
            self.assertTrue(report["checks"]["controller"]["action_required"])
            self.assertEqual(report["checks"]["controller"]["actions"], report["next_actions"])
            self.assertEqual(report["next_actions"], NODE.STATUS_RECOVERY["AWAITING_PEERS"]["next_actions"])
            self.assertEqual(fixture.report()["next_actions"], [])
            fixture.properties["ExecMainStatus"] = "1"
            failed = fixture.report()
            self.assertEqual(failed["overall_state"], "READY")
            self.assertEqual(failed["checks"]["controller"]["display_state"], "failed")
            self.assertEqual(failed["next_actions"], ["usdb-node controller logs --follow"])

    def test_probe_failures_never_hide_runtime_status_or_leak_raw_errors(self):
        for failure in (OSError("secret in raw error"), subprocess.TimeoutExpired("secret", 5),
                        subprocess.CompletedProcess([], 1, "secret", "secret"),
                        subprocess.CompletedProcess([], 0, "ActiveState=active\n", "")):
            with self.subTest(failure=failure), ControllerStatusFixture() as fixture:
                fixture.run.side_effect = failure if isinstance(failure, Exception) else None
                fixture.run.return_value = failure
                report = fixture.report()
                self.assertEqual(report["overall_state"], "READY")
                self.assertEqual(report["checks"]["controller"]["display_state"], "unavailable")
                self.assertNotIn("secret", json.dumps(report))
                self.assertEqual(report["next_actions"], ["usdb-node controller status"])

    def test_dropins_masking_and_unrecognized_commands_require_review(self):
        with ControllerStatusFixture() as fixture:
            original = fixture.unit.read_text()
            variants = [("dropin", {"DropInPaths": "/etc/systemd/system/example.d/custom.conf"}),
                        ("masked", {"LoadState": "masked", "UnitFileState": "masked"}),
                        ("command", {}), ("owner", {}), ("extra_setting", {}), ("extra_environment", {})]
            for name, properties in variants:
                with self.subTest(name=name):
                    fixture.properties.update(DropInPaths="", LoadState="loaded", UnitFileState="enabled")
                    fixture.properties.update(properties)
                    content = original
                    if name == "command":
                        content = content.replace("controller run", "controller run --unknown=secret")
                    if name == "owner":
                        content = content.replace("\nUser=", "\nUser=other-")
                    if name == "extra_setting":
                        content = content.replace("\nType=simple", "\nType=simple\nNice=5")
                    if name == "extra_environment":
                        content = content.replace("\nType=simple", "\nType=simple\nEnvironment=EXTRA=secret")
                    fixture.unit.write_text(content)
                    report = fixture.report()
                    self.assertEqual(report["checks"]["controller"]["display_state"], "review_required")
                    self.assertEqual(report["next_actions"], ["usdb-node controller status"])
                    self.assertNotIn("secret", json.dumps(report))

    def test_reload_needed_is_actionable_even_when_disk_template_matches(self):
        with ControllerStatusFixture() as fixture:
            fixture.properties["NeedDaemonReload"] = "yes"
            report = fixture.report()
            self.assertEqual(report["checks"]["controller"]["display_state"], "update_required")
            self.assertEqual(report["next_actions"], ["usdb-node controller install"])


if __name__ == "__main__":
    unittest.main()
