#!/usr/bin/env python3
"""Exercise backup integrity, interrupted moves, explicit consent and scoped removal."""

from contextlib import ExitStack, redirect_stdout
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
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
            return action == "PREPARE BACKUP DIRECTORY" or (action == "BACKUP" and path == self.fixture.config)

        with mock.patch.object(sys, "argv", argv), mock.patch.object(tool, "build_plan", return_value=plan), \
             mock.patch.object(tool, "check_stopped"), mock.patch.object(sys.stdin, "isatty", return_value=True), \
             mock.patch.object(self.output, "isatty", return_value=True), mock.patch.object(tool, "confirm", side_effect=choose):
            self.assertEqual(tool.main(), 0)
            self.assertEqual(tool.main(), 0)
        self.assertTrue((self.fixture.backup / "objects/config/node.env").is_file())
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())
        self.assertEqual(sum(action == "BACKUP" and path == self.fixture.config for action, path in prompts), 1)

    def test_wrong_host_is_rejected_before_mutation(self):
        result = subprocess.run([sys.executable, tool.__file__, "run", "--operator-home", str(self.fixture.home),
            "--backup-dir", str(self.fixture.backup), "--expect-host", "not-this-host"], capture_output=True, text=True)
        self.assertIn("Host differs", result.stderr)
        self.assertFalse(self.fixture.backup.exists())

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

    def test_skipping_backup_prevents_bh_and_registry_deletion(self):
        session = self.fixture.session(tool)
        with self.operations(False):
            self.assertFalse(session.preserve(self.target(session, "balance-history")))
        with self.operations(True):
            session.clean(self.target(session, "balance-history"))
            session.clean(self.target(session, "legacy-snapshots"))
        self.assertTrue(self.fixture.paths["BH_DATA_HOST_DIR"].exists())
        self.assertTrue(self.fixture.paths["BH_SNAPSHOT_HOST_DIR"].exists())

    def test_copy_verify_and_selected_cleanup_preserve_keys_and_other_paths(self):
        session = self.fixture.session(tool)
        outside = self.root / "unrelated"
        outside.write_bytes(b"preserve me")
        with self.operations():
            for key in ("balance-history", "bitcoin", "chain", "config", self.fixture.release.name):
                target = self.target(session, key)
                session.preserve(target)
                session.verify(target)
                session.clean(target)
        objects = self.fixture.backup / "objects"
        self.assertEqual((objects / "balance-history/auxiliary/registry/000001.sst").read_bytes(), b"scripts")
        self.assertEqual((objects / "bitcoin/wallet.dat").read_bytes(), b"private wallet")
        self.assertFalse((objects / "bitcoin/blocks").exists())
        self.assertEqual((objects / "chain/keystore/key").read_bytes(), b"private chain key")
        self.assertFalse((objects / "chain/geth/chaindata").exists())
        self.assertEqual(outside.read_bytes(), b"preserve me")
        self.assertNotIn("private-test-value", self.output.getvalue())
        # Config and kit may already be deleted when an SSH session is interrupted.
        resumed = self.fixture.session(tool)
        resumed.verify(self.target(resumed, self.fixture.release.name))
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
                    file = (self.fixture.backup / "objects/balance-history" if side == "backup" else target.path) / "config.toml"
                    original = file.read_bytes()
                    file.write_bytes(b"changed")
                    with self.assertRaisesRegex(ValueError, "differs|changed"):
                        session.clean(target)
                    self.assertTrue(target.path.exists())
                    file.write_bytes(original)
                # A changed source has different ctime; do not silently adopt it on retries.
                if side == "backup":
                    session.verify(target)

    def test_new_wallet_after_backup_blocks_bitcoin_cleanup(self):
        session = self.fixture.session(tool)
        target = self.target(session, "bitcoin")
        with self.operations():
            session.preserve(target)
            (target.path / "wallets").mkdir()
            with self.assertRaisesRegex(ValueError, "New identity"):
                session.clean(target)
        self.assertTrue(target.path.exists())

    def test_move_can_reconcile_interruption_after_rename(self):
        session = self.fixture.session(tool, "move")
        target = self.target(session, "balance-history")
        with self.operations(), mock.patch.object(tool, "verify_tree", side_effect=KeyboardInterrupt):
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
        with self.operations(), mock.patch.object(tool, "copy_tree", side_effect=AssertionError("must not copy")):
            session.preserve(self.target(session, "balance-history"))
        self.assertFalse(self.fixture.paths["BH_DATA_HOST_DIR"].exists())

    def test_interrupted_copy_can_retry_without_deleting_source(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        with self.operations(), mock.patch.object(tool, "verify_tree", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                session.preserve(target)
        self.assertTrue(target.path.exists())
        self.assertFalse((self.fixture.backup / "objects/balance-history").exists())
        with self.operations():
            session.preserve(target)
        session.verify(target)

    def test_copy_publication_interruption_can_reconcile(self):
        session = self.fixture.session(tool)
        target = self.target(session, "balance-history")
        original = tool.sync_dir

        def interrupt(path):
            if path == self.fixture.backup / "objects":
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
            (path / "config.toml").unlink()
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
        path = target.path / "db/balance_history/LOCK"
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


if __name__ == "__main__":
    unittest.main()
