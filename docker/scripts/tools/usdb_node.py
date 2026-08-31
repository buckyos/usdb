#!/usr/bin/env python3
"""Configure and operate one node from an immutable USDB release node kit."""

from __future__ import annotations

import argparse
import getpass
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
from release_manifest import (  # noqa: E402
    SCHEMA_VERSION as RELEASE_MANIFEST_SCHEMA_VERSION,
    build_network_identity,
    build_snapshot_state,
)
from snapshot_distribution import (  # noqa: E402
    DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
    DEFAULT_DOWNLOAD_CONCURRENCY,
    install_release as install_snapshot_artifact,
)
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
    snapshot: dict[str, Any]


@dataclass(frozen=True)
class SetupResult:
    node_env: Path
    apply_firewall: bool
    install_snapshot: bool


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
    if manifest.get("schema_version") != RELEASE_MANIFEST_SCHEMA_VERSION:
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
    snapshot = build_snapshot_state(bundle_dir)
    if manifest.get("snapshot") != snapshot:
        raise ValueError("release manifest snapshot binding does not match the bundled network")
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
        snapshot=snapshot,
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


def _require_port(label: str, value: int) -> int:
    if value < 1 or value > 65535:
        raise ValueError(f"{label} must be between 1 and 65535")
    return value


def detect_ssh_server_port(environment: dict[str, str] | None = None) -> int:
    values = os.environ if environment is None else environment
    connection = values.get("SSH_CONNECTION", "").split()
    if len(connection) == 4:
        try:
            return _require_port("detected SSH server port", int(connection[3]))
        except ValueError:
            pass
    return 22


def default_docker_user() -> str:
    return "" if os.geteuid() == 0 else getpass.getuser()


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
    ssh_port: int = 22,
) -> Path:
    if layout.node_env.exists():
        raise ValueError(
            f"refusing to replace existing node configuration: {layout.node_env}; "
            "use set-role for role changes"
        )
    _require_role(role, miner_address, miner_threads)
    if bitcoin_p2p not in {"private", "public"}:
        raise ValueError("bitcoin P2P mode must be private or public")
    _require_port("operator SSH port", ssh_port)
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
            "USDB_OPERATOR_SSH_PORT": str(ssh_port),
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


def _human_bytes(value: int) -> str:
    size = float(value)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if size < 1024 or unit == "TiB":
            return f"{size:.1f} {unit}"
        size /= 1024
    raise AssertionError("unreachable")


def _snapshot_required_free_bytes(snapshot: dict[str, Any]) -> int:
    download_size = snapshot["download_size_bytes"]
    database_size = snapshot["snapshot_db_size_bytes"]
    safety_margin = max(64 * 1024**3, database_size // 5)
    return download_size + database_size + safety_margin


def _disk_free_bytes(path: Path) -> int:
    candidate = path.expanduser().resolve()
    while not candidate.exists() and candidate != candidate.parent:
        candidate = candidate.parent
    return shutil.disk_usage(candidate).free


def setup_node(
    layout: ReleaseLayout,
    *,
    input_fn: Any = input,
    output: Any = sys.stdout,
) -> SetupResult:
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
    ssh_port_text = _prompt(
        "Operator SSH server port",
        default=str(detect_ssh_server_port()),
        input_fn=input_fn,
    )
    try:
        ssh_port = _require_port("operator SSH port", int(ssh_port_text))
    except ValueError as error:
        raise ValueError("operator SSH port must be an integer between 1 and 65535") from error
    install_snapshot = False
    snapshot = layout.snapshot
    if snapshot.get("status") == "available":
        required_free = _snapshot_required_free_bytes(snapshot)
        available_free = _disk_free_bytes(data_root)
        print("", file=output)
        print("Release-approved balance-history snapshot:", file=output)
        print(f"  ID: {snapshot['snapshot_release_id']}", file=output)
        print(f"  BTC height: {snapshot['height']}", file=output)
        print(f"  Download: {_human_bytes(snapshot['download_size_bytes'])}", file=output)
        print(f"  Recommended free space: {_human_bytes(required_free)}", file=output)
        print(f"  Current free space: {_human_bytes(available_free)}", file=output)
        install_snapshot = _prompt_yes_no(
            "Use this release-approved snapshot",
            default=available_free >= required_free,
            input_fn=input_fn,
            output=output,
        )
    print("", file=output)
    print(f"Data root: {data_root.expanduser().resolve()}", file=output)
    print(f"Role: {role}", file=output)
    print("USDB P2P: public TCP/UDP 31303", file=output)
    print(f"Bitcoin P2P: {'public' if bitcoin_public else 'private'}", file=output)
    print(f"Operator SSH port preserved by UFW: {ssh_port}", file=output)
    if not _prompt_yes_no(
        "Write this node configuration",
        default=True,
        input_fn=input_fn,
        output=output,
    ):
        raise ValueError("setup cancelled; no configuration was written")
    path = configure_node(
        layout,
        data_root=data_root,
        role=role,
        miner_address=miner_address,
        miner_threads=miner_threads,
        bootnodes=bootnodes,
        nat="",
        bitcoin_rpc_user=None,
        bitcoin_p2p="public" if bitcoin_public else "private",
        ssh_port=ssh_port,
    )
    apply_firewall = _prompt_yes_no(
        "Apply and verify the UFW firewall profile now",
        default=True,
        input_fn=input_fn,
        output=output,
    )
    return SetupResult(
        node_env=path,
        apply_firewall=apply_firewall,
        install_snapshot=install_snapshot,
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


def _snapshot_trusted_keys(layout: ReleaseLayout) -> Path:
    path = layout.bundle_dir / layout.snapshot["trusted_keys"]["path"]
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"release-approved snapshot trusted-key catalog is missing: {path}")
    if _sha256(path) != layout.snapshot["trusted_keys"]["sha256"]:
        raise ValueError("release-approved snapshot trusted-key catalog hash mismatch")
    return path


def _approved_snapshot_record(layout: ReleaseLayout) -> dict[str, Any]:
    if layout.snapshot.get("status") != "available":
        raise ValueError(f"release {layout.release_id} has no approved snapshot")
    record_path = layout.bundle_dir / layout.snapshot["record"]["path"]
    if not record_path.is_file() or record_path.is_symlink():
        raise ValueError(f"release-approved snapshot record is missing: {record_path}")
    if _sha256(record_path) != layout.snapshot["record"]["sha256"]:
        raise ValueError("release-approved snapshot record hash mismatch")
    return _load_json(record_path)


def _snapshot_env_updates(record: dict[str, Any]) -> dict[str, str]:
    by_role = {item["role"]: item for item in record["files"]}
    release_root = f"/snapshots/{record['snapshot_release_id']}"
    return {
        "SNAPSHOT_MODE": "balance-history",
        "BH_SNAPSHOT_FILE": f"{release_root}/{by_role['snapshot_db']['path']}",
        "BH_SNAPSHOT_MANIFEST": f"{release_root}/{by_role['snapshot_manifest']['path']}",
        "USDB_INDEXER_CHECKPOINT_MANIFEST": "",
    }


def install_snapshot_release(
    layout: ReleaseLayout,
    *,
    download_concurrency: int = DEFAULT_DOWNLOAD_CONCURRENCY,
    download_chunk_size_mib: int = DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
) -> Path:
    if not 1 <= download_concurrency <= 64:
        raise ValueError("download concurrency must be between 1 and 64")
    if not 1 <= download_chunk_size_mib <= 1024:
        raise ValueError("download chunk size must be between 1 and 1024 MiB")
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run setup or configure first")
    env = read_env(layout.node_env)
    snapshot_root = Path(env.get("BH_SNAPSHOT_HOST_DIR", "")).expanduser().resolve()
    balance_history_root = Path(env.get("BH_DATA_HOST_DIR", "")).expanduser().resolve()
    database = balance_history_root / "db"
    if database.is_dir() and any(database.iterdir()):
        raise ValueError(
            "refusing to change snapshot selection after balance-history DB initialization; "
            "use a fresh data root or an explicit reviewed recovery procedure"
        )
    record = _approved_snapshot_record(layout)
    updates = _snapshot_env_updates(record)
    network = validate_network_bundle(layout.bundle_dir)
    btc_source = network.get("btc_source")
    if not isinstance(btc_source, dict) or not isinstance(btc_source.get("index_origin_height"), int):
        raise ValueError("network bundle is missing BTC index origin")
    original = layout.node_env.read_text(encoding="utf-8")
    if not all(env.get(key, "") == value for key, value in updates.items()):
        try:
            _atomic_write_private(layout.node_env, render_env(original, updates))
            validate_node_env(layout.node_env, network, False, False)
        except BaseException:
            _atomic_write_private(layout.node_env, original)
            raise

    installed = install_snapshot_artifact(
        record_url=layout.snapshot["record"]["url"],
        destination_root=snapshot_root,
        trusted_keys=_snapshot_trusted_keys(layout),
        expected_network=env.get("BTC_NETWORK", "bitcoin"),
        max_height=btc_source["index_origin_height"],
        download_concurrency=download_concurrency,
        download_chunk_size_mib=download_chunk_size_mib,
    )
    if installed.release_id != record["snapshot_release_id"]:
        raise ValueError("installed snapshot release ID differs from the approved record")
    validate_node_env(layout.node_env, network, True, False)
    return installed.release_dir


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


def run_host_action(
    layout: ReleaseLayout,
    action: str,
    *,
    docker_user: str = "",
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    if action not in {"check", "install"}:
        raise ValueError(f"unsupported host action: {action}")
    arguments = [action]
    if docker_user:
        arguments.extend(["--docker-user", docker_user])
    return run_helper(layout, "prepare_usdb_host.sh", arguments, check=check)


def prepare_host(
    layout: ReleaseLayout,
    *,
    docker_user: str,
    input_fn: Any = input,
    output: Any = sys.stdout,
) -> None:
    result = run_host_action(layout, "check", docker_user=docker_user, check=False)
    if result.returncode == 0:
        return
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ValueError("host prerequisites failed; run 'usdb-node host install' explicitly")
    if not _prompt_yes_no(
        "Host prerequisites failed; install supported packages now",
        default=False,
        input_fn=input_fn,
        output=output,
    ):
        raise ValueError("host preparation cancelled; no packages were installed")
    run_host_action(layout, "install", docker_user=docker_user)


def run_firewall_action(
    layout: ReleaseLayout,
    action: str,
    *,
    confirm: bool = False,
    ssh_port: int | None = None,
) -> subprocess.CompletedProcess[str]:
    if action not in {"check", "apply"}:
        raise ValueError(f"unsupported firewall action: {action}")
    env = read_env(layout.node_env)
    configured_ssh_port = env.get("USDB_OPERATOR_SSH_PORT", "")
    try:
        effective_ssh_port = _require_port(
            "operator SSH port",
            ssh_port if ssh_port is not None else int(configured_ssh_port),
        )
    except ValueError as error:
        raise ValueError("node configuration contains an invalid operator SSH port") from error
    bitcoin_bind = env.get("BTC_P2P_BIND_ADDRESS", "")
    if bitcoin_bind == "127.0.0.1":
        bitcoin_p2p = "private"
    elif bitcoin_bind == "0.0.0.0":
        bitcoin_p2p = "public"
    else:
        raise ValueError("node configuration contains an invalid Bitcoin P2P bind address")
    arguments = [
        action,
        "--node-env",
        str(layout.node_env),
        "--ssh-port",
        str(effective_ssh_port),
        "--bitcoin-p2p",
        bitcoin_p2p,
    ]
    if confirm:
        arguments.append("--confirm")
    return run_helper(layout, "prepare_usdb_firewall.sh", arguments)


def doctor(layout: ReleaseLayout) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    run_host_action(layout, "check", docker_user=default_docker_user())
    network = validate_network_bundle(layout.bundle_dir)
    validate_node_env(layout.node_env, network, True, True)
    _validate_node_release_images(layout)
    run_helper(layout, "run_testnet_runtime.sh", ["validate-node"])
    run_firewall_action(layout, "check")


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

    prepare_host_parser = subparsers.add_parser(
        "prepare-host",
        help="Check host prerequisites and offer explicit installation when needed",
    )
    prepare_host_parser.add_argument("--docker-user", default=default_docker_user())

    host = subparsers.add_parser("host", help="Check or install host prerequisites")
    host_actions = host.add_subparsers(dest="host_action", required=True)
    for action in ("check", "install"):
        host_action = host_actions.add_parser(action)
        host_action.add_argument("--docker-user", default=default_docker_user())

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
    configure.add_argument(
        "--ssh-port",
        type=int,
        default=detect_ssh_server_port(),
        help="host SSH server port preserved by the managed firewall profile",
    )

    role = subparsers.add_parser("set-role", help="Update only the local full/bootnode/miner role")
    role.add_argument("--role", choices=("bootnode", "full", "miner"), required=True)
    role.add_argument("--miner-address", default="")
    role.add_argument("--miner-threads", type=int, default=1)

    subparsers.add_parser(
        "activate-release",
        help="Update only release-owned image digests in existing private config",
    )

    subparsers.add_parser(
        "doctor",
        help="Read-only host, firewall, release identity and local configuration preflight",
    )

    snapshot = subparsers.add_parser("snapshot", help="Install an immutable signed snapshot release")
    snapshot_actions = snapshot.add_subparsers(dest="snapshot_action", required=True)
    snapshot_install = snapshot_actions.add_parser(
        "install",
        help="Resume download, verify, atomically stage, and select one snapshot",
    )
    snapshot_install.add_argument(
        "--download-concurrency",
        type=int,
        default=DEFAULT_DOWNLOAD_CONCURRENCY,
        help="advanced HTTP Range worker override",
    )
    snapshot_install.add_argument(
        "--download-chunk-size-mib",
        type=int,
        default=DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
        help="advanced HTTP Range chunk size override",
    )

    firewall = subparsers.add_parser("firewall", help="Check or apply the host UFW profile")
    firewall_actions = firewall.add_subparsers(dest="firewall_action", required=True)
    firewall_check = firewall_actions.add_parser("check")
    firewall_check.add_argument("--ssh-port", type=int)
    firewall_apply = firewall_actions.add_parser("apply")
    firewall_apply.add_argument("--ssh-port", type=int)
    firewall_apply.add_argument("--confirm", action="store_true")

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
        if args.command == "prepare-host":
            prepare_host(layout, docker_user=args.docker_user)
            print("USDB host prerequisites are ready.")
            print("If Docker group membership changed, start a new login session before doctor/up.")
        elif args.command == "host":
            run_host_action(layout, args.host_action, docker_user=args.docker_user)
        elif args.command == "setup":
            if not sys.stdin.isatty() or not sys.stdout.isatty():
                raise ValueError("setup requires an interactive terminal; use configure for automation")
            result = setup_node(layout)
            print(f"Configured {layout.release_id} node: {result.node_env}")
            if result.install_snapshot:
                release_dir = install_snapshot_release(layout)
                print(f"Installed and selected signed balance-history snapshot: {release_dir}")
            if result.apply_firewall:
                run_firewall_action(layout, "apply", confirm=True)
                print("Applied and verified the host UFW firewall profile.")
            else:
                print("Firewall was not changed. Run 'usdb-node firewall apply --confirm' before startup.")
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
                ssh_port=args.ssh_port,
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
        elif args.command == "snapshot":
            release_dir = install_snapshot_release(
                layout,
                download_concurrency=args.download_concurrency,
                download_chunk_size_mib=args.download_chunk_size_mib,
            )
            print(f"Installed and selected signed balance-history snapshot: {release_dir}")
        elif args.command == "firewall":
            if args.firewall_action == "apply" and not args.confirm:
                raise ValueError("firewall apply requires --confirm")
            run_firewall_action(
                layout,
                args.firewall_action,
                confirm=getattr(args, "confirm", False),
                ssh_port=args.ssh_port,
            )
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
