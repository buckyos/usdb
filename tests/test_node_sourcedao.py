#!/usr/bin/env python3
"""Accept release-bound managed SourceDAO operations and interruption recovery."""
import json
import io
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_sourcedao as DAO
from common.sourcedao import SourceDaoFixture


class SourceDaoTests(unittest.TestCase):
    def test_check_distinguishes_unstarted_starting_deploying_and_finalized(self):
        with SourceDaoFixture() as f:
            def phase():
                return DAO.check(f.layout, f.ctx, details=True)["deployment"]["phase"]
            self.assertEqual(phase(), "NOT_STARTED")
            self.assertFalse(f.ctx["state"].exists())
            f.start()
            self.assertEqual(phase(), "STARTING")
            f.progress()
            self.assertEqual(phase(), "DEPLOYING")
            f.progress(pending="Dividend.finalizeBootstrap")
            self.assertEqual(phase(), "FINALIZING")
            f.live["finalized"] = True
            self.assertEqual(phase(), "FINALIZED")
            self.assertEqual(DAO.status(f.layout)["outcome"], "RUNNING")

    def test_unmanaged_or_interrupted_deployment_never_looks_unstarted(self):
        with SourceDaoFixture() as f:
            f.live["initialized"] = True
            self.assertEqual(DAO.check(f.layout, f.ctx, details=True)["deployment"]["phase"], "INCOMPLETE")
            f.start()
            f.progress()
            f.finish(exit_code=1, completed=False)
            report = DAO.check(f.layout, f.ctx, details=True)
            self.assertEqual(report["deployment"]["phase"], "INCOMPLETE")
            self.assertEqual(report["local_task"]["outcome"], "FAILED")
            f.container = None
            self.assertEqual(DAO.status(f.layout)["deployment"]["phase"], "INCOMPLETE")

    def test_progress_whitelists_receipts_and_keeps_private_records_unchanged(self):
        with SourceDaoFixture() as f:
            f.start()
            f.progress()
            journal = f.ctx["state"].with_name("state.json.transactions.json")
            before = journal.read_bytes()
            report = DAO.status(f.layout)
            text = DAO.render(report)
            self.assertEqual(report["transactions"]["confirmed"], 1)
            self.assertIn("Waiting: Acquired.deployImplementation", text)
            self.assertIn("Last confirmed: Dao.initialize | block=10", text)
            self.assertIn("0x" + "33" * 32, text)
            self.assertNotIn("SIGNED_TRANSACTION_SENTINEL", json.dumps(report))
            self.assertNotIn("raw_transaction", json.dumps(report))
            self.assertEqual(journal.read_bytes(), before)

    def test_failed_progress_read_or_rpc_does_not_claim_completion_or_stop_task(self):
        with SourceDaoFixture() as f:
            f.start()
            f.progress()
            journal = f.ctx["state"].with_name("state.json.transactions.json")
            data = json.loads(journal.read_text())
            data["identity"]["genesis_hash"] = "0x" + "ff" * 32
            DAO.node._atomic_write_private(journal, json.dumps(data))
            report = DAO.status(f.layout)
            self.assertEqual(report["outcome"], "RUNNING")
            self.assertEqual(report["deployment"]["phase"], "UNKNOWN")
            self.assertIn("identity", report["progress_error"])
            f.progress()
            f.failure = ["run"]
            report = DAO.status(f.layout)
            self.assertEqual(report["outcome"], "RUNNING")
            self.assertEqual(report["deployment"]["phase"], "UNKNOWN")
            self.assertEqual(report["transactions"]["confirmed"], 1)
            self.assertIn("observation_error", report)

    def test_watch_refreshes_tty_separates_plain_frames_and_preserves_json(self):
        for mode in ("tty", "plain", "json"):
            with self.subTest(mode=mode), SourceDaoFixture() as f:
                f.start()
                f.progress()
                running = DAO.status(f.layout)
                f.finish()
                complete = DAO.status(f.layout)
                output = io.StringIO()
                args = DAO.node.build_parser().parse_args(["sourcedao", "status", "--watch"] + (["--json"] if mode == "json" else []))
                with mock.patch.object(DAO.sys, "stdout", output), mock.patch.object(output, "isatty", return_value=mode == "tty"), \
                        mock.patch.dict(DAO.os.environ, {"TERM": "xterm"}), mock.patch.object(DAO.time, "sleep"), \
                        mock.patch.object(DAO, "status", side_effect=[running, running, complete]):
                    self.assertEqual(DAO.execute(f.layout, args), 0)
                content = output.getvalue()
                if mode == "tty":
                    self.assertEqual(content.count(DAO.node.ALT_SCREEN_ENTER), 1)
                    self.assertEqual(content.count(DAO.node.ALT_SCREEN_EXIT), 1)
                    self.assertEqual(content.count(DAO.node.SCREEN_CLEAR), 3)
                    self.assertIn("SUCCEEDED", content.split(DAO.node.ALT_SCREEN_EXIT)[-1])
                elif mode == "plain":
                    self.assertNotIn("\x1b", content)
                    self.assertEqual(len([line for line in content.splitlines() if line and set(line) == {"="}]), 3)
                    self.assertEqual(content.count("Recovery and output paths:"), 2)
                else:
                    self.assertNotIn("\x1b", content)
                    self.assertNotIn("====", content)
                    decoder = json.JSONDecoder()
                    values = []
                    while content.strip():
                        value, end = decoder.raw_decode(content.lstrip())
                        values.append(value)
                        content = content.lstrip()[end:]
                    self.assertEqual([value["outcome"] for value in values], ["RUNNING", "RUNNING", "SUCCEEDED"])

    def test_interrupted_watch_restores_terminal_and_leaves_container_running(self):
        with SourceDaoFixture() as f:
            f.start()
            output = io.StringIO()
            args = DAO.node.build_parser().parse_args(["sourcedao", "status", "--watch"])
            with mock.patch.object(DAO.sys, "stdout", output), mock.patch.object(output, "isatty", return_value=True), \
                    mock.patch.dict(DAO.os.environ, {"TERM": "xterm"}), mock.patch.object(DAO.time, "sleep", side_effect=KeyboardInterrupt):
                self.assertEqual(DAO.execute(f.layout, args), 130)
            self.assertEqual(output.getvalue().count(DAO.node.ALT_SCREEN_EXIT), 1)
            self.assertEqual(f.container["State"]["Status"], "running")

    def test_cli_and_check_need_no_key_or_private_mount(self):
        parser = DAO.node.build_parser()
        for action in ("check", "status", "export", "validate"):
            args = parser.parse_args(["sourcedao", action])
            self.assertFalse(hasattr(args, "key_file"))
        with SourceDaoFixture() as f:
            self.assertTrue(DAO.check(f.layout, f.ctx)["ready_for_bootstrap"])
            self.assertFalse(DAO.task_path(f.layout).exists())
            command = f.calls[-1]
            self.assertNotIn("/private", " ".join(command))
            self.assertNotIn("PRIVATE_KEY", " ".join(command))

    def test_detached_signing_uses_exact_image_readonly_key_and_private_journal(self):
        with SourceDaoFixture() as f:
            before = f.layout.node_env.read_bytes()
            task = f.start()
            command = next(c for c in f.calls if c[0] == "create")
            self.assertIn(f.ctx["binding"]["image"], command)
            self.assertIn("SOURCE_DAO_BOOTSTRAP_PRIVATE_KEY_FILE=/run/bootstrap-admin.key", command)
            self.assertIn(f"type=bind,src={f.key},dst=/run/bootstrap-admin.key,readonly", command)
            self.assertNotIn("PRIVATE_KEY_SENTINEL", json.dumps(f.calls))
            self.assertEqual(command[command.index("--restart") + 1], "no")
            self.assertNotIn("--rm", command)
            self.assertEqual(f.layout.node_env.read_bytes(), before)
            self.assertEqual(DAO.read_task(f.layout)["task_id"], task["task_id"])
            self.assertEqual(DAO.status(f.layout)["outcome"], "RUNNING")

    def test_key_permissions_and_symlinks_fail_before_task_creation(self):
        with SourceDaoFixture() as f:
            f.key.chmod(0o644)
            with self.assertRaisesRegex(ValueError, "0600"):
                f.start()
            f.key.chmod(0o600)
            link = f.key.with_suffix(".link")
            link.symlink_to(f.key)
            with self.assertRaisesRegex(ValueError, "regular file"):
                DAO.start(f.layout, "bootstrap", key=link)
            self.assertFalse(DAO.task_path(f.layout).exists())

    def test_live_blockers_and_identity_mismatch_do_not_mount_a_key(self):
        for mutation in (lambda live: live.update(blockers=["wait for first mined block"]),
                         lambda live: live.update(genesis_hash="0x" + "ee" * 32)):
            with self.subTest(mutation=mutation), SourceDaoFixture() as f:
                mutation(f.live)
                with self.assertRaises(ValueError):
                    f.start()
                self.assertFalse(any(c[0] == "create" for c in f.calls))

    def test_key_cannot_be_reached_through_a_writable_recovery_mount(self):
        with SourceDaoFixture() as f:
            key = f.ctx["private_root"] / "admin.key"
            DAO.node._atomic_write_private(key, "PRIVATE_KEY_SENTINEL")
            with self.assertRaisesRegex(ValueError, "writable recovery mount"):
                DAO.start(f.layout, "bootstrap", key=key)
            self.assertFalse(any(c[0] == "create" for c in f.calls))

    def test_active_task_blocks_restart_upgrade_and_duplicate_bootstrap(self):
        with SourceDaoFixture() as f:
            f.start()
            with self.assertRaisesRegex(ValueError, "already active"):
                f.start()
            for operation in ("activate-release", "down", "mining-enable"):
                with self.assertRaisesRegex(ValueError, "SourceDAO task is active"):
                    with DAO.node.node_operation_lock(f.layout, operation):
                        self.fail("must not change a running ceremony")
            with mock.patch.object(DAO.node, "stop_controller_unit") as stop:
                with self.assertRaisesRegex(ValueError, "SourceDAO task is active"):
                    DAO.node.down_node(f.layout, keep_bitcoin=True)
                stop.assert_not_called()

    def test_too_few_blocks_for_remaining_transactions_rejects_before_signing(self):
        with SourceDaoFixture() as f:
            f.live.update(fee_split_block=8192, checkpoint={"number": 8170})
            with self.assertRaisesRegex(ValueError, "Insufficient blocks"):
                f.start()
            self.assertFalse(any(c[0] == "create" for c in f.calls))

    def test_stopped_container_proves_stale_lock_and_preserves_journal(self):
        with SourceDaoFixture() as f:
            task = f.start()
            journal = f.ctx["state"].with_name("state.json.transactions.json")
            DAO.node._atomic_write_private(journal, "private signed journal")
            lock = f.stale_lock(task["task_id"])
            f.finish(exit_code=137, completed=False)
            self.assertEqual(DAO.status(f.layout)["outcome"], "FAILED")
            retry = f.start()
            self.assertNotEqual(retry["task_id"], task["task_id"])
            self.assertFalse(lock.exists())
            self.assertEqual(journal.read_text(), "private signed journal")
            self.assertTrue((DAO.root(f.layout) / "logs" / f"{task['task_id']}.log").is_file())

    def test_missing_or_foreign_owner_never_clears_state_lock(self):
        for missing in (True, False):
            with self.subTest(missing=missing), SourceDaoFixture() as f:
                task = f.start()
                f.finish(exit_code=137, completed=False)
                lock = f.stale_lock(task["task_id"] if missing else "foreign-owner")
                if missing:
                    f.container = None
                with self.assertRaisesRegex(ValueError, "no proven stopped owner"):
                    f.start()
                self.assertTrue(lock.exists())

    def test_start_failure_leaves_created_task_recoverable(self):
        with SourceDaoFixture() as f:
            f.failure = ["start"]
            with self.assertRaisesRegex(ValueError, "injected"):
                f.start()
            self.assertEqual(DAO.status(f.layout)["outcome"], "STARTING")
            f.failure = None
            f.start()
            self.assertEqual(DAO.status(f.layout)["outcome"], "RUNNING")

    def test_status_requires_state_and_fresh_finalization(self):
        with SourceDaoFixture() as f:
            f.start()
            f.finish(completed=False)
            self.assertEqual(DAO.status(f.layout)["outcome"], "FAILED")
            f.finish()
            self.assertEqual(DAO.status(f.layout)["outcome"], "SUCCEEDED")
            f.live["finalized"] = False
            self.assertEqual(DAO.status(f.layout)["outcome"], "FAILED")
            f.live["finalized"] = True
            f.failure = ["run"]
            self.assertEqual(DAO.status(f.layout)["outcome"], "UNAVAILABLE")

    def test_docker_failure_does_not_mean_task_is_absent(self):
        with SourceDaoFixture() as f:
            f.start()
            f.failure = ["container", "ls"]
            with self.assertRaisesRegex(ValueError, "injected"):
                DAO.require_idle(f.layout)

    def test_readonly_operations_never_receive_a_key_and_validate_has_no_private_mount(self):
        with SourceDaoFixture() as f:
            f.start()
            f.finish()
            f.start("export")
            f.finish()
            DAO.node._atomic_write_private(f.ctx["public_state"], json.dumps({"record_schema": "sourcedao-bootstrap-public-state:v1", "ceremony_identity": f.ctx["binding"]}))
            self.assertEqual(DAO.status(f.layout)["outcome"], "SUCCEEDED")
            f.start("validate")
            commands = [c for c in f.calls if c[0] == "create"]
            for command in commands[1:]:
                self.assertNotIn("PRIVATE_KEY", " ".join(command))
                self.assertNotIn(str(f.key), " ".join(command))
            self.assertNotIn("dst=/private", " ".join(commands[-1]))
            f.finish()
            self.assertEqual(DAO.status(f.layout)["outcome"], "FAILED")
            DAO.node._atomic_write_private(f.ctx["validation"], json.dumps({"status": "ok", "mode": "strict", "evidence": f.ctx["binding"]}))
            self.assertEqual(DAO.status(f.layout)["outcome"], "SUCCEEDED")

    def test_manifest_source_mismatch_is_rejected_before_docker(self):
        with SourceDaoFixture() as f:
            f.node.manifest["images"]["sourcedao_tools"]["source_revision"] = "e" * 40
            f.node.write_manifest()
            with self.assertRaisesRegex(ValueError, "source identity"):
                f.start()
            self.assertEqual(f.calls, [])


if __name__ == "__main__":
    unittest.main()
