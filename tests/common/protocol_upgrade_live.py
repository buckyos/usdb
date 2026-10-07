#!/usr/bin/env python3
"""Regtest-only adapter: real coordinator, offline containers and durable process exits.

The live harness owns/stops the services. This adapter supplies checksummed synthetic
kits and a durable session, without relaxing production network validation or
pretending to test host systemd/sudo installation. No service result is mocked.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'docker/scripts/tools'))
import node_protocol_upgrade as protocol
import node_rebuild as core
import node_upgrade as upgrade
import runtime_compatibility as runtime
import usdb_node as node

BUNDLE = 'usdb-testnet-v999'
SCOPE = 'miner-pass-upgrade-conformance'


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def command(args):
    return subprocess.check_output([str(a) for a in args], text=True).strip()


def build_image(tools):
    """Package already built test executors and their exact host libraries, no downloads."""
    binaries = {'usdb-indexer': tools / 'indexer-conformance', 'geth': tools / 'geth-conformance'}
    with tempfile.TemporaryDirectory(prefix='usdb-offline-image-') as temp:
        root = Path(temp)
        fs = root / 'rootfs'
        for dest, source in [(f'/usr/local/bin/{k}', v) for k, v in binaries.items()] + [('/usr/bin/chown', Path('/usr/bin/chown'))]:
            target = fs / dest.lstrip('/')
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o755)
            for name in re.findall(r'(/[^\s()]+)', command(['ldd', source])):
                library = Path(name)
                output = fs / library.relative_to('/')
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(library, output)
                output.chmod(0o755)
        (fs / 'tmp').mkdir(mode=0o1777)
        (root / 'Dockerfile').write_text('FROM scratch\nCOPY rootfs /\nLABEL usdb.test-only="protocol-upgrade"\n')
        subprocess.run(['docker', 'build', '--network=none', '--iidfile', str(root / 'image.id'), str(root)], check=True)
        image = (root / 'image.id').read_text().strip()
    core.atomic_json(tools / 'offline-image.json', dict(image=image, binaries={k: sha(v) for k, v in binaries.items()}))


def image_identity(tools):
    proof = core.read_json(tools / 'offline-image.json')
    assert proof['binaries'] == {'usdb-indexer': sha(tools / 'indexer-conformance'), 'geth': sha(tools / 'geth-conformance')}
    assert command(['docker', 'image', 'inspect', '--format', '{{.Id}}', proof['image']]) == proof['image']
    return proof['image']


def kit(root, genesis, catalog, block_hash, release):
    """Minimal immutable artifacts accepted by the real source-manifest verifier."""
    catalog = copy.deepcopy(catalog)
    assert all(r['scope']['rules_scope'] == SCOPE and r['scope']['network_id'] == 'btc-regtest' for r in catalog['registries'])
    base = root / 'docker/networks' / BUNDLE
    base.mkdir(parents=True)
    documents = dict(genesis=('usdb-genesis.json', genesis),
                     genesis_manifest=('usdb-genesis.manifest.json', {'block_hash': block_hash}),
                     btc_activation_registry_catalog=('catalog.json', catalog),
                     snapshot_trusted_keys=('trusted-keys.json', {}))
    artifacts = {}
    for name, (filename, value) in documents.items():
        core.atomic_json(base / filename, value)
        artifacts[name] = dict(path=filename, sha256=sha(base / filename))
    raw = dict(network_bundle_id=BUNDLE, status='development-resettable', chain_id=genesis['config']['chainId'],
               network_id=genesis['config']['chainId'], artifacts=artifacts,
               btc_source=dict(network_id='btc-regtest', index_origin_height=1, rules_scope=SCOPE,
                               activation_registry_id=catalog['current_registry_id']))
    core.atomic_json(base / 'network.json', raw)
    identity = dict(bundle_id=BUNDLE, bundle_status=raw['status'], chain_id=raw['chain_id'], network_id=raw['network_id'],
                    genesis_block_hash=block_hash, genesis_sha256=artifacts['genesis']['sha256'],
                    network_json_sha256=sha(base / 'network.json'), btc_network_id='btc-regtest', btc_rules_scope=SCOPE,
                    btc_index_origin_height=1, btc_activation_registry_id=catalog['current_registry_id'],
                    snapshot_trusted_keys_sha256=artifacts['snapshot_trusted_keys']['sha256'])
    manifest = dict(schema_version=node.RELEASE_MANIFEST_SCHEMA_VERSION, release_id=f'{BUNDLE}-r{release}',
                    network_bundle=identity, runtime_compatibility=runtime.build_runtime_compatibility(identity))
    path = root / 'release/usdb-release-manifest.json'
    path.parent.mkdir()
    core.atomic_json(path, manifest)
    path.with_name(path.name + '.sha256').write_text(f'{sha(path)}  {path.name}\n')
    assert upgrade._source_manifest(root, node) == manifest
    return manifest


def stage(root, indexer, chain, genesis_path, source_catalog, target_catalog, block_hash, checkpoint, tools):
    root.mkdir(mode=0o700)
    source = core.read_json(source_catalog)
    target = core.read_json(target_catalog)
    genesis = core.read_json(genesis_path)
    assert genesis['config']['usdb']['btcNetworkId'] == 'btc-regtest'
    assert genesis['config']['usdb']['activations'][0]['btcActivationRegistryId'] == source['current_registry_id']
    previous = kit(root / 'source-kit', genesis, source, block_hash, 1)
    updated = copy.deepcopy(genesis)
    activations = updated['config']['usdb']['activations']
    assert len(activations) == 1 and checkpoint > 0
    activations.append(dict(activations[0], block=checkpoint, btcActivationRegistryId=target['current_registry_id']))
    next_manifest = kit(root / 'target-kit', updated, target, block_hash, 2)
    core.atomic_json(indexer / runtime.DATASET_IDENTITY_FILE, runtime.build_dataset_identity('usdb_indexer', previous['runtime_compatibility']))
    marker = chain / 'bootstrap/ethw-init.done.json'
    marker.parent.mkdir(exist_ok=True)
    core.atomic_json(marker, dict(genesis_sha256=previous['network_bundle']['genesis_sha256'],
        genesis_file='/network/usdb-genesis.json', genesis_manifest_file='/network/usdb-genesis.manifest.json'))
    destination = indexer.with_name(indexer.name + '-adopted')
    plan = dict(source_kit=str(root / 'source-kit'), components=[
        dict(service='usdb_indexer', source=str(indexer), target=str(destination), action='adopt',
             marker_sha256=sha(indexer / runtime.DATASET_IDENTITY_FILE)),
        dict(service='usdb_chain', source=str(chain), target=str(chain), action='reuse')])
    core.atomic_json(root / 'fixture.json', dict(plan=plan, manifest=next_manifest, image=image_identity(tools),
                     inode=core.stamp(indexer)[:2], checkpoint=checkpoint))
    core.atomic_json(root / 'state.json', {})
    Session(root)


class Session:
    """Persist every production coordinator save; fault injection kills this actual process."""
    def __init__(self, root, crash=''):
        self.root = root
        fixture = core.read_json(root / 'fixture.json')
        self.plan = fixture['plan']
        self.node = node
        manifest = upgrade._source_manifest(root / 'target-kit', node)
        assert manifest == fixture['manifest']
        self.layout = SimpleNamespace(kit_root=root / 'target-kit', bundle_id=BUNDLE,
            network_identity=manifest['network_bundle'], runtime_compatibility=manifest['runtime_compatibility'],
            images=dict(USDB_SERVICES_IMAGE=fixture['image'], USDB_CHAIN_IMAGE=fixture['image']))
        assert protocol.candidate(self.plan['source_kit'], upgrade._source_manifest(Path(self.plan['source_kit']), node), self.layout)
        self.state = core.read_json(root / 'state.json')
        self.crash = crash

    def save(self):
        core.atomic_json(self.root / 'state.json', self.state)

    def own(self, path):
        pass  # Fixture files belong to the harness; privileged host ownership is tested separately.

    def event(self, kind, detail):
        entry = dict(event=kind, detail=str(detail))
        with (self.root / 'events.jsonl').open('a') as stream:
            stream.write(json.dumps(entry) + '\n')
            stream.flush()
            os.fsync(stream.fileno())
        print(json.dumps(entry), flush=True)
        if self.crash == kind + (':' + str(detail) if kind in ('protocol_applied', 'protocol_checked') else ''):
            os._exit(86)


def restore_fixture_ownership(root):
    """Containers run as production root; return only isolated test volumes to the harness UID."""
    session = Session(root)
    image = session.layout.images['USDB_SERVICES_IMAGE']
    for item in session.plan['components']:
        data = Path(item['target']) if Path(item['target']).exists() else Path(item['source'])
        assert root.parent.resolve() in data.resolve().parents
        command(['docker', 'run', '--rm', '--network', 'none', '--read-only', '--entrypoint', '/usr/bin/chown',
                 '--mount', f'type=bind,src={data},dst=/dataset', image, '-R', f'{os.getuid()}:{os.getgid()}', '/dataset'])


def exercise(root):
    # Every resume runs in a fresh process and calls both real offline executors.
    points = ['protocol_applied:usdb_chain', 'protocol_applied:usdb_indexer',
              'protocol_dataset_ready', 'protocol_chain_marker_ready']
    for point in points + ['', '']:
        result = subprocess.run([sys.executable, __file__, 'resume', str(root), '--crash', point])
        assert result.returncode == (86 if point else 0), (point, result.returncode)
    session = Session(root)
    assert session.state['protocol']['done']
    fixture = core.read_json(root / 'fixture.json')
    item = protocol.component(session, 'usdb_indexer')
    assert not Path(item['source']).exists()
    assert core.stamp(Path(item['target']))[:2] == fixture['inode']
    assert core.read_json(root / 'protocol/usdb_indexer.json')['adoption']['height'] == 159
    assert core.read_json(root / 'protocol/usdb_chain.json')['checked_height'] == fixture['checkpoint'] - 1
    restore_fixture_ownership(root)
    core.atomic_json(root / 'acceptance.json', dict(status='passed', crash_points=points, idempotent_resume=True,
        source_inode=fixture['inode'], target_inode=core.stamp(Path(item['target']))[:2], image=fixture['image']))


def refusal(root, expected):
    """Both real readonly preflights must finish before any database mutation."""
    session = Session(root)
    paths = [Path(i['source']) for i in session.plan['components']]
    def fingerprint():
        return {str(p): sha(p) for data in paths for p in data.rglob('*') if p.is_file()}
    before = fingerprint()
    try:
        protocol.execute(session)
    except ValueError as error:
        assert expected in str(error), str(error)
        core.atomic_json(root / 'rejection.json', dict(error=str(error), files=before))
    else:
        raise AssertionError('Obsolete history was accepted')
    assert not session.state['protocol']['writes_started']
    assert fingerprint() == before


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('build-image').add_argument('tools', type=Path)
    sub.add_parser('check-image').add_argument('tools', type=Path)
    setup = sub.add_parser('stage')
    for name in ['root', 'indexer', 'chain', 'genesis', 'source-catalog', 'target-catalog', 'tools']:
        setup.add_argument('--' + name, type=Path, required=True)
    setup.add_argument('--genesis-hash', required=True)
    setup.add_argument('--checkpoint', type=int, required=True)
    sub.add_parser('exercise').add_argument('root', type=Path)
    resume = sub.add_parser('resume')
    resume.add_argument('root', type=Path)
    resume.add_argument('--crash', default='')
    rejected = sub.add_parser('refusal')
    rejected.add_argument('root', type=Path)
    rejected.add_argument('expected')
    args = parser.parse_args()
    if args.command == 'build-image':
        build_image(args.tools)
    elif args.command == 'check-image':
        print(image_identity(args.tools))
    elif args.command == 'stage':
        stage(args.root, args.indexer, args.chain, args.genesis, args.source_catalog, args.target_catalog,
              args.genesis_hash, args.checkpoint, args.tools)
    elif args.command == 'resume':
        protocol.execute(Session(args.root, args.crash))
    elif args.command == 'exercise':
        exercise(args.root)
    else:
        refusal(args.root, args.expected)


if __name__ == '__main__':
    main()
