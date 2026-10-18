"""Operator-confirmed P2P identity lifecycle, independent of mining accounts."""
from __future__ import annotations

from contextlib import ExitStack
from dataclasses import replace
import json
import os
from pathlib import Path
import re
import subprocess
import sys

import node_identity_files as files
import node_rebuild as core


def key_path(layout, node):
    """Resolve the configured identity without adopting or moving an old key."""
    env = node.read_env(layout.node_env)
    if not env.get('USDB_CHAIN_DATA_HOST_DIR'):
        raise ValueError('P2P identity requires a configured chain data directory; run usdb-node setup first')
    root = core.absolute(env['USDB_CHAIN_DATA_HOST_DIR'])
    core.safe_path(root)
    return root / 'geth/nodekey'


def observe(layout, path):
    """Use a bounded, read-only helper for container-owned identity files."""
    try:
        return files.inspect(path)
    except PermissionError:
        # Mount the closest inspectable ancestor: geth/ may not exist yet inside
        # a container-owned directory. The helper still rejects every symlink.
        parent = path.parent
        while True:
            try:
                files.safe(parent)
                if parent.is_dir():
                    break
            except PermissionError:
                pass
            if parent == parent.parent:
                raise ValueError('Cannot inspect identity directory') from None
            parent = parent.parent
        if any(c in str(parent) for c in (',', '\n', '\r', '\0')):
            raise ValueError('Unsupported identity mount path') from None
        command = ['docker', 'run', '--rm', '--interactive', '--pull=never', '--network=none',
                   '--read-only', '--user=0:0', '--cap-drop=ALL', '--cap-add=DAC_READ_SEARCH',
                   '--security-opt=no-new-privileges', '--pids-limit=32', '--memory=64m', '--cpus=0.25',
                   '--mount', f'type=bind,src={parent},dst=/identity,readonly',
                   '--entrypoint=python3', layout.images['USDB_CHAIN_IMAGE'], '-', 'status',
                   '/identity/' + str(path.relative_to(parent))]
        result = subprocess.run(command, input=Path(files.__file__).read_text(), capture_output=True,
                                text=True, timeout=30, check=False)
        if result.returncode:
            raise ValueError('P2P identity could not be read; check file permissions. '
                             'If the release chain image is not cached, run usdb-node pull, then identity status')
        value = json.loads(result.stdout)
        if (not isinstance(value, dict) or value.get('schema_version') != files.SCHEMA or value.get('state') not in {'READY', 'MISSING', 'INVALID'}
                or (value['state'] == 'INVALID' and not re.fullmatch(r'[0-9a-f]{64}', str(value.get('fingerprint'))))
                or (value['state'] == 'READY' and (not re.fullmatch(r'[0-9a-f]{64}', str(value.get('fingerprint')))
                    or not re.fullmatch(r'[0-9a-f]{128}', str(value.get('public_key')))))):
            raise ValueError('Invalid identity inspection response')
        return {**{key: value[key] for key in ('schema_version', 'state', 'fingerprint', 'public_key') if key in value},
                'path': str(path)}


def require_ready(layout, node):
    """Fail closed on missing, invalid or unreadable managed node identity."""
    report = observe(layout, key_path(layout, node))
    if report['state'] == 'INVALID':
        raise ValueError('P2P_IDENTITY_INVALID: existing nodekey is invalid; preserve it and use identity archive/restore explicitly')
    if report['state'] != 'READY':
        raise ValueError('P2P_IDENTITY_REQUIRED: no nodekey; run usdb-node identity create --execute '
                         'or identity restore --from-key PATH --execute. Startup never creates an identity.')
    return report


def guidance(layout, node, output=None):
    """Setup only explains the separate identity step; it cannot authorize generation."""
    output = sys.stdout if output is None else output
    print('P2P identity is persistent node data, separate from the miner address.', file=output)
    print('Inspect: usdb-node identity status', file=output)
    print('If missing, explicitly create: usdb-node identity create --execute', file=output)
    print('To reuse a stopped previous network or backup: usdb-node identity restore --from-key PATH --execute', file=output)
    print('Identity file: USDB_CHAIN_DATA_HOST_DIR/geth/nodekey; no identity was generated or replaced by setup.', file=output)


def stopped(layout, node, paths):
    """Require observed quiescence of controllers and all source/target mounts."""
    state = node.controller_observed_state(layout)
    if state not in {'inactive', 'failed', 'uninstalled'}:
        raise ValueError('Stop the controller with usdb-node down before changing identity')
    for item in core.containers():
        if item['state'] in {'exited', 'created', 'dead'}:
            continue
        for mount in item['mounts']:
            source = Path(mount.get('Source', ''))
            if source.is_absolute() and any(core.overlap(source, path) for path in paths):
                raise ValueError('A running container uses the source or target identity; stop its node first')


def add_parser(subparsers):
    """Keep identity mutation separate from configuration and startup commands."""
    parser = subparsers.add_parser('identity', help='Inspect, explicitly create, restore or archive P2P node identity')
    actions = parser.add_subparsers(dest='identity_action', required=True)
    status = actions.add_parser('status', help='Show the public key and file fingerprint without starting services')
    status.add_argument('--json', action='store_true', help='Emit public identity metadata as JSON')
    for action in ('create', 'restore', 'archive'):
        command = actions.add_parser(action, help='Preview by default; --execute requires a separate identity confirmation')
        command.add_argument('--execute', action='store_true', help='Require a terminal and confirm this specific identity operation')
        if action == 'restore':
            command.add_argument('--from-key', type=Path, required=True, help='Explicit source nodekey from a stopped node or private backup')


def dispatch(args, layout, node):
    """Preview, confirm, lock and recheck before any private-file mutation."""
    path = key_path(layout, node)
    action = args.identity_action
    report = observe(layout, path)
    if action == 'status':
        print(json.dumps(report, indent=2) if args.json else render(report))
        return 0 if report['state'] == 'READY' else 1
    import node_upgrade
    import usdb_mining
    import usdb_peers
    node_upgrade.ensure_no_pending(layout)
    if usdb_mining.pending(layout) or usdb_peers.pending(layout):
        raise ValueError('Finish pending mining/peer operations before changing identity')
    source = core.absolute(args.from_key) if action == 'restore' else None
    selected = observe(layout, source) if source else report
    if action in {'create', 'restore'} and report['state'] != 'MISSING':
        raise ValueError('Identity already exists; archive it explicitly before creating or restoring another')
    if (action == 'restore' and selected['state'] != 'READY') or (action == 'archive' and selected['state'] == 'MISSING'):
        raise ValueError('The selected source identity is missing or invalid')
    expected = selected.get('fingerprint', '')
    archive = Path.home() / '.local/state/usdb/identities' / layout.bundle_id / expected / 'nodekey' if action == 'archive' else None
    print(render(selected))
    print(f'Identity action: {action}; target: {path}')
    if archive:
        print(f'Archive: {archive}; the active identity will be removed only after backup verification.')
    if action == 'create':
        print('A new P2P identity changes this node\'s enode. Existing Seed entries will not identify it.')
    if action == 'restore':
        print('Restore keeps the selected public identity. Do not run another node using the same key concurrently.')
    if not args.execute:
        print('Preview only. Repeat with --execute to confirm this identity operation.')
        return 0
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ValueError('Identity changes require an interactive terminal; no unattended confirmation is supported')
    phrase = action.upper() + ' IDENTITY ' + (layout.bundle_id if action == 'create' else expected)
    print(f"Type exactly '{phrase}' to proceed:")
    if input().strip() != phrase:
        print('Cancelled; identity unchanged.')
        return 0
    with ExitStack() as stack:
        paths = [path.parent.parent] + ([source] if source else [])
        layouts = [layout]
        # Coordinate with any configured source network whose key is being reused.
        import node_network_switch
        for other in node_network_switch.configurations(layout, node):
            old_path = Path(other['env'].get('USDB_CHAIN_DATA_HOST_DIR', '/'))
            if source and old_path in source.parents:
                old_layout = replace(layout, node_env=other['path'], bundle_id=other['bundle'])
                layouts.append(old_layout)
        # Stable ordering avoids deadlock when two operators request opposing
        # cross-network restores. All locks remain held through publication.
        for item in sorted(layouts, key=lambda value: str(value.node_env)):
            stack.enter_context(node.node_operation_lock(item, 'identity-' + action))
            stopped(item, node, paths)
        node_upgrade.ensure_no_pending(layout)
        if usdb_mining.pending(layout) or usdb_peers.pending(layout):
            raise ValueError('Finish pending mining/peer operations before changing identity')
        if key_path(layout, node) != path:
            raise ValueError('Configured identity path changed after preview; inspect it again')
        if observe(layout, path) != report or (source and observe(layout, source) != selected):
            raise ValueError('Identity changed after preview; inspect it again')
        try:
            value = files.operate(action, path, source=source, archive=archive, expected=expected)
        except PermissionError:
            # The user has already confirmed this exact operation. The privileged
            # helper rechecks file identity and refuses to overwrite any key.
            command = [sys.executable, str(Path(files.__file__).resolve()), action, str(path),
                       '--expected', expected, '--uid', str(os.getuid()), '--gid', str(os.getgid())]
            if source:
                command += ['--source', str(source)]
            if archive:
                command += ['--archive', str(archive)]
            result = node._privileged_command(command, check=False)
            if result.returncode:
                raise ValueError('Identity operation failed; inspect identity status before retrying')
            return 0
        print(json.dumps(value, indent=2))
    return 0


def render(report):
    """Render only public identity information, never key file contents."""
    lines = [f"P2P identity: {report['state']}", f"File: {report['path']}"]
    if 'public_key' in report:
        lines.append(f"Public key: {report['public_key']}")
    if 'fingerprint' in report:
        lines.append(f"Fingerprint: {report['fingerprint']}")
    return '\n'.join(lines)
