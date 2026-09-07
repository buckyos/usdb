#!/usr/bin/env python3
"""Accept release-bound managed SourceDAO operations and interruption recovery."""
import json
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import usdb_sourcedao as DAO
from common.sourcedao import SourceDaoFixture


class SourceDaoTests(unittest.TestCase):
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
