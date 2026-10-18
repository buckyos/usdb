"""Explicit disposable identity provision for tests that exercise startup."""
from pathlib import Path


def provision_identity(layout, node):
    """Model an operator-created identity; never run against a real deployment."""
    path = Path(node.read_env(layout.node_env)["USDB_CHAIN_DATA_HOST_DIR"]) / "geth/nodekey"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x") as stream:
        stream.write("0" * 63 + "1")
    path.chmod(0o600)
    return path
