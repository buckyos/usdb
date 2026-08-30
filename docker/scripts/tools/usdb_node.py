#!/usr/bin/env python3
"""Configure and operate one node from an immutable USDB release node kit."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
KIT_ROOT = SCRIPT_DIR.parents[2]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from generate_bitcoin_rpcauth import generate as generate_rpcauth  # noqa: E402
from release_manifest import build_network_identity  # noqa: E402
from validate_network_bundle import (  # noqa: E402
    PERSISTENT_DATA_PATHS,
    read_env,
    validate_network_bundle,
    validate_node_env,
)


RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
IMAGE_RE = re.compile(r"^ghcr\.io/buckyos/[a-z0-9-]+@sha256:[0-9a-f]{64}$")
ADDRESS_RE = re.compile(r"^0x[0-9a-fA-F]{40}$")
ENV_KEY_RE = re.compile(r"^([A-Z][A-Z0-9_]*)=(.*)$")


@dataclass(frozen=True)
class ReleaseLayout:
    kit_root: Path
    manifest_path: Path
    release_id: str
    bundle_id: str
    bundle_dir: Path
    node_env: Path
    images: dict[str, str]


def _strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_strict_object)
    if not isinstance(value, dict):
        raise ValueError(f"JSON document must be an object: {path}")
    return value


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_checksum(path: Path) -> None:
    checksum_path = path.with_name(path.name + ".sha256")
    if not checksum_path.is_file():
        raise ValueError(f"missing checksum: {checksum_path}")
    expected = f"{_sha256(path)}  {path.name}\n"
    if checksum_path.read_text(encoding="utf-8") != expected:
        raise ValueError(f"checksum mismatch or non-canonical checksum file: {path}")


def _require_image(manifest: dict[str, Any], key: str, name: str) -> str:
    images = manifest.get("images")
    if not isinstance(images, dict) or not isinstance(images.get(key), dict):
        raise ValueError(f"release manifest is missing images.{key}")
    reference = images[key].get("reference")
    expected_prefix = f"ghcr.io/buckyos/{name}@sha256:"
    if not isinstance(reference, str) or IMAGE_RE.fullmatch(reference) is None:
        raise ValueError(f"release manifest images.{key}.reference is not digest-pinned")
    if not reference.startswith(expected_prefix):
        raise ValueError(f"release manifest images.{key}.reference has the wrong image name")
    return reference


def load_release_layout(
    kit_root: Path = KIT_ROOT,
    node_env: Path | None = None,
) -> ReleaseLayout:
    root = kit_root.expanduser().resolve()
    manifest_path = root / "release/usdb-release-manifest.json"
    if not manifest_path.is_file():
        raise ValueError(f"release node kit is missing its manifest: {manifest_path}")
    _verify_checksum(manifest_path)
    manifest = _load_json(manifest_path)
    if manifest.get("schema_version") != "usdb-release-manifest:v3":
        raise ValueError("unsupported USDB release manifest schema")
    release_id = manifest.get("release_id")
    if not isinstance(release_id, str) or RELEASE_ID_RE.fullmatch(release_id) is None:
        raise ValueError("invalid release ID in release manifest")
    network_identity = manifest.get("network_bundle")
    if not isinstance(network_identity, dict):
        raise ValueError("release manifest is missing network_bundle")
    bundle_id = network_identity.get("bundle_id")
    if not isinstance(bundle_id, str) or not release_id.startswith(f"{bundle_id}-r"):
        raise ValueError("release ID does not belong to the manifest network bundle")
    bundle_dir = root / "docker/networks" / bundle_id
    network = validate_network_bundle(bundle_dir)
    if network_identity != build_network_identity(bundle_dir):
        raise ValueError("release manifest network identity does not match the bundled network")
    if network.get("network_bundle_id") != bundle_id:
        raise ValueError("bundled network ID does not match the release manifest")
    images = {
        "USDB_SERVICES_IMAGE": _require_image(manifest, "usdb_services", "usdb-services"),
        "USDB_CHAIN_IMAGE": _require_image(manifest, "usdb_chain", "usdb-chain"),
        "USDB_BITCOIN_IMAGE": _require_image(manifest, "bitcoin_core", "usdb-bitcoin-core"),
    }
    private_node_env = (
        node_env.expanduser().resolve()
        if node_env is not None
        else (Path.home() / ".config/usdb" / bundle_id / "node.env").resolve()
    )
    return ReleaseLayout(
        kit_root=root,
        manifest_path=manifest_path,
        release_id=release_id,
        bundle_id=bundle_id,
        bundle_dir=bundle_dir,
        node_env=private_node_env,
        images=images,
    )


def _validate_env_value(key: str, value: str) -> None:
    if "\n" in value or "\r" in value:
        raise ValueError(f"{key} contains a newline")


def render_env(template: str, updates: dict[str, str]) -> str:
    for key, value in updates.items():
        _validate_env_value(key, value)
    remaining = set(updates)
    rendered: list[str] = []
    for line in template.splitlines():
        match = ENV_KEY_RE.fullmatch(line)
        if match is not None and match.group(1) in updates:
            key = match.group(1)
            rendered.append(f"{key}={updates[key]}")
            remaining.remove(key)
        else:
            rendered.append(line)
    if remaining:
        raise ValueError(f"node.env template is missing keys: {sorted(remaining)}")
    return "\n".join(rendered) + "\n"


def _atomic_write_private(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as target:
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
        temporary.replace(path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def _require_role(role: str, miner_address: str, miner_threads: int) -> None:
    if role not in {"bootnode", "full", "miner"}:
        raise ValueError(f"unsupported node role: {role}")
    if miner_threads < 0:
        raise ValueError("miner threads cannot be negative")
    if role == "miner":
        if ADDRESS_RE.fullmatch(miner_address) is None or int(miner_address[2:], 16) == 0:
            raise ValueError("miner role requires a non-zero EVM --miner-address")
    elif miner_address:
        raise ValueError("--miner-address is only valid for the miner role")


def default_bitcoin_rpc_user(layout: ReleaseLayout, hostname: str | None = None) -> str:
    host = hostname if hostname is not None else socket.gethostname()
    normalized = re.sub(r"[^A-Za-z0-9._-]+", "-", host).strip("-._") or "node"
    return f"{layout.bundle_id}-{normalized}"[:96].rstrip("-._")


def _data_directories(data_root: Path) -> dict[str, Path]:
    root = data_root.expanduser().resolve()
    return {key: root / relative for key, relative in PERSISTENT_DATA_PATHS.items()}


def configure_node(
    layout: ReleaseLayout,
    *,
    data_root: Path,
    role: str,
    miner_address: str,
    miner_threads: int,
    bootnodes: str,
    nat: str,
    bitcoin_rpc_user: str | None,
    bitcoin_p2p: str,
) -> Path:
    if layout.node_env.exists():
        raise ValueError(
            f"refusing to replace existing node configuration: {layout.node_env}; "
            "use set-role for role changes"
        )
    _require_role(role, miner_address, miner_threads)
    if bitcoin_p2p not in {"private", "public"}:
        raise ValueError("bitcoin P2P mode must be private or public")
    root = data_root.expanduser().resolve()
    secure_dir = root / "secure"
    snapshot_dir = root / "releases/balance-history"
    rpcauth_path = secure_dir / "bitcoin-mainnet-rpcauth"
    data_directories = _data_directories(root)
    for path, mode in (
        (secure_dir, 0o700),
        *((path, 0o700) for path in data_directories.values()),
        (snapshot_dir, 0o755),
    ):
        path.mkdir(mode=mode, parents=True, exist_ok=True)
        path.chmod(mode)

    credentials_created = False
    try:
        credentials = generate_rpcauth(
            bitcoin_rpc_user or default_bitcoin_rpc_user(layout),
            rpcauth_path,
        )
        credentials_created = True
        template_path = layout.bundle_dir / "node.env.example"
        updates = {
            **layout.images,
            "USDB_DATA_ROOT": str(root),
            **{key: str(path) for key, path in data_directories.items()},
            "BTC_RPCAUTH_HOST_FILE": str(rpcauth_path),
            "BTC_RPC_USER": credentials["username"],
            "BTC_RPC_PASSWORD": credentials["password"],
            "BTC_P2P_BIND_ADDRESS": "0.0.0.0" if bitcoin_p2p == "public" else "127.0.0.1",
            "BTC_CONTAINER_UID": str(os.getuid()),
            "BTC_CONTAINER_GID": str(os.getgid()),
            "BH_SNAPSHOT_HOST_DIR": str(snapshot_dir),
            "USDB_NODE_ROLE": role,
            "USDB_BOOTNODES": bootnodes,
            "USDB_NAT": nat,
            "USDB_MINER_ADDRESS": miner_address,
            "USDB_MINER_THREADS": str(miner_threads),
        }
        content = render_env(template_path.read_text(encoding="utf-8"), updates)
        _atomic_write_private(layout.node_env, content)
        network = validate_network_bundle(layout.bundle_dir)
        validate_node_env(layout.node_env, network, False, True)
    except BaseException:
        layout.node_env.unlink(missing_ok=True)
        if credentials_created:
            rpcauth_path.unlink(missing_ok=True)
        raise
    return layout.node_env


def _prompt(label: str, *, default: str = "", input_fn: Any = input) -> str:
    suffix = f" [{default}]" if default else ""
    value = input_fn(f"{label}{suffix}: ").strip()
    return value or default


def _prompt_choice(
    label: str,
    choices: tuple[str, ...],
    *,
    default: str,
    input_fn: Any,
    output: Any,
) -> str:
    while True:
        value = _prompt(
            f"{label} ({'/'.join(choices)})",
            default=default,
            input_fn=input_fn,
        ).lower()
        if value in choices:
            return value
        print(f"Choose one of: {', '.join(choices)}", file=output)


def _prompt_yes_no(label: str, *, default: bool, input_fn: Any, output: Any) -> bool:
    default_text = "Y/n" if default else "y/N"
    while True:
        value = input_fn(f"{label} [{default_text}]: ").strip().lower()
        if not value:
            return default
        if value in {"y", "yes"}:
            return True
        if value in {"n", "no"}:
            return False
        print("Enter yes or no.", file=output)


def setup_node(
    layout: ReleaseLayout,
    *,
    input_fn: Any = input,
    output: Any = sys.stdout,
) -> Path:
    if layout.node_env.exists():
        raise ValueError(
            f"node is already configured: {layout.node_env}; use set-role or activate-release"
        )
    print(f"USDB node setup for immutable release {layout.release_id}", file=output)
    data_root = Path(
        _prompt(
            "Host data root",
            default=str(Path.home() / ".usdb"),
            input_fn=input_fn,
        )
    )
    role = _prompt_choice(
        "Node role",
        ("full", "bootnode", "miner"),
        default="full",
        input_fn=input_fn,
        output=output,
    )
    miner_address = ""
    miner_threads = 1
    if role == "miner":
        miner_address = _prompt("Miner EVM address", input_fn=input_fn)
        miner_threads_text = _prompt("CPU mining threads", default="1", input_fn=input_fn)
        try:
            miner_threads = int(miner_threads_text)
        except ValueError as error:
            raise ValueError("CPU mining threads must be an integer") from error
    bootnodes = ""
    if role != "bootnode":
        bootnodes = _prompt("Bootnode enode(s), comma separated; optional", input_fn=input_fn)
    bitcoin_public = _prompt_yes_no(
        "Accept inbound Bitcoin peers on TCP/8333",
        default=False,
        input_fn=input_fn,
        output=output,
    )
    print("", file=output)
    print(f"Data root: {data_root.expanduser().resolve()}", file=output)
    print(f"Role: {role}", file=output)
    print("USDB P2P: public TCP/UDP 31303", file=output)
    print(f"Bitcoin P2P: {'public' if bitcoin_public else 'private'}", file=output)
    if not _prompt_yes_no(
        "Write this node configuration",
        default=True,
        input_fn=input_fn,
        output=output,
    ):
        raise ValueError("setup cancelled; no configuration was written")
    return configure_node(
        layout,
        data_root=data_root,
        role=role,
        miner_address=miner_address,
        miner_threads=miner_threads,
        bootnodes=bootnodes,
        nat="",
        bitcoin_rpc_user=None,
        bitcoin_p2p="public" if bitcoin_public else "private",
    )


def set_role(
    layout: ReleaseLayout,
    *,
    role: str,
    miner_address: str,
    miner_threads: int,
) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    _require_role(role, miner_address, miner_threads)
    original = layout.node_env.read_text(encoding="utf-8")
    updated = render_env(
        original,
        {
            "USDB_NODE_ROLE": role,
            "USDB_MINER_ADDRESS": miner_address,
            "USDB_MINER_THREADS": str(miner_threads),
        },
    )
    try:
        _atomic_write_private(layout.node_env, updated)
        network = validate_network_bundle(layout.bundle_dir)
        validate_node_env(layout.node_env, network, False, True)
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise


def _validate_node_release_images(layout: ReleaseLayout) -> None:
    env = read_env(layout.node_env)
    for key, expected in layout.images.items():
        if env.get(key) != expected:
            raise ValueError(
                f"{key} does not match {layout.release_id}; run activate-release before startup"
            )


def activate_release(layout: ReleaseLayout) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    original = layout.node_env.read_text(encoding="utf-8")
    updated = render_env(original, layout.images)
    try:
        _atomic_write_private(layout.node_env, updated)
        network = validate_network_bundle(layout.bundle_dir)
        validate_node_env(layout.node_env, network, False, True)
        _validate_node_release_images(layout)
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise


def _helper_environment(layout: ReleaseLayout, sync_timeout_secs: int | None = None) -> dict[str, str]:
    environment = os.environ.copy()
    environment["USDB_TESTNET_BUNDLE_DIR"] = str(layout.bundle_dir)
    environment["USDB_TESTNET_NODE_ENV"] = str(layout.node_env)
    environment["USDB_TESTNET_PROJECT_NAME"] = layout.bundle_id
    environment["USDB_TESTNET_BITCOIN_PROJECT_NAME"] = f"{layout.bundle_id}-bitcoin"
    if sync_timeout_secs is not None:
        environment["BTC_READY_WAIT_TIMEOUT_SECS"] = str(sync_timeout_secs)
    return environment


def run_helper(
    layout: ReleaseLayout,
    helper: str,
    arguments: list[str],
    *,
    check: bool = True,
    sync_timeout_secs: int | None = None,
) -> subprocess.CompletedProcess[str]:
    path = layout.kit_root / "docker/scripts/tools" / helper
    if not path.is_file():
        raise ValueError(f"release node kit is missing helper: {path}")
    return subprocess.run(
        [str(path), *arguments],
        check=check,
        env=_helper_environment(layout, sync_timeout_secs),
        text=True,
    )


def doctor(layout: ReleaseLayout) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    for command in ("docker", "python3", "curl"):
        if shutil.which(command) is None:
            raise ValueError(f"required command is not installed: {command}")
    subprocess.run(["docker", "version"], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(["docker", "compose", "version"], check=True, stdout=subprocess.DEVNULL)
    network = validate_network_bundle(layout.bundle_dir)
    validate_node_env(layout.node_env, network, True, True)
    _validate_node_release_images(layout)
    run_helper(layout, "run_testnet_runtime.sh", ["validate-node"])


def start_node(layout: ReleaseLayout, *, sync_timeout_secs: int, pull: bool) -> None:
    doctor(layout)
    if pull:
        run_helper(layout, "run_testnet_bitcoin.sh", ["pull"])
        run_helper(layout, "run_testnet_runtime.sh", ["pull"])
    run_helper(
        layout,
        "run_testnet_bitcoin.sh",
        ["up"],
        sync_timeout_secs=sync_timeout_secs,
    )
    run_helper(layout, "run_testnet_runtime.sh", ["up-data"])
    run_helper(layout, "run_testnet_runtime.sh", ["wait-data", str(sync_timeout_secs)])
    run_helper(layout, "run_testnet_runtime.sh", ["up"])


def print_status(layout: ReleaseLayout) -> int:
    results = [
        run_helper(layout, "run_testnet_bitcoin.sh", ["status"], check=False),
        run_helper(layout, "run_testnet_runtime.sh", ["ps"], check=False),
        run_helper(layout, "run_testnet_runtime.sh", ["data-status"], check=False),
        run_helper(layout, "run_testnet_runtime.sh", ["indexer-status"], check=False),
    ]
    return 0 if all(result.returncode == 0 for result in results) else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kit-root", type=Path, default=KIT_ROOT, help=argparse.SUPPRESS)
    parser.add_argument(
        "--node-env",
        type=Path,
        help="override the bundle-scoped private node.env path",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("setup", help="Interactively create a safe node configuration")

    configure = subparsers.add_parser("configure", help="Create private node configuration and Bitcoin RPC credentials")
    configure.add_argument("--data-root", type=Path, default=Path.home() / ".usdb")
    configure.add_argument("--role", choices=("bootnode", "full", "miner"), default="full")
    configure.add_argument("--miner-address", default="")
    configure.add_argument("--miner-threads", type=int, default=1)
    configure.add_argument("--bootnodes", default="")
    configure.add_argument("--nat", default="")
    configure.add_argument(
        "--bitcoin-rpc-user",
        help="advanced override; defaults to a bundle- and host-scoped username",
    )
    configure.add_argument("--bitcoin-p2p", choices=("private", "public"), default="private")

    role = subparsers.add_parser("set-role", help="Update only the local full/bootnode/miner role")
    role.add_argument("--role", choices=("bootnode", "full", "miner"), required=True)
    role.add_argument("--miner-address", default="")
    role.add_argument("--miner-threads", type=int, default=1)

    subparsers.add_parser(
        "activate-release",
        help="Update only release-owned image digests in existing private config",
    )

    subparsers.add_parser("doctor", help="Validate release identity, local config and Docker")
    up = subparsers.add_parser("up", help="Pull and start all services in readiness order")
    up.add_argument("--sync-timeout-secs", type=int, default=604800)
    up.add_argument("--skip-pull", action="store_true")
    subparsers.add_parser("status", help="Show Bitcoin and USDB service readiness")

    logs = subparsers.add_parser("logs", help="Follow Bitcoin or runtime service logs")
    logs.add_argument("--bitcoin", action="store_true")
    logs.add_argument("service", nargs="*")

    down = subparsers.add_parser("down", help="Stop USDB runtime without deleting data")
    down.add_argument("--include-bitcoin", action="store_true")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        layout = load_release_layout(args.kit_root, args.node_env)
        if args.command == "setup":
            if not sys.stdin.isatty() or not sys.stdout.isatty():
                raise ValueError("setup requires an interactive terminal; use configure for automation")
            path = setup_node(layout)
            print(f"Configured {layout.release_id} node: {path}")
            print("Run usdb-node doctor, then usdb-node up.")
        elif args.command == "configure":
            path = configure_node(
                layout,
                data_root=args.data_root,
                role=args.role,
                miner_address=args.miner_address,
                miner_threads=args.miner_threads,
                bootnodes=args.bootnodes,
                nat=args.nat,
                bitcoin_rpc_user=args.bitcoin_rpc_user,
                bitcoin_p2p=args.bitcoin_p2p,
            )
            print(f"Configured {layout.release_id} node: {path}")
            print("Bitcoin RPC credentials were generated locally and were not printed.")
        elif args.command == "set-role":
            set_role(
                layout,
                role=args.role,
                miner_address=args.miner_address,
                miner_threads=args.miner_threads,
            )
            print(f"Updated node role to {args.role}; run usdb-node up to reconcile containers.")
        elif args.command == "activate-release":
            activate_release(layout)
            print(f"Activated release images in private node config: {layout.release_id}")
        elif args.command == "doctor":
            doctor(layout)
            print(f"USDB node preflight passed: {layout.release_id}")
        elif args.command == "up":
            if args.sync_timeout_secs <= 0:
                raise ValueError("sync timeout must be positive")
            start_node(layout, sync_timeout_secs=args.sync_timeout_secs, pull=not args.skip_pull)
            print(f"USDB node is ready: {layout.release_id}")
        elif args.command == "status":
            return print_status(layout)
        elif args.command == "logs":
            helper = "run_testnet_bitcoin.sh" if args.bitcoin else "run_testnet_runtime.sh"
            arguments = ["logs", *args.service]
            run_helper(layout, helper, arguments)
        elif args.command == "down":
            run_helper(layout, "run_testnet_runtime.sh", ["down"])
            if args.include_bitcoin:
                run_helper(layout, "run_testnet_bitcoin.sh", ["down"])
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"USDB node operation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
