"""Forward registry/checkpoint adoption; authoritative history checks run in each service.

No database is copied, deleted or rewound. Once metadata writes start, recovery is
roll-forward only, bound to the saved database preflights and immutable releases.
"""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import subprocess

import node_rebuild as core
import node_upgrade as upgrade
from runtime_compatibility import DATASET_IDENTITY_FILE, build_dataset_identity


def artifacts(kit, bundle):
    base = Path(kit) / 'docker/networks' / bundle
    raw = core.read_json(base / 'network.json')
    return {key: base / value['path'] for key, value in raw['artifacts'].items()}


def candidate(source_kit, source, layout):
    """Static eligibility only; never assert that local executed history is compatible."""
    old, new = source['network_bundle'], layout.network_identity
    allowed = {'genesis_sha256', 'btc_activation_registry_id'}
    if any(old.get(k) != new.get(k) for k in upgrade.CHAIN_FIELDS if k not in allowed):
        return False
    registry_changed = old['btc_activation_registry_id'] != new['btc_activation_registry_id']
    if not registry_changed and old['genesis_sha256'] == new['genesis_sha256']:
        return False
    before, after = copy.deepcopy(source['runtime_compatibility']['services']), copy.deepcopy(layout.runtime_compatibility['services'])
    before['usdb_indexer']['identity']['btc_activation_registry_id'] = new['btc_activation_registry_id']
    if before != after:
        return False
    a, b = artifacts(source_kit, layout.bundle_id), artifacts(layout.kit_root, layout.bundle_id)
    if 'btc_activation_registry_catalog' not in a or 'btc_activation_registry_catalog' not in b:
        return False
    ca, cb = (core.read_json(paths['btc_activation_registry_catalog']) for paths in (a, b))
    if (ca.get('current_registry_id') != old['btc_activation_registry_id']
            or cb.get('current_registry_id') != new['btc_activation_registry_id']
            or ca.get('schema_version') != cb.get('schema_version')
            or not ca.get('registries') or len(cb.get('registries', [])) < len(ca['registries']) + int(registry_changed)
            or cb['registries'][:len(ca['registries'])] != ca['registries']):
        return False
    ga, gb = (core.read_json(paths['genesis']) for paths in (a, b))
    try:
        aa, ab = ga['config']['usdb']['activations'], gb['config']['usdb']['activations']
        if not aa or len(ab) < len(aa) + int(registry_changed) or ab[:len(aa)] != aa:
            return False
        if any(x['block'] >= y['block'] for x, y in zip(ab, ab[1:])):
            return False
        gb['config']['usdb']['activations'] = aa
        return ga == gb
    except (KeyError, TypeError):
        return False


def component(session, service):
    return next(i for i in session.plan['components'] if i['service'] == service)


def validate(session):
    """A saved journal cannot supply arbitrary mount paths or changed database proofs."""
    state = session.state.get('protocol')
    if state is None:
        return
    core.require(set(state) <= {'inode', 'proofs', 'writes_started', 'init_marker', 'done'},
                 'Unexpected protocol upgrade journal fields')
    core.require(isinstance(state.get('inode'), list) and len(state['inode']) == 2,
                 'Missing original indexer directory identity')
    core.require(set(state.get('proofs', {})) <= {'usdb_indexer', 'usdb_chain'}, 'Unexpected service proof')
    for service, digest in state.get('proofs', {}).items():
        core.require(upgrade.sha(session.root / 'protocol' / (service + '.json')) == digest,
                     f'Saved {service} preflight changed')
    if state.get('writes_started'):
        core.require(set(state.get('proofs', {})) == {'usdb_indexer', 'usdb_chain'},
                     'Both saved service preflights are required before metadata writes')
    item = component(session, 'usdb_indexer')
    present = sorted({Path(item[k]) for k in ('source', 'target') if Path(item[k]).exists()})
    core.require(len(present) == 1 and core.stamp(present[0])[:2] == state['inode'],
                 'Indexer dataset moved or duplicated outside this upgrade')
    actual = core.read_json(present[0] / DATASET_IDENTITY_FILE)
    target = build_dataset_identity('usdb_indexer', session.layout.runtime_compatibility)
    core.require(upgrade.sha(present[0] / DATASET_IDENTITY_FILE) == item['marker_sha256']
                 or (state.get('writes_started') and actual == target), 'Indexer dataset marker changed')
    marker = Path(component(session, 'usdb_chain')['source']) / 'bootstrap/ethw-init.done.json'
    original = state['init_marker']
    updated = dict(original, genesis_sha256=session.layout.network_identity['genesis_sha256'])
    core.require(core.read_json(marker) == original or (state.get('writes_started') and core.read_json(marker) == updated),
                 'Geth initialization marker changed outside this upgrade')


def run_command(arguments):
    """Long offline scans/pulls keep reporting progress without a short inspection timeout."""
    import tempfile
    with tempfile.TemporaryFile(mode='w+t') as output, tempfile.TemporaryFile(mode='w+t') as errors:
        process = subprocess.Popen(arguments, stdout=output, stderr=errors, text=True)
        try:
            while process.poll() is None:
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    print('Protocol upgrade: service preflight/apply is still running...', flush=True)
        except BaseException:
            process.terminate()
            process.wait()
            raise
        output.seek(0)
        errors.seek(0)
        core.require(process.returncode == 0,
                     f'Protocol upgrade command failed ({arguments[0]} {arguments[1]}): {errors.read()[-8000:]}; preserve the session and resume after diagnosis')
        return output.read()


def offline(session, service, *, apply):
    """Only the selected dataset is writable, and neither service can access the network."""
    item = component(session, service)
    data = Path(item['target']) if Path(item['target']).exists() else Path(item['source'])
    paths = artifacts(session.layout.kit_root, session.layout.bundle_id)
    old = artifacts(session.plan['source_kit'], session.layout.bundle_id)
    image_key, binary = ('USDB_SERVICES_IMAGE', 'usdb-indexer') if service == 'usdb_indexer' else ('USDB_CHAIN_IMAGE', 'geth')
    mounts = [(data, '/dataset', not apply), (session.root / 'protocol', '/upgrade', True),
              (paths['genesis'].parent, '/target', True), (old['genesis'].parent, '/source', True)]
    command = ['docker', 'run', '--rm', '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--cap-add', 'DAC_OVERRIDE',
               '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp', '--entrypoint', binary]
    for path, destination, readonly in mounts:
        core.safe_path(path)
        core.require(',' not in str(path), 'Docker mount paths cannot contain commas')
        command += ['--mount', f'type=bind,src={path},dst={destination}' + (',readonly' if readonly else '')]
    command.append(session.layout.images[image_key])
    if service == 'usdb_indexer':
        command += ['registry-upgrade', '--data-dir', '/dataset/data', '--catalog', '/upgrade/catalog.json',
                    '--target-binding', '/upgrade/binding.json']
    else:
        command += ['usdb-upgrade-config', '--datadir', '/dataset', '--source-genesis', '/source/' + old['genesis'].name,
                    '--target-genesis', '/target/' + paths['genesis'].name]
    if apply:
        command += ['--apply', '--expected-report', '/upgrade/' + service + '.json']
    report = json.loads(run_command(command))
    expected_schema = 'usdb-indexer-registry-preflight:v1' if service == 'usdb_indexer' else 'usdb-chain-config-preflight:v1'
    core.require(report.get('schema_version') == expected_schema, f'Unexpected {service} preflight response')
    if service == 'usdb_indexer':
        source = upgrade._source_manifest(Path(session.plan['source_kit']), session.node)['network_bundle']
        binding = core.read_json(session.root / 'protocol/binding.json')
        expected_source = dict(binding, activation_registry_id=source['btc_activation_registry_id'])
        core.require(report.get('adoption', {}).get('source') == expected_source
                     and report.get('adoption', {}).get('target') == binding,
                     'Indexer database bindings differ from the source/target release identities')
    else:
        core.require(report.get('genesis') == session.layout.network_identity['genesis_block_hash'],
                     'Geth preflight genesis differs from the frozen release')
    return report


def execute(session):
    """Both checks precede the first write; every later action is safe to repeat."""
    item = component(session, 'usdb_indexer')
    source, target = Path(item['source']), Path(item['target'])
    chain = Path(component(session, 'usdb_chain')['source'])
    marker = chain / 'bootstrap/ethw-init.done.json'
    if session.state.get('protocol') is None:
        core.require(source == target or not target.exists(), 'Target indexer dataset already exists')
        core.require(upgrade.sha(source / DATASET_IDENTITY_FILE) == item['marker_sha256'], 'Source indexer marker changed')
        cfg = core.read_json(source / 'config.json')
        core.require(not cfg.get('isolate'), 'Isolated indexer layouts need a separate reviewed upgrade')
        original_marker = core.read_json(marker)
        source_manifest = upgrade._source_manifest(Path(session.plan['source_kit']), session.node)
        core.require(original_marker.get('genesis_sha256') == source_manifest['network_bundle']['genesis_sha256']
                     and original_marker.get('genesis_file') == '/network/usdb-genesis.json'
                     and original_marker.get('genesis_manifest_file') == '/network/usdb-genesis.manifest.json',
                     'Source Geth initialization marker does not match the source release')
        core.check_mounts(source)
        target.parent.mkdir(parents=True, exist_ok=True)
        core.require(source.stat().st_dev == target.parent.stat().st_dev, 'Registry adoption requires a same-filesystem rename')
        session.state['protocol'] = dict(inode=core.stamp(source)[:2], proofs={}, writes_started=False,
                                        init_marker=original_marker)
        session.save()
    validate(session)
    state = session.state['protocol']
    work = session.root / 'protocol'
    work.mkdir(mode=0o700, exist_ok=True)
    session.own(work)
    paths = artifacts(session.layout.kit_root, session.layout.bundle_id)
    # These inputs are always reconstructed from the verified immutable target kit.
    core.atomic_json(work / 'catalog.json', core.read_json(paths['btc_activation_registry_catalog']))
    n = session.layout.network_identity
    core.atomic_json(work / 'binding.json', dict(schema_version='usdb-indexer-rules-binding:v1',
                     btc_network_id=n['btc_network_id'], rules_scope=n.get('btc_rules_scope', 'legacy'),
                     index_origin_height=n['btc_index_origin_height'], activation_registry_id=n['btc_activation_registry_id']))
    for name in ('catalog.json', 'binding.json'):
        session.own(work / name)
    for key in ('USDB_SERVICES_IMAGE', 'USDB_CHAIN_IMAGE'):
        reference = session.layout.images[key]
        try:
            run_command(['docker', 'image', 'inspect', '--format', '{{.Id}}', reference])
        except ValueError:
            run_command(['docker', 'pull', reference])
    if not state['writes_started']:
        for service in ('usdb_chain', 'usdb_indexer'):
            report = offline(session, service, apply=False)
            path = work / (service + '.json')
            if service in state['proofs']:
                core.require(core.read_json(path) == report, f'{service} boundary changed since preflight')
            else:
                core.atomic_json(path, report)
                session.own(path)
                state['proofs'][service] = upgrade.sha(path)
                session.save()
            session.event('protocol_checked', service)
        state['writes_started'] = True
        session.save()
    for service in ('usdb_chain', 'usdb_indexer'):
        report = offline(session, service, apply=True)
        core.require(report == core.read_json(work / (service + '.json')), f'{service} apply differs from saved preflight')
        session.event('protocol_applied', service)
    # A crash between rename and journal save is resolved from the original inode.
    if source != target and source.exists():
        core.require(not target.exists(), 'Target dataset appeared during upgrade')
        os.rename(source, target)
        core.sync_dir(source.parent)
        core.sync_dir(target.parent)
    validate(session)
    if item['action'] == 'adopt':
        core.atomic_json(target / DATASET_IDENTITY_FILE, build_dataset_identity('usdb_indexer', session.layout.runtime_compatibility))
        session.own(target / DATASET_IDENTITY_FILE)
    session.event('protocol_dataset_ready', target)
    core.atomic_json(marker, dict(state['init_marker'], genesis_sha256=n['genesis_sha256']))
    session.own(marker)
    session.event('protocol_chain_marker_ready', marker)
    state['done'] = True
    session.save()
