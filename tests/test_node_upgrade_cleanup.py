"""Destructive upgrade cleanup uses only disposable files and mocked host services."""
from contextlib import ExitStack, redirect_stdout, redirect_stderr
import copy
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'docker/scripts/tools'))
import node_rebuild as core
import node_uninstall as uninstall
import node_upgrade as upgrade
import node_upgrade_archives as archives
import node_upgrade_cleanup as cleanup
import node_upgrade_session as session
import usdb_node as node
from common.node_upgrade import UpgradeFixture


class CleanupTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='usdb-cleanup-test-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.f = UpgradeFixture(self.root)
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.output = io.StringIO()
        self.stack.enter_context(redirect_stdout(self.output))
        self.stack.enter_context(redirect_stderr(io.StringIO()))
        self.stack.enter_context(mock.patch.object(core, 'command', side_effect=self.f.host_command))
        self.stack.enter_context(mock.patch.object(uninstall, 'operator_home', return_value=self.f.home))
        self.stack.enter_context(mock.patch.object(sys.stdin, 'isatty', return_value=True))
        self.stack.enter_context(mock.patch.object(sys.stdout, 'isatty', return_value=True))
        self.stack.enter_context(mock.patch.object(node, 'effective_memory_bytes', return_value=64*1024**3))
        self.stack.enter_context(mock.patch.object(archives, 'configuration_paths', return_value=[self.f.env_path]))
        self.stack.enter_context(mock.patch.object(cleanup, 'open_processes'))
        chain = self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'geth/chaindata'
        chain.mkdir()
        (chain / '0001.ldb').write_bytes(b'old chain database')
        index = self.f.paths['USDB_INDEXER_DATA_HOST_DIR'] / 'data'
        index.mkdir()
        (index / 'miner_pass.db').write_bytes(b'old miner passes')
        (index / 'energy').mkdir()
        (index / 'energy/LOCK').touch()
        (index / 'energy/0001.sst').write_bytes(b'old energy')
        (index / 'unknown-wallet.json').write_bytes(b'private-custom-fixture')
        session.stage(upgrade.plan(self.f.target, node, self.f.source_kit), self.f.backup, node)
        session.Session(self.f.backup, node).run(confirm=lambda: f'UPGRADE {self.f.bundle} {socket.gethostname()}')
        self.state = session.read(self.f.backup)
        self.items = archives.candidates(self.state)

    def run_cleanup(self):
        phrase = f"CLEANUP {self.f.bundle} {self.state['operation_id']} {socket.gethostname()}"
        return cleanup.Cleanup(self.f.backup, node).run(confirm=lambda: phrase)

    def snapshot(self):
        return {str(p): core.stamp(p) for p in self.root.rglob('*')}

    def test_readonly_inventory_shows_real_archives_and_no_private_values(self):
        before = self.snapshot()
        value = archives.report(self.f.backup, node)
        self.assertEqual(before, self.snapshot())
        self.assertTrue(value['executable'])
        self.assertEqual(len(value['archives']), 4)
        self.assertTrue(all(i['logical_bytes'] > 0 and i['allocated_bytes'] > 0 for i in value['archives']))
        self.assertIn(str(self.f.paths['USDB_INDEXER_DATA_HOST_DIR']), [i['path'] for i in value['archives']])
        self.assertNotIn('private-custom-fixture', json.dumps(value))
        self.assertNotIn('bitcoin_core', [i['kind'] for i in value['archives']])

    def test_success_preserves_private_unknown_files_current_data_and_audit(self):
        current = node.read_env(self.f.env_path)
        before = {key: core.inventory(Path(current[key])) for key in self.f.paths}
        self.assertEqual(self.run_cleanup(), 0)
        self.assertTrue(all(not Path(i['path']).exists() for i in self.items))
        self.assertEqual(session.read(self.f.backup)['phase'], 'cleaned')
        for key in self.f.paths:
            self.assertEqual(core.inventory(Path(current[key])), before[key])
        contents = [p.read_bytes() for p in (self.f.backup / 'private/retained').rglob('*') if p.is_file()]
        for secret in (b'private-nodekey-fixture', b'private-wallet-fixture', b'private-custom-fixture'):
            self.assertIn(secret, contents)
        self.assertNotIn(b'old chain database', contents)
        self.assertNotIn(b'old miner passes', contents)
        self.assertNotIn(b'old energy', contents)
        self.assertTrue((self.f.backup / 'private/node.env').exists())
        self.assertEqual(self.run_cleanup(), 0)
        with self.assertRaisesRegex(ValueError, 'relinquished rollback'):
            session.Session(self.f.backup, node)

    def test_r7_without_catalog_and_current_env_changed_after_start(self):
        catalog = self.f.data / archives.CATALOG
        for path in catalog.iterdir():
            path.unlink()
        catalog.rmdir()
        self.f.env_path.write_text(self.f.env_path.read_text() + '\n# operational settings changed after startup\n')
        self.assertEqual(self.run_cleanup(), 0)
        self.assertTrue((catalog / (self.state['operation_id'] + '.json')).exists())

    def test_cancel_and_preview_do_not_create_cleanup_or_change_node(self):
        before = self.snapshot()
        self.assertEqual(cleanup.Cleanup(self.f.backup, node).run(confirm=lambda: 'cancel'), 0)
        self.assertEqual(before, self.snapshot())
        self.assertEqual(session.read(self.f.backup)['phase'], 'applied')

    def test_current_config_reference_blocks_old_indexer(self):
        self.f.env_path.write_text(node.upsert_env(self.f.env_path.read_text(), {'USDB_INDEXER_DATA_HOST_DIR': str(self.f.paths['USDB_INDEXER_DATA_HOST_DIR'])}))
        value = archives.report(self.f.backup, node)
        self.assertFalse(value['executable'])
        self.assertTrue(any('configuration:' in r for i in value['archives'] for r in i['references']))
        with self.assertRaisesRegex(ValueError, 'blocked'):
            self.run_cleanup()

    def test_other_network_configuration_is_checked(self):
        other = self.f.home / '.config/usdb/another/node.env'
        other.parent.mkdir()
        other.write_text('USDB_INDEXER_DATA_HOST_DIR=' + str(self.f.paths['USDB_INDEXER_DATA_HOST_DIR']))
        with mock.patch.object(archives, 'configuration_paths', return_value=[self.f.env_path, other]):
            self.assertFalse(archives.report(self.f.backup, node)['executable'])

    def test_stopped_foreign_container_reference_is_blocked(self):
        container = dict(id='foreign', state='exited', labels={}, mounts=[dict(Source=self.items[0]['path'])])
        with mock.patch.object(core, 'containers', return_value=[container]):
            self.assertFalse(archives.report(self.f.backup, node)['executable'])
            with self.assertRaisesRegex(ValueError, 'blocked'):
                self.run_cleanup()

    def test_reference_probe_failure_is_not_assumed_unused(self):
        with mock.patch.object(core, 'containers', side_effect=ValueError('Docker unavailable')):
            value = archives.report(self.f.backup, node)
            self.assertFalse(value['executable'])
            self.assertIn('Reference inspection incomplete', value['blockers'][0])

    def test_other_registered_and_explicit_legacy_records_block(self):
        extra = self.root / 'different-volume/other-backup'
        extra.mkdir(parents=True)
        state = copy.deepcopy(self.state)
        state['operation_id'] = '1' * 32
        core.atomic_json(extra / 'upgrade.json', state)
        self.assertTrue(archives.report(self.f.backup, node)['executable'])
        self.assertFalse(archives.report(self.f.backup, node, [extra])['executable'])
        archives.register(extra)
        self.assertFalse(archives.report(self.f.backup, node)['executable'])
        state['phase'] = 'cleaned'
        core.atomic_json(extra / 'upgrade.json', state)
        self.assertTrue(archives.report(self.f.backup, node)['executable'])

    def test_missing_registered_record_blocks(self):
        file = self.f.data / archives.CATALOG / ('2' * 32 + '.json')
        core.atomic_json(file, dict(operation_id='2'*32, backup_dir=str(self.root / 'lost-backup')))
        self.assertFalse(archives.report(self.f.backup, node)['executable'])

    def test_changed_archive_and_dataset_marker_fail_before_delete(self):
        archive = Path(self.items[0]['path'])
        (archive / 'new-file').write_text('unexpected')
        with self.assertRaisesRegex(ValueError, 'blocked|entries changed'):
            self.run_cleanup()
        self.assertEqual(session.read(self.f.backup)['phase'], 'applied')
        self.assertTrue(all(Path(i['path']).exists() for i in self.items))

    def test_replaced_old_indexer_marker_blocks(self):
        path = self.f.paths['USDB_INDEXER_DATA_HOST_DIR'] / archives.DATASET_IDENTITY_FILE
        path.write_text('{}')
        with self.assertRaisesRegex(ValueError, 'blocked'):
            self.run_cleanup()

    def test_tampered_paths_and_incomplete_entries_are_rejected(self):
        state = copy.deepcopy(self.state)
        state['entries'][0]['archive'] = str(self.f.paths['BTC_NODE_DATA_HOST_DIR'])
        core.atomic_json(self.f.backup / 'upgrade.json', state)
        with self.assertRaisesRegex(ValueError, 'Unexpected isolation'):
            archives.report(self.f.backup, node)
        state = copy.deepcopy(self.state)
        state['entries'] = []
        core.atomic_json(self.f.backup / 'upgrade.json', state)
        with self.assertRaisesRegex(ValueError, 'Missing retained archive'):
            archives.report(self.f.backup, node)

    def test_symlink_hardlink_and_mount_are_rejected(self):
        directory = Path(self.items[0]['path'])
        link = directory / 'unsafe'
        link.symlink_to(self.f.paths['BTC_NODE_DATA_HOST_DIR'])
        self.assertFalse(archives.report(self.f.backup, node)['executable'])
        link.unlink()
        os.link(directory / 'opaque.db', link)
        self.assertFalse(archives.report(self.f.backup, node)['executable'])
        link.unlink()
        with mock.patch.object(core, 'check_mounts', side_effect=ValueError('nested mount')):
            self.assertFalse(archives.report(self.f.backup, node)['executable'])

    def test_backup_failure_deletes_nothing_and_disables_rollback(self):
        with mock.patch.object(core, 'copy_tree', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(OSError, 'disk full'):
                self.run_cleanup()
        self.assertTrue(all(Path(i['path']).exists() for i in self.items))
        self.assertEqual(session.read(self.f.backup)['phase'], 'cleanup_started')
        with self.assertRaisesRegex(ValueError, 'relinquished'):
            session.Session(self.f.backup, node)
        self.assertEqual(self.run_cleanup(), 0)

    def test_original_private_backup_corruption_blocks_deletion(self):
        (self.f.backup / 'private/config/monitor/config.json').write_text('corrupt')
        with self.assertRaisesRegex(ValueError, 'Backup content differs'):
            self.run_cleanup()
        self.assertTrue(all(Path(i['path']).exists() for i in self.items))

    def test_partial_deletion_resumes_without_recopied_missing_private_files(self):
        def partial(path):
            (path / 'geth/chaindata/0001.ldb').unlink()
            raise KeyboardInterrupt()
        with mock.patch.object(core, 'remove_tree', side_effect=partial):
            with self.assertRaises(KeyboardInterrupt):
                self.run_cleanup()
        self.assertEqual(session.read(self.f.backup)['phase'], 'cleanup_started')
        self.assertEqual(self.run_cleanup(), 0)

    def test_resume_rejects_new_files_in_partial_archive(self):
        with mock.patch.object(core, 'remove_tree', side_effect=KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.run_cleanup()
        (Path(self.items[0]['path']) / 'new-wallet').write_bytes(b'new private data')
        with self.assertRaisesRegex(ValueError, 'blocked|entries changed'):
            self.run_cleanup()

    def test_resume_recovers_crash_after_remove_before_journal_update(self):
        remove = core.remove_tree
        def crash(path):
            remove(path)
            raise KeyboardInterrupt()
        with mock.patch.object(core, 'remove_tree', side_effect=crash):
            with self.assertRaises(KeyboardInterrupt):
                self.run_cleanup()
        self.assertEqual(self.run_cleanup(), 0)

    def test_cleaned_path_recreated_is_not_removed_again(self):
        self.run_cleanup()
        path = Path(self.items[0]['path'])
        path.mkdir()
        (path / 'new-wallet').write_bytes(b'keep')
        with self.assertRaisesRegex(ValueError, 'blocked'):
            self.run_cleanup()
        self.assertEqual((path / 'new-wallet').read_bytes(), b'keep')

    def test_pending_upgrade_and_running_services_block_cleanup(self):
        core.atomic_json(self.f.config / upgrade.PENDING, dict(backup_dir=str(self.f.backup)))
        with self.assertRaisesRegex(ValueError, 'UPGRADE_PENDING'):
            self.run_cleanup()
        (self.f.config / upgrade.PENDING).unlink()
        with mock.patch.object(uninstall, 'check_stopped', side_effect=ValueError('service active')):
            with self.assertRaisesRegex(ValueError, 'service active'):
                self.run_cleanup()
        self.assertFalse((self.f.backup / 'cleanup.json').exists())

    def test_live_database_lock_and_process_reference_block(self):
        lock = self.f.paths['USDB_INDEXER_DATA_HOST_DIR'] / 'data/energy/LOCK'
        script = 'import fcntl,sys; f=open(sys.argv[1],"r+"); fcntl.flock(f,fcntl.LOCK_EX); print("ready",flush=True); sys.stdin.read()'
        process = subprocess.Popen([sys.executable, '-c', script, str(lock)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(process.stdout.readline().strip(), 'ready')
            with self.assertRaisesRegex(ValueError, 'still locked'):
                self.run_cleanup()
        finally:
            process.communicate('quit', timeout=10)
        with mock.patch.object(cleanup, 'open_processes', side_effect=ValueError('open by process')):
            with self.assertRaisesRegex(ValueError, 'open by process'):
                self.run_cleanup()

    def test_cli_preview_defaults_and_json_remains_structured(self):
        args = node.build_parser().parse_args(['upgrade-cleanup', '--backup-dir', str(self.f.backup), '--json'])
        self.assertFalse(args.execute)
        self.output.seek(0); self.output.truncate()
        before = self.snapshot()
        self.assertEqual(archives.dispatch(args, self.f.target, node), 0)
        self.assertEqual(json.loads(self.output.getvalue())['phase'], 'applied')
        self.assertEqual(before, self.snapshot())
        args.execute = True
        with self.assertRaisesRegex(ValueError, 'preview-only'):
            archives.dispatch(args, self.f.target, node)

    def test_corrupt_preserved_copy_on_resume_blocks_remaining_deletions(self):
        with mock.patch.object(core, 'remove_tree', side_effect=KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.run_cleanup()
        files = [p for p in (self.f.backup / 'private/retained').rglob('*') if p.is_file()]
        files[0].write_bytes(b'corrupt backup')
        with self.assertRaisesRegex(ValueError, 'Backup content differs'):
            self.run_cleanup()
        self.assertTrue(all(Path(i['path']).exists() for i in self.items))

    def test_cleanup_journal_target_tampering_and_missing_journal_are_rejected(self):
        with mock.patch.object(core, 'remove_tree', side_effect=KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.run_cleanup()
        path = self.f.backup / 'cleanup.json'
        record = core.read_json(path)
        record['targets'][str(self.f.paths['BTC_NODE_DATA_HOST_DIR'])] = {}
        core.atomic_json(path, record)
        with self.assertRaisesRegex(ValueError, 'target set changed'):
            self.run_cleanup()
        path.unlink()
        with self.assertRaisesRegex(ValueError, 'state is missing'):
            self.run_cleanup()

    def test_unknown_database_named_directory_is_preserved(self):
        tree = {'.': {'kind':'directory'}, 'data': {'kind':'directory'},
                'data/miner_pass.db': {'kind':'directory'}, 'data/miner_pass.db/wallet': {'kind':'file'}}
        self.assertEqual(cleanup.private_paths('usdb_indexer', tree), ['.'])

    def test_foreign_configuration_record_is_rejected_by_cli(self):
        args = node.build_parser().parse_args(['upgrade-status', '--backup-dir', str(self.f.backup)])
        other = copy.copy(self.f.target)
        from dataclasses import replace
        other = replace(other, node_env=self.root / 'other.env')
        with self.assertRaisesRegex(ValueError, 'another node configuration'):
            archives.dispatch(args, other, node)

    def test_privileged_preview_stays_readonly_and_keeps_legacy_reference_arguments(self):
        args = node.build_parser().parse_args(['upgrade-cleanup', '--backup-dir', str(self.f.backup),
                                              '--other-backup-dir', str(self.root / 'legacy'), '--json'])
        with mock.patch.object(archives, 'report', side_effect=PermissionError('protected file')), \
             mock.patch.object(archives.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as runner, \
             mock.patch.object(archives.os, 'geteuid', return_value=1000):
            self.assertEqual(archives.dispatch(args, self.f.target, node), 0)
        command = runner.call_args.args[0]
        self.assertEqual(command[:2], ['sudo', '--'])
        self.assertIn('--inspect', command)
        self.assertIn('--json', command)
        self.assertIn('--other-backup-dir', command)
        self.assertFalse((self.f.backup / 'cleanup.json').exists())

    def test_noninteractive_and_unfinished_or_rolled_back_upgrade_are_rejected(self):
        with mock.patch.object(sys.stdin, 'isatty', return_value=False):
            with self.assertRaisesRegex(ValueError, 'interactive terminal'):
                self.run_cleanup()
        for phase in ('prepared', 'rolled_back'):
            state = dict(self.state, phase=phase)
            core.atomic_json(self.f.backup / 'upgrade.json', state)
            with self.assertRaisesRegex(ValueError, 'requires an applied'):
                cleanup.Cleanup(self.f.backup, node)


class ProcessInspectionTests(unittest.TestCase):
    def test_open_descriptor_mapping_and_working_directory_block(self):
        with tempfile.TemporaryDirectory(prefix='usdb-process-test-') as directory:
            root = Path(directory)
            archive = root / 'archive'
            archive.mkdir()
            file = archive / 'db'
            file.write_bytes(b'data')
            proc = root / 'proc' / str(os.getpid() + 1000000)
            (proc / 'fd').mkdir(parents=True)
            (proc / 'cwd').symlink_to(root)
            (proc / 'root').symlink_to('/')
            (proc / 'maps').write_text('')
            (proc / 'fd/4').symlink_to(file)
            with self.assertRaisesRegex(ValueError, 'open by process'):
                cleanup.open_processes([archive], root / 'proc')
            (proc / 'fd/4').unlink()
            (proc / 'maps').write_text('000-fff r--p 00000000 00:00 1 ' + str(file))
            with self.assertRaisesRegex(ValueError, 'open by process'):
                cleanup.open_processes([archive], root / 'proc')
            (proc / 'maps').write_text('')
            (proc / 'cwd').unlink()
            (proc / 'cwd').symlink_to(archive)
            with self.assertRaisesRegex(ValueError, 'open by process'):
                cleanup.open_processes([archive], root / 'proc')
            (proc / 'cwd').unlink()
            (proc / 'cwd').symlink_to(root)
            cleanup.open_processes([archive], root / 'proc')


if __name__ == '__main__':
    unittest.main()
