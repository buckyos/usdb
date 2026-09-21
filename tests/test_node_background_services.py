"""Upgrade reconciliation must not skip ready nodes or overwrite operator policy."""

from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_background_services as SERVICES
import usdb_node as NODE
from common.background_services import BackgroundServicesFixture


class BackgroundServicesTests(unittest.TestCase):
    def test_ready_up_repairs_legacy_units_and_keeps_installed_options(self):
        with BackgroundServicesFixture() as f:
            f.write_unit(sync_timeout_secs=123, pull=False)
            f.unit.write_text(f.unit.read_text().replace(" docker.service " + f.observer.name, " docker.service"))
            f.remove_observer()
            with mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "READY"}), \
                    mock.patch.object(NODE, "start_controller_unit") as start, \
                    mock.patch.object(NODE, "activate_release") as activate:
                result, code = NODE.submit_up_to_controller(f.layout, dry_run=False, allow_activation=False)
            self.assertEqual((code, result["outcome"]), (0, "ready"))
            self.assertEqual(result["background_services"]["monitor"], "running")
            self.assertIn("--sync-timeout-secs 123 --skip-pull", f.unit.read_text())
            self.assertIn(f.observer.name, f.unit.read_text())
            self.assertIn(["systemctl", "enable", f.observer.name], f.commands)
            self.assertNotIn(["systemctl", "enable", f.unit.name], f.commands)
            f.helper.assert_called_once_with(f.layout, "run_testnet_runtime.sh", ["up-console"], output_to_stderr=True)
            start.assert_not_called()
            activate.assert_not_called()

    def test_existing_disabled_services_stay_disabled_and_new_observer_inherits_policy(self):
        for missing in (False, True):
            with self.subTest(missing=missing), BackgroundServicesFixture() as f:
                for state in f.states.values():
                    state["UnitFileState"] = "disabled"
                if missing:
                    f.remove_observer()
                result = NODE.ensure_background_services(f.layout)
                self.assertEqual(result["controller_autostart"], "disabled")
                self.assertEqual(result["monitor_autostart"], "disabled")
                self.assertFalse(any("enable" in command or "disable" in command for command in f.commands))
                self.assertEqual(result["monitor"], "running")

    def test_repeated_up_does_not_restart_current_running_observer(self):
        with BackgroundServicesFixture() as f:
            NODE.ensure_background_services(f.layout)
            f.commands.clear()
            result = NODE.ensure_background_services(f.layout)
            self.assertEqual(result["actions"], [])
            self.assertEqual(f.commands, [])

    def test_upgrade_restarts_only_observer_to_load_new_release(self):
        with BackgroundServicesFixture() as f:
            f.running_observer()
            f.layout.release_id = "r27"
            original = f.unit.read_text()
            result = NODE.ensure_background_services(f.layout)
            self.assertEqual(f.unit.read_text(), original)
            self.assertIn("USDB_CONSOLE_MONITOR_RELEASE=r27", f.observer.read_text())
            self.assertEqual(result["actions"], ["install:" + f.observer.name, "restart:" + f.observer.name])
            self.assertFalse(any(command[-1] == f.unit.name for command in f.commands))
            f.commands.clear()
            NODE.ensure_background_services(f.layout)
            self.assertEqual(f.commands, [])

    def test_unit_already_refreshed_does_not_hide_running_old_process(self):
        with BackgroundServicesFixture() as f:
            f.running_observer(release="r25")
            result = NODE.ensure_background_services(f.layout)
            self.assertEqual(result["actions"], ["restart:" + f.observer.name])
            self.assertNotIn("SECRET", json.dumps(result))

    def test_activating_observer_joins_start_job_instead_of_false_ready(self):
        with BackgroundServicesFixture() as f:
            f.states[f.observer.name].update(ActiveState="activating", SubState="start")
            result = NODE.ensure_background_services(f.layout)
            self.assertEqual(result["monitor"], "running")
            self.assertIn(["systemctl", "start", f.observer.name], f.commands)

    def test_unreadable_process_stamp_is_actionable_and_not_exported(self):
        with BackgroundServicesFixture() as f:
            f.running_observer()
            (f.proc / "321/environ").unlink()
            (f.proc / "321/environ").mkdir()
            with self.assertRaisesRegex(ValueError, "check access to its /proc"):
                NODE.ensure_background_services(f.layout)
            self.assertEqual(f.commands, [])

    def test_legacy_unstamped_observer_is_recognized(self):
        with BackgroundServicesFixture() as f:
            f.observer.write_text("\n".join(line for line in f.observer.read_text().splitlines()
                                           if "USDB_CONSOLE_MONITOR_RELEASE=" not in line) + "\n")
            NODE.ensure_background_services(f.layout)
            self.assertIn("USDB_CONSOLE_MONITOR_RELEASE=", f.observer.read_text())

    def test_both_units_are_reviewed_before_any_write(self):
        variants = {
            "extra": lambda text: text + "\n[Service]\nNice=5\n",
            "edited": lambda text: text.replace("RestartSec=", "RestartSec=999"),
            "path": lambda text: text.replace(str("node.env"), "another.env"),
            "command": lambda text: text.replace("ExecStart=", "ExecStart=/unrecognized "),
        }
        for target in ("unit", "observer"):
            for name, change in variants.items():
                with self.subTest(target=target, name=name), BackgroundServicesFixture() as f:
                    unit = getattr(f, target)
                    unit.write_text(change(unit.read_text()))
                    original = unit.read_text()
                    f.layout.release_id = "r27"
                    with self.assertRaisesRegex(ValueError, "manual|review"):
                        NODE.ensure_background_services(f.layout)
                    self.assertEqual(unit.read_text(), original)
                    self.assertEqual(f.commands, [])

    def test_systemd_overrides_and_masks_are_never_overwritten(self):
        for target in ("unit", "observer"):
            for properties in ({"DropInPaths": "/etc/systemd/system/custom.conf"},
                               {"UnitFileState": "masked", "LoadState": "masked"},
                               {"FragmentPath": "/usr/lib/systemd/system/custom.service"},
                               {"UnitFileState": "enabled-runtime"}):
                with self.subTest(target=target, properties=properties), BackgroundServicesFixture() as f:
                    f.states[getattr(f, target).name].update(properties)
                    with self.assertRaisesRegex(ValueError, "review"):
                        NODE.ensure_background_services(f.layout)
                    self.assertEqual(f.commands, [])

    def test_missing_observer_with_dropin_is_not_auto_installed(self):
        with BackgroundServicesFixture() as f:
            f.remove_observer()
            f.states[f.observer.name]["DropInPaths"] = "/etc/systemd/system/observer.d/custom.conf"
            with self.assertRaisesRegex(ValueError, "override"):
                NODE.ensure_background_services(f.layout)
            self.assertFalse(f.observer.exists())
            self.assertEqual(f.commands, [])

    def test_symlinked_or_missing_bootstrap_requires_explicit_operator_action(self):
        for target in (None, "/dev/null"):
            with self.subTest(target=target), BackgroundServicesFixture() as f:
                f.unit.unlink()
                if target:
                    f.unit.symlink_to(target)
                with self.assertRaisesRegex(ValueError, "review|foreground"):
                    NODE.ensure_background_services(f.layout)
                self.assertEqual(f.commands, [])

    def test_probe_failure_leaves_both_units_unchanged(self):
        with BackgroundServicesFixture() as f:
            f.run.side_effect = subprocess.TimeoutExpired("systemctl", 5)
            with self.assertRaisesRegex(ValueError, "observation unavailable"):
                NODE.ensure_background_services(f.layout)
            self.assertEqual(f.commands, [])

    def test_sudo_failure_does_not_report_ready(self):
        with BackgroundServicesFixture() as f:
            f.remove_observer()
            f.privileged.side_effect = subprocess.CalledProcessError(1, "sudo")
            with mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "READY"}), \
                    mock.patch.object(NODE, "start_controller_unit") as start:
                with self.assertRaisesRegex(ValueError, "sudo.*retry usdb-node up"):
                    NODE.submit_up_to_controller(f.layout, dry_run=False, allow_activation=False)
            start.assert_not_called()
            f.helper.assert_not_called()

    def test_systemd_start_acknowledgement_without_running_process_is_not_success(self):
        with BackgroundServicesFixture() as f:
            f.privileged.return_value = subprocess.CompletedProcess([], 0)
            f.privileged.side_effect = None
            with self.assertRaisesRegex(ValueError, "did not reach running"):
                NODE.ensure_background_services(f.layout)

    def test_reload_only_does_not_enable_or_reinstall_units(self):
        with BackgroundServicesFixture() as f:
            f.states[f.observer.name].update(NeedDaemonReload="yes", ActiveState="active", SubState="running")
            NODE.ensure_background_services(f.layout)
            self.assertIn(["systemctl", "daemon-reload"], f.commands)
            self.assertIn(["systemctl", "restart", f.observer.name], f.commands)
            self.assertFalse(any(command[0] == "install" or "enable" in command for command in f.commands))

    def test_dry_run_and_foreground_never_install_systemd_services(self):
        with BackgroundServicesFixture() as f:
            with mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "READY"}), \
                    mock.patch.object(NODE, "ensure_background_services", side_effect=AssertionError("systemd")):
                NODE.submit_up_to_controller(f.layout, dry_run=True, allow_activation=False)
                NODE.up_node(f.layout, dry_run=False, allow_activation=False,
                             sync_timeout_secs=60, pull=True, json_output=False)
            self.assertEqual(f.commands, [])

    def test_activation_required_is_not_bypassed_by_background_repair(self):
        with BackgroundServicesFixture() as f:
            f.remove_observer()
            with mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "ACTIVATION_REQUIRED"}):
                result, code = NODE.submit_up_to_controller(f.layout, dry_run=False, allow_activation=False)
            self.assertEqual((code, result["outcome"]), (1, "manual_action_required"))
            self.assertEqual(f.commands, [])

    def test_json_failure_remains_machine_readable(self):
        with BackgroundServicesFixture() as f:
            f.remove_observer()
            f.privileged.side_effect = subprocess.CalledProcessError(1, "sudo")
            output = io.StringIO()
            with mock.patch.object(sys, "argv", ["usdb-node", "up", "--json"]), \
                    mock.patch.object(NODE, "load_release_layout", return_value=f.layout), \
                    mock.patch.object(NODE, "collect_node_status", return_value={"overall_state": "READY"}), \
                    redirect_stdout(output):
                code = NODE.main()
            self.assertEqual(code, 1)
            self.assertEqual(json.loads(output.getvalue())["outcome"], "error")


if __name__ == "__main__":
    unittest.main()
