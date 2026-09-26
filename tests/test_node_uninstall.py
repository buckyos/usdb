"""Operator uninstall boundaries and recovery, using disposable node data only."""
from contextlib import ExitStack, redirect_stdout
import fcntl
import io
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_rebuild as core
import node_uninstall as uninstall
import usdb_node as node
from common.node_rebuild import RebuildFixture


class UninstallTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-uninstall-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.f = RebuildFixture(self.root)
        self.f.units.mkdir()
        for prefix in uninstall.UNITS:
            (self.f.units / f"{prefix}-{self.f.bundle}.service").write_text("[Unit]\nDescription=fixture\n")
        self.output = io.StringIO()
        capture = redirect_stdout(self.output)
        capture.__enter__()
        self.addCleanup(capture.__exit__, None, None, None)

    def plan(self, purge=False):
        return uninstall.plan(self.f.home, self.f.bundle, self.f.backup, purge, self.f.units)

    def session(self, purge=False):
        uninstall.stage(self.plan(purge), self.f.backup)
        return uninstall.Session(self.f.backup)

    def execution(self, purge=False):
        stack = ExitStack()
        phrase = ("PURGE " if purge else "UNINSTALL ") + self.f.bundle + " " + socket.gethostname()
        stack.enter_context(mock.patch.object(uninstall, "check_stopped"))
        stack.enter_context(mock.patch.object(core, "containers", return_value=[]))
        stack.enter_context(mock.patch.object(core, "command", return_value=""))
        stack.enter_context(mock.patch.object(sys.stdin, "isatty", return_value=True))
        stack.enter_context(mock.patch.object(sys.stdout, "isatty", return_value=True))
        stack.enter_context(mock.patch("builtins.input", return_value=phrase))
        return stack

    def test_cli_help_and_default_preview_do_not_write_or_stop_services(self):
        args = node.build_parser().parse_args(["uninstall"])
        self.assertFalse(args.execute)
        self.assertFalse(args.purge_data)
        before = sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*"))
        value = self.plan()
        uninstall.show(value, self.f.backup)
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*")))
        self.assertNotIn("private-test-value", self.output.getvalue())
        self.assertFalse(self.f.backup.exists())
        self.assertIn("Retain:", self.output.getvalue())
        layout = SimpleNamespace(node_env=self.f.env, bundle_id=self.f.bundle)
        with mock.patch.object(uninstall, "operator_home", return_value=self.f.home), \
                mock.patch.object(uninstall, "plan", return_value=value), \
                mock.patch.object(uninstall.subprocess, "run", side_effect=AssertionError("preview must not execute host operations")):
            self.assertEqual(node._execute_command(layout, args), 0)
        self.assertEqual(before, sorted(str(p.relative_to(self.root)) for p in self.root.rglob("*")))

    def test_default_uninstall_preserves_all_config_data_and_wallets(self):
        session = self.session()
        with self.execution():
            self.assertEqual(session.run(), 0)
        for path in self.f.paths.values():
            self.assertTrue(path.exists())
        self.assertTrue(self.f.env.exists())
        self.assertTrue((self.f.paths["BTC_NODE_DATA_HOST_DIR"] / "wallet.dat").exists())
        self.assertTrue((self.f.paths["USDB_CHAIN_DATA_HOST_DIR"] / "keystore/key").exists())
        self.assertFalse(self.f.launcher.is_symlink())
        self.assertFalse(self.f.release.exists())
        self.assertTrue(session.state["complete"])
        self.assertEqual(list(self.f.units.iterdir()), [])

    def test_purge_verifies_private_backup_and_preserves_unrelated_data(self):
        ord_dir = self.f.data / "datasets/ord/btc-mainnet/ord-0.23.3"
        ord_dir.mkdir(parents=True)
        (ord_dir / "index.redb").write_bytes(b"rebuildable")
        (ord_dir / "wallet.json").write_bytes(b"private-wallet")
        with self.f.env.open("a") as output:
            output.write(f"ORD_DATA_HOST_DIR={ord_dir}\n")
        monitor = self.f.config / "monitor"
        monitor.mkdir()
        (monitor / "events.sqlite3").write_bytes(b"diagnostic-history")
        foreign = self.f.data / "datasets/usdb-indexer" / ("c" * 64)
        foreign.mkdir()
        (foreign / "preserve").write_text("other network")
        session = self.session(True)
        with self.execution(True):
            session.run()
        self.assertFalse(self.f.env.exists())
        self.assertFalse(ord_dir.exists())
        self.assertFalse(self.f.launcher.is_symlink())
        self.assertEqual((self.f.backup / "private/bitcoin/wallet.dat").read_bytes(), b"private wallet")
        self.assertEqual(session.state["backups"]["bitcoin/wallet.dat"]["ownership"]["."], dict(uid=os.getuid(), gid=os.getgid()))
        self.assertEqual((self.f.backup / "private/chain/keystore/key").read_bytes(), b"private chain key")
        self.assertEqual((self.f.backup / "private/ord/wallet.json").read_bytes(), b"private-wallet")
        self.assertEqual((self.f.backup / "private/config/monitor/events.sqlite3").read_bytes(), b"diagnostic-history")
        self.assertFalse((self.f.backup / "private/bitcoin/blocks").exists())
        self.assertFalse((self.f.backup / "private/chain/geth/chaindata").exists())
        self.assertTrue(foreign.exists())
        self.assertNotIn("private-test-value", (self.f.backup / "uninstall.json").read_text())

    def test_cancel_and_noninteractive_execution_never_remove_data(self):
        session = self.session(True)
        with self.execution(True), mock.patch("builtins.input", return_value=""):
            self.assertEqual(session.run(), 0)
        self.assertTrue(self.f.env.exists())
        with self.execution(True), mock.patch.object(sys.stdin, "isatty", return_value=False), self.assertRaisesRegex(ValueError, "interactive"):
            session.run()
        self.assertTrue(self.f.launcher.is_symlink())

    def test_corrupt_backup_and_new_wallet_block_deletion(self):
        session = self.session(True)
        session.backup()
        (self.f.backup / "private/bitcoin/wallet.dat").write_bytes(b"corrupt")
        with self.execution(True), self.assertRaises(ValueError):
            session.run()
        self.assertTrue(self.f.env.exists())
        (self.f.backup / "private/bitcoin/wallet.dat").write_bytes(b"private wallet")
        (self.f.paths["BTC_NODE_DATA_HOST_DIR"] / "new-wallet").write_bytes(b"new keys")
        with self.execution(True), self.assertRaisesRegex(ValueError, "New private state"):
            session.run()
        self.assertTrue(self.f.paths["BTC_NODE_DATA_HOST_DIR"].exists())

    def test_interrupted_deletion_resumes_from_staged_runner_after_kit_removal(self):
        session = self.session(True)
        original = core.remove_tree
        def remove(path):
            original(path)
            if path == self.f.release:
                raise OSError("injected interruption after release removal")
        with self.execution(True), mock.patch.object(core, "remove_tree", side_effect=remove), self.assertRaises(OSError):
            session.run()
        self.assertFalse(self.f.release.exists())
        runner = self.f.backup / "runner/node_uninstall.py"
        result = subprocess.run([sys.executable, str(runner), "--resume", str(self.f.backup), "--plan"], capture_output=True, text=True, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        with self.execution(True):
            self.assertEqual(uninstall.Session(self.f.backup).run(), 0)
        self.assertFalse(self.f.env.exists())
        self.assertTrue((self.f.backup / "private/config/node.env").is_file())

    def test_changed_config_or_replaced_target_blocks_execution(self):
        session = self.session()
        self.f.env.write_text(self.f.env.read_text() + "NEW=value\n")
        with self.execution(), self.assertRaisesRegex(ValueError, "node.env changed"):
            session.run()
        self.assertTrue(self.f.release.exists())

    def test_running_monitor_shared_container_and_mounts_block_cleanup(self):
        value = self.plan(True)
        active = "LoadState=loaded\nActiveState=active\nUnitFileState=enabled\n"
        stopped = "LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\n"
        with mock.patch.object(core, "command", side_effect=[stopped, active]), self.assertRaisesRegex(ValueError, "still active or enabled"):
            uninstall.check_stopped(value)
        container = dict(id="a"*64, state="running", labels={}, mounts=[{"Source": str(self.f.paths["BTC_NODE_DATA_HOST_DIR"])}])
        with mock.patch.object(core, "command", return_value=stopped), mock.patch.object(core, "containers", return_value=[container]), self.assertRaisesRegex(ValueError, "still uses this node"):
            uninstall.check_stopped(value)
        container.update(state="exited", labels={"com.docker.compose.project": "other-network"})
        with mock.patch.object(core, "command", return_value=stopped), mock.patch.object(core, "containers", return_value=[container]), self.assertRaisesRegex(ValueError, "still references"):
            uninstall.check_stopped(value, deleting=self.f.paths["BTC_NODE_DATA_HOST_DIR"])
        with mock.patch.object(core, "mounts", return_value=[self.f.paths["BTC_NODE_DATA_HOST_DIR"]]), self.assertRaisesRegex(ValueError, "mount point"):
            self.plan(True)

    def test_active_sourcedao_task_blocks_before_staging_or_sudo(self):
        import usdb_sourcedao
        args = node.build_parser().parse_args(["uninstall", "--execute", "--backup-dir", str(self.f.backup)])
        layout = SimpleNamespace(node_env=self.f.env, bundle_id=self.f.bundle)
        value = self.plan()
        with self.execution(), mock.patch.object(uninstall, "operator_home", return_value=self.f.home), \
                mock.patch.object(uninstall, "plan", return_value=value), \
                mock.patch.object(usdb_sourcedao, "require_idle", side_effect=ValueError("A SourceDAO task is active")), \
                mock.patch.object(uninstall.subprocess, "run", side_effect=AssertionError("must not invoke sudo")), \
                self.assertRaisesRegex(ValueError, "SourceDAO task"):
            node._execute_command(layout, args)
        self.assertFalse(self.f.backup.exists())

    def test_unreadable_state_and_custom_unit_overrides_refuse_cleanup(self):
        value = self.plan(True)
        with mock.patch.object(core, "command", return_value=""), self.assertRaisesRegex(ValueError, "Cannot inspect"):
            uninstall.check_stopped(value)
        Path(value["units"][1] + ".d").mkdir()
        stopped = "LoadState=loaded\nActiveState=inactive\nUnitFileState=disabled\n"
        with mock.patch.object(core, "command", return_value=stopped), self.assertRaisesRegex(ValueError, "manual review"):
            uninstall.check_stopped(value)

    def test_private_symlink_or_live_operation_lock_blocks_before_removal(self):
        outside = self.root / "private-wallet"
        outside.write_bytes(b"external private state")
        (self.f.paths["BTC_NODE_DATA_HOST_DIR"] / "linked-wallet").symlink_to(outside)
        session = self.session(True)
        with self.execution(True), self.assertRaisesRegex(ValueError, "symlink"):
            session.run()
        self.assertEqual(outside.read_bytes(), b"external private state")
        self.assertTrue(self.f.env.exists())
        (self.f.paths["BTC_NODE_DATA_HOST_DIR"] / "linked-wallet").unlink()
        with (self.f.config / ".usdb-node-operation.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.execution(True), self.assertRaisesRegex(ValueError, "still locked"):
                session.run()
        self.assertTrue(self.f.env.exists())

    def test_backup_location_and_unrecognized_ord_layout_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "Backup"):
            uninstall.plan(self.f.home, self.f.bundle, self.f.data / "backup", True, self.f.units)
        with self.f.env.open("a") as output:
            output.write(f"ORD_DATA_HOST_DIR={self.root}\n")
        with self.assertRaisesRegex(ValueError, "Unexpected Ord"):
            self.plan(True)


if __name__ == "__main__":
    unittest.main()
