"""Release compatibility, preserved data and failure/recovery with real temporary files."""
import copy
from contextlib import ExitStack, redirect_stdout
from dataclasses import replace
import io
import json
import os
import shlex
from pathlib import Path
import socket
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'docker/scripts/tools'))
import node_rebuild as core
import node_uninstall as uninstall
import node_upgrade as upgrade
import node_upgrade_session as session
import runtime_compatibility as runtime
import usdb_node as node
from common.node_upgrade import UpgradeFixture, write_manifest


class UpgradeTests(unittest.TestCase):
    def setUp(self):
        temp=tempfile.TemporaryDirectory(prefix='usdb-upgrade-test-')
        self.addCleanup(temp.cleanup)
        self.root=Path(temp.name)
        self.f=UpgradeFixture(self.root)
        self.output=io.StringIO()
        self.stack=ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(redirect_stdout(self.output))
        self.stack.enter_context(mock.patch.object(core,'command',side_effect=self.f.host_command))
        self.stack.enter_context(mock.patch.object(uninstall,'operator_home',return_value=self.f.home))
        self.stack.enter_context(mock.patch.object(sys.stdin,'isatty',return_value=True))
        self.stack.enter_context(mock.patch.object(sys.stdout,'isatty',return_value=True))
        self.stack.enter_context(mock.patch.object(node,'effective_memory_bytes',return_value=64*1024**3))

    def plan(self):
        return upgrade.plan(self.f.target,node,self.f.source_kit)

    def stage(self):
        return session.stage(self.plan(),self.f.backup,node)

    def run_session(self,*,rollback=False):
        return session.Session(self.f.backup,node).run(rollback=rollback,confirm=lambda:f"{'ROLLBACK' if rollback else 'UPGRADE'} {self.f.bundle} {socket.gethostname()}")

    def test_preview_is_read_only_and_excludes_secrets(self):
        before={str(p):p.stat().st_mtime_ns for p in self.root.rglob('*')}
        value=self.plan()
        after={str(p):p.stat().st_mtime_ns for p in self.root.rglob('*')}
        self.assertEqual(before,after)
        self.assertTrue(value['executable'])
        self.assertEqual(value['classification'],'network_reset')
        self.assertEqual([i['service'] for i in value['components'] if i['action']=='reuse'],['bitcoin_core','balance_history'])
        self.assertNotIn('private-',json.dumps(value))
        self.assertFalse(self.f.backup.exists())

    def test_same_genesis_and_chain_id_still_require_reset_for_registry(self):
        value=self.plan()
        self.assertNotIn('chain_id',value['changed_chain_fields'])
        self.assertNotIn('genesis_block_hash',value['changed_chain_fields'])
        self.assertIn('btc_activation_registry_id',value['changed_chain_fields'])
        self.assertEqual(next(i['action'] for i in value['components'] if i['service']=='usdb_chain'),'rebuild')

    def test_discovers_matching_installed_source_not_latest_tag(self):
        self.assertEqual(upgrade.plan(self.f.target,node)['source_kit'],str(self.f.source_kit))
        with self.assertRaisesRegex(ValueError,'does not match'):
            upgrade.plan(self.f.target,node,self.f.target_kit)

    def test_bad_source_checksum_and_dataset_marker_are_rejected(self):
        marker=self.f.paths['BH_DATA_HOST_DIR']/runtime.DATASET_IDENTITY_FILE
        marker.write_text('{}')
        with self.assertRaisesRegex(ValueError,'marker mismatch'):
            self.plan()
        manifest=self.f.source_kit/'release/usdb-release-manifest.json'
        manifest.write_text(manifest.read_text()+' ')
        with self.assertRaisesRegex(ValueError,'checksum'):
            self.plan()

    def test_path_override_symlink_and_existing_destination_are_rejected(self):
        env=self.f.env_path.read_text()
        node._atomic_write_private(self.f.env_path,node.upsert_env(env,{'BH_DATA_HOST_DIR':str(self.root/'unexpected')}))
        with self.assertRaisesRegex(ValueError,'derived path'):
            self.plan()
        self.f.env_path.write_text(env)
        destination=next(Path(i['target']) for i in self.plan()['components'] if i['service']=='usdb_indexer')
        destination.mkdir()
        self.assertFalse(self.plan()['executable'])
        destination.rmdir()
        destination.symlink_to(self.f.paths['USDB_INDEXER_DATA_HOST_DIR'],target_is_directory=True)
        with self.assertRaisesRegex(ValueError,'symlink'):
            self.plan()

    def test_mainnet_reset_and_source_contract_changes_are_blocked(self):
        network={**self.f.target.network_identity,'bundle_status':'production'}
        self.assertFalse(upgrade.plan(replace(self.f.target,network_identity=network),node,self.f.source_kit)['executable'])
        contract=copy.deepcopy(self.f.target.runtime_compatibility)
        contract['services']['balance_history']['storage_schema']='unknown-new-schema'
        value=upgrade.plan(replace(self.f.target,runtime_compatibility=contract),node,self.f.source_kit)
        self.assertTrue(any('balance_history contract changed' in b for b in value['blockers']))

    def test_compatible_update_routes_to_activate_without_rebuild(self):
        old=upgrade._source_manifest(self.f.source_kit,node)
        target=replace(self.f.target,network_identity=old['network_bundle'],runtime_compatibility=old['runtime_compatibility'])
        value=upgrade.plan(target,node,self.f.source_kit)
        self.assertEqual(value['classification'],'compatible')
        self.assertTrue(all(i['action']=='reuse' for i in value['components']))

    def test_stage_rejects_overlapping_backup_and_running_shared_container(self):
        with self.assertRaisesRegex(ValueError,'overlaps'):
            session.stage(self.plan(),self.f.data/'backup',node)
        container=dict(id='foreign',state='running',labels={},mounts=[{'Source':str(self.f.paths['BH_DATA_HOST_DIR'])}])
        with mock.patch.object(core,'containers',return_value=[container]):
            with self.assertRaisesRegex(ValueError,'Container still uses'):
                self.stage()
        self.assertFalse(self.f.backup.exists())

    def test_cancel_does_not_mutate_node(self):
        self.stage()
        before=self.f.env_path.read_bytes()
        self.assertEqual(session.Session(self.f.backup,node).run(confirm=lambda:'cancel'),0)
        self.assertEqual(before,self.f.env_path.read_bytes())
        self.assertFalse((self.f.config/upgrade.PENDING).exists())
        self.assertTrue((self.f.paths['USDB_CHAIN_DATA_HOST_DIR']/'opaque.db').exists())

    def test_upgrade_preserves_bitcoin_bh_wallet_identity_and_settings(self):
        old=self.f.env_path.read_text()
        self.f.env_path.write_text(node.upsert_env(old,{'USDB_NODE_ROLE':'miner','USDB_MINER_ADDRESS':'0x'+'1'*40}))
        self.stage()
        self.run_session()
        env=node.read_env(self.f.env_path)
        self.assertEqual(env['USDB_NODE_ROLE'],'full')
        self.assertEqual(env['USDB_MINER_ADDRESS'],'')
        self.assertEqual(env['BTC_RPC_PASSWORD'],node.read_env(self.f.backup/'private/node.env')['BTC_RPC_PASSWORD'])
        for key in ('BTC_NODE_DATA_HOST_DIR','BH_DATA_HOST_DIR'):
            self.assertEqual(env[key],str(self.f.paths[key]))
            self.assertTrue((Path(env[key])/'opaque.db').exists())
        chain=Path(env['USDB_CHAIN_DATA_HOST_DIR'])
        self.assertFalse((chain/'opaque.db').exists())
        self.assertEqual((chain/'geth/nodekey').read_text(),'private-nodekey-fixture')
        self.assertEqual((chain/'keystore/wallet.json').read_text(),'private-wallet-fixture')
        self.assertNotEqual(env['USDB_INDEXER_DATA_HOST_DIR'],str(self.f.paths['USDB_INDEXER_DATA_HOST_DIR']))
        self.assertTrue((self.f.paths['USDB_INDEXER_DATA_HOST_DIR']/'opaque.db').exists())
        self.assertFalse((self.f.config/'monitor/events.sqlite3').exists())
        self.assertTrue((self.f.config/'monitor/notifications/config.json').exists())
        self.assertEqual(session.read(self.f.backup)['phase'],'applied')
        self.assertFalse((self.f.config/upgrade.PENDING).exists())
        node._validate_node_config(self.f.target,require_runtime=False,require_bitcoin_runtime=True)
        self.assertNotIn('private-nodekey-fixture',self.output.getvalue())

    def test_completed_upgrade_is_idempotent_and_rolls_back_before_start(self):
        original=self.f.env_path.read_bytes()
        self.stage()
        self.run_session()
        self.run_session()
        self.run_session(rollback=True)
        self.assertEqual(original,self.f.env_path.read_bytes())
        self.assertTrue((self.f.paths['USDB_CHAIN_DATA_HOST_DIR']/'opaque.db').exists())
        self.assertTrue((self.f.config/'monitor/events.sqlite3').exists())
        self.assertEqual(session.read(self.f.backup)['phase'],'rolled_back')

    def test_interruption_after_rename_and_after_config_is_recoverable(self):
        self.stage()
        real=session.Session.event
        fired=[False]
        def crash(s,phase,path=''):
            real(s,phase,path)
            if phase=='isolated' and not fired[0]:
                fired[0]=True
                raise RuntimeError('injected interruption')
        with mock.patch.object(session.Session,'event',crash):
            with self.assertRaisesRegex(RuntimeError,'injected'):
                self.run_session()
        with self.assertRaisesRegex(ValueError,'UPGRADE_PENDING'):
            node.activate_release(self.f.target)
        with self.assertRaisesRegex(ValueError,'UPGRADE_PENDING'):
            with node.node_operation_lock(self.f.target,'up'):
                self.fail('startup must be blocked')
        def crash_config(s,phase,path=''):
            real(s,phase,path)
            if phase=='configuration_ready':
                raise RuntimeError('config interruption')
        with mock.patch.object(session.Session,'event',crash_config):
            with self.assertRaisesRegex(RuntimeError,'config interruption'):
                self.run_session()
        self.run_session()
        self.assertEqual(session.read(self.f.backup)['phase'],'applied')

    def test_rollback_refuses_new_data_or_external_config_changes(self):
        self.stage()
        self.run_session()
        target=Path(node.read_env(self.f.env_path)['USDB_INDEXER_DATA_HOST_DIR'])
        (target/'new-block.db').write_text('new work must survive')
        with self.assertRaisesRegex(ValueError,'entries changed'):
            self.run_session(rollback=True)
        self.assertEqual((target/'new-block.db').read_text(),'new work must survive')
        self.f.env_path.write_text(self.f.env_path.read_text()+'# operator changed config\n')
        with self.assertRaisesRegex(ValueError,'configuration changed'):
            self.run_session(rollback=True)

    def test_empty_preparation_can_be_rolled_back(self):
        self.stage()
        with mock.patch.object(session.Session,'backup',side_effect=OSError('disk full')):
            with self.assertRaisesRegex(OSError,'disk full'):
                self.run_session()
        self.run_session(rollback=True)
        self.assertFalse((self.f.config/upgrade.PENDING).exists())

    def test_resume_rejects_changed_manifest(self):
        self.stage()
        value=copy.deepcopy(self.f.manifest)
        value['images']['usdb_services']['reference']='ghcr.io/buckyos/usdb-services@sha256:'+'8'*64
        write_manifest(self.f.target_kit,value)
        with self.assertRaisesRegex(ValueError,'Target release changed'):
            session.Session(self.f.backup,node)

    def test_native_assumeutxo_reuses_bitcoin_bh_and_bootstrap_inputs(self):
        root = self.root / 'native'
        root.mkdir()
        self.f = UpgradeFixture(root, native=True)
        env = node.read_env(self.f.env_path)
        self.assertEqual(env['SNAPSHOT_MODE'], 'assumeutxo')
        with mock.patch.object(uninstall, 'operator_home', return_value=self.f.home):
            self.stage()
            self.run_session()
        updated = node.read_env(self.f.env_path)
        for key in ('BTC_NODE_DATA_HOST_DIR', 'BH_DATA_HOST_DIR', 'SNAPSHOT_MODE',
                    'BH_ASSUMEUTXO_SNAPSHOT_FILE', 'BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH'):
            self.assertEqual(env[key], updated[key])
        self.assertTrue((self.f.paths['BH_DATA_HOST_DIR'] / 'opaque.db').exists())

    def test_activate_checks_full_genesis_configuration_even_with_same_runtime_id(self):
        network = {**self.f.old['network_bundle'], 'genesis_sha256': 'c' * 64}
        target = replace(self.f.target, network_identity=network,
                         runtime_compatibility=self.f.old['runtime_compatibility'])
        original = self.f.env_path.read_bytes()
        with self.assertRaisesRegex(ValueError, 'explicit rebuild'):
            node.activate_release(target, from_kit=self.f.source_kit)
        self.assertEqual(original, self.f.env_path.read_bytes())

    def test_corrupt_source_artifact_and_network_identity_are_rejected(self):
        self.f.old['network_bundle']['network_id'] += 1
        self.f.rewrite_source()
        with self.assertRaisesRegex(ValueError, 'Source network identity'):
            self.plan()
        self.f.old['network_bundle']['network_id'] -= 1
        self.f.rewrite_source()
        genesis = self.f.source_kit / 'docker/networks' / self.f.bundle / 'artifacts/usdb-genesis.json'
        genesis.write_text(genesis.read_text() + ' ')
        with self.assertRaisesRegex(ValueError, 'artifact checksum'):
            self.plan()

    def test_recovery_rejects_edited_destinations_and_private_backup(self):
        self.stage()
        record = session.read(self.f.backup)
        record['plan']['components'][2]['target'] = str(self.root / 'unrelated')
        core.atomic_json(self.f.backup / 'upgrade.json', record)
        with self.assertRaisesRegex(ValueError, 'path/action mismatch'):
            session.Session(self.f.backup, node)
        record['plan'] = {**self.plan(), **{k: record['plan'][k] for k in ('operator_home', 'operator_uid', 'operator_gid')}}
        core.atomic_json(self.f.backup / 'upgrade.json', record)
        self.run_session()
        backup = self.f.backup / 'private/node.env'
        backup.write_text(backup.read_text() + '# corruption\n')
        with self.assertRaisesRegex(ValueError, 'Saved original configuration changed'):
            self.run_session(rollback=True)
        self.assertFalse((self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'opaque.db').exists())

    def test_pending_upgrade_rejects_destructive_cli_and_preserves_status(self):
        core.atomic_json(self.f.config / upgrade.PENDING, dict(backup_dir=str(self.f.backup)))
        args = node.build_parser().parse_args(['uninstall', '--execute'])
        with self.assertRaisesRegex(ValueError, 'UPGRADE_PENDING'):
            node._execute_command(self.f.target, args)
        import usdb_sourcedao
        args = node.build_parser().parse_args(['sourcedao', 'status'])
        with mock.patch.object(usdb_sourcedao, 'execute', return_value=0):
            self.assertEqual(node._execute_command(self.f.target, args), 0)

    def test_interrupt_after_publish_and_during_rollback_can_resume(self):
        self.stage()
        real = session.Session.event
        def crash(s, phase, path=''):
            real(s, phase, path)
            if phase == 'dataset_ready':
                raise RuntimeError('publication interrupted')
        with mock.patch.object(session.Session, 'event', crash):
            with self.assertRaisesRegex(RuntimeError, 'publication interrupted'):
                self.run_session()
        self.run_session()
        real_rename = os.rename
        fired = [False]
        def interrupted_restore(source, destination):
            real_rename(source, destination)
            if '.before-upgrade-' in str(source) and not fired[0]:
                fired[0] = True
                raise OSError('rollback interrupted')
        with mock.patch.object(os, 'rename', interrupted_restore):
            with self.assertRaisesRegex(OSError, 'rollback interrupted'):
                self.run_session(rollback=True)
        self.run_session(rollback=True)
        self.assertTrue((self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'opaque.db').exists())
        self.assertEqual(session.read(self.f.backup)['phase'], 'rolled_back')

    def test_autostart_stays_disabled_after_apply_and_restores_only_on_rollback(self):
        self.stage()
        states = {Path(p).name: 'enabled' for p in session.runtime_plan(session.read(self.f.backup)['plan'])['units']}
        calls = []
        real_sha = upgrade.sha
        def stopped(value, *, allow_enabled=False, **kwargs):
            enabled = {k: v for k, v in states.items() if v == 'enabled'}
            if not allow_enabled and enabled:
                raise ValueError('enabled service')
            return enabled
        def command(args, **kwargs):
            calls.append(args)
            if args[:2] == ['docker', 'ps']:
                return ''
            if args[:2] == ['systemctl', 'disable']:
                states[args[-1]] = 'disabled'
            elif args[:2] == ['systemctl', 'enable']:
                states[args[-1]] = 'enabled'
            else:
                raise AssertionError(args)
            return ''
        with mock.patch.object(uninstall, 'check_stopped', side_effect=stopped), \
                mock.patch.object(core, 'command', side_effect=command), \
                mock.patch.object(upgrade, 'sha', side_effect=lambda p: 'f' * 64 if p.parent == Path('/etc/systemd/system') else real_sha(p)):
            self.run_session()
            self.assertTrue(all(v == 'disabled' for v in states.values()))
            self.run_session(rollback=True)
            self.assertTrue(all(v == 'enabled' for v in states.values()))
        self.assertFalse(any('--now' in c for c in calls))

    def test_noninteractive_execution_and_optional_ord_migration_are_blocked(self):
        self.stage()
        with mock.patch.object(sys.stdin, 'isatty', return_value=False):
            with self.assertRaisesRegex(ValueError, 'interactive terminal'):
                self.run_session()
        import usdb_minting
        with mock.patch.object(usdb_minting, 'activation_updates', return_value={'ORD_DATA_HOST_DIR': 'new'}):
            value = self.plan()
            self.assertFalse(value['executable'])
            self.assertIn('Ord version migration', value['blockers'][0])

    def test_rollback_recovers_interruption_between_mkdir_and_inode_journal(self):
        self.stage()
        original = session.Session.own
        def interrupt(s, path):
            if '.new-upgrade-' in path.name:
                raise OSError('mkdir interrupted')
            original(s, path)
        with mock.patch.object(session.Session, 'own', interrupt):
            with self.assertRaisesRegex(OSError, 'mkdir interrupted'):
                self.run_session()
        self.run_session(rollback=True)
        self.assertEqual(session.read(self.f.backup)['phase'], 'rolled_back')
        self.assertTrue((self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'opaque.db').exists())

    def test_packaged_cli_previews_real_config_without_host_mutation(self):
        import prepare_release_node_kit as builder
        import subprocess
        target = self.root / 'packaged-target'
        manifest = self.f.target.manifest_path
        builder.build_node_kit(repository_root=Path(__file__).resolve().parents[1], bundle_dir=self.f.target.bundle_dir,
                               manifest_path=manifest, manifest_checksum_path=manifest.with_name(manifest.name + '.sha256'),
                               output_dir=target)
        before = self.f.env_path.read_bytes()
        script = target / 'docker/scripts/tools/usdb_node.py'
        for action in ('upgrade-plan', 'upgrade-release'):
            result = subprocess.run([sys.executable, '-B', str(script), '--node-env', str(self.f.env_path),
                                     action, '--from-kit', str(self.f.source_kit), '--json'],
                                    cwd=self.root, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            value = json.loads(result.stdout)
            self.assertEqual(value['classification'], 'network_reset')
            self.assertEqual(value['target_kit'], str(target))
        for action in ('upgrade-plan', 'upgrade-release'):
            for flags in ([], ['--details']):
                result = subprocess.run([sys.executable, '-B', str(script), '--node-env', str(self.f.env_path),
                                         action, '--from-kit', str(self.f.source_kit), *flags],
                                        cwd=self.root, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('Next steps (not executed)', result.stdout)
                self.assertEqual('Runtime ID (source)' in result.stdout, bool(flags))
        self.assertEqual(before, self.f.env_path.read_bytes())
        self.assertFalse((self.f.config / upgrade.PENDING).exists())

    def preview(self, *arguments, layout=None):
        args = node.build_parser().parse_args(list(arguments))
        self.output.seek(0)
        self.output.truncate()
        code = node._execute_command(layout or self.f.target, args)
        return code, self.output.getvalue()

    def test_concise_preview_explains_rebuild_without_raw_hashes_or_duplicate_paths(self):
        value = self.plan()
        before = self.f.env_path.read_bytes()
        code, output = self.preview('upgrade-plan')
        self.assertEqual(code, 0)
        self.assertIn('Preflight: PASSED', output)
        self.assertIn('Component', output)
        self.assertIn('Bitcoin Core', output)
        self.assertIn('USDB indexer', output)
        self.assertIn('Changes: genesis configuration; BTC activation registry', output)
        self.assertNotIn(value['previous_compatibility_id'], output)
        self.assertNotIn(value['components'][0]['source'], output)
        self.assertIn('upgrade-release --backup-dir', output)
        self.assertIn('--execute', output)
        self.assertNotIn('activate-release', output)
        self.assertIn('mining must be reauthorized', output)
        self.assertEqual(before, self.f.env_path.read_bytes())
        self.assertFalse(self.f.backup.exists())

    def test_details_and_json_retain_complete_diagnostics(self):
        value = self.plan()
        for action in ('upgrade-plan', 'upgrade-release'):
            with self.subTest(action=action):
                code, output = self.preview(action, '--details')
                self.assertEqual(code, 0)
                self.assertIn(value['previous_compatibility_id'], output)
                for item in value['components']:
                    self.assertIn(item['source'], output)
                    self.assertIn(item['target'], output)
                self.assertIn('network.genesis_sha256', output)
                code, output = self.preview(action, '--json')
                self.assertEqual(json.loads(output), value)
                self.assertNotIn('Next steps', output)

    def test_blocked_plan_never_suggests_activation_or_execution(self):
        target = next(Path(i['target']) for i in self.plan()['components'] if i['service'] == 'usdb_indexer')
        target.mkdir()
        for action in ('upgrade-plan', 'upgrade-release'):
            code, output = self.preview(action)
            self.assertEqual(code, 2)
            self.assertIn('Preflight: BLOCKED', output)
            self.assertIn('Target dataset already exists', output)
            self.assertNotIn('--execute', output)
            self.assertNotIn('activate-release', output)
        with self.assertRaisesRegex(ValueError, 'Upgrade plan is blocked'):
            self.preview('upgrade-release', '--execute')
        self.assertIn('Target dataset already exists', self.output.getvalue())
        self.assertFalse(self.f.backup.exists())

    def test_compatible_preview_routes_to_activate_without_rebuild_or_controller_install(self):
        target = replace(self.f.target, network_identity=self.f.old['network_bundle'],
                         runtime_compatibility=self.f.old['runtime_compatibility'])
        code, output = self.preview('upgrade-plan', layout=target)
        self.assertEqual(code, 0)
        self.assertIn('usdb-node activate-release', output)
        self.assertNotIn('upgrade-release', output)
        self.assertNotIn('controller install', output)
        self.assertNotIn('mining must be reauthorized', output)

    def test_resume_guidance_uses_saved_operation_and_terminal_state(self):
        self.stage()
        record = session.read(self.f.backup)
        for phase in ('staged', 'rolling_back', 'applied', 'rolled_back'):
            with self.subTest(phase=phase):
                record['phase'] = phase
                core.atomic_json(self.f.backup / 'upgrade.json', record)
                code, output = self.preview('upgrade-release', '--resume', str(self.f.backup))
                self.assertEqual(code, 0)
                self.assertNotIn('--backup-dir', output)
                if phase in ('staged', 'rolling_back'):
                    self.assertIn('--resume ' + str(self.f.backup), output)
                    self.assertIn('--execute', output)
                    self.assertEqual('--rollback' in output, phase == 'rolling_back')
                elif phase == 'applied':
                    self.assertIn('Upgrade already applied', output)
                    self.assertIn('usdb-node controller install', output)
                    self.assertNotIn('--execute', output)
                    _, rollback = self.preview('upgrade-release', '--resume', str(self.f.backup), '--rollback')
                    self.assertIn('--rollback --execute', rollback)
                    self.assertNotIn('  usdb-node up\n', rollback)
                else:
                    self.assertIn('Use the original kit', output)
                    self.assertNotIn('  usdb-node up\n', output)
                    self.assertNotIn('--execute', output)

    def test_recommendations_preserve_custom_paths_and_shell_quote_arguments(self):
        # Presentation must not redirect an explicitly scoped command to another installed node.
        source = self.root / "old kit ' $(false)"
        backup = self.root / 'backup space'
        config = self.root / 'private config/node.env'
        value = self.plan()
        with mock.patch.object(upgrade, 'plan', return_value=value):
            code, output = self.preview('--kit-root', str(self.f.target_kit), '--node-env', str(config),
                                        'upgrade-release', '--from-kit', str(source), '--backup-dir', str(backup))
        self.assertEqual(code, 0)
        line = next(line for line in output.splitlines() if line.startswith('  usdb-node ') and '--backup-dir' in line)
        command = shlex.split(line)
        for flag, expected in (('--kit-root', self.f.target_kit), ('--node-env', config),
                               ('--from-kit', source), ('--backup-dir', backup)):
            self.assertEqual(command[command.index(flag) + 1], str(expected))
        self.assertFalse(backup.exists())


if __name__=='__main__':
    unittest.main()
