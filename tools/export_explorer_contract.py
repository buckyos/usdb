#!/usr/bin/env python3
"""Export an immutable network identity and RPC requirements for independent explorers."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
from release_manifest import build_network_identity

RPC_PROFILE = {
    "profile_id": "usdb-explorer-rpc:v1",
    "read_methods": ["eth_chainId", "net_version", "eth_syncing", "eth_blockNumber",
                     "eth_getBlockByNumber", "eth_getBlockByHash", "eth_getTransactionByHash",
                     "eth_getTransactionReceipt", "eth_getBalance", "eth_getCode", "eth_call", "eth_getLogs"],
    "trace_methods": ["debug_traceTransaction", "debug_traceBlockByNumber"],
    "tracer": "callTracer",
    "broadcast_methods": ["eth_sendRawTransaction"],
    "semantics": {"extra_data": "preserve", "rewards_supply": "not_qualified", "fee_distribution": "not_qualified"},
}


def export_contract(repo, revision, output):
    """Read the selected commit, never an uncommitted network bundle or live RPC identity."""
    if re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        raise ValueError("--revision must be an exact 40-character source commit")
    if subprocess.check_output(["git", "-C", str(repo), "cat-file", "-t", revision]).strip() != b"commit":
        raise ValueError("--revision must identify a commit, not a tag object")
    prefix = "docker/networks/testnet-v0"
    payload = subprocess.check_output(["git", "-C", str(repo), "archive", revision, prefix])
    with tempfile.TemporaryDirectory(prefix="usdb-explorer-contract-") as temporary:
        root = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
            for member in archive:
                target = root / member.name
                if not target.resolve().is_relative_to(root) or not (member.isdir() or member.isfile()):
                    raise ValueError("network bundle contains an unsafe archive entry")
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(archive.extractfile(member).read())
        identity = build_network_identity(root / prefix)
    catalog = (json.dumps(identity, indent=2) + "\n").encode()
    contract = {"schema_version": "usdb-explorer-network-contract:v1",
                "source": {"repository": "buckyos/usdb", "revision": revision, "path": prefix},
                "catalog": {"file": "usdb-testnet-v0.json", "sha256": hashlib.sha256(catalog).hexdigest()},
                "rpc": RPC_PROFILE}
    output.mkdir(parents=True, exist_ok=True)
    for name, body in (("usdb-testnet-v0.json", catalog),
                       ("usdb-testnet-v0.contract.json", (json.dumps(contract, indent=2) + "\n").encode())):
        with (output / name).open("xb") as destination:
            destination.write(body)
    return contract


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        value = export_contract(ROOT, args.revision, args.output_dir)
        print(json.dumps(value, indent=2))
    except (OSError, ValueError, subprocess.SubprocessError, tarfile.TarError) as error:
        parser.exit(1, f"Explorer contract export failed: {error}\n")


if __name__ == "__main__":
    main()
