"""Process diagnostics survive startup failures without mutating a packaged node."""

from contextlib import ExitStack, redirect_stderr, redirect_stdout
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
import node_top as top
import usdb_node as node
from common.native_node import native_kit
from common.node_top import ProcessHost


class TopTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        self.layout.node_env.write_text("USDB_MINTING_ENABLED=0\nUSDB_MONITOR_ENABLED=1\nSNAPSHOT_MODE=assumeutxo\n")
        self.host = ProcessHost(self.layout.bundle_id)
        self.host.unit("controller", active="failed", sub="failed", pid=0, code=1, status=2, result="exit-code")
        self.host.unit("monitor")

    def command(self, *options):
        out, err = io.StringIO(), io.StringIO()
        argv = ["usdb-node", "--kit-root", str(self.layout.kit_root), "--node-env", str(self.layout.node_env), "top", *options]
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        with ExitStack() as stack:
            stack.enter_context(mock.patch.object(sys, "argv", argv))
            stack.enter_context(redirect_stdout(out))
            stack.enter_context(redirect_stderr(err))
            for name in ("load_release_layout", "validate_network_bundle", "run_helper", "node_operation_lock",
                         "_atomic_write_private", "effective_memory_bytes", "collect_node_status", "collect_node_progress"):
                stack.enter_context(mock.patch.object(node, name, side_effect=AssertionError(name)))
            stack.enter_context(mock.patch.object(top.subprocess, "run", side_effect=self.host.run))
            code = node.main()
        after = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(before, after)
        self.assertNotIn("DO_NOT_PRINT", out.getvalue() + err.getvalue())
        return code, out.getvalue(), err.getvalue()

    def report(self, *options):
        code, text, err = self.command("--json", *options)
        self.assertEqual(err, "")
        report = json.loads(text)
        self.assertEqual(report["schema_version"], "usdb-node-top:v1")
        return code, report, {row["service"]: row for row in report["services"]}

    def test_running_exited_missing_and_controller_failure_are_independent(self):
        btc = self.host.container("btc-node")
        self.host.container("balance-history", state="exited", pid=0, code=137)
        code, report, rows = self.report()
        self.assertEqual(code, 0)  # Successful observation is not a readiness claim.
        self.assertEqual((rows["controller"]["state"], rows["controller"]["exit_code"]), ("FAILED", 2))
        self.assertEqual((rows["monitor"]["state"], rows["monitor"]["host_pid"]), ("RUNNING", 1234))
        self.assertEqual((rows["btc-node"]["state"], rows["btc-node"]["container_id"]), ("RUNNING", btc))
        self.assertGreaterEqual(rows["btc-node"]["uptime_secs"], 3599)
        self.assertEqual(rows["btc-node"]["memory_used_bytes"], int(2.5 * 1024**3))
        self.assertEqual(rows["btc-node"]["memory_limit_bytes"], 4 * 1024**3)
        self.assertIsNone(rows["btc-node"]["exit_code"])
        self.assertEqual((rows["balance-history"]["state"], rows["balance-history"]["exit_code"]), ("EXITED", 137))
        self.assertFalse(rows["balance-history"]["oom_killed"])
        self.assertIsNone(rows["balance-history"]["uptime_secs"])
        self.assertEqual(rows["usdb-indexer"]["state"], "NOT_CREATED")
        self.assertEqual(rows["ord-server"]["state"], "DISABLED")
        self.assertEqual(report["hidden_jobs"], 5)

    def test_invalid_budget_snapshot_and_node_config_do_not_gate_process_probes(self):
        self.host.container("usdb-chain")
        for content in ("USDB_RESOURCE_MODE=auto\nUSDB_RESOURCE_HOST_MEMORY_BYTES=20000000000\nUSDB_RESOURCE_PHASE=steady\n",
                        "this is an invalid secret configuration line\n"):
            self.layout.node_env.write_text(content)
            code, _, rows = self.report()
            self.assertEqual(code, 0)
            self.assertEqual(rows["usdb-chain"]["state"], "RUNNING")
        self.layout.node_env.unlink()
        code, report, rows = self.report()
        self.assertEqual((code, report["configuration"]), (0, "missing"))
        self.assertEqual(rows["usdb-chain"]["state"], "RUNNING")
        self.assertIsNone(rows["ord-server"]["configured_enabled"])
        self.assertFalse(self.layout.node_env.exists())

    def test_disabled_but_running_is_visible_for_ord_and_monitor(self):
        self.layout.node_env.write_text("USDB_MINTING_ENABLED=0\nUSDB_MONITOR_ENABLED=0\n")
        self.host.container("ord-server")
        code, _, rows = self.report()
        self.assertEqual(code, 0)
        for service in ("monitor", "ord-server"):
            self.assertEqual(rows[service]["state"], "RUNNING")
            self.assertFalse(rows[service]["configured_enabled"])
        _, text, _ = self.command()
        self.assertEqual(text.count("configured disabled, but process is present"), 2)

    def test_jobs_are_expanded_or_automatically_visible_when_active_or_failed(self):
        self.host.container("btc-snapshot-bootstrap")
        self.host.container("usdb-chain-init", state="exited", pid=0, code=1)
        self.host.container("snapshot-loader", state="exited", pid=0)
        _, report, rows = self.report()
        self.assertIn("btc-snapshot-bootstrap", rows)
        self.assertIn("usdb-chain-init", rows)
        self.assertNotIn("snapshot-loader", rows)
        self.assertEqual(report["hidden_jobs"], 3)
        _, _, rows = self.report("--all")
        self.assertEqual(rows["snapshot-loader"]["state"], "EXITED")
        self.assertIn("paired-checkpoint-recovery", rows)

    def test_failed_docker_project_preserves_bitcoin_and_systemd(self):
        self.host.container("btc-node")
        self.host.fail_projects.add(self.layout.bundle_id)
        code, report, rows = self.report()
        self.assertEqual((code, report["observation"]), (1, "partial"))
        self.assertEqual(rows["btc-node"]["state"], "RUNNING")
        self.assertEqual(rows["controller"]["state"], "FAILED")
        for name in ("balance-history", "ord-server"):
            self.assertEqual(rows[name]["state"], "UNKNOWN")

    def test_systemd_timeout_preserves_docker(self):
        self.host.fail_units.update(self.host.systemd)
        self.host.container("usdb-chain")
        code, _, rows = self.report()
        self.assertEqual(code, 1)
        self.assertEqual(rows["controller"]["state"], "UNKNOWN")
        self.assertEqual(rows["monitor"]["state"], "UNKNOWN")
        self.assertEqual(rows["usdb-chain"]["state"], "RUNNING")

    def test_absent_systemd_unit_and_successful_oneshot_are_not_running(self):
        self.host.systemd.clear()
        self.host.unit("controller", active="active", sub="exited", pid=0, code=1)
        code, _, rows = self.report()
        self.assertEqual(code, 0)
        self.assertEqual(rows["controller"]["state"], "COMPLETED")
        self.assertEqual(rows["controller"]["exit_code"], 0)
        self.assertIsNone(rows["controller"]["host_pid"])
        self.assertEqual(rows["monitor"]["state"], "NOT_INSTALLED")

    def test_termination_signal_is_not_exit_code_or_oom(self):
        self.host.unit("controller", active="failed", sub="failed", pid=0, code=2, status=9, result="signal")
        _, _, rows = self.report()
        self.assertEqual(rows["controller"]["exit_signal"], 9)
        self.assertIsNone(rows["controller"]["exit_code"])
        self.assertIsNone(rows["controller"]["oom_killed"])
        _, text, _ = self.command()
        self.assertIn("sig:9", text)

    def test_inspect_race_keeps_known_containers_and_labels_disappeared_one_unknown(self):
        missing = self.host.container("balance-history")
        self.host.container("usdb-chain")
        self.host.missing_details.add(missing)
        code, _, rows = self.report()
        self.assertEqual(code, 1)
        self.assertEqual(rows["balance-history"]["state"], "UNKNOWN")
        self.assertEqual(rows["balance-history"]["container_id"], missing)
        self.assertEqual(rows["usdb-chain"]["state"], "RUNNING")

    def test_duplicate_services_are_not_collapsed(self):
        first = self.host.container("usdb-chain")
        second = self.host.container("usdb-chain", state="exited", pid=0, code=1)
        code, report, _ = self.report()
        self.assertEqual(code, 0)
        rows = [row for row in report["services"] if row["service"] == "usdb-chain"]
        self.assertEqual({row["container_id"] for row in rows}, {first, second})
        self.assertEqual({row["state"] for row in rows}, {"RUNNING", "EXITED"})

    def test_other_project_and_invalid_container_ids_are_rejected_before_inspect(self):
        for item in (dict(id="a" * 64, service="usdb-chain", project="another-network"),
                     dict(id="--privileged", service="usdb-chain", project=self.layout.bundle_id), None):
            self.host.list_override[self.layout.bundle_id] = [item]
            code, _, rows = self.report()
            self.assertEqual(code, 1)
            self.assertEqual(rows["usdb-chain"]["state"], "UNKNOWN")
        self.assertFalse(any(call[:2] == ["docker", "inspect"] for call in self.host.calls))

    def test_inspected_identity_must_match_discovery(self):
        identifier = self.host.container("usdb-chain")
        self.host.inspect_override[identifier] = {**self.host.containers[identifier], "project": "another-network"}
        code, _, rows = self.report()
        self.assertEqual(code, 1)
        self.assertEqual(rows["usdb-chain"]["state"], "UNKNOWN")

    def test_missing_memory_accounting_keeps_process_details(self):
        self.host.container("btc-node")
        self.host.fail_stats = True
        code, _, rows = self.report()
        self.assertEqual(code, 0)
        self.assertEqual(rows["btc-node"]["state"], "RUNNING")
        self.assertIsNone(rows["btc-node"]["memory_used_bytes"])
        self.assertEqual(rows["btc-node"]["memory_limit_bytes"], 4 * 1024**3)

    def test_paused_restarting_and_created_containers_keep_their_actual_state(self):
        self.host.container("btc-node", state="paused")
        self.host.container("balance-history", state="restarting", pid=0, code=1)
        self.host.container("usdb-chain", state="created", pid=0)
        _, _, rows = self.report()
        self.assertEqual(rows["btc-node"]["state"], "PAUSED")
        self.assertEqual(rows["balance-history"]["state"], "RESTARTING")
        self.assertEqual(rows["balance-history"]["exit_code"], 1)
        self.assertEqual(rows["usdb-chain"]["state"], "CREATED")
        self.assertIsNone(rows["usdb-chain"]["exit_code"])

    def test_systemd_restart_and_never_started_exit_values_remain_distinct(self):
        self.host.unit("controller", active="activating", sub="auto-restart", pid=0, code=1, status=1)
        self.host.unit("monitor", active="inactive", sub="dead", pid=0)
        _, _, rows = self.report()
        self.assertEqual(rows["controller"]["state"], "RESTARTING")
        self.assertEqual(rows["controller"]["exit_code"], 1)
        self.assertEqual(rows["monitor"]["state"], "STOPPED")
        self.assertIsNone(rows["monitor"]["exit_code"])

    def test_watch_uses_fresh_inventory_after_exit_and_restart(self):
        identifier = self.host.container("btc-node")
        samples = []
        collect = top.collect
        def observe(*args, **kwargs):
            report = collect(*args, **kwargs)
            samples.append(next(row for row in report["services"] if row["service"] == "btc-node"))
            return report
        def advance(_seconds):
            if len(samples) == 1:
                self.host.changed(identifier, state="exited", pid=0, exit_code=1)
            elif len(samples) == 2:
                self.host.changed(identifier, state="running", pid=3333)
            else:
                raise KeyboardInterrupt()
        with mock.patch.object(top, "collect", side_effect=observe), mock.patch.object(top.time, "sleep", side_effect=advance):
            code, _, _ = self.command("--watch")
        self.assertEqual(code, 0)
        self.assertEqual([row["state"] for row in samples], ["RUNNING", "EXITED", "RUNNING"])
        self.assertEqual([row["host_pid"] for row in samples], [2222, None, 3333])
        self.assertIsNone(samples[-1]["exit_code"])

    def test_oom_evidence_and_unlimited_memory_are_explicit(self):
        identifier = self.host.container("balance-history", state="exited", pid=0, code=137, oom=True)
        self.host.changed(identifier, memory_limit_bytes=0)
        _, text, _ = self.command()
        self.assertIn("balance-history: OOMKilled=true", text)
        self.assertIn("- / none", text)

    def test_watch_refreshes_without_actions_and_restores_terminal_on_ctrl_c(self):
        with mock.patch.object(node, "_terminal_refresh_supported", return_value=True), \
             mock.patch.object(top.time, "sleep", side_effect=[None, KeyboardInterrupt()]) as sleep:
            code, out, err = self.command("--watch", "--refresh-secs", "0.1")
        self.assertEqual((code, out), (0, ""))
        self.assertEqual(err.count("USDB processes |"), 2)
        self.assertIn(node.CURSOR_SHOW + node.ALT_SCREEN_EXIT, err)
        self.assertEqual(sleep.call_args_list, [mock.call(0.1), mock.call(0.1)])

    def test_bad_refresh_and_json_errors_do_not_probe_services(self):
        for value in ("0", "-1", "nan", "inf"):
            code, text, err = self.command("--json", "--refresh-secs", value)
            self.assertEqual((code, err), (1, ""))
            self.assertEqual(json.loads(text)["outcome"], "error")
        self.assertEqual(self.host.calls, [])
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            node.build_parser().parse_args(["top", "--watch", "--json"])

    def test_packaged_cli_contains_top_module_and_read_only_help(self):
        result = subprocess.run([sys.executable, str(self.layout.kit_root / "docker/scripts/tools/usdb_node.py"),
            "top", "--help"], check=True, capture_output=True, text=True)
        for flag in ("--watch", "--all", "--json", "--refresh-secs"):
            self.assertIn(flag, result.stdout)
        self.assertTrue((self.layout.kit_root / "docker/scripts/tools/node_top.py").is_file())

    def test_packaged_cli_observes_processes_with_broken_config_and_bundle(self):
        self.host.container("usdb-chain")
        commands = self.root / "fake-bin"
        self.host.install_cli(commands)
        self.layout.node_env.write_text("secret invalid config line\n")
        (self.layout.bundle_dir / "network.json").write_text("invalid bundle for diagnosis\n")
        before = {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        result = subprocess.run([sys.executable, str(self.layout.kit_root / "docker/scripts/tools/usdb_node.py"),
            "--node-env", str(self.layout.node_env), "top", "--json"], capture_output=True, text=True,
            env={**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1"}, timeout=20)
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assertNotIn("secret", result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["configuration"], "unavailable")
        chain = next(row for row in report["services"] if row["service"] == "usdb-chain")
        self.assertEqual((chain["state"], chain["host_pid"]), ("RUNNING", 2222))
        self.assertEqual(before, {str(p): p.read_bytes() for p in self.root.rglob("*") if p.is_file()})


if __name__ == "__main__":
    unittest.main()
