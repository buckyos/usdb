"""Stopped-node coordination with real release validation and injected service boundaries."""
import json
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'docker/scripts/tools'))
import node_rebuild as core
import node_protocol_upgrade as protocol
import node_upgrade as upgrade
import node_upgrade_session as session
import node_upgrade_cleanup as cleanup
import node_upgrade_archives as archives
import runtime_compatibility as runtime
import usdb_node as node
from common.node_protocol_upgrade import ProtocolFixture


class ProtocolUpgradeTests(ProtocolFixture):
    def test_read_only_plan_and_success_keep_history_and_wallets(self):
        before = {str(p): p.stat().st_mtime_ns for p in self.root.rglob('*')}
        plan = self.plan()
        self.assertEqual(before, {str(p): p.stat().st_mtime_ns for p in self.root.rglob('*')})
        self.assertEqual(plan['classification'], 'protocol_upgrade')
        self.assertTrue(plan['executable'])
        self.assertEqual([i['action'] for i in plan['components']], ['reuse', 'reuse', 'adopt', 'reuse', 'reuse'])
        old_index = self.f.paths['USDB_INDEXER_DATA_HOST_DIR']
        inode = core.stamp(old_index)[:2]
        self.stage()
        self.run_session()
        self.assertEqual(self.calls, [('usdb_chain', False), ('usdb_indexer', False), ('usdb_chain', True), ('usdb_indexer', True)])
        new_index = Path(node.read_env(self.f.env_path)['USDB_INDEXER_DATA_HOST_DIR'])
        self.assertFalse(old_index.exists())
        self.assertEqual(core.stamp(new_index)[:2], inode)
        self.assertEqual((new_index / 'opaque.db').read_text(), 'usdb_indexer-old-state')
        self.assertEqual((self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'opaque.db').read_text(), 'usdb_chain-old-state')
        self.assertEqual((self.f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'geth/nodekey').read_text(), 'private-nodekey-fixture')
        self.assertEqual(core.read_json(self.marker)['genesis_sha256'], self.f.network['genesis_sha256'])
        self.assertEqual(session.read(self.f.backup)['phase'], 'applied')
        self.assertFalse((self.f.config / upgrade.PENDING).exists())
        self.assertEqual(archives.candidates(archives.load(self.f.backup, node)), [])
        with self.assertRaisesRegex(ValueError, 'no obsolete dataset archive'):
            cleanup.Cleanup(self.f.backup, node)

    def test_second_preflight_failure_writes_neither_database_and_can_cancel(self):
        self.stage()
        def fail(runner, service, *, apply):
            self.assertFalse(apply)
            if service == 'usdb_indexer':
                raise ValueError('executed past incompatible BTC activation')
            return self.offline(runner, service, apply=apply)
        with mock.patch.object(protocol, 'offline', side_effect=fail):
            with self.assertRaisesRegex(ValueError, 'incompatible BTC'):
                self.run_session()
        self.assertFalse(session.read(self.f.backup)['protocol']['writes_started'])
        self.run_session(rollback=True)
        self.assertEqual(session.read(self.f.backup)['phase'], 'rolled_back')
        self.assertEqual(core.read_json(self.marker)['genesis_sha256'], self.f.old['network_bundle']['genesis_sha256'])

    def resume_after(self, boundary, service=None):
        self.stage()
        event = session.Session.event
        def interrupted(runner, phase, path=''):
            event(runner, phase, path)
            if phase == boundary and (service is None or path == service):
                raise OSError('injected interruption')
        with mock.patch.object(session.Session, 'event', interrupted):
            with self.assertRaisesRegex(OSError, 'injected'):
                self.run_session()
        with self.assertRaisesRegex(ValueError, 'resume forward'):
            self.run_session(rollback=True)
        args = node.build_parser().parse_args(['upgrade-release', '--resume', str(self.f.backup), '--rollback'])
        with self.assertRaisesRegex(ValueError, 'rollback is unavailable'):
            upgrade.dispatch(args, self.f.target, node)
        self.run_session()
        self.assertEqual(session.read(self.f.backup)['phase'], 'applied')

    def test_resume_after_chain_write(self):
        self.resume_after('protocol_applied', 'usdb_chain')

    def test_resume_after_indexer_write(self):
        self.resume_after('protocol_applied', 'usdb_indexer')

    def test_resume_after_dataset_move(self):
        self.resume_after('protocol_dataset_ready')

    def test_resume_after_chain_marker(self):
        self.resume_after('protocol_chain_marker_ready')

    def test_resume_after_configuration(self):
        self.resume_after('configuration_ready')

    def test_saved_proof_change_blocks_resume(self):
        self.stage()
        event = session.Session.event
        def interrupted(runner, phase, path=''):
            event(runner, phase, path)
            if phase == 'protocol_applied':
                raise OSError('stop')
        with mock.patch.object(session.Session, 'event', interrupted):
            with self.assertRaises(OSError):
                self.run_session()
        proof = self.f.backup / 'protocol/usdb_chain.json'
        proof.write_text('{}')
        with self.assertRaisesRegex(ValueError, 'preflight changed'):
            self.run_session()

    def test_offline_container_has_no_network_and_only_selected_data_is_writable(self):
        self.stage()
        runner = session.Session(self.f.backup, node)
        work = self.f.backup / 'protocol'
        work.mkdir()
        target_binding = dict(schema_version='usdb-indexer-rules-binding:v1', btc_network_id='btc-mainnet',
            rules_scope='usdb-testnet-v1', index_origin_height=self.f.network['btc_index_origin_height'],
            activation_registry_id=self.f.network['btc_activation_registry_id'])
        core.atomic_json(work / 'binding.json', target_binding)
        source_binding = dict(target_binding, activation_registry_id=self.f.old['network_bundle']['btc_activation_registry_id'])
        for service in ('usdb_indexer', 'usdb_chain'):
            for apply in (False, True):
                schema = 'usdb-indexer-registry-preflight:v1' if service == 'usdb_indexer' else 'usdb-chain-config-preflight:v1'
                # Call the real implementation despite the injected service fixture above.
                with mock.patch.object(protocol, 'run_command', return_value=json.dumps(dict(schema_version=schema, genesis=self.f.network['genesis_block_hash'], adoption=dict(source=source_binding, target=target_binding)))) as command:
                    real_offline(runner, service, apply=apply)
                arguments = command.call_args.args[0]
                self.assertIn('none', arguments)
                mounts = [arguments[i + 1] for i, arg in enumerate(arguments) if arg == '--mount']
                self.assertEqual(sum(not mount.endswith(',readonly') for mount in mounts), int(apply))

    def test_removed_catalog_history_is_not_adoption(self):
        paths = protocol.artifacts(self.f.target_kit, self.f.bundle)
        doc = core.read_json(paths['btc_activation_registry_catalog'])
        doc['registries'].pop(0)
        core.atomic_json(paths['btc_activation_registry_catalog'], doc)
        self.assertFalse(protocol.candidate(self.f.source_kit, self.f.old, self.f.target))

    def test_rewritten_genesis_checkpoint_is_not_adoption(self):
        paths = protocol.artifacts(self.f.target_kit, self.f.bundle)
        doc = core.read_json(paths['genesis'])
        doc['config']['usdb']['activations'][0]['btcAnchorMaxAgeBlocks'] += 1
        core.atomic_json(paths['genesis'], doc)
        self.assertFalse(protocol.candidate(self.f.source_kit, self.f.old, self.f.target))

    def test_external_indexer_directory_change_blocks_resume(self):
        self.stage()
        event = session.Session.event
        def stop(runner, phase, path=''):
            event(runner, phase, path)
            if phase == 'protocol_checked':
                raise OSError('stop')
        with mock.patch.object(session.Session, 'event', stop):
            with self.assertRaises(OSError):
                self.run_session()
        source = self.f.paths['USDB_INDEXER_DATA_HOST_DIR']
        source.rename(source.with_name(source.name + '-foreign'))
        source.mkdir()
        with self.assertRaisesRegex(ValueError, 'moved or duplicated'):
            self.run_session()


class CheckpointOnlyTests(ProtocolFixture):
    chain_only = True

    def test_future_chain_checkpoint_keeps_indexer_path_and_binding(self):
        plan = self.plan()
        self.assertEqual(plan['classification'], 'protocol_upgrade')
        self.assertEqual({item['action'] for item in plan['components']}, {'reuse'})
        marker = self.f.paths['USDB_INDEXER_DATA_HOST_DIR'] / runtime.DATASET_IDENTITY_FILE
        before = marker.read_bytes()
        self.stage()
        self.run_session()
        self.assertEqual(marker.read_bytes(), before)
        self.assertEqual(node.read_env(self.f.env_path)['USDB_INDEXER_DATA_HOST_DIR'], str(marker.parent))
        self.assertEqual(core.read_json(self.marker)['genesis_sha256'], self.f.network['genesis_sha256'])


real_offline = protocol.offline


if __name__ == "__main__":
    unittest.main()
