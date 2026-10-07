"""Stopped-node coordination with real release validation and injected service boundaries."""
import copy
from contextlib import ExitStack, redirect_stdout
import io
from pathlib import Path
import shutil
import socket
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'docker/scripts/tools'))
import node_rebuild as core
import node_protocol_upgrade as protocol
import node_upgrade as upgrade
import node_upgrade_session as session
import node_uninstall as uninstall
import runtime_compatibility as runtime
import usdb_node as node
import validate_network_bundle as validation
from common.node_upgrade import UpgradeFixture, write_manifest


class ProtocolFixture(unittest.TestCase):
    chain_only = False

    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='usdb-protocol-upgrade-')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.f = UpgradeFixture(self.root)
        f = self.f
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(redirect_stdout(io.StringIO()))
        self.stack.enter_context(mock.patch.object(core, 'command', side_effect=f.host_command))
        self.stack.enter_context(mock.patch.object(uninstall, 'operator_home', return_value=f.home))
        self.stack.enter_context(mock.patch.object(sys.stdin, 'isatty', return_value=True))
        self.stack.enter_context(mock.patch.object(sys.stdout, 'isatty', return_value=True))
        self.stack.enter_context(mock.patch.object(node, 'effective_memory_bytes', return_value=64 * 1024**3))
        bundle = f.target.bundle_dir
        source_bundle = f.source_kit / 'docker/networks' / f.bundle
        shutil.rmtree(source_bundle)
        shutil.copytree(bundle, source_bundle)
        old_paths = f.paths
        f.old = copy.deepcopy(f.manifest)
        f.old['release_id'] = f.bundle + '-r4'
        f.old['images']['usdb_services']['reference'] = 'ghcr.io/buckyos/usdb-services@sha256:' + '9' * 64
        f.paths = runtime.build_persistent_data_paths(f.data, f.old['network_bundle'], f.old['runtime_compatibility'])
        old_paths['USDB_INDEXER_DATA_HOST_DIR'].rename(f.paths['USDB_INDEXER_DATA_HOST_DIR'])
        for key, service in runtime.PERSISTENT_DATA_SERVICES.items():
            core.atomic_json(f.paths[key] / runtime.DATASET_IDENTITY_FILE, runtime.build_dataset_identity(service, f.old['runtime_compatibility']))
        node._atomic_write_private(f.env_path, node.upsert_env(f.env_path.read_text(), {
            **{key: str(path) for key, path in f.paths.items()},
            'USDB_RUNTIME_COMPATIBILITY_ID': f.old['runtime_compatibility']['compatibility_id'],
            'USDB_SERVICES_IMAGE': f.old['images']['usdb_services']['reference']}))
        write_manifest(f.source_kit, f.old)
        core.atomic_json(f.paths['USDB_INDEXER_DATA_HOST_DIR'] / 'config.json', {'isolate': None})
        self.marker = f.paths['USDB_CHAIN_DATA_HOST_DIR'] / 'bootstrap/ethw-init.done.json'
        self.marker.parent.mkdir(exist_ok=True)
        core.atomic_json(self.marker, dict(genesis_file='/network/usdb-genesis.json',
            genesis_manifest_file='/network/usdb-genesis.manifest.json',
            genesis_sha256=f.old['network_bundle']['genesis_sha256'], initialized_at='fixture'))
        raw = core.read_json(bundle / 'network.json')
        paths = {k: bundle / v['path'] for k, v in raw['artifacts'].items()}
        old_id = raw['btc_source']['activation_registry_id']
        new_id = old_id if self.chain_only else 'b' * 64
        catalog = core.read_json(paths['btc_activation_registry_catalog'])
        next_registry = copy.deepcopy(catalog['registries'][-1])
        record = copy.deepcopy(next_registry['records'][0])
        record.update(activation_height=9999999, supersedes=record['version_value'], version_value='test-future-schema:v2')
        next_registry['records'].append(record)
        if not self.chain_only:
            catalog['registries'].append(next_registry)
        catalog['current_registry_id'] = new_id
        core.atomic_json(paths['btc_activation_registry_catalog'], catalog)
        for key, field in [('genesis', ('config', 'usdb')), ('chain_bootstrap', ('usdbConsensus',))]:
            doc = core.read_json(paths[key])
            config = doc
            for name in field:
                config = config[name]
            config['activations'].append(dict(config['activations'][0], block=2000, btcActivationRegistryId=new_id,
                btcAnchorMaxAgeBlocks=config['activations'][0]['btcAnchorMaxAgeBlocks'] + 1))
            core.atomic_json(paths[key], doc)
        genesis_manifest = core.read_json(paths['genesis_manifest'])
        genesis_manifest.update(file_sha256=upgrade.sha(paths['genesis']), bootstrap_config_sha256=upgrade.sha(paths['chain_bootstrap']))
        core.atomic_json(paths['genesis_manifest'], genesis_manifest)
        paths['network_environment'].write_text(paths['network_environment'].read_text().replace(old_id, new_id))
        raw['btc_source']['activation_registry_id'] = new_id
        for key, value in raw['artifacts'].items():
            value['sha256'] = upgrade.sha(paths[key])
        core.atomic_json(bundle / 'network.json', raw)
        profile = dict(validation.NETWORK_PROFILES[f.bundle], registry=new_id, genesis_registry=old_id,
                       catalog_sha256=upgrade.sha(paths['btc_activation_registry_catalog']))
        self.stack.enter_context(mock.patch.dict(validation.NETWORK_PROFILES, {f.bundle: profile}))
        self.stack.enter_context(mock.patch.dict(validation.BTC_REGISTRY_STABLE_LAG_BLOCKS, {new_id: 10}))
        template = bundle / 'node.env.example'
        env = node.read_env(template)
        identity = dict(f.old['network_bundle'], btc_activation_registry_id=new_id)
        contract = runtime.build_runtime_compatibility(identity)
        node._atomic_write_private(template, node.upsert_env(template.read_text(), {
            'USDB_RUNTIME_COMPATIBILITY_ID': contract['compatibility_id'],
            **{key: str(path) for key, path in runtime.build_persistent_data_paths(Path(env['USDB_DATA_ROOT']), identity, contract).items()}}))
        f.network = node.build_network_identity(bundle)
        f.manifest['network_bundle'] = f.network
        f.manifest['runtime_compatibility'] = runtime.build_runtime_compatibility(f.network)
        write_manifest(f.target_kit, f.manifest)
        f.target = node.load_release_layout(f.target_kit, f.env_path)
        self.calls = []
        self.stack.enter_context(mock.patch.object(protocol, 'run_command', return_value=''))
        self.stack.enter_context(mock.patch.object(protocol, 'offline', side_effect=self.offline))

    def offline(self, runner, service, *, apply):
        self.calls.append((service, apply))
        return dict(schema_version='test-' + service, height=1999)

    def plan(self):
        return upgrade.plan(self.f.target, node, self.f.source_kit)

    def stage(self):
        session.stage(self.plan(), self.f.backup, node)

    def run_session(self, rollback=False):
        return session.Session(self.f.backup, node).run(rollback=rollback,
            confirm=lambda: f"{'ROLLBACK' if rollback else 'UPGRADE'} {self.f.bundle} {socket.gethostname()}")

