"""Explicit disposable identity provision for tests that exercise startup."""
from contextlib import contextmanager
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace
from unittest import mock


def provision_identity(layout, node):
    """Model an operator-created identity; never run against a real deployment."""
    path = Path(node.read_env(layout.node_env)["USDB_CHAIN_DATA_HOST_DIR"]) / "geth/nodekey"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x") as stream:
        stream.write("0" * 63 + "1")
    path.chmod(0o600)
    return path


@contextmanager
def container_identity(layout, node):
    """Model an unreadable host key and an initially absent pinned helper image."""
    import node_identity as identity
    path = identity.key_path(layout, node)
    state = SimpleNamespace(cached=False, report=identity.files.inspect(path), commands=[], events=[])
    image = layout.images['USDB_CHAIN_IMAGE']

    def run(command, **kwargs):
        state.commands.append(command)
        if command[:3] == ['docker', 'image', 'inspect']:
            assert command[3:] == [image]
            return subprocess.CompletedProcess(command, 0 if state.cached else 1, '[]',
                '' if state.cached else f'Error response from daemon: No such image: {image}\n')
        assert command[:2] == ['docker', 'run'] and image in command
        assert '--pull=never' in command and '--read-only' in command and '--network=none' in command
        if not state.cached:
            return subprocess.CompletedProcess(command, 125, '', 'image unavailable')
        state.events.append('identity-' + state.report['state'])
        return subprocess.CompletedProcess(command, 0, json.dumps(state.report), '')

    with mock.patch.object(identity.files, 'inspect', side_effect=PermissionError), \
            mock.patch.object(identity, 'subprocess', SimpleNamespace(run=run)):
        yield state
