"""Explicit P2P key operations. Never print private key material or overwrite a key.

This standalone helper also runs through sudo for container-owned directories.
OpenSSL derives public keys; secrets supplies uniformly sampled private scalars.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
import tempfile

ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
SCHEMA = "usdb-node-identity:v1"


def safe(path):
    """Reject links, including dangling links and symlink parents."""
    if not path.is_absolute() or '..' in path.parts:
        raise ValueError("Identity paths must be absolute and normalized")
    for item in (path, *path.parents):
        if item.is_symlink():
            raise ValueError("Identity paths must not contain symbolic links")


def read_file(path):
    """Read a bounded, unshared regular key without following a replaced link."""
    safe(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > 4096:
            raise ValueError("Invalid nodekey file type, link count or size")
        data = source.read(4097)
    if len(data) > 4096:
        raise ValueError("Nodekey changed while reading")
    return data


def read_key(path):
    data = read_file(path)
    if not re.fullmatch(rb'[0-9a-fA-F]{64}\n?', data) or not 0 < int(data.strip(), 16) < ORDER:
        raise ValueError("Invalid secp256k1 nodekey; preserve the file and restore explicitly")
    return data


def public_key(data):
    """Derive only the public point using OpenSSL's secp256k1 implementation."""
    der = bytes.fromhex('302e0201010420') + bytes.fromhex(data.decode().strip()) + bytes.fromhex('a00706052b8104000a')
    try:
        result = subprocess.run(['openssl', 'ec', '-inform', 'DER', '-pubout', '-outform', 'DER'],
                                input=der, capture_output=True, timeout=10, check=False)
    except FileNotFoundError:
        raise ValueError('OpenSSL is required to inspect P2P identity; run usdb-node prepare-host') from None
    prefix = bytes.fromhex('3056301006072a8648ce3d020106052b8104000a03420004')
    if result.returncode or not result.stdout.startswith(prefix) or len(result.stdout) != len(prefix) + 64:
        raise ValueError("OpenSSL could not validate the P2P public key")
    return result.stdout[len(prefix):].hex()


def inspect(path):
    """Report public metadata; absence and invalid contents are distinct states."""
    safe(path)
    try:
        data = read_file(path)
    except FileNotFoundError:
        return dict(schema_version=SCHEMA, state='MISSING', path=str(path))
    report = dict(schema_version=SCHEMA, state='READY', path=str(path), fingerprint=hashlib.sha256(data).hexdigest())
    if not re.fullmatch(rb'[0-9a-fA-F]{64}\n?', data) or not 0 < int(data.strip(), 16) < ORDER:
        report.update(state='INVALID', detail='Invalid nodekey; archive explicitly before restoring a verified backup')
    else:
        report['public_key'] = public_key(data)
    return report


def sync_dir(path):
    """Persist directory entry changes before reporting identity publication."""
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_new(path, data, uid, gid):
    """Publish a private key exclusively; a crash never replaces an existing key."""
    safe(path)
    missing = []
    parent = path.parent
    while not parent.exists():
        missing.append(parent)
        parent = parent.parent
    for directory in reversed(missing):
        directory.mkdir(mode=0o700)
        if os.geteuid() == 0:
            os.chown(directory, uid, gid)
    fd, temporary = tempfile.mkstemp(prefix='.nodekey-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as target:
            if os.geteuid() == 0:
                os.fchown(target.fileno(), uid, gid)
            target.write(data)
            target.flush()
            os.fsync(target.fileno())
        os.link(temporary, path, follow_symlinks=False)
        os.unlink(temporary)
        sync_dir(path.parent)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def operate(action, path, *, source=None, archive=None, expected='', uid=None, gid=None):
    """Mutate only after the CLI has confirmed the displayed identity and stopped state."""
    uid, gid = os.getuid() if uid is None else uid, os.getgid() if gid is None else gid
    current = inspect(path)
    if action == 'status':
        return current
    if action in {'create', 'restore'}:
        if current['state'] != 'MISSING':
            raise ValueError("Identity already exists; replacement requires a separate explicit archive operation")
        legacy = path.parent.parent / 'nodekey'
        if (legacy.exists() or legacy.is_symlink()) and not (action == 'restore' and source == legacy):
            raise ValueError("Legacy nodekey exists; select it with identity restore after reviewing its location")
        if action == 'create':
            data = f'{secrets.randbelow(ORDER - 1) + 1:064x}'.encode()
        else:
            data = read_key(source)
            if hashlib.sha256(data).hexdigest() != expected:
                raise ValueError("Source identity changed after confirmation")
        public_key(data)
        write_new(path, data, uid, gid)
        return {**inspect(path), 'action':action}
    if action != 'archive' or current['state'] not in {'READY', 'INVALID'} or current['fingerprint'] != expected:
        raise ValueError("Identity changed or is missing; refresh identity status before modifying it")
    safe(archive)
    if path == archive or path.parent.parent in archive.parents:
        raise ValueError("Identity backup must be outside the active chain directory")
    data = read_file(path)
    if hashlib.sha256(data).hexdigest() != expected:
        raise ValueError("Identity changed after confirmation")
    if archive.exists():
        if read_file(archive) != data:
            raise ValueError("Identity backup already exists with different contents")
    else:
        write_new(archive, data, uid, gid)
    if read_file(archive) != data or read_file(path) != data:
        raise ValueError("Identity backup verification failed; active identity was retained")
    path.unlink()
    sync_dir(path.parent)
    return dict(schema_version=SCHEMA, state='ARCHIVED', action=action, fingerprint=expected,
                public_key=current.get('public_key'), backup=str(archive), at=datetime.now(timezone.utc).isoformat())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('status', 'create', 'restore', 'archive'))
    parser.add_argument('path', type=Path)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--archive', type=Path)
    parser.add_argument('--expected', default='')
    parser.add_argument('--uid', type=int, default=os.getuid())
    parser.add_argument('--gid', type=int, default=os.getgid())
    args = parser.parse_args()
    try:
        print(json.dumps(operate(args.action, args.path, source=args.source, archive=args.archive,
                                 expected=args.expected, uid=args.uid, gid=args.gid)))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # Only our ValueErrors contain operator-safe messages. Never echo the
        # OpenSSL input/output, which may hold private material on failure.
        detail = str(error) if isinstance(error, ValueError) else type(error).__name__
        print(f'Identity operation failed: {detail}; no automatic replacement is allowed', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
