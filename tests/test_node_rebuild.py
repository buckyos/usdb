#!/usr/bin/env python3
"""Exercise backup integrity, interrupted moves, explicit consent and scoped removal."""

from contextlib import ExitStack, redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import node_rebuild as tool
from common.node_rebuild import RebuildFixture


class RebuildTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-rebuild-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.fixture = RebuildFixture(self.root)
        self.output = io.StringIO()
        capture = redirect_stdout(self.output)
        capture.__enter__()
        self.addCleanup(capture.__exit__, None, None, None)
        container_patch = mock.patch.object(tool, "containers", return_value=[])
        container_patch.start()
        self.addCleanup(container_patch.stop)

    def target(self, session, key):
        return next(t for t in session.plan.targets if t.key == key)

    def operations(self, answer=True):
        stack = ExitStack()
        stack.enter_context(mock.patch.object(tool, "check_stopped"))
        stack.enter_context(mock.patch.object(tool, "confirm", return_value=answer))
        return stack

    def test_standalone_plan_is_read_only_and_does_not_disclose_config(self):
        script = self.root / "standalone.py"
        shutil.copy2(tool.__file__, script)
        result = subprocess.run([sys.executable, str(script), "plan", "--operator-home", str(self.fixture.home),
                                 "--backup-dir", str(self.fixture.backup)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(self.fixture.paths["BH_DATA_HOST_DIR"]), result.stdout)
        self.assertNotIn("private-test-value", result.stdout + result.stderr)
        self.assertFalse(self.fixture.backup.exists())

    def test_run_rejects_piped_yes_before_any_mutation(self):
        result = subprocess.run([sys.executable, tool.__file__, "run", "--operator-home", str(self.fixture.home),
            "--backup-dir", str(self.fixture.backup), "--expect-host", socket.gethostname()],
            input="yes\nyes\n", capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("interactive terminal", result.stderr)
        self.assertFalse(self.fixture.backup.exists())

    def test_interactive_main_backups_only_confirmed_paths_and_resumes(self):
        plan = self.fixture.plan(tool)
        argv = [tool.__file__, "run", "--operator-home", str(self.fixture.home), "--backup-dir", str(self.fixture.backup),
                "--expect-host", socket.gethostname()]
        prompts = []

        def choose(action, path, extra=""):
            prompts.append((action, path))
            return action == "PREPARE BACKUP DIRECTORY" or (action == "BACKUP" and path == self.fixture.paths["BH_DATA_HOST_DIR"] / "db/balance_history")

        with mock.patch.object(sys, "argv", argv), mock.patch.object(tool, "build_plan", return_value=plan), \
             mock.patch.object(tool, "check_stopped"), mock.patch.object(sys.stdin, "isatty", return_value=True), \
             mock.patch.object(self.output, "isatty", return_value=True), mock.patch.object(tool, "confirm", side_effect=choose):
            self.assertEqual(tool.main(), 0)
            self.assertEqual(tool.main(), 0)
        self.assertTrue((self.fixture.backup / "objects/balance-history/db/balance_history/000001.sst").is_file())
        self.assertFalse((self.fixture.backup / "objects/config").exists())
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())
        self.assertEqual(sum(action == "BACKUP" for action, path in prompts), 1)

    def test_wrong_host_is_rejected_before_mutation(self):
        result = subprocess.run([sys.executable, tool.__file__, "run", "--operator-home", str(self.fixture.home),
            "--backup-dir", str(self.fixture.backup), "--expect-host", "not-this-host"], capture_output=True, text=True)
        self.assertIn("Host differs", result.stderr)
        self.assertFalse(self.fixture.backup.exists())

    @unittest.skipIf(os.geteuid() == 0, "Requires an unprivileged process to exercise denied access")
    def test_unreadable_directory_cannot_hide_database_locks(self):
        chain = self.fixture.paths["USDB_CHAIN_DATA_HOST_DIR"]
        locked = chain / "geth"
        (locked / "LOCK").write_bytes(b"")
        mode = locked.stat().st_mode & 0o777
        try:
            locked.chmod(0)
            with ExitStack() as locks, self.assertRaises(PermissionError):
                tool.acquire_locks(chain, locks)
        finally:
            locked.chmod(mode)

    @unittest.skipIf(os.geteuid() == 0, "Requires an unprivileged process to exercise denied access")
    def test_permission_failure_reports_sudo_and_preserves_resumable_session(self):
        plan = self.fixture.plan(tool)
        chain = self.fixture.paths["BH_DATA_HOST_DIR"] / "db/balance_history"
        keystore = chain / "unreadable"
        keystore.mkdir()
        mode = keystore.stat().st_mode & 0o777
        argv = [tool.__file__, "run", "--operator-home", str(self.fixture.home), "--backup-dir", str(self.fixture.backup),
                "--expect-host", socket.gethostname()]
        errors = io.StringIO()

        def choose(action, path, extra=""):
            return action == "PREPARE BACKUP DIRECTORY" or (action == "BACKUP" and path == chain)

        with mock.patch.object(sys, "argv", argv), mock.patch.object(tool, "build_plan", return_value=plan), \
             mock.patch.object(tool, "check_stopped"), mock.patch.object(sys.stdin, "isatty", return_value=True), \
             mock.patch.object(self.output, "isatty", return_value=True), mock.patch.object(tool, "confirm", side_effect=choose), \
             redirect_stderr(errors):
            try:
                keystore.chmod(0)
                self.assertEqual(tool.main(), 1)
            finally:
                keystore.chmod(mode)
            self.assertIn(str(keystore), errors.getvalue())
            self.assertIn("sudo", errors.getvalue())
            self.assertIn("--operator-home", errors.getvalue())
            self.assertNotIn("private chain key", errors.getvalue())
            saved = json.loads((self.fixture.backup / "session.json").read_text())
            self.assertNotIn("balance-history", saved["items"])
            self.assertFalse(any(event["action"] == "delete_started" for event in saved["events"]))
            self.assertFalse((self.fixture.backup / "objects/chain").exists())
            self.assertEqual(tool.main(), 0)
        self.assertEqual((self.fixture.backup / "objects/balance-history/db/balance_history/000001.sst").read_bytes(), b"coins" * 1000)
        self.assertEqual((chain / "000001.sst").read_bytes(), b"coins" * 1000)

    def test_target_overlap_and_env_path_injection_are_rejected(self):
        for backup in (self.fixture.data, self.fixture.paths["BH_DATA_HOST_DIR"] / "backup", self.root):
            with self.subTest(backup=backup), self.assertRaises(ValueError):
                tool.build_plan(self.fixture.home, self.fixture.bundle, backup, self.fixture.units)
        old = self.fixture.env.read_text()
        self.fixture.env.write_text(old.replace(str(self.fixture.paths["BTC_NODE_DATA_HOST_DIR"]), "/"))
        with self.assertRaisesRegex(ValueError, "Unexpected data path"):
            self.fixture.plan(tool)

    def test_symlink_ancestor_and_nested_mount_are_rejected(self):
        target = self.fixture.paths["BH_DATA_HOST_DIR"]
        target.rename(target.with_name("actual"))
        target.symlink_to(target.with_name("actual"), target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.fixture.plan(tool)
        with mock.patch.object(tool, "mounts", return_value=[self.fixture.paths["BTC_NODE_DATA_HOST_DIR"] / "blocks"]), \
             self.assertRaisesRegex(ValueError, "nested mount"):
            tool.inventory(self.fixture.paths["BTC_NODE_DATA_HOST_DIR"])

    def test_symlink_or_hardlink_inside_partial_copy_cannot_overwrite_other_data(self):
        source, partial = self.root / "source", self.root / "partial"
        source.mkdir()
        partial.mkdir()
        (source / "file").write_bytes(b"new data")
        outside = self.root / "outside"
        outside.write_bytes(b"keep")
        for link in ("symlink", "hardlink"):
            with self.subTest(link=link):
                if link == "symlink":
                    (partial / "file").symlink_to(outside)
                else:
                    os.link(outside, partial / "file")
                with self.assertRaises(ValueError):
                    tool.copy_tree(source, partial)
                self.assertEqual(outside.read_bytes(), b"keep")
                (partial / "file").unlink()

    def test_skipping_backup_protects_bh_but_does_not_block_other_cleanup(self):
        session = self.fixture.session(tool)
        with self.operations(False):
            self.assertFalse(session.preserve(self.target(session, "balance-history")))
        with self.operations(True):
            session.clean(self.target(session, "balance-history"))
            session.clean(self.target(session, "balance-history-root"))
            session.clean(self.target(session, "legacy-snapshots"))
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())
        self.assertFalse(self.fixture.paths["BH_SNAPSHOT_HOST_DIR"].exists())

    def test_only_rocksdb_is_archived_and_other_selected_paths_need_no_backup(self):
        session = self.fixture.session(tool)
        outside = self.root / "unrelated"
        outside.write_bytes(b"preserve me")
        with self.operations():
            session.preserve(self.target(session, "balance-history"))
            for key in ("balance-history", "balance-history-root", "bitcoin", "chain", "config", self.fixture.release.name):
                target = self.target(session, key)
                session.clean(target)
        objects = self.fixture.backup / "objects"
        self.assertEqual((objects / "balance-history/db/balance_history/000001.sst").read_bytes(), b"coins" * 1000)
        self.assertFalse((objects / "balance-history/auxiliary").exists())
        self.assertEqual([p.name for p in objects.iterdir()], ["balance-history"])
        self.assertEqual(outside.read_bytes(), b"preserve me")
        self.assertNotIn("private-test-value", self.output.getvalue())
        self.assertNotIn("private-test-value", (self.fixture.backup / "session.json").read_text())
        # Config and kit may already be deleted when an SSH session is interrupted.
        resumed = self.fixture.session(tool)
        resumed.verify(self.target(resumed, "balance-history"))
        with self.operations():
            resumed.clean(self.target(resumed, "launcher"))
        self.assertFalse(self.fixture.launcher.is_symlink())

    def test_backup_corruption_or_changed_source_blocks_deletion(self):
        for side in ("backup", "source"):
            with self.subTest(side=side):
                session = self.fixture.session(tool)
                target = self.target(session, "balance-history")
                with self.operations():
                    session.preserve(target)
                    file = (session.destination(target) if side == "backup" else target.path) / "000001.sst"
                    original = file.read_bytes()
                    file.write_bytes(b"changed")
                    with self.assertRaisesRegex(ValueError, "differs|changed"):
                        session.clean(target)
                    self.assertTrue(target.path.exists())
                    file.write_bytes(original)
                # A changed source has different ctime; do not silently adopt it on retries.
                if side == "backup":
                    session.verify(target)

    def test_bitcoin_wallet_cleanup_requires_confirmation_but_no_backup(self):
        session = self.fixture.session(tool)
        target = self.target(session, "bitcoin")
        with self.operations(False):
            session.clean(target)
        self.assertTrue((target.path / "wallet.dat").exists())
        with self.operations(True):
            (target.path / "wallets").mkdir()
            session.clean(target)
        self.assertFalse(target.path.exists())
        self.assertFalse((self.fixture.backup / "objects/bitcoin").exists())

    def test_move_can_reconcile_interruption_after_rename(self):
        session = self.fixture.session(tool, "move")
        target = self.target(session, "balance-history")
        with self.operations(), mock.patch.object(tool, "verify_rename", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                session.preserve(target)
        self.assertFalse(target.path.exists())
        self.assertTrue((self.fixture.backup / "objects/balance-history/db/balance_history/000001.sst").exists())
        resumed = self.fixture.session(tool, "move")
        with self.operations():
            self.assertTrue(resumed.preserve(self.target(resumed, "balance-history")))
        self.assertTrue(resumed.state["items"]["balance-history"]["complete"])

    def test_move_is_same_filesystem_only_and_uses_no_copy(self):
        session = self.fixture.session(tool, "move")
        target = self.target(session, "balance-history")
        inode = (target.path / "000001.sst").stat().st_ino
        with self.operations(), mock.patch.object(tool, "copy_tree", side_effect=AssertionError("must not copy")), \
             mock.patch.object(tool, "digest", side_effect=AssertionError("must not hash all file contents during rename")):
            session.preserve(target)
            session.verify(target)
        self.assertFalse(target.path.exists())
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())
        self.assertEqual((session.destination(target) / "000001.sst").stat().st_ino, inode)

    def test_cross_filesystem_move_fails_before_scanning_or_prompting(self):
        session = self.fixture.session(tool, "move")
        original = Path.stat

        def other_device(path, *args, **kwargs):
            value = original(path, *args, **kwargs)
            return SimpleNamespace(st_dev=value.st_dev + 1, st_mode=value.st_mode) if path == self.fixture.backup or self.fixture.backup in path.parents else value

        with self.operations(), mock.patch.object(Path, "stat", other_device), \
             mock.patch.object(tool, "inventory", side_effect=AssertionError("must fail before scanning")), \
             mock.patch.object(tool, "confirm", side_effect=AssertionError("must fail before confirmation")), \
             self.assertRaisesRegex(ValueError, "same filesystem.*Relocate old_backup"):
            session.preserve(self.target(session, "balance-history"))
        self.assertFalse((self.fixture.backup / "objects/balance-history").exists())

    def test_changed_moved_database_blocks_bh_root_cleanup(self):
        session = self.fixture.session(tool, "move")
        target = self.target(session, "balance-history")
        with self.operations():
            session.preserve(target)
            (session.destination(target) / "000001.sst").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "metadata changed"):
                session.clean(self.target(session, "balance-history-root"))
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())

    def test_removing_old_snapshot_hardlinks_does_not_invalidate_the_moved_db(self):
        session = self.fixture.session(tool, "move")
        target = self.target(session, "balance-history")
        os.link(target.path / "000001.sst", self.fixture.paths["BH_DATA_HOST_DIR"] / "old-snapshot.sst")
        with self.operations():
            session.preserve(target)
            session.clean(self.target(session, "balance-history-root"))
        session.verify(target)
        self.assertEqual((session.destination(target) / "000001.sst").read_bytes(), b"coins" * 1000)

    def test_relocated_v1_session_resumes_without_requiring_other_backups(self):
        self.fixture.legacy_session(tool)
        shutil.rmtree(self.fixture.config)
        old_backup = self.fixture.backup
        self.fixture.backup = self.root / "relocated-old-backup"
        # Copying the archive changes its inodes, as a /home -> /data relocation does.
        shutil.copytree(old_backup, self.fixture.backup)
        shutil.rmtree(old_backup)
        session = self.fixture.session(tool, "move")
        self.assertEqual(session.state["identity"]["schema_version"], tool.SCHEMA)
        self.assertNotIn("private-test-value", json.dumps(session.state["layout"]))
        target = self.target(session, "balance-history")
        self.assertEqual(target.path, self.fixture.paths["BH_DATA_HOST_DIR"] / "db/balance_history")
        with self.operations():
            session.preserve(target)
            session.clean(self.target(session, "balance-history-root"))
            session.clean(self.target(session, self.fixture.release.name))
        # New sessions only need the saved path layout after config/kits are removed.
        shutil.rmtree(self.fixture.backup / "objects/config")
        shutil.rmtree(self.fixture.backup / "objects" / self.fixture.release.name)
        resumed = self.fixture.session(tool, "move")
        resumed.verify(self.target(resumed, "balance-history"))
        self.assertFalse(self.fixture.paths["BH_DATA_HOST_DIR"].exists())

    def test_completed_v1_bh_archive_keeps_its_original_boundary(self):
        self.fixture.legacy_session(tool, "copy", completed_bh=True)
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        self.assertEqual(target.path, self.fixture.paths["BH_DATA_HOST_DIR"])
        with self.operations():
            session.preserve(target)
            session.clean(target)
        session.verify(target)
        self.assertTrue((self.fixture.backup / "objects/balance-history/db/balance_history/000001.sst").exists())

    def test_nested_archive_reports_the_actual_session_directory(self):
        self.fixture.legacy_session(tool)
        outer = self.root / "outer-backup"
        outer.mkdir()
        self.fixture.backup.rename(outer / "old_backup")
        with self.assertRaisesRegex(ValueError, "Nested backup session.*outer-backup/old_backup"):
            tool.build_plan(self.fixture.home, self.fixture.bundle, outer, self.fixture.units)
        self.assertFalse((outer / "session.json").exists())

    def test_old_session_rejects_changed_host_or_bh_source(self):
        self.fixture.legacy_session(tool)
        path = self.fixture.backup / "session.json"
        original = path.read_text()
        saved = json.loads(original)
        saved["identity"]["hostname"] = "another-host"
        path.write_text(json.dumps(saved))
        with self.assertRaisesRegex(ValueError, "another host"):
            self.fixture.session(tool, "move")
        path.write_text(original)
        source = self.fixture.paths["BH_DATA_HOST_DIR"]
        source.rename(source.with_name("original"))
        (source / "db/balance_history").mkdir(parents=True)
        with self.assertRaisesRegex(ValueError, "Legacy BH source changed"):
            self.fixture.session(tool, "move")

    def test_interrupted_copy_can_retry_without_deleting_source(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        with self.operations(), mock.patch.object(tool, "verify_tree", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                session.preserve(target)
        self.assertTrue(target.path.exists())
        self.assertFalse(session.destination(target).exists())
        with self.operations():
            session.preserve(target)
        session.verify(target)

    def test_copy_publication_interruption_can_reconcile(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        original = tool.sync_dir

        def interrupt(path):
            if path == session.destination(target).parent:
                raise KeyboardInterrupt
            original(path)

        with self.operations(), mock.patch.object(tool, "sync_dir", side_effect=interrupt):
            with self.assertRaises(KeyboardInterrupt):
                session.preserve(target)
        resumed = self.fixture.session(tool)
        with self.operations():
            self.assertTrue(resumed.preserve(self.target(resumed, "balance-history")))

    def test_interrupted_deletion_can_resume_only_unchanged_remaining_data(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")

        def interrupted_remove(path):
            (path / "000001.sst").unlink()
            raise KeyboardInterrupt

        with self.operations():
            session.preserve(target)
            with mock.patch.object(tool, "remove_tree", side_effect=interrupted_remove), self.assertRaises(KeyboardInterrupt):
                session.clean(target)
            resumed = self.fixture.session(tool)
            # A new file after interruption must not be swept up by the old confirmation.
            unexpected = target.path / "new-user-file"
            unexpected.write_bytes(b"preserve")
            with self.assertRaisesRegex(ValueError, "New data"):
                resumed.clean(target)
            unexpected.unlink()
            resumed.clean(target)
        self.assertFalse(target.path.exists())
        session.verify(target)

    def test_enospc_or_source_replacement_never_deletes_source(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        with self.operations(), mock.patch.object(tool.shutil, "disk_usage", return_value=shutil._ntuple_diskusage(1, 1, 0)):
            with self.assertRaisesRegex(ValueError, "Insufficient backup space"):
                session.preserve(target)
        self.assertTrue(target.path.exists())
        target.path.rename(target.path.with_name("preserved"))
        target.path.mkdir()
        with self.operations(), self.assertRaisesRegex(ValueError, "replaced"):
            session.clean(target)

    def test_active_rocksdb_lock_is_rejected(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        path = target.path / "LOCK"
        child = subprocess.Popen([sys.executable, "-c", "import fcntl,sys; f=open(sys.argv[1],'r+'); fcntl.lockf(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.read()", str(path)],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.addCleanup(lambda: child.poll() is None and child.kill())
        self.assertEqual(child.stdout.readline().strip(), "locked")
        try:
            with self.operations(), self.assertRaisesRegex(ValueError, "still locked"):
                session.preserve(target)
        finally:
            child.communicate(timeout=10)

    def test_db_lock_survives_copy_readers_closing_the_same_file(self):
        path = self.fixture.paths["BH_DATA_HOST_DIR"] / "db/balance_history/LOCK"
        with ExitStack() as locks:
            tool.lock_file(path, locks)
            path.read_bytes()
            code = "import fcntl,sys\nf=open(sys.argv[1],'r+')\ntry: fcntl.lockf(f,fcntl.LOCK_EX|fcntl.LOCK_NB)\nexcept BlockingIOError: sys.exit(0)\nsys.exit(1)"
            result = subprocess.run([sys.executable, "-c", code, str(path)], capture_output=True)
            self.assertEqual(result.returncode, 0)

    def test_stopped_checks_include_shared_and_stopped_mounts(self):
        plan = self.fixture.plan(tool)
        source = self.fixture.paths["BH_DATA_HOST_DIR"]
        status = "LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\n"
        record = dict(id="abc123", state="running", mounts=[dict(Source=str(source))], labels={})
        with mock.patch.object(tool, "command", return_value=status), mock.patch.object(tool, "containers", return_value=[record]):
            with self.assertRaisesRegex(ValueError, "still uses"):
                tool.check_stopped(plan)
            record["state"] = "exited"
            tool.check_stopped(plan)
            with self.assertRaisesRegex(ValueError, "still references"):
                tool.check_stopped(plan, deleting=source)
        with mock.patch.object(tool, "command", return_value=status.replace("disabled", "enabled")), \
             self.assertRaisesRegex(ValueError, "Disable and stop"):
            tool.check_stopped(plan)

    def test_confirmation_defaults_to_skip_and_eof_cancels(self):
        with mock.patch("builtins.input", return_value=""):
            self.assertFalse(tool.confirm("DELETE", self.root))
        with mock.patch("builtins.input", return_value="q"), self.assertRaises(KeyboardInterrupt):
            tool.confirm("DELETE", self.root)
        with mock.patch("builtins.input", side_effect=EOFError), self.assertRaises(EOFError):
            tool.confirm("DELETE", self.root)

    def test_stopped_helper_removal_is_scoped_and_individually_confirmed(self):
        plan = self.fixture.plan(tool)
        helper = dict(id="a" * 64, name="sourcedao", state="exited", mounts=[dict(Source=str(self.fixture.release))], labels={})
        broad = dict(id="b" * 64, state="exited", mounts=[dict(Source=str(self.root))], labels={})
        other = dict(id="c" * 64, state="exited", mounts=helper["mounts"], labels={"com.docker.compose.project": "other-node"})
        with mock.patch.object(tool, "containers", return_value=[helper, broad, other]), mock.patch.object(tool, "command") as execute:
            with self.operations(False):
                tool.remove_stopped_containers(plan)
            execute.assert_not_called()
            with self.operations(True):
                tool.remove_stopped_containers(plan)
            execute.assert_called_once_with(["docker", "rm", "--", helper["id"]])

    def test_helper_started_after_confirmation_is_not_removed(self):
        helper = dict(id="a" * 64, name="sourcedao", state="exited", mounts=[dict(Source=str(self.fixture.release))], labels={})

        def start_after_confirmation(*_args):
            helper["state"] = "running"
            return True

        with mock.patch.object(tool, "containers", return_value=[helper]), mock.patch.object(tool, "command") as execute, \
             self.operations(), mock.patch.object(tool, "confirm", side_effect=start_after_confirmation), \
             self.assertRaisesRegex(ValueError, "Container changed"):
            tool.remove_stopped_containers(self.fixture.plan(tool))
        execute.assert_not_called()


if __name__ == "__main__":
    unittest.main()
