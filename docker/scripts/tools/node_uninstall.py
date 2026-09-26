#!/usr/bin/env python3
"""Operator uninstall: preview first, preserve data by default, archive private state before purge.

The execution copy lives outside release/data roots and only depends on node_rebuild.py.
It can resume after the launcher, release kits or node.env have been removed.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import fcntl
import hashlib
import os
from pathlib import Path
import pwd
import re
import shutil
import socket
import stat
import subprocess
import sys

import node_rebuild as core

SCHEMA = "usdb-node-uninstall:v1"
UNITS = ("usdb-node-bootstrap", "usdb-node-monitor", "usdb-console-monitor")
# Everything else in these directories is archived, including nonstandard wallets.
BITCOIN_REBUILDABLE = {"blocks", "chainstate", "chainstate_snapshot", "indexes", "debug.log"}
CHAIN_REBUILDABLE = {"chaindata", "ancient", "lightchaindata", "triecache"}


def operator_home():
    """Keep the original operator identity when an explicit sudo invocation is used."""
    uid = int(os.environ.get("SUDO_UID", os.getuid())) if os.geteuid() == 0 else os.getuid()
    return Path(pwd.getpwuid(uid).pw_dir)


def read_env(path):
    values = {}
    for line in path.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, separator, value = line.partition("=")
        core.require(separator and key not in values, "Invalid or duplicate node.env field")
        values[key] = value
    return values


def plan(home, bundle, backup, purge=False, unit_dir=Path("/etc/systemd/system")):
    """Reuse the reviewed v2 path boundary; never select arbitrary roots or glob data."""
    # Public uninstall does not need to inspect the private RocksDB child to plan.
    base = core.build_plan(home, bundle, backup, unit_dir, protect_script=False, archive_bh=False)
    env = read_env(base.env_path)
    candidates = [target for target in base.targets if target.key != "balance-history"]
    for prefix in UNITS[1:]:
        candidates.append(core.Target(prefix, unit_dir / f"{prefix}-{bundle}.service", None))
    ord_path = env.get("ORD_DATA_HOST_DIR")
    if ord_path:
        path = core.absolute(ord_path)
        core.require(path.parent == base.root / "datasets/ord/btc-mainnet" and re.fullmatch(r"ord-[0-9]+\.[0-9]+\.[0-9]+", path.name),
                     "Unexpected Ord data path; review the configured dataset before uninstalling")
        candidates.append(core.Target("ord", path, None))
    artifact = env.get("BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR")
    state = env.get("BTC_ASSUMEUTXO_STATE_HOST_DIR")
    for key, expected in ((artifact, base.root / "artifacts/assumeutxo/mainnet-935000"),
                          (state, base.root / "networks" / bundle / "assumeutxo")):
        core.require(not key or key == str(expected), "Unexpected AssumeUTXO path; review before uninstalling")
    software = {"launcher", "controller-unit", *UNITS[1:]}
    selected = [target for target in candidates if purge or target.key in software or target.key.startswith(bundle + "-r")]
    # Remove definitions first and launcher last; an interrupted uninstall stays recoverable.
    selected.sort(key=lambda target: (target.key == "launcher", target.key == "config"))
    for target in candidates:
        core.safe_path(target.path, leaf_link=bool(target.link))
        core.require(not core.overlap(target.path, backup), "Backup overlaps an uninstall target")
        if target in selected and target.path.exists() and not target.link:
            core.check_mounts(target.path)
    return dict(schema_version=SCHEMA, hostname=socket.gethostname(), operator_uid=home.stat().st_uid,
                home=str(home), bundle=bundle, data_root=str(base.root), env_path=str(base.env_path),
                env_sha256=base.env_sha256, purge_data=purge,
                targets=[dict(key=t.key, path=str(t.path), link=t.link, identity=core.stamp(t.path)[:2]
                              if t.path.exists() or t.path.is_symlink() else None) for t in selected],
                retained=[str(t.path) for t in candidates if t not in selected],
                scope=[str(t.path) for t in candidates], units=[str(unit_dir / f"{p}-{bundle}.service") for p in UNITS])


def show(value, backup):
    """Show exact removal/retention boundaries without exposing config contents."""
    print(f"USDB uninstall | host={value['hostname']} | network={value['bundle']}")
    print("Mode: remove software and configured node data" if value["purge_data"] else "Mode: remove software; retain configuration and all node data")
    print("\nRemove after execution confirmation:")
    for item in value["targets"]:
        print(f"  {item['key']}: {item['path']}" + (" (not present at planning)" if item["identity"] is None else ""))
    if value["retained"]:
        print("\nRetain:")
        for path in value["retained"]:
            print("  " + path)
    print(f"\nPrivate backup and resumable record: {backup}")
    if value["purge_data"]:
        print("Before deletion: archive and SHA-256 verify configuration, monitor history, node identities, wallet/private state and service definitions.")
        print("Bitcoin blocks, BH/indexer/chain databases, Ord index and downloaded snapshots are NOT backed up; synchronization will restart.")
    print("Docker images/volumes, Docker packages, firewall rules, unrelated networks and unreferenced old datasets remain installed.")


def runtime_plan(value):
    """The original scope includes retained data so shared active users also block uninstall."""
    return core.Plan(Path(value["home"]), Path(value["data_root"]), value["bundle"], Path(value["env_path"]),
                     [core.Target(str(i), Path(path), None) for i, path in enumerate(value["scope"])],
                     value["env_sha256"], Path(value["data_root"]), {})


def check_stopped(value, *, deleting=None):
    """Require down/disable first; observation failure never authorizes removal."""
    for path in value["units"]:
        unit = Path(path).name
        output = core.command(["systemctl", "show", unit, "--property=LoadState,ActiveState,UnitFileState"], accepted=(0, 1, 4))
        fields = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
        core.require("LoadState" in fields and "ActiveState" in fields, f"Cannot inspect {unit}; no paths will be changed")
        core.require(fields["LoadState"] == "not-found" or (fields["ActiveState"] in {"inactive", "failed"}
                     and fields.get("UnitFileState") in {"disabled", "masked", "static"}),
                     f"Stop the node with usdb-node down, then usdb-node controller disable; {unit} is still active or enabled")
        # Drop-ins can start another executable or contain private environment values.
        core.require(not Path(path + ".d").exists(), f"Custom service overrides require manual review: {path}.d")
    paths = [Path(p) for p in value["scope"]]
    for item in core.containers():
        sources = [Path(m["Source"]) for m in item["mounts"] if m.get("Source", "").startswith("/")]
        affected = core.selected_container(runtime_plan(value), item) or any(core.overlap(p, s) for p in paths for s in sources)
        core.require(not affected or item["state"] in {"exited", "created", "dead"},
                     f"Container still uses this node: {item['id'][:12]}; stop it before uninstalling")
        core.require(not deleting or not any(core.overlap(deleting, s) for s in sources),
                     f"Container still references {deleting}; review/remove it before retrying")


def private_sources(value):
    """Keep unknown files, wallets and identities; exclude only known large rebuildable DBs."""
    sources = []
    for item in value["targets"]:
        root = Path(item["path"])
        key = item["key"]
        if not root.exists() or item["link"] or key.startswith(value["bundle"] + "-r"):
            continue
        if key in {"config", "secure", "control-plane", "controller-unit", *UNITS[1:]}:
            sources.append((key, root))
        elif key in {"bitcoin", "chain", "ord"}:
            for child in sorted(root.iterdir()):
                if key == "bitcoin" and child.name in BITCOIN_REBUILDABLE:
                    continue
                if key == "ord" and child.name in {"index.redb", "index.redb.lock"}:
                    continue
                if key == "chain" and child.name == "geth" and child.is_dir() and not child.is_symlink():
                    sources.extend(("chain/geth/" + entry.name, entry) for entry in sorted(child.iterdir()) if entry.name not in CHAIN_REBUILDABLE)
                else:
                    sources.append((key + "/" + child.name, child))
    return sources


def check_targets(value):
    """Refuse replacements/new nodes; partially removed original targets can resume."""
    for item in value["targets"]:
        path = Path(item["path"])
        core.safe_path(path, leaf_link=bool(item["link"]))
        if path.exists() or path.is_symlink():
            core.require(core.stamp(path)[:2] == item["identity"], f"Target changed since planning: {path}")
            if item["link"]:
                core.require(os.readlink(path) == item["link"], "Launcher changed since planning")
            else:
                core.check_mounts(path)
    env = Path(value["env_path"])
    if env.exists():
        core.require(hashlib.sha256(env.read_bytes()).hexdigest() == value["env_sha256"], "node.env changed since planning; inspect before retrying")


class Session:
    """Verified private archives and an fsynced journal precede every destructive step."""

    def __init__(self, root):
        self.root, self.path = root, root / "uninstall.json"
        core.safe_path(root)
        core.require(stat.S_IMODE(root.stat().st_mode) & 0o077 == 0, "Uninstall backup must be private (0700)")
        core.safe_path(self.path)
        metadata = self.path.stat()
        core.require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and not metadata.st_mode & 0o077,
                     "Uninstall session must be a private regular file (0600)")
        self.state = core.read_json(self.path)
        self.plan = self.state["plan"]
        core.require(self.plan["schema_version"] == SCHEMA and self.plan["hostname"] == socket.gethostname(), "Uninstall session belongs to a different host or version")
        core.require(os.geteuid() in {0, self.plan["operator_uid"]}, "Run uninstall as the original operator")
        for path in self.plan["scope"]:
            core.require(not core.overlap(root, core.absolute(path)), "Uninstall backup overlaps a selected path")
        core.require(all(item["path"] in self.plan["scope"] for item in self.plan["targets"]), "Uninstall target is outside its recorded scope")

    def save(self):
        core.atomic_json(self.path, self.state)

    def event(self, action, path):
        self.state["events"].append(dict(action=action, path=str(path), at=datetime.now(timezone.utc).isoformat()))
        self.save()

    def backup(self, only=None):
        """Prepare once; before each removal, reverify only that target's private state."""
        records = self.state["backups"]
        if not self.state.get("backup_complete"):
            for key, source in private_sources(self.plan):
                destination = self.root / "private" / key
                core.safe_path(destination)
                destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                if key not in records:
                    tree = core.copy_tree(source, destination)
                    ownership = {}
                    for relative in tree:
                        metadata = (source if relative == "." else source / relative).lstat()
                        ownership[relative] = dict(uid=metadata.st_uid, gid=metadata.st_gid)
                    records[key] = dict(source=str(source), tree=tree, ownership=ownership)
                    self.event("backup_verified", source)
            self.state["backup_complete"] = True
            self.save()
        for key, record in records.items():
            if only is not None and not core.overlap(only, Path(record["source"])):
                continue
            core.verify_tree(self.root / "private" / key, record["tree"], reject_hardlinks=True)
        # Do not accept a newly created wallet that was absent from the verified selection.
        for key, source in private_sources(self.plan):
            if only is not None and not core.overlap(only, source):
                continue
            core.require(key in records, f"New private state appeared after backup: {source}")
            recovering = any(e["action"] == "delete_started" and core.overlap(Path(e["path"]), source) for e in self.state["events"])
            core.check_original(source, records[key]["tree"], recovering=recovering)

    def run(self):
        core.require(sys.stdin.isatty() and sys.stdout.isatty(), "Uninstall execution requires an interactive terminal; no --yes or piped confirmation is supported")
        check_targets(self.plan)
        check_stopped(self.plan)
        show(self.plan, self.root)
        phrase = ("PURGE " if self.plan["purge_data"] else "UNINSTALL ") + self.plan["bundle"] + " " + self.plan["hostname"]
        print(f"\nType exactly '{phrase}' to execute; anything else cancels:", flush=True)
        if input().strip() != phrase:
            print("Cancelled; no node files were removed.")
            return 0
        with ExitStack() as locks:
            lock_path = self.root / ".lock"
            core.safe_path(lock_path)
            lock = locks.enter_context(lock_path.open("a+"))
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            # Preserve the operator and foreground-monitor locks throughout deletion.
            config = Path(self.plan["env_path"]).parent
            for path in (config / ".usdb-node-operation.lock", config / "monitor/run.lock", config / "sourcedao/.operation.lock"):
                if path.exists():
                    core.lock_file(path, locks)
            check_targets(self.plan)
            check_stopped(self.plan)
            for item in self.plan["targets"]:
                core.acquire_locks(Path(item["path"]), locks)
            self.backup()
            for item in core.containers():
                if core.selected_container(runtime_plan(self.plan), item):
                    check_stopped(self.plan)
                    core.require(item["state"] in {"exited", "created", "dead"}, "Container started during uninstall")
                    # rm without --force or --volumes refuses a concurrently started container.
                    core.command(["docker", "rm", "--", item["id"]])
                    self.event("container_removed", item["id"])
            for item in self.plan["targets"]:
                source = Path(item["path"])
                if not source.exists() and not source.is_symlink():
                    continue
                check_targets(self.plan)
                check_stopped(self.plan, deleting=source)
                self.backup(only=source)
                self.event("delete_started", source)
                if source.is_dir() and not source.is_symlink():
                    core.remove_tree(source)
                else:
                    source.unlink()
                core.sync_dir(source.parent)
                self.event("deleted", source)
                if str(source) in self.plan["units"]:
                    # Refresh cached unit metadata before the next stopped-state check.
                    core.command(["systemctl", "daemon-reload"])
            core.command(["systemctl", "daemon-reload"])
            self.state["complete"] = True
            self.save()
        print(f"Uninstall complete. Private backup and operation record: {self.root}")
        if self.plan["purge_data"]:
            print("Install a release and run setup to synchronize again; retain the private backup for wallet/node identity recovery.")
        else:
            print("Configuration and data retained. Reinstall the same network, activate-release, doctor, then up to resume.")
        return 0


def stage(value, backup):
    """Create an independent runner before deleting any installed release kit."""
    core.safe_path(backup)
    core.require(not backup.exists(), "Choose a new backup directory, or resume the existing uninstall with its saved runner")
    backup.mkdir(parents=True, mode=0o700)
    runner = backup / "runner"
    runner.mkdir(mode=0o700)
    for source in (Path(__file__).resolve(), Path(core.__file__).resolve()):
        shutil.copyfile(source, runner / source.name)
        (runner / source.name).chmod(0o600)
    core.atomic_json(backup / "uninstall.json", dict(plan=value, backups={}, events=[], complete=False))
    return runner / "node_uninstall.py"


def add_parser(subparsers):
    parser = subparsers.add_parser("uninstall", help="Preview node removal; retain data unless --purge-data is explicit",
        description="Preview exact paths by default. Execution requires a stopped/disabled node and interactive confirmation; private state is verified before any purge.")
    parser.add_argument("--purge-data", action="store_true", help="Also remove configured datasets and config after private backup; block/index data must be synchronized again")
    parser.add_argument("--backup-dir", type=Path, help="New private backup/resume directory outside node data and release roots")
    parser.add_argument("--execute", action="store_true", help="Stage a resumable runner and request interactive execution (may use sudo)")


def dispatch(args, layout, node):
    home = operator_home()
    core.require(layout.node_env == home / ".config/usdb" / layout.bundle_id / "node.env", "Uninstall currently requires the standard bundle-scoped node.env path; custom layouts need manual review")
    backup = args.backup_dir or home / ".local/state/usdb" / ("uninstall-" + layout.bundle_id + "-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"))
    backup = core.absolute(backup)
    value = plan(home, layout.bundle_id, backup, args.purge_data)
    show(value, backup)
    if not args.execute:
        print("\nPreview only. Before execution: usdb-node down, then usdb-node controller disable.")
        print("Repeat with --execute and the desired --backup-dir; use --purge-data only for a clean resynchronization.")
        return 0
    core.require(sys.stdin.isatty() and sys.stdout.isatty(), "Uninstall execution requires an interactive terminal")
    import usdb_sourcedao
    usdb_sourcedao.require_idle(layout)
    check_stopped(value)
    script = stage(value, backup)
    command = [sys.executable, str(script), "--resume", str(backup)]
    if os.geteuid() != 0:
        command = ["sudo", "--", *command]
    import shlex
    print("Resume after interruption: " + shlex.join(command), flush=True)
    return subprocess.run(command, check=False).returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resume", type=Path, required=True, help="Use the same private uninstall directory after interruption")
    parser.add_argument("--plan", action="store_true", help="Inspect the saved plan without execution")
    args = parser.parse_args()
    try:
        session = Session(core.absolute(args.resume))
        if args.plan:
            show(session.plan, session.root)
            return 0
        return session.run()
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"Node uninstall failed: {error}. Retain this session and resume after resolving the blocker.", file=sys.stderr)
        return 1
    except (KeyboardInterrupt, EOFError):
        print("Cancelled; private backups and the operation journal are retained.", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
