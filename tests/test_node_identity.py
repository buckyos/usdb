"""Explicit identity operations and deletion consent with disposable files only."""
from contextlib import nullcontext, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
from types import SimpleNamespace
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'docker/scripts/tools'))
import node_identity as identity
import node_identity_files as files
import node_rebuild as core
import usdb_node as node
import test_node_uninstall as uninstall_tests
from common.node_identity import container_identity

KEY = ('0' * 63 + '1').encode()
PUBLIC = ('79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798'
          '483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8')


class IdentityTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='usdb-identity-test-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.chain = self.root / 'chain'
        self.chain.mkdir()
        self.path = self.chain / 'geth/nodekey'
        config = self.root / '.config/usdb/usdb-testnet-v1'
        config.mkdir(parents=True)
        env = config / 'node.env'
        env.write_text('USDB_CHAIN_DATA_HOST_DIR=' + str(self.chain) + '\n')
        self.layout = SimpleNamespace(node_env=env, bundle_id='usdb-testnet-v1', images={})
        self.node = SimpleNamespace(read_env=node.read_env, controller_observed_state=lambda _: 'uninstalled',
                                    node_operation_lock=lambda *args: nullcontext())
        self.output = io.StringIO()
        self.source = self.root / 'backup/nodekey'
        self.source.parent.mkdir()
        self.source.write_bytes(KEY)

    def run_action(self, action, *, execute=False, answer=None):
        args = SimpleNamespace(identity_action=action, execute=execute, from_key=self.source, json=True)
        fingerprint = hashlib.sha256(KEY).hexdigest()
        phrase = action.upper() + ' IDENTITY ' + (self.layout.bundle_id if action == 'create' else fingerprint)
        with redirect_stdout(self.output), mock.patch('builtins.input', return_value=phrase if answer is None else answer), \
                mock.patch.object(sys.stdin, 'isatty', return_value=True), \
                mock.patch.object(sys.stdout, 'isatty', return_value=True), \
                mock.patch.object(Path, 'home', return_value=self.root), \
                mock.patch.object(core, 'containers', return_value=[]):
            return identity.dispatch(args, self.layout, self.node)

    def test_known_key_and_read_only_missing_status(self):
        self.assertEqual(files.inspect(self.path)['state'], 'MISSING')
        self.assertFalse(self.path.parent.exists())
        self.assertEqual(files.public_key(KEY), PUBLIC)

    def test_create_preview_cancel_and_explicit_execution(self):
        self.run_action('create')
        self.assertFalse(self.path.exists())
        self.run_action('create', execute=True, answer='yes')
        self.assertFalse(self.path.exists())
        self.run_action('create', execute=True)
        self.assertEqual(files.inspect(self.path)['state'], 'READY')
        self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)
        self.assertNotIn(self.path.read_text(), self.output.getvalue())
        with self.assertRaisesRegex(ValueError, 'already exists'):
            self.run_action('create', execute=True)

    def test_restore_and_archive_keep_exact_identity(self):
        self.run_action('restore', execute=True)
        self.assertEqual(self.path.read_bytes(), KEY)
        self.run_action('archive', execute=True)
        archive = self.root / '.local/state/usdb/identities/usdb-testnet-v1' / hashlib.sha256(KEY).hexdigest() / 'nodekey'
        self.assertFalse(self.path.exists())
        self.assertEqual(archive.read_bytes(), KEY)
        self.source = archive
        self.run_action('restore', execute=True)
        self.assertEqual(self.path.read_bytes(), KEY)
        self.assertNotIn(KEY.decode(), self.output.getvalue())

    def test_invalid_key_never_replaced_but_can_be_explicitly_archived(self):
        self.path.parent.mkdir()
        self.path.write_bytes(b'corrupt')
        self.assertEqual(files.inspect(self.path)['state'], 'INVALID')
        with self.assertRaisesRegex(ValueError, 'P2P_IDENTITY_INVALID'):
            identity.require_ready(self.layout, self.node)
        with self.assertRaisesRegex(ValueError, 'already exists'):
            self.run_action('restore', execute=True)
        fingerprint = hashlib.sha256(b'corrupt').hexdigest()
        self.run_action('archive', execute=True, answer='ARCHIVE IDENTITY ' + fingerprint)
        self.run_action('restore', execute=True)
        self.assertEqual(self.path.read_bytes(), KEY)

    def test_restore_is_bound_to_reviewed_source_and_never_overwrites(self):
        fingerprint = files.inspect(self.source)['fingerprint']
        self.source.write_bytes(('0' * 63 + '2').encode())
        with self.assertRaisesRegex(ValueError, 'changed'):
            files.operate('restore', self.path, source=self.source, expected=fingerprint)
        self.assertFalse(self.path.exists())
        files.write_new(self.path, KEY, os.getuid(), os.getgid())
        with self.assertRaises(FileExistsError):
            files.write_new(self.path, b'other', os.getuid(), os.getgid())
        self.assertEqual(self.path.read_bytes(), KEY)

    def test_symlinks_and_shared_files_are_rejected(self):
        self.path.parent.mkdir()
        self.path.symlink_to(self.source)
        with self.assertRaisesRegex(ValueError, 'symbolic links'):
            files.inspect(self.path)
        self.path.unlink()
        os.link(self.source, self.path)
        with self.assertRaisesRegex(ValueError, 'link count'):
            files.inspect(self.path)

    def test_running_controller_prevents_identity_changes(self):
        self.node.controller_observed_state = lambda _: 'active'
        with self.assertRaisesRegex(ValueError, 'Stop the controller'):
            self.run_action('create', execute=True)
        self.assertFalse(self.path.exists())

    def test_missing_key_blocks_startup_with_restore_guidance(self):
        with self.assertRaisesRegex(ValueError, 'identity restore'):
            identity.require_ready(self.layout, self.node)
        self.assertFalse(self.path.exists())

    def test_unknown_controller_and_live_source_mount_prevent_restore(self):
        self.node.controller_observed_state = lambda _: 'unknown'
        with self.assertRaisesRegex(ValueError, 'Stop the controller'):
            self.run_action('restore', execute=True)
        self.node.controller_observed_state = lambda _: 'inactive'
        with mock.patch.object(core, 'containers', return_value=[{
                'state': 'running', 'mounts': [{'Source': str(self.source.parent)}]}]):
            with self.assertRaisesRegex(ValueError, 'running container'):
                identity.stopped(self.layout, self.node, [self.source])
        self.assertFalse(self.path.exists())

    def test_conflicting_backup_never_removes_active_key(self):
        self.run_action('restore', execute=True)
        fingerprint = files.inspect(self.path)['fingerprint']
        backup = self.root / 'conflicting/nodekey'
        backup.parent.mkdir()
        backup.write_bytes(b'different')
        with self.assertRaisesRegex(ValueError, 'different contents'):
            files.operate('archive', self.path, archive=backup, expected=fingerprint)
        self.assertEqual(self.path.read_bytes(), KEY)
        self.assertEqual(backup.read_bytes(), b'different')

    def test_legacy_identity_must_be_explicitly_selected(self):
        legacy = self.chain / 'nodekey'
        legacy.write_bytes(KEY)
        with self.assertRaisesRegex(ValueError, 'Legacy nodekey'):
            self.run_action('create', execute=True)
        self.assertFalse(self.path.exists())
        self.source = legacy
        self.run_action('restore', execute=True)
        self.assertEqual(self.path.read_bytes(), legacy.read_bytes())

    def test_source_change_during_confirmation_is_rejected(self):
        def change():
            self.source.write_bytes(('0' * 63 + '2').encode())
            return True
        # isatty is evaluated after the preview, before consuming confirmation.
        args = SimpleNamespace(identity_action='restore', execute=True, from_key=self.source)
        phrase = 'RESTORE IDENTITY ' + hashlib.sha256(KEY).hexdigest()
        with redirect_stdout(self.output), mock.patch.object(sys.stdin, 'isatty', side_effect=change), \
                mock.patch.object(sys.stdout, 'isatty', return_value=True), \
                mock.patch('builtins.input', return_value=phrase), \
                mock.patch.object(core, 'containers', return_value=[]), \
                mock.patch.object(Path, 'home', return_value=self.root):
            with self.assertRaisesRegex(ValueError, 'changed after preview'):
                identity.dispatch(args, self.layout, self.node)
        self.assertFalse(self.path.exists())

    def test_noninteractive_execution_is_rejected(self):
        args = SimpleNamespace(identity_action='create', execute=True)
        with redirect_stdout(self.output), mock.patch.object(sys.stdin, 'isatty', return_value=False):
            with self.assertRaisesRegex(ValueError, 'interactive terminal'):
                identity.dispatch(args, self.layout, self.node)
        self.assertFalse(self.path.exists())

    def test_operation_started_during_confirmation_blocks_identity_change(self):
        import usdb_mining
        with mock.patch.object(usdb_mining, 'pending', side_effect=[False, True]):
            with self.assertRaisesRegex(ValueError, 'pending mining/peer'):
                self.run_action('create', execute=True)
        self.assertFalse(self.path.exists())

    def test_permission_error_uses_only_public_readonly_helper_result(self):
        self.layout.images = {'USDB_CHAIN_IMAGE': 'chain@sha256:' + '1' * 64}
        report = files.inspect(self.source)
        import subprocess
        with mock.patch.object(files, 'inspect', side_effect=PermissionError), \
                mock.patch.object(subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout=json.dumps(report))) as run:
            observed = identity.observe(self.layout, self.path)
        self.assertEqual(observed['public_key'], PUBLIC)
        args = run.call_args.args[0]
        self.assertIn('--pull=never', args)
        self.assertIn('--read-only', args)
        self.assertIn('--network=none', args)
        self.assertIn('type=bind,src=' + str(self.chain) + ',dst=/identity,readonly', args)
        self.assertNotIn(KEY.decode(), run.call_args.kwargs['input'])

    def test_readable_identity_does_not_require_a_cached_image(self):
        with mock.patch.object(identity, 'subprocess') as docker:
            report = identity.observe(self.layout, self.source)
        self.assertEqual(report['state'], 'READY')
        docker.run.assert_not_called()

    def test_only_explicit_image_absence_can_defer_helper_failure(self):
        self.layout.images = {'USDB_CHAIN_IMAGE': 'chain@sha256:' + '1' * 64}
        image = self.layout.images['USDB_CHAIN_IMAGE']
        cases = [
            (125, 1, f'Error response from daemon: No such image: {image}\n', identity.InspectionPending),
            (125, 1, 'Cannot connect to the Docker daemon', ValueError),
            (125, 1, 'permission denied while trying to connect to the Docker daemon socket', ValueError),
            (125, 1, 'No such image: another-image', ValueError),
            (125, 0, '', ValueError),
            (1, 1, f'No such image: {image}', ValueError),
        ]
        for helper_code, inspect_code, stderr, error_type in cases:
            results = [SimpleNamespace(returncode=helper_code),
                       SimpleNamespace(returncode=inspect_code, stdout='[]', stderr=stderr)]
            with self.subTest(case=(helper_code, stderr)), \
                    mock.patch.object(files, 'inspect', side_effect=PermissionError), \
                    mock.patch.object(identity, 'subprocess', SimpleNamespace(run=mock.Mock(side_effect=results))):
                with self.assertRaises(error_type) as raised:
                    identity.observe(self.layout, self.path)
                self.assertIs(type(raised.exception), error_type)
                self.assertNotIn('usdb-node pull', str(raised.exception))

    def test_pending_status_does_not_authorize_identity_mutation(self):
        self.path.parent.mkdir()
        self.path.write_bytes(KEY)
        self.layout.images = {'USDB_CHAIN_IMAGE': 'chain@sha256:' + '1' * 64}
        with container_identity(self.layout, self.node):
            with redirect_stdout(self.output):
                code = identity.dispatch(SimpleNamespace(identity_action='status', json=True), self.layout, self.node)
            report = json.loads(self.output.getvalue())
            self.assertEqual((code, report['state']), (1, 'PENDING'))
            self.assertNotIn('public_key', report)
            self.assertIn('P2P identity: PENDING', identity.render(report))
            for action in ('create', 'restore', 'archive'):
                with self.subTest(action=action), self.assertRaises(identity.InspectionPending):
                    self.run_action(action, execute=True)
        self.assertEqual(self.path.read_bytes(), KEY)

    def test_helper_timeout_is_not_an_image_preparation_pending_result(self):
        import subprocess
        self.layout.images = {'USDB_CHAIN_IMAGE': 'chain@sha256:' + '1' * 64}
        for results in ([subprocess.TimeoutExpired('docker run', 30)],
                        [SimpleNamespace(returncode=125), subprocess.TimeoutExpired('docker image inspect', 15)]):
            with self.subTest(results=results), mock.patch.object(files, 'inspect', side_effect=PermissionError), \
                    mock.patch.object(identity, 'subprocess', SimpleNamespace(run=mock.Mock(side_effect=results))):
                with self.assertRaises(subprocess.TimeoutExpired):
                    identity.observe(self.layout, self.path)


class IdentityRemovalTests(unittest.TestCase):
    def test_ordinary_uninstall_keeps_identity_without_removal_confirmation(self):
        case = uninstall_tests.UninstallTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        source = case.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'geth/nodekey'
        source.write_bytes(KEY)
        session = case.session(False)
        with case.execution(False):
            self.assertEqual(session.run(), 0)
        self.assertEqual(source.read_bytes(), KEY)
        self.assertTrue(session.state['complete'])

    def test_generic_purge_confirmation_cannot_remove_nodekey(self):
        case = uninstall_tests.UninstallTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        source = case.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'geth/nodekey'
        source.write_bytes(KEY)
        session = case.session(True)
        with case.execution(True):
            self.assertEqual(session.run(), 0)
        self.assertEqual(source.read_bytes(), KEY)
        self.assertTrue(case.f.env.exists())
        self.assertFalse(session.state['complete'])

    def test_separate_fingerprint_confirmation_backs_up_before_removal(self):
        case = uninstall_tests.UninstallTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        source = case.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'geth/nodekey'
        source.write_bytes(KEY)
        session = case.session(True, keep_bitcoin=True)
        import socket
        answers = ['PURGE ' + case.f.bundle + ' ' + socket.gethostname(),
                   'REMOVE IDENTITY ' + hashlib.sha256(KEY).hexdigest()]
        with case.execution(True), mock.patch('builtins.input', side_effect=answers):
            self.assertEqual(session.run(), 0)
        self.assertFalse(source.exists())
        self.assertEqual((case.f.backup / 'private/chain/geth/nodekey').read_bytes(), KEY)
        self.assertTrue(session.state['complete'])


if __name__ == '__main__':
    unittest.main()
