#!/usr/bin/env python3
"""Configure and operate one node from an immutable USDB release node kit."""

from __future__ import annotations

import argparse
import fcntl
import getpass
import hashlib
import json
import os
import pwd
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from contextlib import contextmanager, nullcontext, redirect_stdout
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator


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
from runtime_compatibility import (  # noqa: E402
    DATA_LAYOUT_VERSION,
    DATASET_IDENTITY_FILE,
    PERSISTENT_DATA_SERVICES,
    build_dataset_identity,
    build_legacy_data_paths,
    build_persistent_data_paths,
    build_runtime_compatibility,
    network_secure_dir,
    snapshot_artifact_dir,
)
from snapshot_distribution import (  # noqa: E402
    DEFAULT_DOWNLOAD_CHUNK_SIZE_MIB,
    DEFAULT_DOWNLOAD_CONCURRENCY,
    install_release as install_snapshot_artifact,
)
from validate_network_bundle import (  # noqa: E402
    BITCOIN_RESOURCE_PROFILES,
    DEFAULT_BITCOIN_RESOURCE_PROFILE,
    btc_registry_stable_lag_blocks,
    read_env,
    validate_network_bundle,
    validate_node_env,
)


RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
IMAGE_RE = re.compile(r"^ghcr\.io/buckyos/[a-z0-9-]+@sha256:[0-9a-f]{64}$")
ADDRESS_RE = re.compile(r"^0x[0-9a-fA-F]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
ENV_KEY_RE = re.compile(r"^([A-Z][A-Z0-9_]*)=(.*)$")
# A fresh node temporarily retains source data, signed snapshot artifacts, and
# installed databases together, so setup needs substantial free headroom.
MIN_DATA_ROOT_BYTES = 3 * 1024**4 // 2
RECOMMENDED_DATA_ROOT_BYTES = 2 * 1024**4
FIREWALL_MODES = ("external", "managed")
NODE_STATUS_SCHEMA_VERSION = "usdb-node-status:v2"
NODE_UP_SCHEMA_VERSION = "usdb-node-up:v1"
NODE_PROGRESS_SCHEMA_VERSION = "usdb-node-progress:v3"
SNAPSHOT_IMPORT_PROGRESS_SCHEMA_VERSION = "balance-history-snapshot-install-progress:v1"
CONTROLLER_MANUAL_EXIT_CODE = 2
CONTROLLER_UNIT_PREFIX = "usdb-node-bootstrap"
CONTROLLER_RESTART_SECS = 30
CONTROLLER_START_GRACE_SECS = 10
CONTROLLER_START_LIMIT_INTERVAL_SECS = 1800
CONTROLLER_START_LIMIT_BURST = 20
DEFAULT_SYNC_TIMEOUT_SECS = 604800
MAX_UP_TRANSITIONS = 4
DEFAULT_PROGRESS_REFRESH_SECS = 5.0
MAX_STALE_PROGRESS_AGE_SECS = 60.0
AUTO_BITCOIN_RESOURCE_PROFILE = "auto"
PERFORMANCE_BITCOIN_RESOURCE_PROFILE = "performance-64g"
IBD_BITCOIN_RESOURCE_PROFILE = "ibd-64g"
HIGH_MEMORY_BITCOIN_RESOURCE_PROFILES = frozenset(
    {PERFORMANCE_BITCOIN_RESOURCE_PROFILE, IBD_BITCOIN_RESOURCE_PROFILE}
)
HIGH_MEMORY_PROFILE_MIN_HOST_MEMORY_BYTES = 56 * 1024**3
ALT_SCREEN_ENTER = "\x1b[?1049h"
ALT_SCREEN_EXIT = "\x1b[?1049l"
CURSOR_HIDE = "\x1b[?25l"
CURSOR_SHOW = "\x1b[?25h"
SCREEN_CLEAR = "\x1b[H\x1b[2J"
PROGRESS_COMPONENTS = (
    ("snapshot", "Snapshot"),
    ("bitcoin", "Bitcoin"),
    ("balance_history", "Balance history"),
    ("usdb_indexer", "USDB indexer"),
    ("usdb_chain", "USDB chain"),
)
CORE_RUNTIME_SERVICES = (
    "btc-node",
    "balance-history",
    "usdb-indexer",
    "usdb-chain",
    "usdb-control-plane",
)


@dataclass(frozen=True)
class BitcoinDataStartAnchor:
    stable_height: int
    minimum_tip_height: int
    stable_lag_blocks: int
    block_hash: str | None


@dataclass(frozen=True)
class ReleaseLayout:
    kit_root: Path
    manifest_path: Path
    release_id: str
    bundle_id: str
    bundle_dir: Path
    node_env: Path
    network_identity: dict[str, Any]
    runtime_compatibility: dict[str, Any]
    images: dict[str, str]
    snapshot: dict[str, Any]


@dataclass(frozen=True)
class SetupResult:
    node_env: Path
    apply_firewall: bool
    install_snapshot: bool


@dataclass(frozen=True)
class DataRootCapacity:
    filesystem_path: Path
    total_bytes: int
    free_bytes: int


@dataclass(frozen=True)
class ControllerInstallContext:
    launcher: Path
    docker_launcher: Path
    service_user: str
    home: Path


def controller_unit_name(layout: ReleaseLayout) -> str:
    """Return the systemd unit name scoped to one immutable network bundle."""
    return f"{CONTROLLER_UNIT_PREFIX}-{layout.bundle_id}.service"


def controller_unit_path(layout: ReleaseLayout) -> Path:
    """Return the system-level unit path, allowing an isolated test override."""
    root = Path(os.environ.get("USDB_SYSTEMD_UNIT_DIR", "/etc/systemd/system"))
    return root / controller_unit_name(layout)


def _systemd_quote(value: str) -> str:
    if any(character in value for character in ("\n", "\r", "\0")):
        raise ValueError("systemd unit value contains an unsupported control character")
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    return f'"{escaped}"'


def _controller_launcher_path(launcher: Path | None = None) -> Path:
    candidate = launcher
    if candidate is None:
        configured = os.environ.get("USDB_NODE_LAUNCHER", "")
        if configured:
            candidate = Path(configured)
        else:
            discovered = shutil.which("usdb-node")
            if discovered is None:
                raise ValueError(
                    "the stable usdb-node launcher is unavailable; install the release node kit first"
                )
            candidate = Path(discovered)
    path = candidate.expanduser().absolute()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f"usdb-node launcher is not executable: {path}")
    return path


def render_controller_unit(
    layout: ReleaseLayout,
    *,
    launcher: Path,
    service_user: str,
    home: Path,
    docker_launcher: Path = Path("/usr/bin/docker"),
    sync_timeout_secs: int = DEFAULT_SYNC_TIMEOUT_SECS,
    pull: bool = True,
) -> str:
    """Render the non-activating bootstrap controller as a systemd service."""
    if not service_user or re.fullmatch(r"[A-Za-z0-9_.-]+", service_user) is None:
        raise ValueError(f"invalid systemd service user: {service_user}")
    if sync_timeout_secs <= 0:
        raise ValueError("controller sync timeout must be positive")
    command_parts = [
        _systemd_quote(str(launcher)),
        "--node-env",
        _systemd_quote(str(layout.node_env)),
        "controller",
        "run",
        "--sync-timeout-secs",
        str(sync_timeout_secs),
    ]
    if not pull:
        command_parts.append("--skip-pull")
    command = " ".join(command_parts)
    return f"""[Unit]
Description=USDB bootstrap controller for {layout.bundle_id}
Wants=network-online.target docker.service
After=network-online.target docker.service
StartLimitIntervalSec={CONTROLLER_START_LIMIT_INTERVAL_SECS}s
StartLimitBurst={CONTROLLER_START_LIMIT_BURST}

[Service]
Type=simple
User={service_user}
Environment={_systemd_quote(f"HOME={home}")}
Environment=PYTHONDONTWRITEBYTECODE=1
ExecStartPre={_systemd_quote(str(docker_launcher))} info --format {{{{.ServerVersion}}}}
ExecStart={command}
Restart=on-failure
RestartSec={CONTROLLER_RESTART_SECS}s
RestartPreventExitStatus={CONTROLLER_MANUAL_EXIT_CODE}
TimeoutStartSec=infinity
TimeoutStopSec=45s
KillMode=control-group
UMask=0077

[Install]
WantedBy=multi-user.target
"""


def _privileged_command(
    arguments: list[str],
    *,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    command = arguments
    if os.geteuid() != 0:
        sudo = shutil.which("sudo")
        if sudo is None:
            raise ValueError("this controller operation requires root privileges or sudo")
        command = [sudo, *arguments]
    return subprocess.run(command, check=check, text=True)


def _systemd_available() -> bool:
    return shutil.which("systemctl") is not None and Path("/run/systemd/system").is_dir()


def _controller_install_context(launcher: Path | None = None) -> ControllerInstallContext:
    """Validate host prerequisites before node configuration or unit installation."""
    if not _systemd_available():
        raise ValueError(
            "a running systemd system manager is required; "
            "use setup --no-controller only for explicit foreground operation"
        )
    stable_launcher = _controller_launcher_path(launcher)
    docker_command = shutil.which("docker")
    if docker_command is None:
        raise ValueError("docker is required before installing the bootstrap controller")
    if os.geteuid() != 0 and shutil.which("sudo") is None:
        raise ValueError("sudo is required to install the systemd bootstrap controller")
    account = pwd.getpwuid(os.getuid())
    return ControllerInstallContext(
        launcher=stable_launcher,
        docker_launcher=Path(docker_command).absolute(),
        service_user=account.pw_name,
        home=Path(account.pw_dir),
    )


def install_controller_unit(
    layout: ReleaseLayout,
    *,
    launcher: Path | None = None,
    sync_timeout_secs: int = DEFAULT_SYNC_TIMEOUT_SECS,
    pull: bool = True,
) -> Path:
    """Install and enable the restartable controller without starting node services."""
    if not layout.node_env.is_file():
        raise ValueError("configure the node before installing its bootstrap controller")
    context = _controller_install_context(launcher)
    content = render_controller_unit(
        layout,
        launcher=context.launcher,
        service_user=context.service_user,
        home=context.home,
        docker_launcher=context.docker_launcher,
        sync_timeout_secs=sync_timeout_secs,
        pull=pull,
    )
    destination = controller_unit_path(layout)
    if destination.is_symlink():
        raise ValueError(f"refusing to replace symlinked controller unit: {destination}")
    if destination.is_file():
        existing_user = next(
            (
                line.removeprefix("User=")
                for line in destination.read_text(encoding="utf-8").splitlines()
                if line.startswith("User=")
            ),
            "",
        )
        if existing_user != context.service_user:
            raise ValueError(
                "refusing to change the bootstrap controller service user from "
                f"{existing_user or 'unknown'} to {context.service_user}"
            )
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.")
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as target:
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
        _privileged_command(["install", "-m", "0644", str(temporary), str(destination)])
        _privileged_command(["systemctl", "daemon-reload"])
        _privileged_command(["systemctl", "enable", destination.name])
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def _require_controller_unit(layout: ReleaseLayout) -> Path:
    path = controller_unit_path(layout)
    if not path.is_file():
        raise ValueError(
            f"bootstrap controller is not installed: {path}; run usdb-node controller install"
        )
    return path


def start_controller_unit(layout: ReleaseLayout) -> str:
    """Submit the controller to systemd and return its stable unit name."""
    unit = _require_controller_unit(layout).name
    # An explicit operator start is allowed to clear a previous bounded retry stop.
    _privileged_command(["systemctl", "reset-failed", unit], check=False)
    _privileged_command(["systemctl", "start", "--no-block", unit])
    return unit


def stop_controller_unit(layout: ReleaseLayout) -> None:
    """Stop only bootstrap orchestration; detached Docker services remain running."""
    unit = _require_controller_unit(layout).name
    _privileged_command(["systemctl", "stop", unit])


def disable_controller_unit(layout: ReleaseLayout) -> None:
    """Stop and disable bootstrap orchestration without deleting its audit trail."""
    unit = _require_controller_unit(layout).name
    _privileged_command(["systemctl", "disable", "--now", unit])


def down_node(layout: ReleaseLayout, *, keep_bitcoin: bool) -> None:
    """Stop bootstrap orchestration and then stop node services in dependency order."""
    if controller_unit_path(layout).is_file():
        stop_controller_unit(layout)
    with node_operation_lock(layout, "down"):
        run_helper(layout, "run_testnet_runtime.sh", ["down"])
        if not keep_bitcoin:
            run_helper(layout, "run_testnet_bitcoin.sh", ["down"])


def show_controller_unit(layout: ReleaseLayout) -> int:
    """Print the systemd controller status without changing node state."""
    unit = _require_controller_unit(layout).name
    return_code = subprocess.run(
        ["systemctl", "status", "--no-pager", unit],
        check=False,
        text=True,
    ).returncode
    if return_code != 0 and collect_node_status(layout)["overall_state"] == "READY":
        return 0
    return return_code


def follow_controller_logs(layout: ReleaseLayout, *, follow: bool) -> int:
    """Print controller journal records, optionally following new entries."""
    unit = _require_controller_unit(layout).name
    command = ["journalctl", "--unit", unit, "--no-pager"]
    if follow:
        command.append("--follow")
    return subprocess.run(command, check=False, text=True).returncode


def controller_active_state(layout: ReleaseLayout) -> str:
    """Return systemd's current ActiveState for the installed controller."""
    unit = _require_controller_unit(layout).name
    result = subprocess.run(
        ["systemctl", "show", "--property=ActiveState", "--value", unit],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "systemctl show failed"
        raise ValueError(f"failed to query bootstrap controller: {detail}")
    state = result.stdout.strip()
    if not state:
        raise ValueError("systemd returned an empty bootstrap controller state")
    return state


def controller_observed_state(layout: ReleaseLayout) -> str:
    """Return a non-failing controller state for read-only progress reports."""
    if not controller_unit_path(layout).is_file():
        return "uninstalled"
    try:
        return controller_active_state(layout)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return "unavailable"


@contextmanager
def node_operation_lock(layout: ReleaseLayout, operation: str) -> Iterator[None]:
    """Serialize bundle-scoped operations that mutate configuration or runtime state."""
    lock_path = layout.node_env.parent / ".usdb-node-operation.lock"
    lock_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    flags = os.O_RDWR | os.O_CREAT
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(lock_path, flags, 0o600)
    acquired = False
    with os.fdopen(descriptor, "r+", encoding="utf-8") as lock:
        os.fchmod(lock.fileno(), 0o600)
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            acquired = True
        except BlockingIOError as error:
            lock.seek(0)
            holder = lock.read().strip()
            detail = f": {holder}" if holder else ""
            guidance = (
                " Wait for the active operation to finish and retry; do not remove "
                "the lock file while its process is running."
            )
            try:
                holder_metadata = json.loads(holder) if holder else {}
            except json.JSONDecodeError:
                holder_metadata = {}
            if (
                operation == "activate-release"
                and holder_metadata.get("operation") == "up"
            ):
                guidance = (
                    " Release activation requires stopped node services. Run "
                    "'usdb-node down', wait for it to complete, then rerun "
                    "'usdb-node activate-release'."
                )
            raise ValueError(
                f"another usdb-node operation is already running for "
                f"{layout.bundle_id}{detail}.{guidance}"
            ) from error

        metadata = {
            "operation": operation,
            "pid": os.getpid(),
            "release_id": layout.release_id,
            "started_at": datetime.now(timezone.utc).isoformat(),
        }
        lock.seek(0)
        lock.truncate()
        lock.write(json.dumps(metadata, sort_keys=True) + "\n")
        lock.flush()
        os.fsync(lock.fileno())
        try:
            yield
        finally:
            if acquired:
                lock.seek(0)
                lock.truncate()
                lock.flush()
                os.fsync(lock.fileno())
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


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
    runtime_compatibility = build_runtime_compatibility(network_identity)
    if manifest.get("runtime_compatibility") != runtime_compatibility:
        raise ValueError(
            "release manifest runtime compatibility does not match the bundled network"
        )
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
        network_identity=network_identity,
        runtime_compatibility=runtime_compatibility,
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


def upsert_env(template: str, updates: dict[str, str]) -> str:
    """Replace existing env keys and append explicitly approved new keys."""
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
        if rendered and rendered[-1]:
            rendered.append("")
        rendered.extend(f"{key}={updates[key]}" for key in sorted(remaining))
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


def _atomic_write_public(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o644)
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


def _require_firewall_mode(mode: str) -> str:
    if mode not in FIREWALL_MODES:
        raise ValueError("firewall mode must be external or managed")
    return mode


def _host_memory_bytes() -> int:
    """Return physical host memory used to choose a conservative local profile."""
    try:
        values = Path("/proc/meminfo").read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ValueError(f"failed to read host memory from /proc/meminfo: {error}") from error
    for line in values:
        if line.startswith("MemTotal:"):
            fields = line.split()
            if len(fields) == 3 and fields[2] == "kB" and fields[1].isdigit():
                return int(fields[1]) * 1024
    raise ValueError("/proc/meminfo does not contain a valid MemTotal value")


def resolve_bitcoin_resource_profile(
    profile: str,
    *,
    host_memory_bytes: int | None = None,
) -> tuple[str, dict[str, str]]:
    """Resolve an operator profile and reject unsafe high-memory selection."""
    memory_bytes = _host_memory_bytes() if host_memory_bytes is None else host_memory_bytes
    selected = profile
    if profile == AUTO_BITCOIN_RESOURCE_PROFILE:
        selected = (
            PERFORMANCE_BITCOIN_RESOURCE_PROFILE
            if memory_bytes >= HIGH_MEMORY_PROFILE_MIN_HOST_MEMORY_BYTES
            else DEFAULT_BITCOIN_RESOURCE_PROFILE
        )
    resources = BITCOIN_RESOURCE_PROFILES.get(selected)
    if resources is None:
        raise ValueError(
            "Bitcoin resource profile must be auto or one of: "
            + ", ".join(BITCOIN_RESOURCE_PROFILES)
        )
    if (
        selected in HIGH_MEMORY_BITCOIN_RESOURCE_PROFILES
        and memory_bytes < HIGH_MEMORY_PROFILE_MIN_HOST_MEMORY_BYTES
    ):
        raise ValueError(
            f"{selected} requires at least 56 GiB of physical host memory"
        )
    return selected, resources


def configured_firewall_mode(layout: ReleaseLayout) -> str:
    env = read_env(layout.node_env)
    # Missing means the mandatory managed behavior used by older node kits.
    return _require_firewall_mode(env.get("USDB_FIREWALL_MODE", "managed"))


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


def _data_directories(layout: ReleaseLayout, data_root: Path) -> dict[str, Path]:
    return build_persistent_data_paths(
        data_root,
        layout.network_identity,
        layout.runtime_compatibility,
    )


def _dataset_marker_content(service: str, layout: ReleaseLayout) -> str:
    identity = build_dataset_identity(service, layout.runtime_compatibility)
    return json.dumps(identity, indent=2, sort_keys=True) + "\n"


def _initialize_dataset_directory(path: Path, service: str, layout: ReleaseLayout) -> None:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)
    marker = path / DATASET_IDENTITY_FILE
    expected = _dataset_marker_content(service, layout)
    if marker.exists():
        if not marker.is_file() or marker.is_symlink():
            raise ValueError(f"dataset identity marker is not a regular file: {marker}")
        if marker.read_text(encoding="utf-8") != expected:
            raise ValueError(
                f"dataset identity mismatch for {service} at {path}; use a separate "
                "data root or rebuild the dataset"
            )
        return
    if any(path.iterdir()):
        raise ValueError(
            f"refusing to adopt non-empty unmarked {service} data directory: {path}; "
            "use the original release or an explicit reviewed migration"
        )
    _atomic_write_public(marker, expected)


def _validate_dataset_directories(layout: ReleaseLayout, env: dict[str, str]) -> None:
    layout_version = env.get("USDB_DATA_LAYOUT", "")
    if layout_version != DATA_LAYOUT_VERSION:
        raise ValueError(
            "legacy node data layout cannot be activated by this release; keep the old "
            "release or create a new node configuration and rebuild explicitly"
        )
    expected_compatibility_id = layout.runtime_compatibility["compatibility_id"]
    if env.get("USDB_RUNTIME_COMPATIBILITY_ID") != expected_compatibility_id:
        raise ValueError(
            "node runtime compatibility ID does not match this release; automatic data "
            "migration is not supported"
        )
    for key, service in PERSISTENT_DATA_SERVICES.items():
        data_dir = Path(env.get(key, "")).expanduser().resolve()
        marker = data_dir / DATASET_IDENTITY_FILE
        if not marker.is_file() or marker.is_symlink():
            raise ValueError(f"dataset identity marker is missing or invalid: {marker}")
        if marker.read_text(encoding="utf-8") != _dataset_marker_content(service, layout):
            raise ValueError(
                f"dataset identity mismatch for {service} at {data_dir}; rebuild is required"
            )


def _validate_node_config(
    layout: ReleaseLayout,
    *,
    require_runtime: bool,
    require_bitcoin_runtime: bool,
) -> None:
    network = validate_network_bundle(layout.bundle_dir)
    env = read_env(layout.node_env)
    _validate_dataset_directories(layout, env)
    expected_paths = _data_directories(layout, Path(env.get("USDB_DATA_ROOT", "")))
    validate_node_env(
        layout.node_env,
        network,
        require_runtime,
        require_bitcoin_runtime,
        expected_paths,
        layout.runtime_compatibility["compatibility_id"],
    )


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
    firewall_mode: str = "external",
    select_snapshot: bool = False,
    bitcoin_resource_profile: str = DEFAULT_BITCOIN_RESOURCE_PROFILE,
) -> Path:
    if layout.node_env.exists():
        raise ValueError(
            f"refusing to replace existing node configuration: {layout.node_env}; "
            "use set-role for role changes"
        )
    _require_role(role, miner_address, miner_threads)
    if bitcoin_p2p not in {"private", "public"}:
        raise ValueError("bitcoin P2P mode must be private or public")
    _require_firewall_mode(firewall_mode)
    _require_port("operator SSH port", ssh_port)
    bitcoin_profile, bitcoin_resources = resolve_bitcoin_resource_profile(
        bitcoin_resource_profile
    )
    _validate_data_root_capacity(data_root)
    root = data_root.expanduser().resolve()
    secure_dir = network_secure_dir(root, layout.bundle_id)
    snapshot_dir = snapshot_artifact_dir(root)
    rpcauth_path = secure_dir / "bitcoin-mainnet-rpcauth"
    data_directories = _data_directories(layout, root)
    for path, mode in ((secure_dir, 0o700), (snapshot_dir, 0o755)):
        path.mkdir(mode=mode, parents=True, exist_ok=True)
        path.chmod(mode)
    for key, path in data_directories.items():
        _initialize_dataset_directory(path, PERSISTENT_DATA_SERVICES[key], layout)

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
            "USDB_DATA_LAYOUT": DATA_LAYOUT_VERSION,
            "USDB_RUNTIME_COMPATIBILITY_ID": layout.runtime_compatibility[
                "compatibility_id"
            ],
            **{key: str(path) for key, path in data_directories.items()},
            "BTC_RPCAUTH_HOST_FILE": str(rpcauth_path),
            "BTC_RPC_USER": credentials["username"],
            "BTC_RPC_PASSWORD": credentials["password"],
            "BTC_P2P_BIND_ADDRESS": "0.0.0.0" if bitcoin_p2p == "public" else "127.0.0.1",
            "USDB_FIREWALL_MODE": firewall_mode,
            "USDB_OPERATOR_SSH_PORT": str(ssh_port),
            "BTC_CONTAINER_UID": str(os.getuid()),
            "BTC_CONTAINER_GID": str(os.getgid()),
            "BTC_RESOURCE_PROFILE": bitcoin_profile,
            "BTC_MEMORY_LIMIT": bitcoin_resources["memory_limit"],
            "BTC_MEMORY_SWAP_LIMIT": bitcoin_resources["memory_swap_limit"],
            "BTC_DBCACHE_MB": bitcoin_resources["dbcache_mb"],
            "BH_SNAPSHOT_HOST_DIR": str(snapshot_dir),
            "USDB_NODE_ROLE": role,
            "USDB_BOOTNODES": bootnodes,
            "USDB_NAT": nat,
            "USDB_MINER_ADDRESS": miner_address,
            "USDB_MINER_THREADS": str(miner_threads),
        }
        if select_snapshot:
            updates.update(_snapshot_env_updates(_approved_snapshot_record(layout)))
        content = render_env(template_path.read_text(encoding="utf-8"), updates)
        _atomic_write_private(layout.node_env, content)
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
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


def _data_root_capacity(path: Path) -> DataRootCapacity:
    candidate = path.expanduser().resolve()
    while not candidate.exists() and candidate != candidate.parent:
        candidate = candidate.parent
    usage = shutil.disk_usage(candidate)
    return DataRootCapacity(
        filesystem_path=candidate,
        total_bytes=usage.total,
        free_bytes=usage.free,
    )


def _validate_data_root_capacity(path: Path) -> DataRootCapacity:
    capacity = _data_root_capacity(path)
    if capacity.total_bytes < MIN_DATA_ROOT_BYTES:
        raise ValueError(
            f"data root filesystem is too small: {capacity.filesystem_path} has "
            f"{_human_bytes(capacity.total_bytes)} total; at least "
            f"{_human_bytes(MIN_DATA_ROOT_BYTES)} is required"
        )
    if capacity.free_bytes < MIN_DATA_ROOT_BYTES:
        raise ValueError(
            f"data root filesystem has insufficient available space: "
            f"{capacity.filesystem_path} has {_human_bytes(capacity.free_bytes)} free; "
            f"at least {_human_bytes(MIN_DATA_ROOT_BYTES)} is required before setup"
        )
    return capacity


def _disk_free_bytes(path: Path) -> int:
    return _data_root_capacity(path).free_bytes


def setup_node(
    layout: ReleaseLayout,
    *,
    input_fn: Any = input,
    output: Any = sys.stdout,
    bitcoin_resource_profile: str = DEFAULT_BITCOIN_RESOURCE_PROFILE,
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
    capacity = _validate_data_root_capacity(data_root)
    host_memory_bytes = _host_memory_bytes()
    bitcoin_profile, bitcoin_resources = resolve_bitcoin_resource_profile(
        bitcoin_resource_profile,
        host_memory_bytes=host_memory_bytes,
    )
    print("Data root filesystem:", file=output)
    print(f"  Resolved through: {capacity.filesystem_path}", file=output)
    print(f"  Total capacity: {_human_bytes(capacity.total_bytes)}", file=output)
    print(f"  Available now: {_human_bytes(capacity.free_bytes)}", file=output)
    print(
        f"  Required/recommended: {_human_bytes(MIN_DATA_ROOT_BYTES)} / "
        f"{_human_bytes(RECOMMENDED_DATA_ROOT_BYTES)}",
        file=output,
    )
    if (
        capacity.total_bytes < RECOMMENDED_DATA_ROOT_BYTES
        or capacity.free_bytes < RECOMMENDED_DATA_ROOT_BYTES
    ):
        print(
            "  Warning: filesystem meets the hard minimum but total capacity or "
            "current free space is below the recommended long-running headroom.",
            file=output,
        )
    print("Bitcoin resource profile:", file=output)
    print(f"  Selected: {bitcoin_profile}", file=output)
    print(f"  Host memory: {_human_bytes(host_memory_bytes)}", file=output)
    print(f"  Container limit: {bitcoin_resources['memory_limit']}", file=output)
    print(
        "  Memory + swap limit: "
        f"{bitcoin_resources['memory_swap_limit']}",
        file=output,
    )
    print(f"  Bitcoin dbcache: {bitcoin_resources['dbcache_mb']} MiB", file=output)
    if bitcoin_profile == IBD_BITCOIN_RESOURCE_PROFILE:
        print(
            "  Temporary profile: switch to performance-64g after IBD and txindex complete.",
            file=output,
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
    manage_firewall = _prompt_yes_no(
        "Manage this host firewall with the bundled UFW profile",
        default=False,
        input_fn=input_fn,
        output=output,
    )
    firewall_mode = "managed" if manage_firewall else "external"
    ssh_port = detect_ssh_server_port()
    if manage_firewall:
        ssh_port_text = _prompt(
            "Operator SSH server port",
            default=str(ssh_port),
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
    print(f"Host firewall: {firewall_mode}", file=output)
    if manage_firewall:
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
        firewall_mode=firewall_mode,
        select_snapshot=install_snapshot,
        bitcoin_resource_profile=bitcoin_profile,
    )
    return SetupResult(
        node_env=path,
        apply_firewall=manage_firewall,
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
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise


def set_firewall_mode(layout: ReleaseLayout, mode: str) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    _require_firewall_mode(mode)
    original = layout.node_env.read_text(encoding="utf-8")
    updated = upsert_env(original, {"USDB_FIREWALL_MODE": mode})
    try:
        _atomic_write_private(layout.node_env, updated)
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise


def set_bitcoin_resource_profile(
    layout: ReleaseLayout,
    profile: str,
) -> tuple[str, dict[str, str]]:
    """Atomically update operator-owned Bitcoin memory settings for the next reconcile."""
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    services = _collect_compose_services(layout)
    active_services = sorted(
        name
        for name, service in services.items()
        if service.get("state") not in {"exited", "dead"}
    )
    if active_services:
        raise ValueError(
            "Bitcoin resource profile can only change while all node containers are stopped; "
            "run 'usdb-node down' first; active services: "
            + ", ".join(active_services)
        )
    selected, resources = resolve_bitcoin_resource_profile(profile)
    original = layout.node_env.read_text(encoding="utf-8")
    updated = upsert_env(
        original,
        {
            "BTC_RESOURCE_PROFILE": selected,
            "BTC_MEMORY_LIMIT": resources["memory_limit"],
            "BTC_MEMORY_SWAP_LIMIT": resources["memory_swap_limit"],
            "BTC_DBCACHE_MB": resources["dbcache_mb"],
        },
    )
    try:
        _atomic_write_private(layout.node_env, updated)
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise
    return selected, resources


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
    _validate_node_config(
        layout,
        require_runtime=False,
        require_bitcoin_runtime=True,
    )
    updates = {
        **layout.images,
        "USDB_FIREWALL_MODE": configured_firewall_mode(layout),
    }
    updated = upsert_env(original, updates)
    try:
        _atomic_write_private(layout.node_env, updated)
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
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


def _bitcoin_data_start_anchor(
    layout: ReleaseLayout,
    env: dict[str, str],
) -> BitcoinDataStartAnchor:
    """Resolve the BTC boundary after which balance-history may safely start."""
    origin_height = layout.network_identity.get("btc_index_origin_height")
    if not isinstance(origin_height, int) or isinstance(origin_height, bool):
        raise ValueError("release manifest has no valid BTC index origin height")
    registry_id = layout.network_identity.get("btc_activation_registry_id")
    stable_lag = btc_registry_stable_lag_blocks(registry_id)
    snapshot_mode = env.get("SNAPSHOT_MODE", "none")
    if snapshot_mode == "none":
        stable_height = origin_height
        block_hash = None
    else:
        if snapshot_mode not in {"balance-history", "paired-checkpoint"}:
            raise ValueError(f"unsupported SNAPSHOT_MODE={snapshot_mode}")
        record = _approved_snapshot_record(layout)
        stable_height = record.get("height")
        block_hash = record.get("btc_block_hash")
        if not isinstance(stable_height, int) or isinstance(stable_height, bool):
            raise ValueError("release-approved snapshot has no valid BTC height")
        if not isinstance(block_hash, str) or SHA256_RE.fullmatch(block_hash) is None:
            raise ValueError("release-approved snapshot has no valid BTC block hash")
    minimum_tip_height = stable_height + stable_lag
    if minimum_tip_height > 0xFFFFFFFF:
        raise ValueError("BTC data-start height exceeds u32")
    return BitcoinDataStartAnchor(
        stable_height=stable_height,
        minimum_tip_height=minimum_tip_height,
        stable_lag_blocks=stable_lag,
        block_hash=block_hash,
    )


def _snapshot_env_updates(record: dict[str, Any]) -> dict[str, str]:
    by_role = {item["role"]: item for item in record["files"]}
    release_root = f"/snapshots/{record['snapshot_release_id']}"
    return {
        "SNAPSHOT_MODE": "balance-history",
        "BH_SNAPSHOT_FILE": f"{release_root}/{by_role['snapshot_db']['path']}",
        "BH_SNAPSHOT_MANIFEST": f"{release_root}/{by_role['snapshot_manifest']['path']}",
        "USDB_INDEXER_CHECKPOINT_MANIFEST": "",
    }


def select_snapshot_release(layout: ReleaseLayout) -> dict[str, Any]:
    """Persist the approved snapshot choice before any long-running download begins."""
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run setup or configure first")
    env = read_env(layout.node_env)
    balance_history_root = Path(env.get("BH_DATA_HOST_DIR", "")).expanduser().resolve()
    database = balance_history_root / "db"
    if database.is_dir() and any(database.iterdir()):
        raise ValueError(
            "refusing to change snapshot selection after balance-history DB initialization; "
            "use a fresh data root or an explicit reviewed recovery procedure"
        )
    record = _approved_snapshot_record(layout)
    updates = _snapshot_env_updates(record)
    if all(env.get(key, "") == value for key, value in updates.items()):
        return record

    original = layout.node_env.read_text(encoding="utf-8")
    try:
        _atomic_write_private(layout.node_env, render_env(original, updates))
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=False,
        )
    except BaseException:
        _atomic_write_private(layout.node_env, original)
        raise
    return record


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
    record = select_snapshot_release(layout)
    env = read_env(layout.node_env)
    snapshot_root = Path(env.get("BH_SNAPSHOT_HOST_DIR", "")).expanduser().resolve()
    network = validate_network_bundle(layout.bundle_dir)
    btc_source = network.get("btc_source")
    if not isinstance(btc_source, dict) or not isinstance(btc_source.get("index_origin_height"), int):
        raise ValueError("network bundle is missing BTC index origin")
    installed = install_snapshot_artifact(
        record_url=layout.snapshot["record"]["url"],
        destination_root=snapshot_root,
        trusted_keys=_snapshot_trusted_keys(layout),
        approved_record_path=layout.bundle_dir / layout.snapshot["record"]["path"],
        expected_network=env.get("BTC_NETWORK", "bitcoin"),
        max_height=btc_source["index_origin_height"],
        download_concurrency=download_concurrency,
        download_chunk_size_mib=download_chunk_size_mib,
    )
    if installed.release_id != record["snapshot_release_id"]:
        raise ValueError("installed snapshot release ID differs from the approved record")
    _validate_node_config(
        layout,
        require_runtime=True,
        require_bitcoin_runtime=False,
    )
    return installed.release_dir


def _helper_environment(
    layout: ReleaseLayout,
    sync_timeout_secs: int | None = None,
    *,
    quiet_progress: bool = False,
) -> dict[str, str]:
    environment = os.environ.copy()
    environment["USDB_TESTNET_BUNDLE_DIR"] = str(layout.bundle_dir)
    environment["USDB_TESTNET_NODE_ENV"] = str(layout.node_env)
    environment["USDB_TESTNET_PROJECT_NAME"] = layout.bundle_id
    environment["USDB_TESTNET_BITCOIN_PROJECT_NAME"] = f"{layout.bundle_id}-bitcoin"
    if sync_timeout_secs is not None:
        environment["BTC_READY_WAIT_TIMEOUT_SECS"] = str(sync_timeout_secs)
    if quiet_progress:
        environment["BTC_READY_PROGRESS_INTERVAL_SECS"] = "0"
        environment["USDB_READINESS_PROGRESS_INTERVAL_SECS"] = "0"
    return environment


def run_helper(
    layout: ReleaseLayout,
    helper: str,
    arguments: list[str],
    *,
    check: bool = True,
    sync_timeout_secs: int | None = None,
    capture_output: bool = False,
    output_to_stderr: bool = False,
    quiet_progress: bool = False,
    command_timeout_secs: float | None = None,
) -> subprocess.CompletedProcess[str]:
    path = layout.kit_root / "docker/scripts/tools" / helper
    if not path.is_file():
        raise ValueError(f"release node kit is missing helper: {path}")
    if capture_output and output_to_stderr:
        raise ValueError("helper output cannot be captured and redirected simultaneously")
    options: dict[str, Any] = {
        "check": check,
        "cwd": str(layout.kit_root),
        "env": _helper_environment(
            layout,
            sync_timeout_secs,
            quiet_progress=quiet_progress,
        ),
        "text": True,
        "capture_output": capture_output,
    }
    if output_to_stderr:
        options["stdout"] = sys.stderr
    if command_timeout_secs is not None:
        options["timeout"] = command_timeout_secs
    return subprocess.run([str(path), *arguments], **options)


def run_host_action(
    layout: ReleaseLayout,
    action: str,
    *,
    docker_user: str = "",
    check: bool = True,
    output_to_stderr: bool = False,
) -> subprocess.CompletedProcess[str]:
    if action not in {"check", "install"}:
        raise ValueError(f"unsupported host action: {action}")
    arguments = [action]
    if docker_user:
        arguments.extend(["--docker-user", docker_user])
    return run_helper(
        layout,
        "prepare_usdb_host.sh",
        arguments,
        check=check,
        output_to_stderr=output_to_stderr,
    )


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
    output_to_stderr: bool = False,
) -> subprocess.CompletedProcess[str]:
    if action not in {"check", "apply"}:
        raise ValueError(f"unsupported firewall action: {action}")
    if configured_firewall_mode(layout) != "managed":
        raise ValueError(
            "host firewall mode is external; use set-firewall-mode --mode managed "
            "before invoking the bundled UFW profile"
        )
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
    return run_helper(
        layout,
        "prepare_usdb_firewall.sh",
        arguments,
        output_to_stderr=output_to_stderr,
    )


def doctor(layout: ReleaseLayout, *, output_to_stderr: bool = False) -> None:
    if not layout.node_env.is_file():
        raise ValueError("node is not configured; run configure first")
    run_host_action(
        layout,
        "check",
        docker_user=default_docker_user(),
        output_to_stderr=output_to_stderr,
    )
    _validate_node_config(
        layout,
        require_runtime=True,
        require_bitcoin_runtime=True,
    )
    _validate_node_release_images(layout)
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["validate-node"],
        output_to_stderr=output_to_stderr,
    )
    if configured_firewall_mode(layout) == "managed":
        run_firewall_action(layout, "check", output_to_stderr=output_to_stderr)
    else:
        print(
            "Host firewall mode is external; skipped UFW inspection. "
            "Container bind-address validation still passed."
        )


def _print_startup_phase(
    phase: str,
    detail: str,
    *,
    output_to_stderr: bool,
) -> None:
    output = sys.stderr if output_to_stderr else sys.stdout
    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    print(f"[{timestamp}] [usdb-node] phase={phase}: {detail}", file=output, flush=True)


def start_node(
    layout: ReleaseLayout,
    *,
    sync_timeout_secs: int,
    pull: bool,
    output_to_stderr: bool = False,
    progress_monitor: NodeProgressMonitor | None = None,
) -> None:
    owns_monitor = progress_monitor is None
    monitor = progress_monitor or NodeProgressMonitor(
        layout,
        enabled=(
            not output_to_stderr and sys.stdout.isatty() and sys.stderr.isatty()
        ),
    )
    context = monitor if owns_monitor else nullcontext(monitor)
    with context:
        _start_node(
            layout,
            sync_timeout_secs=sync_timeout_secs,
            pull=pull,
            output_to_stderr=output_to_stderr,
            progress_monitor=monitor,
        )


def _start_node(
    layout: ReleaseLayout,
    *,
    sync_timeout_secs: int,
    pull: bool,
    output_to_stderr: bool,
    progress_monitor: NodeProgressMonitor,
) -> None:
    progress_monitor.set_phase("preflight")
    _print_startup_phase(
        "preflight",
        "validating host, release, configuration, data identity and network policy",
        output_to_stderr=output_to_stderr,
    )
    doctor(layout, output_to_stderr=output_to_stderr)
    if pull:
        progress_monitor.set_phase("images")
        _print_startup_phase(
            "images",
            "pulling digest-pinned Bitcoin and USDB runtime images",
            output_to_stderr=output_to_stderr,
        )
        run_helper(
            layout,
            "run_testnet_bitcoin.sh",
            ["pull"],
            output_to_stderr=output_to_stderr,
            quiet_progress=progress_monitor.enabled,
        )
        run_helper(
            layout,
            "run_testnet_runtime.sh",
            ["pull"],
            output_to_stderr=output_to_stderr,
            quiet_progress=progress_monitor.enabled,
        )
    progress_monitor.set_phase("bitcoin-start")
    _print_startup_phase(
        "bitcoin-start",
        "starting Bitcoin Core without serializing downstream historical indexing behind full IBD",
        output_to_stderr=output_to_stderr,
    )
    run_helper(
        layout,
        "run_testnet_bitcoin.sh",
        ["start"],
        sync_timeout_secs=sync_timeout_secs,
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    env = read_env(layout.node_env)
    data_start = _bitcoin_data_start_anchor(layout, env)
    progress_monitor.set_phase("balance-history")
    _print_startup_phase(
        "balance-history",
        (
            "waiting for the Bitcoin data-start anchor, then starting snapshot loader "
            f"and balance-history at stable height {data_start.stable_height} "
            f"after Bitcoin tip reaches {data_start.minimum_tip_height}"
        ),
        output_to_stderr=output_to_stderr,
    )
    up_data_args = ["up-data", str(data_start.minimum_tip_height)]
    if data_start.block_hash is not None:
        up_data_args.extend([str(data_start.stable_height), data_start.block_hash])
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        up_data_args,
        sync_timeout_secs=sync_timeout_secs,
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    origin_height = layout.network_identity["btc_index_origin_height"]
    progress_monitor.set_phase("balance-history-origin")
    _print_startup_phase(
        "balance-history-origin",
        f"waiting for query-ready balance-history state at USDB origin height {origin_height}",
        output_to_stderr=output_to_stderr,
    )
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["wait-data-origin", str(origin_height), str(sync_timeout_secs)],
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    progress_monitor.set_phase("indexer")
    _print_startup_phase(
        "indexer",
        "starting usdb-indexer while balance-history continues catching up",
        output_to_stderr=output_to_stderr,
    )
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["up-indexer", str(origin_height)],
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    progress_monitor.set_phase("final-readiness")
    _print_startup_phase(
        "final-readiness",
        "waiting for Bitcoin, balance-history and usdb-indexer consensus readiness",
        output_to_stderr=output_to_stderr,
    )
    run_helper(
        layout,
        "run_testnet_bitcoin.sh",
        ["wait"],
        sync_timeout_secs=sync_timeout_secs,
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["wait-data", str(sync_timeout_secs)],
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["wait-indexer", str(sync_timeout_secs)],
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    progress_monitor.set_phase("chain")
    _print_startup_phase(
        "chain",
        "rechecking final readiness gates and starting the USDB chain",
        output_to_stderr=output_to_stderr,
    )
    run_helper(
        layout,
        "run_testnet_runtime.sh",
        ["up-chain"],
        sync_timeout_secs=sync_timeout_secs,
        output_to_stderr=output_to_stderr,
        quiet_progress=progress_monitor.enabled,
    )
    progress_monitor.set_phase("ready")
    _print_startup_phase(
        "ready",
        "Bitcoin, balance-history, usdb-indexer and USDB chain startup gates passed",
        output_to_stderr=output_to_stderr,
    )


def _parse_compose_ps(output: str) -> list[dict[str, Any]]:
    text = output.strip()
    if not text:
        return []
    try:
        value = json.loads(text)
        if isinstance(value, dict):
            return [value]
        if isinstance(value, list) and all(isinstance(item, dict) for item in value):
            return value
    except json.JSONDecodeError:
        pass

    items: list[dict[str, Any]] = []
    for line in text.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError("Docker Compose returned invalid JSON status") from error
        if not isinstance(value, dict):
            raise ValueError("Docker Compose status entry must be an object")
        items.append(value)
    return items


def _normalize_compose_service(item: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    service = item.get("Service", item.get("service", ""))
    if not isinstance(service, str) or not service:
        raise ValueError("Docker Compose status entry is missing Service")
    state = item.get("State", item.get("state", "unknown"))
    health = item.get("Health", item.get("health", ""))
    exit_code = item.get("ExitCode", item.get("exit_code"))
    return service, {
        "state": str(state).lower(),
        "health": str(health).lower(),
        "exit_code": exit_code if isinstance(exit_code, int) else None,
    }


def _bitcoin_startup_progress(
    layout: ReleaseLayout,
    *,
    command_timeout_secs: float | None = None,
) -> dict[str, Any] | None:
    result = run_helper(
        layout,
        "run_testnet_bitcoin.sh",
        ["progress"],
        check=False,
        capture_output=True,
        command_timeout_secs=command_timeout_secs,
    )
    if result.returncode != 0:
        return None
    try:
        report = json.loads((result.stdout or "").strip())
    except json.JSONDecodeError:
        return None
    if (
        not isinstance(report, dict)
        or report.get("schema_version") != "usdb-bitcoin-readiness:v1"
        or not isinstance(report.get("ready"), bool)
        or not isinstance(report.get("blockers"), list)
    ):
        return None
    status = report.get("status")
    if status is not None and not isinstance(status, dict):
        return None
    return report


def _bitcoin_progress_summary(
    report: dict[str, Any],
    resource_profile: str | None = None,
) -> str:
    status = report.get("status")
    if not isinstance(status, dict):
        blockers = report.get("blockers", [])
        return f"readiness unavailable: {'; '.join(str(item) for item in blockers)}"
    blocks = status.get("blocks", "unknown")
    headers = status.get("headers", "unknown")
    verification = status.get("verification_progress")
    verification_text = (
        f"{verification * 100:.2f}%"
        if isinstance(verification, (int, float)) and not isinstance(verification, bool)
        else "unknown"
    )
    txindex_state = "synced" if status.get("txindex_synced") is True else "indexing"
    txindex_height = status.get("txindex_height", "unknown")
    connections = status.get("connections", "unknown")
    profile_text = f", profile={resource_profile}" if resource_profile else ""
    return (
        f"blocks={blocks}/{headers}, verification={verification_text}, "
        f"txindex={txindex_state}@{txindex_height}, peers={connections}{profile_text}"
    )


def _snapshot_staging_bytes(staging: Path, record: dict[str, Any]) -> int:
    completed_bytes = 0
    for item in record["files"]:
        expected_size = item["size"]
        final_path = staging / item["path"]
        if final_path.is_file() and not final_path.is_symlink():
            completed_bytes += min(final_path.stat().st_size, expected_size)
            continue

        part_path = final_path.with_name(final_path.name + ".part")
        if not part_path.is_file() or part_path.is_symlink():
            continue
        range_state_path = part_path.with_name(part_path.name + ".ranges.json")
        if not range_state_path.is_file() or range_state_path.is_symlink():
            completed_bytes += min(part_path.stat().st_size, expected_size)
            continue
        try:
            state = _load_json(range_state_path)
            chunk_size = state.get("chunk_size_bytes")
            completed_chunks = state.get("completed_chunks")
            if (
                not isinstance(chunk_size, int)
                or isinstance(chunk_size, bool)
                or chunk_size <= 0
                or not isinstance(completed_chunks, list)
            ):
                continue
            chunk_count = (expected_size + chunk_size - 1) // chunk_size
            indexes = {
                index
                for index in completed_chunks
                if isinstance(index, int)
                and not isinstance(index, bool)
                and 0 <= index < chunk_count
            }
            completed_bytes += sum(
                min(chunk_size, expected_size - index * chunk_size)
                for index in indexes
            )
        except (OSError, ValueError):
            continue
    return completed_bytes


def _snapshot_lifecycle_status(
    layout: ReleaseLayout,
    env: dict[str, str],
) -> dict[str, Any]:
    mode = env.get("SNAPSHOT_MODE", "none")
    if mode == "none":
        return {
            "state": "not_selected",
            "summary": "no snapshot selected; full synchronization is configured",
            "mode": mode,
        }
    if mode != "balance-history":
        return {
            "state": "configured",
            "summary": f"{mode} snapshot is configured; deep validation remains in doctor",
            "mode": mode,
        }

    try:
        record = _approved_snapshot_record(layout)
    except (OSError, ValueError) as error:
        return {
            "state": "invalid",
            "summary": str(error),
            "mode": mode,
        }
    expected = _snapshot_env_updates(record)
    expected_bytes = sum(item["size"] for item in record["files"])
    for key in ("BH_SNAPSHOT_FILE", "BH_SNAPSHOT_MANIFEST"):
        if env.get(key) != expected[key]:
            return {
                "state": "invalid",
                "summary": f"{key} does not select the release-approved snapshot",
                "mode": mode,
                "snapshot_release_id": record["snapshot_release_id"],
            }

    root = Path(env.get("BH_SNAPSHOT_HOST_DIR", "")).expanduser().resolve()
    release_id = record["snapshot_release_id"]
    destination = root / release_id
    staging = root / f".{release_id}.installing"
    if destination.exists():
        if not destination.is_dir() or destination.is_symlink():
            return {
                "state": "invalid",
                "summary": "snapshot final path is not a regular directory",
                "mode": mode,
                "snapshot_release_id": release_id,
            }
        approved_record = layout.bundle_dir / layout.snapshot["record"]["path"]
        installed_record = destination / "snapshot-release-record.json"
        if (
            not installed_record.is_file()
            or installed_record.is_symlink()
            or installed_record.read_bytes() != approved_record.read_bytes()
        ):
            return {
                "state": "invalid",
                "summary": "snapshot installed record is missing or differs from the release record",
                "mode": mode,
                "snapshot_release_id": release_id,
            }
        for item in record["files"]:
            path = destination / item["path"]
            if not path.is_file() or path.is_symlink() or path.stat().st_size != item["size"]:
                return {
                    "state": "invalid",
                    "summary": f"snapshot file is missing or has the wrong size: {item['path']}",
                    "mode": mode,
                    "snapshot_release_id": release_id,
                }
        return {
            "state": "installed",
            "summary": f"release-approved snapshot is installed at BTC height {record['height']}",
            "mode": mode,
            "snapshot_release_id": release_id,
            "height": record["height"],
            "completed_bytes": expected_bytes,
            "expected_bytes": expected_bytes,
        }

    if staging.exists():
        if not staging.is_dir() or staging.is_symlink():
            return {
                "state": "invalid",
                "summary": "snapshot staging path is not a regular directory",
                "mode": mode,
                "snapshot_release_id": release_id,
            }
        present_files = sum(
            1
            for item in record["files"]
            if (staging / item["path"]).is_file()
        )
        completed_bytes = _snapshot_staging_bytes(staging, record)
        return {
            "state": "incomplete",
            "summary": (
                "snapshot verification is in progress"
                if completed_bytes >= expected_bytes
                else "snapshot download is resumable"
            ),
            "mode": mode,
            "snapshot_release_id": release_id,
            "complete_file_count": present_files,
            "expected_file_count": len(record["files"]),
            "completed_bytes": completed_bytes,
            "expected_bytes": expected_bytes,
        }
    return {
        "state": "incomplete",
        "summary": "snapshot is selected but its local artifact is missing",
        "mode": mode,
        "snapshot_release_id": release_id,
        "complete_file_count": 0,
        "expected_file_count": len(record["files"]),
        "completed_bytes": 0,
        "expected_bytes": expected_bytes,
    }


def _collect_compose_services(
    layout: ReleaseLayout,
    *,
    command_timeout_secs: float | None = None,
) -> dict[str, dict[str, Any]]:
    commands = (
        ("run_testnet_bitcoin.sh", ["ps", "--all", "--format", "json"]),
        ("run_testnet_runtime.sh", ["ps", "--all", "--format", "json"]),
    )
    services: dict[str, dict[str, Any]] = {}
    for helper, arguments in commands:
        result = run_helper(
            layout,
            helper,
            arguments,
            check=False,
            capture_output=True,
            command_timeout_secs=command_timeout_secs,
        )
        if result.returncode != 0:
            raise ValueError("Docker Compose service state could not be read")
        for item in _parse_compose_ps(result.stdout or ""):
            service, status = _normalize_compose_service(item)
            services[service] = status
    return services


def _runtime_lifecycle_status(layout: ReleaseLayout) -> dict[str, Any]:
    try:
        services = _collect_compose_services(layout)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return {
            "state": "unavailable",
            "summary": "Docker Compose service state could not be read",
            "services": {},
        }

    present_core = [name for name in CORE_RUNTIME_SERVICES if name in services]
    if not present_core:
        return {
            "state": "stopped",
            "summary": "USDB services have not been started",
            "services": services,
        }

    problem_states = {"dead", "exited", "removing", "restarting"}
    failed_processes = [
        name
        for name in CORE_RUNTIME_SERVICES
        if name in services
        and services[name]["state"] in problem_states
    ]
    if failed_processes:
        return {
            "state": "degraded",
            "summary": f"core service processes failed: {', '.join(failed_processes)}",
            "services": services,
        }

    missing = [name for name in CORE_RUNTIME_SERVICES if name not in services]
    not_running = [
        name
        for name in CORE_RUNTIME_SERVICES
        if name in services and services[name]["state"] != "running"
    ]
    health_starting = [
        name
        for name in CORE_RUNTIME_SERVICES
        if name in services and services[name]["health"] == "starting"
    ]
    unhealthy = [
        name
        for name in CORE_RUNTIME_SERVICES
        if name in services and services[name]["health"] == "unhealthy"
    ]
    if missing or not_running or health_starting:
        pending = [*missing, *not_running, *health_starting, *unhealthy]
        result: dict[str, Any] = {
            "state": "starting",
            "summary": f"waiting for core services: {', '.join(dict.fromkeys(pending))}",
            "services": services,
        }
        bitcoin = services.get("btc-node")
        if (
            bitcoin is not None
            and bitcoin["state"] == "running"
            and bitcoin["health"] == "unhealthy"
        ):
            progress = _bitcoin_startup_progress(layout)
            if progress is not None:
                result["bitcoin_progress"] = progress
        return result
    if unhealthy:
        return {
            "state": "degraded",
            "summary": f"core services are unhealthy: {', '.join(unhealthy)}",
            "services": services,
        }

    readiness_commands = (
        ("btc-node", "run_testnet_bitcoin.sh", ["status"]),
        ("balance-history", "run_testnet_runtime.sh", ["data-status"]),
        ("usdb-indexer", "run_testnet_runtime.sh", ["indexer-status"]),
    )
    readiness: dict[str, str] = {}
    for service, helper, arguments in readiness_commands:
        result = run_helper(
            layout,
            helper,
            arguments,
            check=False,
            capture_output=True,
        )
        readiness[service] = "ready" if result.returncode == 0 else "not_ready"
    failed = [service for service, state in readiness.items() if state != "ready"]
    if failed:
        return {
            "state": "degraded",
            "summary": f"service readiness checks failed: {', '.join(failed)}",
            "services": services,
            "readiness": readiness,
        }
    return {
        "state": "ready",
        "summary": "all core services are running and ready",
        "services": services,
        "readiness": readiness,
    }


def _component_progress(
    component_id: str,
    state: str,
    detail: str,
    *,
    current: int | None = None,
    total: int | None = None,
    progress_percent: float | None = None,
    unit: str = "blocks",
) -> dict[str, Any]:
    label = dict(PROGRESS_COMPONENTS)[component_id]
    if progress_percent is None and current is not None and total is not None and total > 0:
        progress_percent = min(100.0, max(0.0, current * 100.0 / total))
    return {
        "id": component_id,
        "label": label,
        "state": state,
        "detail": detail,
        "current": current,
        "total": total,
        "progress_percent": progress_percent,
        "unit": unit,
    }


def _directory_has_entries(path: Path) -> bool:
    try:
        return path.is_dir() and next(path.iterdir(), None) is not None
    except OSError:
        return False


def _snapshot_import_state(env: dict[str, str]) -> dict[str, Any]:
    root_value = env.get("BH_DATA_HOST_DIR", "")
    if not root_value:
        return {"state": "invalid", "summary": "BH_DATA_HOST_DIR is not configured"}
    root = Path(root_value).expanduser().resolve()
    marker_path = root / "bootstrap/snapshot-loader.done.json"
    db_has_entries = _directory_has_entries(root / "db")
    if not marker_path.exists():
        return {
            "state": "unmarked",
            "summary": "live RocksDB snapshot import marker is absent",
            "db_has_entries": db_has_entries,
        }
    if not marker_path.is_file() or marker_path.is_symlink():
        return {
            "state": "invalid",
            "summary": "snapshot import marker is not a regular file",
            "db_has_entries": db_has_entries,
        }
    try:
        marker = _load_json(marker_path)
    except (OSError, ValueError) as error:
        return {
            "state": "invalid",
            "summary": f"snapshot import marker is invalid: {error}",
            "db_has_entries": db_has_entries,
        }
    expected = {
        "snapshot_mode": env.get("SNAPSHOT_MODE", "none"),
        "snapshot_file": env.get("BH_SNAPSHOT_FILE", ""),
        "snapshot_manifest": env.get("BH_SNAPSHOT_MANIFEST", ""),
    }
    for key, value in expected.items():
        if marker.get(key) != value:
            return {
                "state": "invalid",
                "summary": f"snapshot import marker {key} does not match node configuration",
                "db_has_entries": db_has_entries,
            }
    if not db_has_entries:
        return {
            "state": "invalid",
            "summary": "snapshot import marker exists but live RocksDB is empty",
            "db_has_entries": False,
        }
    return {
        "state": "installed",
        "summary": "snapshot is imported into live balance-history RocksDB",
        "db_has_entries": True,
        "installed_at": marker.get("installed_at"),
    }


def _read_snapshot_import_progress(env: dict[str, str]) -> dict[str, Any]:
    root_value = env.get("BH_DATA_HOST_DIR", "")
    if not root_value:
        raise ValueError("BH_DATA_HOST_DIR is not configured")
    progress_path = (
        Path(root_value).expanduser().resolve()
        / "bootstrap/snapshot-loader.progress.json"
    )
    if not progress_path.is_file() or progress_path.is_symlink():
        raise ValueError("snapshot import progress is not available yet")
    progress = _load_json(progress_path)
    if progress.get("schema_version") != SNAPSHOT_IMPORT_PROGRESS_SCHEMA_VERSION:
        raise ValueError("snapshot import progress has an unsupported schema")
    if progress.get("snapshot_file") != env.get("BH_SNAPSHOT_FILE", ""):
        raise ValueError("snapshot import progress does not match the selected artifact")
    if progress.get("state") not in {"running", "complete"}:
        raise ValueError("snapshot import progress has an invalid state")
    if not isinstance(progress.get("stage"), str) or not progress["stage"]:
        raise ValueError("snapshot import progress has no stage")
    for key in (
        "stage_index",
        "stage_count",
        "current",
        "total",
        "stage_current",
        "stage_total",
        "updated_at_unix",
    ):
        value = progress.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise ValueError(f"snapshot import progress has invalid {key}")
    if (
        progress["stage_index"] == 0
        or progress["stage_count"] == 0
        or progress["stage_index"] > progress["stage_count"]
    ):
        raise ValueError("snapshot import progress has an invalid stage range")
    if progress["total"] > 0 and progress["current"] > progress["total"]:
        raise ValueError("snapshot import progress exceeds its aggregate total")
    if (
        progress["stage_total"] > 0
        and progress["stage_current"] > progress["stage_total"]
    ):
        raise ValueError("snapshot import progress exceeds its stage total")
    if progress.get("unit") not in {"bytes", "entries"}:
        raise ValueError("snapshot import progress has an invalid unit")
    if not isinstance(progress.get("message"), str):
        raise ValueError("snapshot import progress has an invalid message")
    return progress


def _snapshot_component(
    snapshot: dict[str, Any],
    env: dict[str, str],
    loader_service: dict[str, Any] | None,
) -> dict[str, Any]:
    state = snapshot["state"]
    if state == "not_selected":
        return _component_progress("snapshot", "SKIPPED", snapshot["summary"])
    if state == "invalid":
        return _component_progress("snapshot", "BLOCKED", snapshot["summary"])
    if state == "incomplete":
        current = snapshot.get("completed_bytes")
        total = snapshot.get("expected_bytes")
        progress_state = (
            "VERIFYING"
            if isinstance(current, int)
            and isinstance(total, int)
            and total > 0
            and current >= total
            else "INSTALLING"
        )
        return _component_progress(
            "snapshot",
            progress_state,
            snapshot["summary"],
            current=current if isinstance(current, int) else None,
            total=total if isinstance(total, int) else None,
            unit="bytes",
        )
    if state not in {"installed", "configured"}:
        return _component_progress("snapshot", "WAITING", snapshot["summary"])

    import_state = _snapshot_import_state(env)
    if import_state["state"] == "installed":
        return _component_progress(
            "snapshot",
            "READY",
            import_state["summary"],
            current=snapshot.get("completed_bytes"),
            total=snapshot.get("expected_bytes"),
            unit="bytes",
        )
    if import_state["state"] == "invalid":
        return _component_progress("snapshot", "BLOCKED", import_state["summary"])

    loader_state = loader_service.get("state") if loader_service is not None else None
    if loader_state == "running":
        try:
            progress = _read_snapshot_import_progress(env)
        except (OSError, ValueError) as error:
            return _component_progress(
                "snapshot",
                "IMPORTING",
                f"artifact verified; RocksDB import started; {error}",
            )
        stage = progress["stage"].replace("_", " ")
        detail = (
            f"stage={stage} ({progress['stage_index']}/{progress['stage_count']}), "
            f"stage_progress={progress['stage_current']}/{progress['stage_total']}; "
            f"{progress['message']}"
        )
        component = _component_progress(
            "snapshot",
            "IMPORTING",
            detail,
            current=progress["current"],
            total=progress["total"],
            unit=progress["unit"],
        )
        component["stage"] = progress["stage"]
        component["stage_index"] = progress["stage_index"]
        component["stage_count"] = progress["stage_count"]
        component["stage_current"] = progress["stage_current"]
        component["stage_total"] = progress["stage_total"]
        component["updated_at_unix"] = progress["updated_at_unix"]
        return component
    if loader_state in {"dead", "exited", "removing", "restarting"}:
        exit_code = loader_service.get("exit_code") if loader_service is not None else None
        try:
            last_progress = _read_snapshot_import_progress(env)
            last_stage = f"; last_stage={last_progress['stage']}"
        except (OSError, ValueError):
            last_stage = ""
        if loader_state == "exited" and exit_code == 0:
            return _component_progress(
                "snapshot",
                "BLOCKED",
                "snapshot-loader exited successfully but the live RocksDB marker is absent"
                + last_stage,
            )
        suffix = f", exit_code={exit_code}" if exit_code is not None else ""
        return _component_progress(
            "snapshot",
            "FAILED",
            f"snapshot-loader state={loader_state}{suffix}; "
            f"live RocksDB import is incomplete{last_stage}",
        )
    if import_state.get("db_has_entries") is True:
        return _component_progress(
            "snapshot",
            "BLOCKED",
            "live balance-history RocksDB exists without a matching snapshot import marker",
        )
    return _component_progress(
        "snapshot",
        "WAITING",
        "artifact verified; RocksDB import runs after Bitcoin reaches the signed data-start anchor",
        current=snapshot.get("completed_bytes"),
        total=snapshot.get("expected_bytes"),
        unit="bytes",
    )


def _failed_container_component(
    component_id: str,
    service: dict[str, Any] | None,
    waiting_detail: str,
) -> dict[str, Any] | None:
    if service is None:
        return _component_progress(component_id, "WAITING", waiting_detail)
    state = service["state"]
    if state in {"dead", "exited", "removing", "restarting"}:
        exit_code = service.get("exit_code")
        suffix = f", exit_code={exit_code}" if exit_code is not None else ""
        return _component_progress(
            component_id,
            "FAILED",
            f"container state={state}{suffix}",
        )
    if state != "running":
        return _component_progress(component_id, "STARTING", f"container state={state}")
    return None


def _read_service_readiness(
    layout: ReleaseLayout,
    helper: str,
    arguments: list[str],
    expected_service: str,
) -> tuple[dict[str, Any] | None, str | None]:
    try:
        result = run_helper(
            layout,
            helper,
            arguments,
            check=False,
            capture_output=True,
            command_timeout_secs=8,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return None, str(error)
    if result.returncode != 0:
        detail = (result.stderr or "").strip().splitlines()
        return None, detail[-1] if detail else "readiness RPC is unavailable"
    try:
        readiness = json.loads((result.stdout or "").strip())
    except json.JSONDecodeError:
        return None, "readiness RPC returned invalid JSON"
    if not isinstance(readiness, dict):
        return None, "readiness RPC result is not an object"
    if readiness.get("service") != expected_service:
        return None, "readiness RPC returned the wrong service identity"
    if not isinstance(readiness.get("consensus_ready"), bool):
        return None, "readiness RPC omitted consensus_ready"
    return readiness, None


def _indexed_service_component(
    component_id: str,
    service: dict[str, Any] | None,
    readiness: dict[str, Any] | None,
    error: str | None,
    waiting_detail: str,
) -> dict[str, Any]:
    container_state = _failed_container_component(component_id, service, waiting_detail)
    if container_state is not None:
        return container_state
    if readiness is None:
        return _component_progress(
            component_id,
            "STARTING",
            error or "waiting for readiness RPC",
        )

    current = readiness.get("current")
    total = readiness.get("total")
    current_value = current if isinstance(current, int) and not isinstance(current, bool) else None
    total_value = total if isinstance(total, int) and not isinstance(total, bool) else None
    detail_parts: list[str] = []
    phase = readiness.get("phase")
    if isinstance(phase, str) and phase:
        detail_parts.append(f"phase={phase}")
    message = readiness.get("message")
    if isinstance(message, str) and message:
        detail_parts.append(message)
    blockers = readiness.get("blockers")
    if isinstance(blockers, list) and blockers:
        detail_parts.append("blockers=" + ",".join(str(item) for item in blockers))
    state = "READY" if readiness["consensus_ready"] else "SYNCING"
    return _component_progress(
        component_id,
        state,
        "; ".join(detail_parts) or "readiness state received",
        current=current_value,
        total=total_value,
    )


def _host_rpc_url(env: dict[str, str], address_key: str, port_key: str, default_port: int) -> str:
    address = env.get(address_key, "127.0.0.1") or "127.0.0.1"
    if address == "0.0.0.0":
        address = "127.0.0.1"
    elif address == "::":
        address = "::1"
    if ":" in address and not address.startswith("["):
        address = f"[{address}]"
    port = env.get(port_key, str(default_port)) or str(default_port)
    return f"http://{address}:{port}"


def _json_rpc_batch(
    url: str,
    calls: tuple[tuple[str, list[Any]], ...],
    *,
    timeout_secs: float = 3,
) -> dict[str, Any]:
    payload = [
        {"jsonrpc": "2.0", "id": index, "method": method, "params": params}
        for index, (method, params) in enumerate(calls, start=1)
    ]
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_secs) as response:
            values = json.load(response)
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
        raise ValueError(f"USDB chain RPC is unavailable: {error}") from error
    if not isinstance(values, list):
        raise ValueError("USDB chain RPC batch response must be a list")
    by_id: dict[int, Any] = {}
    for value in values:
        if not isinstance(value, dict) or not isinstance(value.get("id"), int):
            raise ValueError("USDB chain RPC returned an invalid batch item")
        if value.get("error") is not None:
            raise ValueError(f"USDB chain RPC returned an error: {value['error']}")
        by_id[value["id"]] = value.get("result")
    if set(by_id) != set(range(1, len(calls) + 1)):
        raise ValueError("USDB chain RPC returned an incomplete batch")
    return {method: by_id[index] for index, (method, _params) in enumerate(calls, start=1)}


def _hex_quantity(value: Any, label: str) -> int:
    if not isinstance(value, str) or re.fullmatch(r"0x(?:0|[1-9a-fA-F][0-9a-fA-F]*)", value) is None:
        raise ValueError(f"{label} is not a canonical hex quantity")
    return int(value, 16)


def _chain_component(
    layout: ReleaseLayout,
    env: dict[str, str],
    service: dict[str, Any] | None,
) -> dict[str, Any]:
    container_state = _failed_container_component(
        "usdb_chain",
        service,
        "waiting for the USDB indexer readiness gate",
    )
    if container_state is not None:
        return container_state
    try:
        results = _json_rpc_batch(
            _host_rpc_url(env, "USDB_HTTP_BIND_ADDRESS", "USDB_HTTP_BIND_PORT", 8545),
            (
                ("eth_chainId", []),
                ("eth_getBlockByNumber", ["0x0", False]),
                ("eth_syncing", []),
                ("eth_blockNumber", []),
                ("net_peerCount", []),
            ),
        )
        chain_id = _hex_quantity(results["eth_chainId"], "eth_chainId")
        block_number = _hex_quantity(results["eth_blockNumber"], "eth_blockNumber")
        peer_count = _hex_quantity(results["net_peerCount"], "net_peerCount")
        genesis = results["eth_getBlockByNumber"]
        if not isinstance(genesis, dict) or not isinstance(genesis.get("hash"), str):
            raise ValueError("USDB chain genesis block is unavailable")
        expected_chain_id = layout.network_identity["chain_id"]
        expected_genesis = layout.network_identity["genesis_block_hash"]
        if chain_id != expected_chain_id or genesis["hash"].lower() != expected_genesis.lower():
            return _component_progress(
                "usdb_chain",
                "BLOCKED",
                "chain identity differs from the release manifest",
            )
        syncing = results["eth_syncing"]
        if syncing is False:
            return _component_progress(
                "usdb_chain",
                "READY",
                f"block={block_number}, peers={peer_count}",
                current=block_number,
            )
        if not isinstance(syncing, dict):
            raise ValueError("eth_syncing returned an invalid result")
        current = _hex_quantity(syncing.get("currentBlock"), "eth_syncing.currentBlock")
        highest = _hex_quantity(syncing.get("highestBlock"), "eth_syncing.highestBlock")
        return _component_progress(
            "usdb_chain",
            "SYNCING",
            f"peers={peer_count}",
            current=current,
            total=highest,
        )
    except ValueError as error:
        return _component_progress("usdb_chain", "STARTING", str(error))


def _overall_progress_state(components: list[dict[str, Any]]) -> str:
    states = {component["state"] for component in components}
    for state in (
        "FAILED",
        "BLOCKED",
        "VERIFYING",
        "INSTALLING",
        "IMPORTING",
        "SYNCING",
        "STARTING",
    ):
        if state in states:
            return state
    if states <= {"READY", "SKIPPED"}:
        return "READY"
    return "WAITING"


def collect_node_progress(layout: ReleaseLayout) -> dict[str, Any]:
    observed_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    controller_state = controller_observed_state(layout)
    if not layout.node_env.is_file():
        components = [
            _component_progress(component_id, "WAITING", "node configuration is missing")
            for component_id, _label in PROGRESS_COMPONENTS
        ]
        return {
            "schema_version": NODE_PROGRESS_SCHEMA_VERSION,
            "release_id": layout.release_id,
            "network_bundle_id": layout.bundle_id,
            "observed_at": observed_at,
            "controller_state": controller_state,
            "overall_state": "WAITING",
            "components": components,
        }
    try:
        env = read_env(layout.node_env)
        snapshot_lifecycle = _snapshot_lifecycle_status(layout, env)
    except (OSError, ValueError) as error:
        env = {}
        snapshot_lifecycle = {
            "state": "invalid",
            "summary": str(error),
        }

    try:
        services = _collect_compose_services(layout, command_timeout_secs=8)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        snapshot_component = _snapshot_component(snapshot_lifecycle, env, None)
        components = [snapshot_component]
        components.extend(
            _component_progress(component_id, "BLOCKED", str(error))
            for component_id in ("bitcoin", "balance_history", "usdb_indexer", "usdb_chain")
        )
        return {
            "schema_version": NODE_PROGRESS_SCHEMA_VERSION,
            "release_id": layout.release_id,
            "network_bundle_id": layout.bundle_id,
            "observed_at": observed_at,
            "controller_state": controller_state,
            "overall_state": _overall_progress_state(components),
            "components": components,
        }

    snapshot_component = _snapshot_component(
        snapshot_lifecycle,
        env,
        services.get("snapshot-loader"),
    )

    bitcoin_service = services.get("btc-node")
    bitcoin: dict[str, Any] | None = None
    bitcoin_component = _failed_container_component(
        "bitcoin",
        bitcoin_service,
        "waiting for Bitcoin Core startup",
    )
    if bitcoin_component is None:
        try:
            bitcoin = _bitcoin_startup_progress(layout, command_timeout_secs=8)
        except (OSError, subprocess.TimeoutExpired):
            bitcoin = None
        if bitcoin is None:
            bitcoin_component = _component_progress(
                "bitcoin",
                "STARTING",
                "container is running; height is unavailable until the Bitcoin readiness RPC responds",
            )
        else:
            status = bitcoin.get("status")
            if not isinstance(status, dict):
                bitcoin_component = _component_progress(
                    "bitcoin",
                    "STARTING",
                    _bitcoin_progress_summary(
                        bitcoin,
                        env.get("BTC_RESOURCE_PROFILE", DEFAULT_BITCOIN_RESOURCE_PROFILE),
                    ),
                )
            else:
                verification = status.get("verification_progress")
                bitcoin_component = _component_progress(
                    "bitcoin",
                    "READY" if bitcoin["ready"] else "SYNCING",
                    _bitcoin_progress_summary(
                        bitcoin,
                        env.get("BTC_RESOURCE_PROFILE", DEFAULT_BITCOIN_RESOURCE_PROFILE),
                    ),
                    current=status.get("blocks") if isinstance(status.get("blocks"), int) else None,
                    total=status.get("headers") if isinstance(status.get("headers"), int) else None,
                    progress_percent=(
                        verification * 100
                        if isinstance(verification, (int, float))
                        and not isinstance(verification, bool)
                        else None
                    ),
                )

    balance_service = services.get("balance-history")
    balance_readiness: dict[str, Any] | None = None
    balance_error: str | None = None
    if balance_service is not None and balance_service["state"] == "running":
        balance_readiness, balance_error = _read_service_readiness(
            layout,
            "run_testnet_runtime.sh",
            ["data-status"],
            "balance-history",
        )
    try:
        data_start = _bitcoin_data_start_anchor(layout, env)
        data_start_detail = (
            f"waiting for Bitcoin tip {data_start.minimum_tip_height}; stable anchor "
            f"{data_start.stable_height} uses lag {data_start.stable_lag_blocks}"
        )
        if data_start.block_hash is not None:
            data_start_detail += " with the signed snapshot block-hash match"
    except ValueError as error:
        data_start_detail = f"Bitcoin data-start gate is invalid: {error}"
    balance_component = _indexed_service_component(
        "balance_history",
        balance_service,
        balance_readiness,
        balance_error,
        data_start_detail,
    )

    indexer_service = services.get("usdb-indexer")
    indexer_readiness: dict[str, Any] | None = None
    indexer_error: str | None = None
    if indexer_service is not None and indexer_service["state"] == "running":
        indexer_readiness, indexer_error = _read_service_readiness(
            layout,
            "run_testnet_runtime.sh",
            ["indexer-status"],
            "usdb-indexer",
        )
    origin_height = layout.network_identity.get("btc_index_origin_height")
    indexer_wait_detail = (
        "waiting for query-ready balance-history state at USDB origin height "
        f"{origin_height if isinstance(origin_height, int) else 'unknown'}"
    )
    indexer_component = _indexed_service_component(
        "usdb_indexer",
        indexer_service,
        indexer_readiness,
        indexer_error,
        indexer_wait_detail,
    )
    chain_component = _chain_component(layout, env, services.get("usdb-chain"))
    components = [
        snapshot_component,
        bitcoin_component,
        balance_component,
        indexer_component,
        chain_component,
    ]
    return {
        "schema_version": NODE_PROGRESS_SCHEMA_VERSION,
        "release_id": layout.release_id,
        "network_bundle_id": layout.bundle_id,
        "observed_at": observed_at,
        "controller_state": controller_state,
        "overall_state": _overall_progress_state(components),
        "components": components,
    }


def _human_size(value: int) -> str:
    size = float(max(0, value))
    for unit in ("B", "KiB", "MiB", "GiB"):
        if size < 1024:
            return f"{size:.1f}{unit}"
        size /= 1024
    return f"{size:.1f}TiB"


def render_node_progress(
    report: dict[str, Any],
    *,
    phase: str = "observe",
    width: int = 120,
) -> str:
    lines = [
        f"USDB node progress | {report['release_id']} | phase={phase}",
        f"Observed {report['observed_at']} | overall={report['overall_state']} | "
        f"controller={report.get('controller_state', 'unknown')}",
    ]
    bar_width = 24
    for component in report["components"]:
        percent = component.get("progress_percent")
        if isinstance(percent, (int, float)) and not isinstance(percent, bool):
            bounded = min(100.0, max(0.0, float(percent)))
            filled = min(bar_width, int(bounded * bar_width / 100))
            bar = "#" * filled + "-" * (bar_width - filled)
            percent_text = f"{bounded:6.2f}%"
        else:
            bar = "-" * bar_width
            percent_text = "    -- "
        current = component.get("current")
        total = component.get("total")
        progress_text = ""
        if isinstance(current, int) and isinstance(total, int):
            if component.get("unit") == "bytes":
                progress_text = f" {_human_size(current)}/{_human_size(total)}"
            else:
                progress_text = f" {current}/{total}"
        elif isinstance(current, int):
            progress_text = f" {current}"
        prefix = (
            f"{component['label']:<17} {component['state']:<10} "
            f"[{bar}] {percent_text}{progress_text} "
        )
        available = max(20, width - len(prefix))
        detail = component["detail"]
        if len(detail) > available:
            detail = detail[: max(0, available - 3)] + "..."
        lines.append(prefix + detail)
    return "\n".join(lines)


def _terminal_refresh_supported(output: Any) -> bool:
    try:
        is_tty = output.isatty()
    except (AttributeError, OSError):
        return False
    terminal = os.environ.get("TERM", "").strip().lower()
    return bool(is_tty and terminal not in {"", "dumb", "unknown"})


class TerminalProgressDisplay:
    """Render a live dashboard in an alternate screen when the TTY supports it."""

    def __init__(self, output: Any, *, enabled: bool = True) -> None:
        self.output = output
        self.live = enabled and _terminal_refresh_supported(output)
        self._active = False

    def start(self) -> None:
        if not self.live or self._active:
            return
        self.output.write(ALT_SCREEN_ENTER + CURSOR_HIDE)
        self.output.flush()
        self._active = True

    def render(self, content: str) -> None:
        prefix = SCREEN_CLEAR if self._active else ""
        self.output.write(prefix + content.rstrip() + "\n")
        self.output.flush()

    def close(self) -> None:
        if not self._active:
            return
        self.output.write(CURSOR_SHOW + ALT_SCREEN_EXIT)
        self.output.flush()
        self._active = False


class NodeProgressHistory:
    """Retain bounded last-good values for transient display-only RPC failures."""

    _COMPONENT_IDS = frozenset(
        {"bitcoin", "balance_history", "usdb_indexer", "usdb_chain"}
    )

    def __init__(self, max_stale_age_secs: float = MAX_STALE_PROGRESS_AGE_SECS) -> None:
        if max_stale_age_secs <= 0:
            raise ValueError("maximum stale progress age must be positive")
        self.max_stale_age_secs = max_stale_age_secs
        self._last_good: dict[str, tuple[float, str, dict[str, Any]]] = {}

    @staticmethod
    def _has_progress(component: dict[str, Any]) -> bool:
        return any(
            isinstance(component.get(key), (int, float))
            and not isinstance(component.get(key), bool)
            for key in ("current", "total", "progress_percent")
        )

    def apply(
        self,
        report: dict[str, Any],
        *,
        observed_monotonic: float | None = None,
    ) -> dict[str, Any]:
        """Return a display copy with recent progress retained during STARTING probes."""
        now = time.monotonic() if observed_monotonic is None else observed_monotonic
        observed_at = str(report.get("observed_at", "unknown"))
        components: list[dict[str, Any]] = []
        for original in report["components"]:
            component = dict(original)
            component_id = component.get("id")
            if component_id not in self._COMPONENT_IDS:
                components.append(component)
                continue

            state = component.get("state")
            if state in {"SYNCING", "READY"} and self._has_progress(component):
                self._last_good[component_id] = (now, observed_at, dict(component))
            elif state in {"WAITING", "BLOCKED", "FAILED"}:
                self._last_good.pop(component_id, None)
            elif state == "STARTING" and not self._has_progress(component):
                cached = self._last_good.get(component_id)
                if cached is not None:
                    cached_at, cached_observed_at, cached_component = cached
                    if now - cached_at <= self.max_stale_age_secs:
                        for key in ("current", "total", "progress_percent", "unit"):
                            if cached_component.get(key) is not None:
                                component[key] = cached_component[key]
                        component["detail"] = (
                            f"STALE from {cached_observed_at}: {cached_component['detail']}; "
                            f"latest probe: {component['detail']}"
                        )
                    else:
                        self._last_good.pop(component_id, None)
            components.append(component)

        merged = dict(report)
        merged["components"] = components
        return merged


class NodeProgressMonitor:
    """Render read-only node progress without participating in startup decisions."""

    def __init__(
        self,
        layout: ReleaseLayout,
        *,
        enabled: bool,
        refresh_secs: float = DEFAULT_PROGRESS_REFRESH_SECS,
        output: Any | None = None,
    ) -> None:
        self.layout = layout
        self.refresh_secs = refresh_secs
        self.output = sys.stderr if output is None else output
        self._display = TerminalProgressDisplay(self.output, enabled=enabled)
        self.enabled = self._display.live
        self._phase = "startup"
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._refresh = threading.Event()
        self._thread: threading.Thread | None = None
        self._latest: dict[str, Any] | None = None
        self._history = NodeProgressHistory()

    def __enter__(self) -> NodeProgressMonitor:
        if self.enabled:
            self._display.start()
            try:
                self._thread = threading.Thread(
                    target=self._run,
                    name="usdb-node-progress",
                    daemon=False,
                )
                self._thread.start()
            except Exception:
                self._display.close()
                raise
        return self

    def __exit__(self, _type: Any, _value: Any, _traceback: Any) -> None:
        self.stop()

    def set_phase(self, phase: str) -> None:
        with self._lock:
            self._phase = phase
        self._refresh.set()

    def _render(self, report: dict[str, Any]) -> None:
        with self._lock:
            phase = self._phase
        width = shutil.get_terminal_size(fallback=(120, 24)).columns
        self._display.render(render_node_progress(report, phase=phase, width=width))

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                report = self._history.apply(collect_node_progress(self.layout))
                self._latest = report
                self._render(report)
            except Exception as error:  # Display failures must never change node control flow.
                self.output.write(f"[usdb-node] progress observation failed: {error}\n")
                self.output.flush()
            self._refresh.wait(self.refresh_secs)
            self._refresh.clear()

    def stop(self) -> None:
        if not self.enabled or self._thread is None:
            return
        self._stop.set()
        self._refresh.set()
        self._thread.join(timeout=20)
        if self._thread.is_alive():
            self.output.write("[usdb-node] progress observer did not stop within 20 seconds\n")
        elif self._latest is not None:
            self._render(self._latest)
        self._display.close()
        self.output.write("\n")
        self.output.flush()
        self._thread = None


def print_progress_status(
    layout: ReleaseLayout,
    *,
    json_output: bool,
    watch: bool,
    refresh_secs: float,
) -> int:
    if refresh_secs <= 0:
        raise ValueError("progress refresh interval must be positive")
    if json_output and watch:
        raise ValueError("--progress-json and --watch cannot be combined")
    if not watch:
        report = collect_node_progress(layout)
        if json_output:
            print(json.dumps(report, indent=2, sort_keys=True))
        else:
            print(render_node_progress(report, width=shutil.get_terminal_size().columns))
        return 0 if report["overall_state"] == "READY" else 1

    output = sys.stderr
    display = TerminalProgressDisplay(output)
    history = NodeProgressHistory()
    display.start()
    try:
        while True:
            report = history.apply(collect_node_progress(layout))
            rendered = render_node_progress(
                report,
                phase="observe",
                width=shutil.get_terminal_size(fallback=(120, 24)).columns,
            )
            if display.live:
                display.render(rendered)
            else:
                output.write(f"\n[{report['observed_at']}]\n")
                output.write(rendered + "\n")
                output.flush()
            time.sleep(refresh_secs)
    except KeyboardInterrupt:
        output.write("\n")
        output.flush()
        return 0
    finally:
        display.close()


STATUS_RECOVERY: dict[str, dict[str, Any]] = {
    "UNCONFIGURED": {
        "up_mode": "manual",
        "up_summary": "up cannot invent operator-owned node configuration",
        "next_actions": ["usdb-node setup"],
        "guidance": [
            "Run interactive setup to choose the data root, node role and network exposure.",
            "No existing node.env will be created or overwritten by up.",
        ],
    },
    "ACTIVATION_REQUIRED": {
        "up_mode": "explicit",
        "up_summary": "release activation requires explicit operator approval",
        "next_actions": [
            "usdb-node up --activate-release",
            "usdb-node activate-release",
        ],
        "guidance": [
            "Review the installed release ID and image digests before activation.",
            "Activation is allowed only when the runtime compatibility contract still matches.",
        ],
    },
    "SNAPSHOT_INCOMPLETE": {
        "up_mode": "automatic",
        "up_summary": "up can continue the approved snapshot installation",
        "next_actions": ["usdb-node up", "usdb-node snapshot install"],
        "guidance": [
            "Keep the .installing, .part and .ranges.json files; they contain resumable state.",
            "The immutable final directory will be published only after full verification.",
        ],
    },
    "READY_TO_START": {
        "up_mode": "automatic",
        "up_summary": "up can enter the readiness-ordered startup path",
        "next_actions": ["usdb-node up"],
        "guidance": [
            "Configuration and local data contracts passed; services may be started safely.",
        ],
    },
    "STARTING": {
        "up_mode": "automatic",
        "up_summary": "up can reconcile containers and continue readiness waits",
        "next_actions": ["usdb-node up", "usdb-node logs"],
        "guidance": [
            "If another usdb-node operation is still active, up will refuse to run concurrently.",
            "Use logs when synchronization or readiness is making no visible progress.",
        ],
    },
    "READY": {
        "up_mode": "noop",
        "up_summary": "the node is already ready",
        "next_actions": [],
        "guidance": [],
    },
    "DEGRADED": {
        "up_mode": "manual",
        "up_summary": "usdb-node will not initiate recovery for unhealthy services",
        "next_actions": ["usdb-node logs", "usdb-node doctor"],
        "guidance": [
            "Inspect service logs before restarting; the Docker restart policy may already be retrying a failed container.",
            "Run doctor to recheck release identity, configuration, data contracts and safe bindings.",
            "Preserve node.env and persistent data until the failing service is understood.",
        ],
    },
    "BLOCKED": {
        "up_mode": "manual",
        "up_summary": "automatic changes are disabled because local state could not be trusted",
        "next_actions": ["usdb-node doctor"],
        "guidance": [
            "Read the first INVALID or UNAVAILABLE check above and run doctor for full diagnostics.",
            "Do not overwrite node.env, delete artifacts or reuse another data directory as a shortcut.",
            "Preserve the failing files and logs for operator review before any recovery action.",
        ],
    },
}


def _finish_node_status(
    report: dict[str, Any],
    overall_state: str,
    *,
    additional_guidance: tuple[str, ...] = (),
) -> dict[str, Any]:
    recovery = STATUS_RECOVERY[overall_state]
    report["overall_state"] = overall_state
    report["next_actions"] = list(recovery["next_actions"])
    report["up"] = {
        "mode": recovery["up_mode"],
        "summary": recovery["up_summary"],
    }
    report["operator_guidance"] = [*recovery["guidance"], *additional_guidance]
    controller = report["checks"].get("controller")
    if (
        overall_state
        in {"ACTIVATION_REQUIRED", "SNAPSHOT_INCOMPLETE", "READY_TO_START", "STARTING"}
        and isinstance(controller, dict)
        and controller.get("state") == "missing"
    ):
        report["next_actions"].insert(0, "usdb-node controller install")
        report["operator_guidance"].append(
            "Install the systemd bootstrap controller before starting a multi-stage operation."
        )
    return report


def collect_node_status(layout: ReleaseLayout) -> dict[str, Any]:
    checks: dict[str, dict[str, Any]] = {
        "release": {
            "state": "ok",
            "summary": f"immutable node kit {layout.release_id} is valid",
        }
    }
    report: dict[str, Any] = {
        "schema_version": NODE_STATUS_SCHEMA_VERSION,
        "release_id": layout.release_id,
        "network_bundle_id": layout.bundle_id,
        "overall_state": "BLOCKED",
        "checks": checks,
        "next_actions": [],
        "up": {},
        "operator_guidance": [],
    }
    if not layout.node_env.is_file():
        checks["configuration"] = {
            "state": "missing",
            "summary": f"node configuration is missing: {layout.node_env}",
        }
        return _finish_node_status(report, "UNCONFIGURED")

    try:
        env = read_env(layout.node_env)
    except (OSError, ValueError) as error:
        checks["configuration"] = {"state": "invalid", "summary": str(error)}
        return _finish_node_status(
            report,
            "BLOCKED",
            additional_guidance=(
                "Inspect node.env syntax and permissions; do not rerun setup over the existing file.",
            ),
        )

    checks["configuration"] = {
        "state": "ok",
        "summary": "private node configuration is present and parseable",
        "firewall_mode": env.get("USDB_FIREWALL_MODE", "managed"),
    }
    controller_path = controller_unit_path(layout)
    checks["controller"] = {
        "state": "installed" if controller_path.is_file() else "missing",
        "summary": (
            f"systemd bootstrap controller is installed: {controller_path.name}"
            if controller_path.is_file()
            else f"systemd bootstrap controller is not installed: {controller_path}"
        ),
    }
    mismatched_images = [key for key, expected in layout.images.items() if env.get(key) != expected]
    activation_required = bool(mismatched_images)
    checks["activation"] = {
        "state": "required" if activation_required else "active",
        "summary": (
            f"release activation is required for: {', '.join(mismatched_images)}"
            if activation_required
            else "private configuration uses this release's image digests"
        ),
    }

    try:
        _validate_node_config(
            layout,
            require_runtime=False,
            require_bitcoin_runtime=True,
        )
    except (OSError, ValueError) as error:
        checks["data"] = {"state": "invalid", "summary": str(error)}
        return _finish_node_status(
            report,
            "BLOCKED",
            additional_guidance=(
                "A data path, credential or dataset identity check failed; do not move "
                "or adopt data automatically.",
            ),
        )
    checks["data"] = {
        "state": "ok",
        "summary": "local paths, credentials and dataset contracts are valid",
    }

    snapshot = _snapshot_lifecycle_status(layout, env)
    checks["snapshot"] = snapshot
    if snapshot["state"] == "invalid":
        return _finish_node_status(
            report,
            "BLOCKED",
            additional_guidance=(
                "The selected snapshot is not a valid resumable staging state; compare it "
                "with the release-approved record.",
            ),
        )

    if activation_required:
        return _finish_node_status(report, "ACTIVATION_REQUIRED")
    if snapshot["state"] == "incomplete":
        return _finish_node_status(report, "SNAPSHOT_INCOMPLETE")

    try:
        runtime = _runtime_lifecycle_status(layout)
    except (OSError, ValueError) as error:
        checks["runtime"] = {"state": "unavailable", "summary": str(error)}
        return _finish_node_status(
            report,
            "BLOCKED",
            additional_guidance=(
                "Verify that Docker is running and that the current operator can access its daemon.",
            ),
        )
    checks["runtime"] = runtime
    runtime_state = runtime["state"]
    if runtime_state == "stopped":
        return _finish_node_status(report, "READY_TO_START")
    elif runtime_state == "starting":
        return _finish_node_status(report, "STARTING")
    elif runtime_state == "ready":
        return _finish_node_status(report, "READY")
    elif runtime_state == "degraded":
        return _finish_node_status(report, "DEGRADED")
    elif runtime_state == "unavailable":
        return _finish_node_status(
            report,
            "BLOCKED",
            additional_guidance=(
                "Docker Compose state could not be read; verify the daemon, Compose plugin "
                "and operator permissions.",
            ),
        )
    return _finish_node_status(
        report,
        "BLOCKED",
        additional_guidance=(
            "Docker Compose state could not be classified; retain its raw service state for review.",
        ),
    )


def _print_node_status_report(report: dict[str, Any]) -> None:
    print(f"USDB node lifecycle status: {report['release_id']}")
    labels = {
        "release": "Release kit",
        "configuration": "Node config",
        "activation": "Activation",
        "data": "Local data",
        "snapshot": "Snapshot",
        "runtime": "Runtime",
    }
    for key, label in labels.items():
        check = report["checks"].get(key)
        if check is not None:
            print(f"{label:<14} {check['state'].upper():<12} {check['summary']}")
            if key == "runtime" and isinstance(check.get("bitcoin_progress"), dict):
                print(
                    f"{'Bitcoin sync':<14} {'WAITING':<12} "
                    f"{_bitcoin_progress_summary(check['bitcoin_progress'])}"
                )
    print(f"Overall        {report['overall_state']}")
    print(f"Up             {report['up']['mode'].upper():<12} {report['up']['summary']}")
    if report["next_actions"]:
        print("Next actions:")
        for index, action in enumerate(report["next_actions"], start=1):
            print(f"  {index}. {action}")
    if report["operator_guidance"]:
        print("Operator guidance:")
        for item in report["operator_guidance"]:
            print(f"  - {item}")


def print_status(layout: ReleaseLayout, *, json_output: bool = False) -> int:
    report = collect_node_status(layout)
    if json_output:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        _print_node_status_report(report)
    return 0 if report["overall_state"] == "READY" else 1


def _up_action(report: dict[str, Any], allow_activation: bool) -> str | None:
    state = report["overall_state"]
    if state == "ACTIVATION_REQUIRED" and allow_activation:
        return "activate-release"
    if state == "SNAPSHOT_INCOMPLETE":
        return "snapshot-install"
    if state in {"READY_TO_START", "STARTING"}:
        return "up"
    return None


def up_node(
    layout: ReleaseLayout,
    *,
    dry_run: bool,
    allow_activation: bool,
    sync_timeout_secs: int,
    pull: bool,
    json_output: bool,
    enable_progress: bool = False,
) -> tuple[dict[str, Any], int]:
    initial = collect_node_status(layout)
    action = _up_action(initial, allow_activation)
    result: dict[str, Any] = {
        "schema_version": NODE_UP_SCHEMA_VERSION,
        "release_id": layout.release_id,
        "network_bundle_id": layout.bundle_id,
        "initial_state": initial["overall_state"],
        "completed_actions": [],
        "status": initial,
    }
    if dry_run:
        result["outcome"] = "dry_run"
        result["planned_actions"] = [action] if action is not None else []
        return result, 0
    if initial["overall_state"] == "READY":
        result["outcome"] = "ready"
        return result, 0
    if action is None:
        result["outcome"] = "manual_action_required"
        return result, 1

    completed: list[str] = []
    output_context = redirect_stdout(sys.stderr) if json_output else nullcontext()
    progress_monitor = NodeProgressMonitor(
        layout,
        enabled=enable_progress and not json_output,
    )
    with progress_monitor, node_operation_lock(layout, "up"):
        report = collect_node_status(layout)
        with output_context:
            for _transition in range(MAX_UP_TRANSITIONS):
                if report["overall_state"] == "READY":
                    result["outcome"] = "ready"
                    result["completed_actions"] = completed
                    result["status"] = report
                    return result, 0

                action = _up_action(report, allow_activation)
                if action is None:
                    result["outcome"] = "manual_action_required"
                    result["completed_actions"] = completed
                    result["status"] = report
                    return result, 1
                if action in completed:
                    result["outcome"] = "no_forward_progress"
                    result["completed_actions"] = completed
                    result["status"] = report
                    result["operator_guidance"] = [
                        "The same resumable action completed without changing lifecycle state.",
                        "Inspect status and logs before another attempt; automatic retry stopped.",
                    ]
                    return result, 1

                if action == "activate-release":
                    progress_monitor.set_phase("activate-release")
                    activate_release(layout)
                elif action == "snapshot-install":
                    progress_monitor.set_phase("snapshot-install")
                    install_snapshot_release(layout)
                elif action == "up":
                    startup_options: dict[str, Any] = {
                        "sync_timeout_secs": sync_timeout_secs,
                        "pull": pull,
                        "output_to_stderr": json_output,
                    }
                    if progress_monitor.enabled:
                        startup_options["progress_monitor"] = progress_monitor
                    start_node(layout, **startup_options)
                else:
                    raise AssertionError(f"unsupported internal up action: {action}")
                completed.append(action)
                report = collect_node_status(layout)

    result["outcome"] = "transition_limit_reached"
    result["completed_actions"] = completed
    result["status"] = report
    result["operator_guidance"] = [
        "Up reached its bounded transition limit and stopped without forcing another action.",
        "Inspect status and logs before continuing manually.",
    ]
    return result, 1


def run_bootstrap_controller(
    layout: ReleaseLayout,
    *,
    sync_timeout_secs: int,
    pull: bool,
) -> int:
    """Run the non-interactive, non-activating up state machine for systemd."""
    result, return_code = up_node(
        layout,
        dry_run=False,
        allow_activation=False,
        sync_timeout_secs=sync_timeout_secs,
        pull=pull,
        json_output=False,
        enable_progress=False,
    )
    print_up_result(result, json_output=False)
    if return_code == 0:
        return 0
    if result["outcome"] in {
        "manual_action_required",
        "no_forward_progress",
        "transition_limit_reached",
    }:
        return CONTROLLER_MANUAL_EXIT_CODE
    return return_code


def submit_up_to_controller(
    layout: ReleaseLayout,
    *,
    dry_run: bool,
    allow_activation: bool,
) -> tuple[dict[str, Any], int]:
    """Apply explicit activation when allowed, then submit safe up work to systemd."""
    if dry_run:
        return up_node(
            layout,
            dry_run=True,
            allow_activation=allow_activation,
            sync_timeout_secs=1,
            pull=True,
            json_output=False,
        )

    initial = collect_node_status(layout)
    result: dict[str, Any] = {
        "schema_version": NODE_UP_SCHEMA_VERSION,
        "release_id": layout.release_id,
        "network_bundle_id": layout.bundle_id,
        "initial_state": initial["overall_state"],
        "completed_actions": [],
        "status": initial,
    }
    if initial["overall_state"] == "READY":
        result["outcome"] = "ready"
        return result, 0

    report = initial
    if report["overall_state"] == "ACTIVATION_REQUIRED":
        if not allow_activation:
            result["outcome"] = "manual_action_required"
            return result, 1
        _require_controller_unit(layout)
        with node_operation_lock(layout, "activate-release"):
            report = collect_node_status(layout)
            if report["overall_state"] == "ACTIVATION_REQUIRED":
                activate_release(layout)
                result["completed_actions"].append("activate-release")
            report = collect_node_status(layout)

    if report["overall_state"] == "READY":
        result["outcome"] = "ready"
        result["status"] = report
        return result, 0
    if _up_action(report, False) is None:
        result["outcome"] = "manual_action_required"
        result["status"] = report
        return result, 1

    _require_controller_unit(layout)
    unit = start_controller_unit(layout)
    result["outcome"] = "controller_started"
    result["controller_unit"] = unit
    result["status"] = report
    result["operator_guidance"] = [
        "Bootstrap continues under systemd if this terminal disconnects.",
        "Ctrl+C detaches only this progress view; use usdb-node status --watch to reattach.",
    ]
    return result, 0


def follow_submitted_controller(
    layout: ReleaseLayout,
    result: dict[str, Any],
    *,
    refresh_secs: float = DEFAULT_PROGRESS_REFRESH_SECS,
) -> tuple[dict[str, Any], int]:
    """Render controller progress until READY, a manual stop, or operator detach."""
    output = sys.stderr
    display = TerminalProgressDisplay(output)
    started_at = time.monotonic()
    observed_running = False
    display.start()
    try:
        while True:
            progress = collect_node_progress(layout)
            display.render(
                render_node_progress(
                    progress,
                    phase="bootstrap-controller",
                    width=shutil.get_terminal_size(fallback=(120, 24)).columns,
                )
            )

            if progress["overall_state"] == "READY":
                result["outcome"] = "ready"
                result["status"] = collect_node_status(layout)
                return result, 0

            state = controller_active_state(layout)
            if state in {"active", "activating", "reloading"}:
                observed_running = True
            elif (
                not observed_running
                and time.monotonic() - started_at < CONTROLLER_START_GRACE_SECS
            ):
                time.sleep(refresh_secs)
                continue
            else:
                report = collect_node_status(layout)
                result["status"] = report
                if report["overall_state"] == "READY":
                    result["outcome"] = "ready"
                    return result, 0
                result["outcome"] = "controller_stopped"
                result["controller_state"] = state
                result["operator_guidance"] = [
                    "The bootstrap controller stopped before the node became ready.",
                    "Inspect usdb-node controller status and controller logs before restarting it.",
                ]
                return result, 1
            time.sleep(refresh_secs)
    except KeyboardInterrupt:
        result["outcome"] = "controller_detached"
        result["status"] = collect_node_status(layout)
        result["operator_guidance"] = [
            "The bootstrap controller is still running under systemd.",
            "Use usdb-node status --watch to reattach without starting another controller.",
        ]
        output.write("\n")
        output.flush()
        return result, 0
    finally:
        display.close()


def print_up_result(result: dict[str, Any], *, json_output: bool) -> None:
    if json_output:
        print(json.dumps(result, indent=2, sort_keys=True))
        return
    print(f"USDB node up outcome: {result['outcome']}")
    print(f"Initial state: {result['initial_state']}")
    completed = result.get("completed_actions", [])
    if completed:
        print(f"Completed actions: {', '.join(completed)}")
    planned = result.get("planned_actions", [])
    if planned:
        print(f"Planned actions: {', '.join(planned)}")
    _print_node_status_report(result["status"])
    for item in result.get("operator_guidance", []):
        print(f"  - {item}")


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

    setup = subparsers.add_parser(
        "setup",
        help="Interactively configure the node and install its bootstrap controller",
    )
    setup.add_argument(
        "--no-controller",
        action="store_true",
        help="write configuration without installing the default systemd controller",
    )
    setup.add_argument(
        "--bitcoin-profile",
        choices=(AUTO_BITCOIN_RESOURCE_PROFILE, *BITCOIN_RESOURCE_PROFILES),
        default=AUTO_BITCOIN_RESOURCE_PROFILE,
        help=(
            "select Bitcoin memory tuning; auto uses a steady-state profile from "
            "host memory and never selects the temporary ibd-64g profile"
        ),
    )

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
        "--bitcoin-profile",
        choices=tuple(BITCOIN_RESOURCE_PROFILES),
        default=DEFAULT_BITCOIN_RESOURCE_PROFILE,
    )
    configure.add_argument(
        "--firewall-mode",
        choices=FIREWALL_MODES,
        default="external",
        help="external leaves host policy untouched; managed enables bundled UFW checks",
    )
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

    firewall_mode = subparsers.add_parser(
        "set-firewall-mode",
        help="Select externally managed firewall policy or bundled UFW management",
    )
    firewall_mode.add_argument("--mode", choices=FIREWALL_MODES, required=True)

    bitcoin_profile = subparsers.add_parser(
        "set-bitcoin-profile",
        help=(
            "Update Bitcoin memory tuning after stopping all node services; "
            "ibd-64g is temporary and explicit-only"
        ),
        description=(
            "Select the Bitcoin Core container memory and dbcache profile. "
            "All node containers must be stopped first; changing profiles preserves "
            "the existing Bitcoin data directory."
        ),
        epilog="""profiles:
  auto             Select a host-appropriate steady-state profile: performance-64g
                   with at least 56 GiB physical memory, otherwise balanced-32g.
                   Never selects the temporary ibd-64g profile.
  balanced-32g     Co-located baseline: 5 GiB memory, 6 GiB memory+swap,
                   3072 MiB dbcache.
  performance-64g  Steady-state 64 GiB host profile: 24 GiB memory,
                   26 GiB memory+swap, 12288 MiB dbcache. Requires 56 GiB.
  ibd-64g          Temporary initial Bitcoin IBD/txindex profile: 32 GiB memory,
                   34 GiB memory+swap, 20480 MiB dbcache. Requires 56 GiB;
                   switch to performance-64g after initial sync.

workflow:
  usdb-node down
  usdb-node set-bitcoin-profile --profile auto
  usdb-node up""",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    bitcoin_profile.add_argument(
        "--profile",
        choices=(AUTO_BITCOIN_RESOURCE_PROFILE, *BITCOIN_RESOURCE_PROFILES),
        required=True,
        help="profile to persist; use auto to restore host-appropriate steady-state tuning",
    )

    subparsers.add_parser(
        "activate-release",
        help="Update only release-owned image digests in existing private config",
    )

    subparsers.add_parser(
        "doctor",
        help="Read-only host, release identity, config and selected firewall preflight",
    )

    snapshot = subparsers.add_parser("snapshot", help="Install an immutable signed snapshot release")
    snapshot_actions = snapshot.add_subparsers(dest="snapshot_action", required=True)
    snapshot_install = snapshot_actions.add_parser(
        "install",
        help="Continue download, verify, atomically stage, and select one snapshot",
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

    up = subparsers.add_parser(
        "up",
        help="Idempotently bring a configured node to READY and attach progress",
    )
    up.add_argument(
        "--dry-run",
        action="store_true",
        help="report the allowed next transition without changing local state",
    )
    up.add_argument(
        "--json",
        action="store_true",
        help="emit one machine-readable up result on stdout",
    )
    up.add_argument(
        "--activate-release",
        action="store_true",
        help="explicitly allow same-contract release image activation",
    )
    up.add_argument(
        "--foreground",
        action="store_true",
        help="run bootstrap in this terminal instead of submitting the installed controller",
    )
    up.add_argument("--sync-timeout-secs", type=int, default=DEFAULT_SYNC_TIMEOUT_SECS)
    up.add_argument("--skip-pull", action="store_true")
    status = subparsers.add_parser(
        "status",
        help="Show node installation, activation, snapshot and runtime lifecycle state",
    )
    status_output = status.add_mutually_exclusive_group()
    status_output.add_argument("--json", action="store_true", help="emit lifecycle JSON")
    status_output.add_argument(
        "--progress-json",
        action="store_true",
        help="emit one machine-readable five-component progress observation",
    )
    status_output.add_argument(
        "--watch",
        action="store_true",
        help="continuously render snapshot, Bitcoin, indexer and chain progress",
    )
    status.add_argument(
        "--refresh-secs",
        type=float,
        default=DEFAULT_PROGRESS_REFRESH_SECS,
        help="dashboard refresh interval (default: 5 seconds)",
    )

    controller = subparsers.add_parser(
        "controller",
        help="Install and operate the systemd-supervised bootstrap controller",
    )
    controller_actions = controller.add_subparsers(dest="controller_action", required=True)
    controller_install = controller_actions.add_parser(
        "install",
        help="install and enable the bundle-scoped systemd controller",
    )
    controller_install.add_argument(
        "--launcher",
        type=Path,
        help="stable usdb-node launcher path; defaults to the launcher found in PATH",
    )
    controller_install.add_argument(
        "--sync-timeout-secs",
        type=int,
        default=DEFAULT_SYNC_TIMEOUT_SECS,
    )
    controller_install.add_argument("--skip-pull", action="store_true")
    controller_actions.add_parser(
        "stop",
        help="stop bootstrap orchestration without stopping Docker services",
    )
    controller_actions.add_parser(
        "disable",
        help="stop the controller and prevent it from resuming after host reboot",
    )
    controller_actions.add_parser("status", help="show the systemd controller status")
    controller_logs = controller_actions.add_parser("logs", help="show controller journal logs")
    controller_logs.add_argument("--follow", action="store_true")
    controller_run = controller_actions.add_parser(
        "run",
        help=argparse.SUPPRESS,
    )
    controller_run.add_argument(
        "--sync-timeout-secs",
        type=int,
        default=DEFAULT_SYNC_TIMEOUT_SECS,
    )
    controller_run.add_argument("--skip-pull", action="store_true")

    logs = subparsers.add_parser("logs", help="Follow Bitcoin or runtime service logs")
    logs.add_argument("--bitcoin", action="store_true")
    logs.add_argument("service", nargs="*")

    down = subparsers.add_parser(
        "down",
        help="Stop the bootstrap controller and all node services without deleting data",
    )
    down.add_argument(
        "--keep-bitcoin",
        action="store_true",
        help="stop USDB runtime but leave Bitcoin Core running",
    )
    return parser


def _operation_name(args: argparse.Namespace) -> str | None:
    if args.command == "up":
        return None
    if args.command == "controller":
        return "controller-install" if args.controller_action == "install" else None
    if args.command == "firewall" and args.firewall_action != "apply":
        return None
    if args.command in {
        "setup",
        "configure",
        "set-role",
        "set-firewall-mode",
        "set-bitcoin-profile",
        "activate-release",
        "snapshot",
        "firewall",
    }:
        return args.command
    return None


def _execute_command(layout: ReleaseLayout, args: argparse.Namespace) -> int:
    if args.command == "prepare-host":
        prepare_host(layout, docker_user=args.docker_user)
        print("USDB host prerequisites are ready.")
        print("If Docker group membership changed, start a new login session before doctor/up.")
    elif args.command == "host":
        run_host_action(layout, args.host_action, docker_user=args.docker_user)
    elif args.command == "setup":
        if not sys.stdin.isatty() or not sys.stdout.isatty():
            raise ValueError("setup requires an interactive terminal; use configure for automation")
        if not args.no_controller:
            _controller_install_context()
        result = setup_node(layout, bitcoin_resource_profile=args.bitcoin_profile)
        print(f"Configured {layout.release_id} node: {result.node_env}")
        if result.apply_firewall:
            print(
                "Applying the UFW firewall profile; sudo may request the operator password.",
                flush=True,
            )
            run_firewall_action(layout, "apply", confirm=True)
            print("Applied and verified the host UFW firewall profile.")
        else:
            print("Host firewall mode is external; UFW was not installed, inspected, or modified.")
        if result.install_snapshot:
            if args.no_controller:
                print(
                    "Selected the release-approved balance-history snapshot; "
                    "up --foreground or a later controller installation will download and verify it."
                )
            else:
                print(
                    "Selected the release-approved balance-history snapshot; "
                    "the bootstrap controller will download and verify it."
                )
        if args.no_controller:
            print(
                "Skipped the bootstrap controller by explicit request; "
                "use usdb-node up --foreground or install the controller later."
            )
            print("Run usdb-node doctor, then usdb-node up --foreground.")
        else:
            try:
                path = install_controller_unit(layout)
            except (OSError, ValueError, subprocess.CalledProcessError):
                print(
                    f"Node configuration was preserved at {result.node_env}; "
                    "fix the reported host issue, then run usdb-node controller install.",
                    file=sys.stderr,
                )
                raise
            print(f"Installed and enabled USDB bootstrap controller: {path.name}")
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
            firewall_mode=args.firewall_mode,
            bitcoin_resource_profile=args.bitcoin_profile,
        )
        print(f"Configured {layout.release_id} node: {path}")
        print("Bitcoin RPC credentials were generated locally and were not printed.")
        print(
            "Run usdb-node controller install before background up, "
            "or use usdb-node up --foreground for explicit foreground operation."
        )
    elif args.command == "set-role":
        set_role(
            layout,
            role=args.role,
            miner_address=args.miner_address,
            miner_threads=args.miner_threads,
        )
        print(f"Updated node role to {args.role}; run usdb-node up to reconcile containers.")
    elif args.command == "set-firewall-mode":
        set_firewall_mode(layout, args.mode)
        if args.mode == "managed":
            print(
                "Host firewall mode is managed; run "
                "'usdb-node firewall apply --confirm' before startup."
            )
        else:
            print("Host firewall mode is external; usdb-node will not inspect or modify UFW.")
    elif args.command == "set-bitcoin-profile":
        selected_profile, resources = set_bitcoin_resource_profile(
            layout,
            args.profile,
        )
        print(
            f"Updated Bitcoin resource profile to {selected_profile}: "
            f"memory={resources['memory_limit']}, "
            f"memory+swap={resources['memory_swap_limit']}, "
            f"dbcache={resources['dbcache_mb']} MiB."
        )
        if selected_profile == IBD_BITCOIN_RESOURCE_PROFILE:
            print(
                "ibd-64g is temporary; switch to performance-64g after IBD and txindex complete."
            )
        print("Run usdb-node up to start the node and resume the existing Bitcoin data directory.")
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
        if args.foreground:
            result, return_code = up_node(
                layout,
                dry_run=args.dry_run,
                allow_activation=args.activate_release,
                sync_timeout_secs=args.sync_timeout_secs,
                pull=not args.skip_pull,
                json_output=args.json,
                enable_progress=(
                    not args.json and sys.stdout.isatty() and sys.stderr.isatty()
                ),
            )
        else:
            if args.sync_timeout_secs != DEFAULT_SYNC_TIMEOUT_SECS or args.skip_pull:
                raise ValueError(
                    "background controller settings are frozen by controller install; "
                    "reinstall it with the desired options or use up --foreground"
                )
            result, return_code = submit_up_to_controller(
                layout,
                dry_run=args.dry_run,
                allow_activation=args.activate_release,
            )
            if (
                return_code == 0
                and result["outcome"] == "controller_started"
                and not args.json
                and sys.stdout.isatty()
                and _terminal_refresh_supported(sys.stderr)
            ):
                result, return_code = follow_submitted_controller(layout, result)
        print_up_result(result, json_output=args.json)
        return return_code
    elif args.command == "status":
        if args.progress_json or args.watch:
            return print_progress_status(
                layout,
                json_output=args.progress_json,
                watch=args.watch,
                refresh_secs=args.refresh_secs,
            )
        return print_status(layout, json_output=args.json)
    elif args.command == "controller":
        if args.controller_action == "install":
            if args.sync_timeout_secs <= 0:
                raise ValueError("sync timeout must be positive")
            path = install_controller_unit(
                layout,
                launcher=args.launcher,
                sync_timeout_secs=args.sync_timeout_secs,
                pull=not args.skip_pull,
            )
            print(f"Installed and enabled USDB bootstrap controller: {path.name}")
            print("Run usdb-node up to start it and attach the progress view.")
        elif args.controller_action == "stop":
            stop_controller_unit(layout)
            print("Stopped bootstrap orchestration; Docker services were left running.")
        elif args.controller_action == "disable":
            disable_controller_unit(layout)
            print(
                "Disabled bootstrap orchestration; Docker services and journal records were left intact."
            )
        elif args.controller_action == "status":
            return show_controller_unit(layout)
        elif args.controller_action == "logs":
            return follow_controller_logs(layout, follow=args.follow)
        elif args.controller_action == "run":
            if args.sync_timeout_secs <= 0:
                raise ValueError("sync timeout must be positive")
            return run_bootstrap_controller(
                layout,
                sync_timeout_secs=args.sync_timeout_secs,
                pull=not args.skip_pull,
            )
        else:
            raise AssertionError(f"unsupported controller action: {args.controller_action}")
    elif args.command == "logs":
        helper = "run_testnet_bitcoin.sh" if args.bitcoin else "run_testnet_runtime.sh"
        arguments = ["logs", *args.service]
        run_helper(layout, helper, arguments)
    elif args.command == "down":
        down_node(layout, keep_bitcoin=args.keep_bitcoin)
        if args.keep_bitcoin:
            print("Stopped bootstrap orchestration and USDB runtime; Bitcoin Core was left running.")
        else:
            print("Stopped bootstrap orchestration, USDB runtime, and Bitcoin Core.")
    return 0


def main() -> int:
    args = build_parser().parse_args()
    try:
        layout = load_release_layout(args.kit_root, args.node_env)
        operation = _operation_name(args)
        operation_context = (
            node_operation_lock(layout, operation) if operation is not None else nullcontext()
        )
        with operation_context:
            return _execute_command(layout, args)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        if args.command == "up" and args.json:
            print(
                json.dumps(
                    {
                        "schema_version": NODE_UP_SCHEMA_VERSION,
                        "outcome": "error",
                        "error": str(error),
                    },
                    indent=2,
                    sort_keys=True,
                )
            )
        else:
            print(f"USDB node operation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
