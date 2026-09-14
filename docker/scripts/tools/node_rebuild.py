#!/usr/bin/env python3
"""Interactively preserve and remove v2 node paths before a clean installation.

Standalone Python/Linux tool: no imports from the installed (possibly old) node kit.
It never stops services, prunes Docker, or treats a live RocksDB copy as a backup.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack
import ctypes
from dataclasses import dataclass
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
import time

SCHEMA = "usdb-node-rebuild:v1"
CHUNK = 4 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def absolute(value: str | Path) -> Path:
    """Require literal normalized paths; never expand shell expressions from node.env."""
    path = Path(value).expanduser()
    require(path.is_absolute() and ".." not in path.parts and str(path) == str(value), f"Expected a normalized absolute path: {value}")
    return path


def overlap(left: Path, right: Path) -> bool:
    return left == right or left in right.parents or right in left.parents


def safe_path(path: Path, *, leaf_link=False):
    for item in (path, *path.parents):
        require(not item.is_symlink() or (item == path and leaf_link), f"Refusing symlink path: {item}")


def read_json(path: Path):
    safe_path(path)
    return json.loads(path.read_text())


def atomic_json(path: Path, value):
    safe_path(path)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            json.dump(value, output, sort_keys=True, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)
    sync_dir(path.parent)


def sync_dir(path: Path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class Target:
    key: str
    path: Path
    # None: discardable data; (): preserve entire target; otherwise selected identities.
    preserve: tuple[str, ...] | None
    link: str = ""


@dataclass
class Plan:
    home: Path
    root: Path
    bundle: str
    env_path: Path
    targets: list[Target]
    env_sha256: str


def build_plan(home: Path, bundle: str, backup: Path, unit_dir=Path("/etc/systemd/system")) -> Plan:
    """Derive only known v2 service paths; archived config supports interrupted cleanup."""
    home, backup = absolute(home), absolute(backup)
    require(re.fullmatch(r"usdb-(testnet|mainnet)-v[0-9]+", bundle), "Invalid bundle ID")
    config = home / ".config/usdb" / bundle
    env_path = config / "node.env"
    source = env_path if env_path.exists() else backup / "objects/config/node.env"
    safe_path(source)
    env = {}
    env_content = source.read_bytes()
    for line in env_content.decode().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, separator, value = line.partition("=")
        require(separator and key not in env, "Invalid or duplicate node.env field")
        env[key] = value
    require(env.get("USDB_DATA_LAYOUT") == "usdb-node-data-layout:v2", "Only reviewed v2 data layouts are supported")
    root = absolute(env["USDB_DATA_ROOT"])
    require(len(root.parts) >= 2 and root != home, "Data root must be a dedicated directory")
    network = root / "networks" / bundle
    bh = absolute(env["BH_DATA_HOST_DIR"])
    indexer = absolute(env["USDB_INDEXER_DATA_HOST_DIR"])
    require(re.fullmatch(r"[0-9a-f]{64}", bh.name) and bh.parent == root / "datasets/balance-history/btc-mainnet", "Unexpected BH data path")
    require(re.fullmatch(r"[0-9a-f]{64}", indexer.name) and indexer.parent == root / "datasets/usdb-indexer", "Unexpected indexer data path")
    expected = {
        "BTC_NODE_DATA_HOST_DIR": root / "datasets/bitcoin/btc-mainnet",
        "USDB_CHAIN_DATA_HOST_DIR": network / "usdb-chain",
        "CONTROL_PLANE_DATA_HOST_DIR": network / "control-plane",
        "BH_SNAPSHOT_HOST_DIR": root / "artifacts/balance-history",
    }
    for key, path in expected.items():
        require(env.get(key) == str(path), f"Unexpected data path for {key}")
    targets = [
        Target("balance-history", bh, ()),
        Target("bitcoin", expected["BTC_NODE_DATA_HOST_DIR"], ("wallets", "wallet.dat", "debug.log", "settings.json", ".usdb-dataset-identity.json")),
        Target("indexer", indexer, None),
        Target("chain", expected["USDB_CHAIN_DATA_HOST_DIR"], ("keystore", "geth/nodekey", "bootstrap", "recovery", ".usdb-dataset-identity.json")),
        Target("control-plane", expected["CONTROL_PLANE_DATA_HOST_DIR"], ()),
        Target("legacy-snapshots", expected["BH_SNAPSHOT_HOST_DIR"], None),
        Target("utxo-file", root / "artifacts/assumeutxo/mainnet-935000", None),
        Target("utxo-state", network / "assumeutxo", ()),
        Target("secure", network / "secure", ()),
    ]
    # Preserve every selected old kit; they are small and may be needed to read the old DB.
    releases = home / ".local/share/usdb/releases"
    safe_path(releases)
    release_names = {p.name for p in releases.iterdir()} if releases.exists() else set()
    if (backup / "session.json").exists():
        release_names.update(read_json(backup / "session.json").get("sources", {}))
    for name in sorted(release_names):
        if re.fullmatch(re.escape(bundle) + r"-r[1-9][0-9]*", name):
            path = releases / name
            manifest = (path if path.exists() else backup / "objects" / name) / "release/usdb-release-manifest.json"
            require(read_json(manifest)["release_id"] == name, "Release directory identity mismatch")
            targets.append(Target(name, path, ()))
    launcher = home / ".local/bin/usdb-node"
    if launcher.is_symlink():
        link = os.readlink(launcher)
        destination = (launcher.parent / link).resolve()
        relative = destination.relative_to(releases) if releases in destination.parents else Path("invalid")
        require(len(relative.parts) == 5 and re.fullmatch(re.escape(bundle) + r"-r[1-9][0-9]*", relative.parts[0])
                and relative.parts[1:] == ("docker", "scripts", "tools", "usdb_node.py"), "Launcher points outside the selected release kits")
        targets.append(Target("launcher", launcher, None, link))
    else:
        require(not launcher.exists(), "Refusing a non-symlink launcher")
    unit = unit_dir / f"usdb-node-bootstrap-{bundle}.service"
    targets += [Target("controller-unit", unit, ()), Target("config", config, ())]
    for target in targets:
        safe_path(target.path, leaf_link=bool(target.link))
        require(not overlap(target.path, backup), f"Backup overlaps cleanup path: {target.path}")
        require(not overlap(target.path, Path(__file__).resolve()), "Copy this standalone script outside all cleanup targets before running it")
    require(all(not overlap(left.path, right.path) for i, left in enumerate(targets) for right in targets[i + 1:]),
            "Cleanup targets overlap each other; this data layout needs manual review")
    require(not overlap(root, backup) and not overlap(config.parent, backup) and not overlap(releases, backup), "Backup must be outside the data, configuration and release roots")
    safe_path(backup)
    return Plan(home, root, bundle, env_path, targets, hashlib.sha256(env_content).hexdigest())


def command(args: list[str], *, accepted=(0,)) -> str:
    """Keep captured command diagnostics out of logs; they can contain private config."""
    result = subprocess.run(args, capture_output=True, text=True, timeout=30)
    require(result.returncode in accepted, f"Inspection failed: {args[0]} {args[1]}; check it separately")
    return result.stdout


def mounts() -> list[Path]:
    def decode(value):
        return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), value)
    return [Path(decode(line.split()[4])) for line in Path("/proc/self/mountinfo").read_text().splitlines()]


def check_mounts(path: Path):
    require(not any(m == path or path in m.parents for m in mounts()), f"Refusing a mount point or nested mount: {path}")


def containers() -> list[dict]:
    ids = command(["docker", "ps", "-aq"]).split()
    if not ids:
        return []
    # Never capture Config.Env or authenticated command arguments.
    template = '{"id":{{json .Id}},"state":{{json .State.Status}},"mounts":{{json .Mounts}},"labels":{{json .Config.Labels}}}'
    return [json.loads(line) for line in command(["docker", "inspect", "--format", template, *ids]).splitlines()]


def check_stopped(plan: Plan, *, deleting: Path | None = None):
    """Fail closed on unknown host state and any container sharing a selected data path."""
    unit = f"usdb-node-bootstrap-{plan.bundle}.service"
    values = dict(line.split("=", 1) for line in command(["systemctl", "show", unit,
        "--property=LoadState,ActiveState,UnitFileState"], accepted=(0, 1, 4)).splitlines() if "=" in line)
    require("LoadState" in values and "ActiveState" in values, "Controller inspection unavailable; no paths will be changed")
    require(values.get("LoadState") == "not-found" or (
        values.get("ActiveState") in {"inactive", "failed"} and values.get("UnitFileState") in {"disabled", "masked"}),
        f"Disable and stop {unit} with the old usdb-node controller before continuing")
    paths = [t.path for t in plan.targets]
    for item in containers():
        sources = [Path(m["Source"]) for m in item["mounts"] if m.get("Source", "").startswith("/")]
        shared = any(overlap(p, s) for p in paths for s in sources)
        project = (item.get("labels") or {}).get("com.docker.compose.project", "")
        selected = project in {plan.bundle, plan.bundle + "-bitcoin"}
        require(not ((shared or selected) and item["state"] not in {"exited", "created", "dead"}),
                f"Container still uses this node: {item['id'][:12]}; run the old usdb-node down first")
        require(not (deleting and any(overlap(deleting, s) for s in sources)),
                f"Stopped container still references {deleting}: {item['id'][:12]}; review/remove that container separately")


def stamp(path: Path) -> list[int]:
    value = path.stat(follow_symlinks=False)
    return [value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns, stat.S_IMODE(value.st_mode)]


def inventory(path: Path) -> dict:
    """Record structure and stable file identity without reading large file contents."""
    safe_path(path)
    check_mounts(path)
    result, started, last = {}, time.monotonic(), time.monotonic()
    stack = [(path, ".")]
    while stack:
        current, relative = stack.pop()
        mode = current.lstat().st_mode
        require(stat.S_ISDIR(mode) or stat.S_ISREG(mode), f"Backup contains a link or special file: {current}")
        directory = stat.S_ISDIR(mode)
        result[relative] = dict(kind="directory" if directory else "file", stamp=stamp(current))
        if directory:
            stack.extend((entry, entry.name if relative == "." else relative + "/" + entry.name)
                         for entry in sorted(current.iterdir(), reverse=True))
        if time.monotonic() - last >= 10:
            print(f"Inventory progress: path={path}, entries={len(result)}, elapsed_seconds={time.monotonic() - started:.1f}", flush=True)
            last = time.monotonic()
    return result


class Progress:
    def __init__(self, action, path):
        self.action, self.path, self.start, self.last, self.bytes = action, path, time.monotonic(), 0, 0
        self.show("started")

    def show(self, state):
        print(f"{self.action} {state}: path={self.path}, bytes={self.bytes}, elapsed_seconds={time.monotonic() - self.start:.1f}", flush=True)
        self.last = time.monotonic()

    def add(self, size):
        self.bytes += size
        if time.monotonic() - self.last >= 10:
            self.show("progress")


def digest(path: Path, progress: Progress) -> str:
    safe_path(path)
    before = stamp(path)
    value = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(CHUNK):
            value.update(chunk)
            progress.add(len(chunk))
    require(stamp(path) == before, f"File changed during verification: {path}")
    return value.hexdigest()


def same_content(left: dict, right: dict) -> bool:
    return {k: (v["kind"], v.get("sha256")) for k, v in left.items()} == {k: (v["kind"], v.get("sha256")) for k, v in right.items()}


def verify_tree(path: Path, expected: dict, *, reject_hardlinks=False):
    progress = Progress("Verify", path)
    actual = inventory(path)
    for name, entry in actual.items():
        if entry["kind"] == "file":
            file = path / name if name != "." else path
            require(not reject_hardlinks or file.stat().st_nlink == 1, f"Copy backup contains a hardlink: {file}")
            entry["sha256"] = digest(file, progress)
    require(same_content(actual, expected), f"Backup content differs: {path}")
    progress.show("finished")


def copy_tree(source: Path, destination: Path) -> dict:
    """Resume a private partial copy by hashing existing files; publish only an exact copy."""
    progress = Progress("Backup", source)
    before = inventory(source)
    required = sum(v["stamp"][2] for v in before.values() if v["kind"] == "file")
    existing = sum(v["stamp"][2] for v in inventory(destination).values() if v["kind"] == "file") if destination.exists() else 0
    require(shutil.disk_usage(destination.parent).free >= max(0, required - existing) + 1024**3,
            "Insufficient backup space; choose another filesystem or --bh-backup-mode move")
    for name in sorted(before, key=lambda v: (v.count("/"), v)):
        entry = before[name]
        src, dst = (source, destination) if name == "." else (source / name, destination / name)
        safe_path(dst)
        if entry["kind"] == "directory":
            dst.mkdir(mode=0o700, exist_ok=True)
            continue
        require(not dst.exists() or (dst.is_file() and dst.stat().st_nlink == 1), f"Unsafe partial backup file: {dst}")
        sha = digest(src, progress) if dst.exists() else None
        if sha is None or digest(dst, progress) != sha:
            # A failed copy remains private and may be retried; no source is removed here.
            copied = hashlib.sha256()
            with src.open("rb") as inp, dst.open("wb") as output:
                while chunk := inp.read(CHUNK):
                    output.write(chunk)
                    copied.update(chunk)
                    progress.add(len(chunk))
                output.flush()
                os.fsync(output.fileno())
            shutil.copystat(src, dst)
            require(sha is None or sha == copied.hexdigest(), f"Source changed during copy: {src}")
            sha = copied.hexdigest()
        entry["sha256"] = sha
    after = inventory(source)
    require({k: {a: b for a, b in v.items() if a != "sha256"} for k, v in before.items()} == after,
            f"Source changed during backup: {source}")
    verify_tree(destination, before, reject_hardlinks=True)
    progress.show("finished")
    return before


def check_original(source: Path, expected: dict, *, recovering: bool):
    """An interrupted deletion may remove entries, but cannot introduce replacement data."""
    if not source.exists() and recovering:
        return
    current = inventory(source)
    if not recovering:
        require(current == {k: {a: b for a, b in v.items() if a != "sha256"} for k, v in expected.items()},
                f"Source changed after backup; refusing cleanup: {source}")
        return
    require(current.keys() <= expected.keys(), f"New data appeared during interrupted cleanup: {source}")
    progress = Progress("Check remaining source", source)
    for relative, item in current.items():
        old = expected[relative]
        require(item["kind"] == old["kind"] and item["stamp"][:2] == old["stamp"][:2], f"Source entry replaced during cleanup: {source / relative}")
        # Removing another hardlink can change ctime without changing the retained file.
        if item["kind"] == "file" and item["stamp"] != old["stamp"]:
            require(digest(source / relative, progress) == old["sha256"], f"Remaining source changed: {source / relative}")
    progress.show("finished")


def lock_file(path: Path, stack: ExitStack):
    """OFD locks survive closing other readers of LOCK during copying and hashing."""
    safe_path(path)
    descriptor = os.open(path, os.O_RDWR | os.O_NOFOLLOW)
    stack.callback(os.close, descriptor)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        require(hasattr(fcntl, "F_OFD_SETLK") and ctypes.sizeof(ctypes.c_void_p) == 8, "Backup locking requires 64-bit Linux with OFD locks")

        class FileLock(ctypes.Structure):
            _fields_ = [("type", ctypes.c_short), ("whence", ctypes.c_short),
                        ("start", ctypes.c_longlong), ("length", ctypes.c_longlong), ("pid", ctypes.c_int)]

        fcntl.fcntl(descriptor, fcntl.F_OFD_SETLK, bytes(FileLock(fcntl.F_WRLCK, os.SEEK_SET, 0, 0, 0)))
    except BlockingIOError as error:
        raise ValueError(f"Database or node operation is still locked: {path}") from error


def acquire_locks(path: Path, stack: ExitStack):
    """Hold both Unix lock variants on existing DB/control lock files while preserving data."""
    if not path.is_dir():
        return
    for directory, dirs, files in os.walk(path, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            if name not in {"LOCK", ".lock"}:
                continue
            lock_file(Path(directory) / name, stack)


def remove_tree(path: Path):
    """Keep a heartbeat during potentially long safe recursive removal."""
    require(shutil.rmtree.avoids_symlink_attacks, "Platform lacks safe directory removal")
    progress, stop = Progress("Delete", path), threading.Event()

    def heartbeat():
        while not stop.wait(10):
            progress.show("progress")

    thread = threading.Thread(target=heartbeat, daemon=True)
    thread.start()
    try:
        shutil.rmtree(path)
        progress.show("finished")
    finally:
        stop.set()
        thread.join()


def confirm(action: str, path: Path, extra="") -> bool:
    """Every source mutation gets a fresh, default-no confirmation; EOF cancels."""
    print(f"\n{action}: {path}\n{extra}", flush=True)
    reply = input("Type yes to proceed, Enter to skip, or q to quit: ").strip().lower()
    if reply in {"q", "quit"}:
        raise KeyboardInterrupt
    return reply == "yes"


class Session:
    def __init__(self, plan: Plan, root: Path, mode: str):
        self.plan, self.root, self.mode = plan, root, mode
        self.manifest = root / "session.json"
        identity = dict(schema_version=SCHEMA, hostname=socket.gethostname(), home=str(plan.home),
                        data_root=str(plan.root), bundle=plan.bundle, bh_backup_mode=mode, env_sha256=plan.env_sha256)
        if self.manifest.exists():
            self.state = read_json(self.manifest)
            require(self.state["identity"] == identity, "Backup session belongs to another host, node or mode")
        else:
            require(all(p.name == ".lock" and p.is_file() and not p.is_symlink() for p in root.iterdir()), "Backup directory must be empty or contain this tool's session")
            self.state = dict(identity=identity, items={}, events=[],
                              sources={t.key: stamp(t.path)[:2] if t.path.exists() or t.path.is_symlink() else None for t in plan.targets})
            self.save()
        safe_path(root / "objects")
        (root / "objects").mkdir(mode=0o700, exist_ok=True)

    def check_target(self, target):
        safe_path(target.path, leaf_link=bool(target.link))
        if target.path.exists() or target.path.is_symlink():
            require(stamp(target.path)[:2] == self.state["sources"].get(target.key), f"Target replaced or created after planning: {target.path}")

    def save(self):
        atomic_json(self.manifest, self.state)

    def event(self, action, path):
        self.state["events"].append(dict(action=action, path=str(path), time=time.time()))
        self.save()

    def preserve(self, target: Target) -> bool:
        source = target.path
        self.check_target(target)
        previous = self.state["items"].get(target.key)
        destination = self.root / "objects" / target.key
        # A persisted intent precedes rename, allowing copy and move publication to reconcile.
        if previous and not previous.get("complete") and destination.exists():
            require(previous["source"] == str(source), "Backup source changed")
            require(previous["mode"] != "move" or not source.exists(), "Both source and moved destination exist; inspect before retrying")
            for relative, tree in previous["trees"].items():
                require(relative == "." or relative in (target.preserve or ()), "Unexpected backup selection")
                verify_tree(destination if relative == "." else destination / relative, tree, reject_hardlinks=previous["mode"] == "copy")
            previous["complete"] = True
            self.event("backup_reconciled", source)
        if previous and previous.get("complete"):
            require(previous["source"] == str(source), "Backup source changed")
            self.verify(target)
            return True
        if not source.exists():
            return False
        partial = destination.with_name(destination.name + ".partial")
        safe_path(partial)
        moving = self.mode == "move" and target.key == "balance-history"
        scope = "entire path" if not target.preserve else "selected identities/logs only: " + ", ".join(target.preserve)
        if not confirm("MOVE TO ARCHIVE" if moving else "BACKUP", source,
                       f"Destination: {destination}\nPreserve: {scope}" + ("; the active BH path will disappear, leaving one offline copy" if moving else "; source remains until separately confirmed cleanup")):
            return False
        check_stopped(self.plan)
        with ExitStack() as locks:
            acquire_locks(source, locks)
            if self.mode == "move" and target.key == "balance-history":
                require(source.stat().st_dev == destination.parent.stat().st_dev, "BH move requires the same filesystem")
                require(not destination.exists() and not partial.exists(), "Move destination already exists")
                progress = Progress("Verify before move", source)
                before = inventory(source)
                for name, entry in before.items():
                    if entry["kind"] == "file":
                        entry["sha256"] = digest(source / name, progress)
                progress.show("finished")
                require(inventory(source) == {k: {a: b for a, b in v.items() if a != "sha256"} for k, v in before.items()}, "BH changed during move verification")
                check_stopped(self.plan, deleting=source)
                self.state["items"][target.key] = dict(source=str(source), mode="move", complete=False, trees={".": before})
                self.save()
                os.rename(source, destination)
                sync_dir(source.parent)
                sync_dir(destination.parent)
                verify_tree(destination, before)
                trees = {".": before}
            else:
                require(not destination.exists(), "Unverified final backup exists; preserve it and inspect before retrying")
                trees = {}
                if target.preserve:
                    partial.mkdir(mode=0o700, exist_ok=True)
                    for relative in target.preserve:
                        item = source / relative
                        safe_path(item)
                        if item.exists():
                            (partial / relative).parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                            trees[relative] = copy_tree(item, partial / relative)
                else:
                    trees["."] = copy_tree(source, partial)
                check_stopped(self.plan)
                self.state["items"][target.key] = dict(source=str(source), mode="copy", complete=False, trees=trees)
                self.save()
                os.rename(partial, destination)
                sync_dir(destination.parent)
            self.state["items"][target.key] = dict(source=str(source), mode="move" if target.key == "balance-history" and self.mode == "move" else "copy", complete=True, trees=trees)
            self.event("backup_verified", source)
        return True

    def verify(self, target: Target):
        record = self.state["items"][target.key]
        require(record["complete"] and record["source"] == str(target.path), "No completed backup for target")
        for relative, tree in record["trees"].items():
            require(relative == "." or relative in (target.preserve or ()), "Unexpected backup selection")
            path = self.root / "objects" / target.key
            verify_tree(path if relative == "." else path / relative, tree, reject_hardlinks=record["mode"] == "copy")

    def clean(self, target: Target):
        source = target.path
        self.check_target(target)
        safe_path(source, leaf_link=bool(target.link))
        if not source.exists() and not source.is_symlink():
            return
        if target.preserve is not None and not self.state["items"].get(target.key, {}).get("complete"):
            print(f"SKIPPED cleanup without verified backup: {source}")
            return
        # Old registry artifacts are retained until a full BH backup exists.
        if target.key == "legacy-snapshots" and not self.state["items"].get("balance-history", {}).get("complete"):
            print(f"SKIPPED cleanup without BH/registry backup: {source}")
            return
        backup_note = "No full data backup is made for this target." if target.preserve is None or target.preserve else "An independently verified full copy is retained."
        if not confirm("DELETE", source, "This removes the selected path permanently. " + backup_note):
            return
        check_stopped(self.plan, deleting=source)
        safe_path(source, leaf_link=bool(target.link))
        check_mounts(source)
        with ExitStack() as locks:
            acquire_locks(source, locks)
            if target.preserve is not None:
                self.verify(target)
                record = self.state["items"][target.key]
                recovering = any(e["action"] == "delete_started" and e["path"] == str(source) for e in self.state["events"])
                for relative, tree in record["trees"].items():
                    original = source if relative == "." else source / relative
                    check_original(original, tree, recovering=recovering)
                if target.preserve:
                    present = {p for p in target.preserve if (source / p).exists()}
                    require(present <= set(record["trees"]) if recovering else present == set(record["trees"]), "New identity material appeared after backup")
            if target.key == "legacy-snapshots":
                self.verify(next(t for t in self.plan.targets if t.key == "balance-history"))
            if target.link:
                require(os.readlink(source) == target.link, "Launcher changed after planning")
            self.check_target(target)
            check_stopped(self.plan, deleting=source)
            check_mounts(source)
            self.event("delete_started", source)
            if target.key == "controller-unit" and not os.access(source.parent, os.W_OK):
                subprocess.run(["sudo", "rm", "--", str(source)], check=True)
            elif source.is_dir():
                remove_tree(source)
            else:
                source.unlink()
            sync_dir(source.parent)
            self.event("deleted", source)
        if target.key == "controller-unit":
            subprocess.run(["sudo", "systemctl", "daemon-reload"], check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("plan", "run", "verify"))
    parser.add_argument("--backup-dir", required=True, type=Path, help="Dedicated directory outside all active node roots")
    parser.add_argument("--operator-home", type=Path, default=Path.home())
    parser.add_argument("--bundle-id", default="usdb-testnet-v0")
    parser.add_argument("--expect-host", default="bucky04", help="Execution host guard (node1 defaults to bucky04)")
    parser.add_argument("--bh-backup-mode", choices=("copy", "move"), default="copy", help="move preserves a single offline BH copy on the same filesystem")
    args = parser.parse_args()
    try:
        plan = build_plan(args.operator_home, args.bundle_id, args.backup_dir)
        print(f"Host={socket.gethostname()}, bundle={plan.bundle}, data_root={plan.root}, backup={args.backup_dir}")
        for t in plan.targets:
            scope = "none" if t.preserve is None else "entire path" if not t.preserve else ", ".join(t.preserve)
            print(f"{t.key}: {t.path}\n  preserve={scope}; exists={t.path.exists() or t.path.is_symlink()}")
        if args.action == "plan":
            return 0
        require(socket.gethostname() == args.expect_host, "Host differs from --expect-host")
        if args.action == "run":
            require(sys.stdin.isatty() and sys.stdout.isatty(), "run requires an interactive terminal; no --yes or piped approval is supported")
            check_stopped(plan)
            if not confirm("PREPARE BACKUP DIRECTORY", args.backup_dir, "Private configurations and keys will be stored here. Copy does not free space; move keeps only one offline BH copy."):
                return 0
            args.backup_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        require(args.backup_dir.is_dir() and stat.S_IMODE(args.backup_dir.stat().st_mode) & 0o077 == 0, "Backup directory must be private (mode 0700)")
        require(args.action != "verify" or (args.backup_dir / "session.json").is_file(), "No backup session to verify")
        with ExitStack() as locks:
            safe_path(args.backup_dir / ".lock")
            lock = locks.enter_context((args.backup_dir / ".lock").open("a+"))
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            session = Session(plan, args.backup_dir, args.bh_backup_mode)
            if args.action == "verify":
                require(any(r.get("complete") for r in session.state["items"].values()), "No completed backups; run again to resume preparation")
                require(all(r.get("complete") for r in session.state["items"].values()), "An interrupted backup needs reconciliation with run")
                for target in plan.targets:
                    if session.state["items"].get(target.key, {}).get("complete"):
                        session.verify(target)
                return 0
            operation_lock = plan.env_path.parent / ".usdb-node-operation.lock"
            if operation_lock.exists():
                lock_file(operation_lock, locks)
            # Preserve private config and old readers first, then the large database.
            for target in sorted(plan.targets, key=lambda t: t.key == "balance-history"):
                if target.preserve is not None:
                    session.preserve(target)
            for target in plan.targets:
                session.clean(target)
            print(f"Finished selected operations. Free data bytes={shutil.disk_usage(plan.root).free}; setup requires {3 * 1024**4 // 2}. Inspect skipped paths before reinstalling.")
            print("Retained paths: " + ", ".join(str(t.path) for t in plan.targets if t.path.exists() or t.path.is_symlink()))
        return 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"Node rebuild failed: {error}", file=sys.stderr)
        return 1
    except (KeyboardInterrupt, EOFError):
        print("Cancelled; retained backups and partial copies have not been removed.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
