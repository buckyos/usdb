"""Explicit, resumable cleanup of one applied upgrade's unreferenced archives.

Only immutable-kit-derived old paths are removable. All non-database files are
copied and hash-verified first; the upgrade journal disables rollback durably
before deletion, including when an older r7 recovery runner is used.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack, redirect_stdout
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
from types import SimpleNamespace

import node_rebuild as core
import node_upgrade as upgrade
import node_upgrade_archives as archives
import node_upgrade_session as session
import node_uninstall as uninstall

SCHEMA = "usdb-upgrade-cleanup:v1"
INDEX_DATABASES = {"sync_state.db", "transfer.db", "inscriptions.db", "miner_pass.db", "address_balance.db"}


def catalog_lock(data_root, stack, owner=None):
    """Serialize upgrade/cleanup decisions sharing a dataset namespace."""
    directory = Path(data_root) / archives.CATALOG
    core.safe_path(directory)
    directory.mkdir(mode=0o700, exist_ok=True)
    path = directory / ".lock"
    core.safe_path(path)
    if not path.exists():
        path.touch(mode=0o600, exist_ok=False)
    if os.geteuid() == 0 and owner is not None:
        for item in (directory, path):
            os.chown(item, *owner)
    core.lock_file(path, stack)


def binding(state):
    """Bind cleanup to the complete original upgrade, apart from its terminal phase."""
    value = dict(state, phase="applied")
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def private_paths(kind, tree):
    """Exempt only standard derived database locations; preserve all unknown files."""
    def database(name):
        parts = Path(name).parts
        if kind == "usdb_chain":
            return (len(parts) >= 2 and parts[0] == "geth" and parts[1] in uninstall.CHAIN_REBUILDABLE
                    and tree.get("/".join(parts[:2]), {}).get("kind") == "directory")
        if kind == "usdb_indexer" and len(parts) >= 2 and parts[0] == "data":
            return ((parts[1] == "energy" and tree.get("data/energy", {}).get("kind") == "directory")
                    or (len(parts) == 2 and tree[name]["kind"] == "file" and parts[1] in
                        {n + suffix for n in INDEX_DATABASES for suffix in ("", "-wal", "-shm", "-journal")}))
        return False
    # Mark ancestors once; an index can contain millions of database files.
    mixed = {"."}
    excluded = {name for name in tree if database(name)}
    if not excluded:
        return ["."]
    for name in excluded:
        mixed.update(str(parent) for parent in Path(name).parents)
    result = []
    selected = set()
    for name in sorted(tree, key=lambda n: (len(Path(n).parts), n)):
        core.require(name == "." or (not Path(name).is_absolute() and ".." not in Path(name).parts), "Unsafe cleanup inventory entry")
        if name in excluded or any(str(parent) in selected for parent in Path(name).parents):
            continue
        if name not in mixed:
            result.append(name)
            selected.add(name)
    return result


def validate_record(record, state, items):
    """Validate durable cleanup metadata before reporting or resuming it."""
    core.require(record.get("schema_version") == SCHEMA and record.get("upgrade_sha256") == binding(state),
                 "Cleanup journal does not match this upgrade")
    core.require(set(record["targets"]) == {i["path"] for i in items}, "Cleanup target set changed")
    for item in items:
        target = record["targets"][item["path"]]
        core.require(target["preserve"] == private_paths(item["kind"], target["tree"]), "Cleanup preservation set changed")
        core.require(set(target["backups"]) <= set(target["preserve"]), "Unexpected cleanup backup entries")
        core.require(not target["deleted"] or target["delete_started"], "Invalid cleanup delete completion")
    core.require(state["phase"] != "cleaned" or all(r["deleted"] for r in record["targets"].values()),
                 "Cleanup completion record is incomplete")


def verify_remaining(path, expected, partial=False):
    """A resumed deletion may lose old entries but must never gain/reuse new ones."""
    actual, _ = archives.measure(path)
    core.require(actual.keys() <= expected.keys() and (partial or actual.keys() == expected.keys()),
                 f"Archive entries changed during cleanup: {path}")
    for name, item in actual.items():
        previous = expected[name]
        left, right = item["stamp"][:], previous["stamp"][:]
        # Removing children changes directory size and timestamps, not identity/mode.
        indices = (0, 1, 5) if partial and item["kind"] == "directory" else (0, 1, 2, 3, 5)
        core.require(item["kind"] == previous["kind"] and all(left[i] == right[i] for i in indices),
                     f"Archive identity or contents changed: {path / name}")


def open_processes(paths, proc_root=Path("/proc")):
    """Reject native processes holding archive files, mappings or working directories."""
    for process in proc_root.iterdir():
        if not process.name.isdigit() or int(process.name) == os.getpid():
            continue
        try:
            links = [process / "cwd", process / "root", *list((process / "fd").iterdir())]
            values = []
            for link in links:
                try:
                    values.append(os.readlink(link))
                except FileNotFoundError:
                    continue
            # mmap can keep a database in use after its descriptor was closed.
            for line in (process / "maps").read_text().splitlines():
                fields = line.split(maxsplit=5)
                if len(fields) == 6:
                    values.append(fields[5])
            for value in values:
                if value.startswith("/"):
                    path = Path(value.removesuffix(" (deleted)"))
                    # '/' as a process root is not a consumer of all child datasets.
                    if any(path == p or p in path.parents for p in paths):
                        raise ValueError(f"Archive is open by process pid={process.name}: {path}")
        except FileNotFoundError:
            continue  # A process exited during this read-only scan.


class Cleanup:
    """Persist preservation evidence and delete intent before touching each old tree."""
    def __init__(self, root, node, extra=()):
        self.root, self.node, self.extra = core.absolute(root), node, extra
        self.state = archives.load(self.root, node)
        self.plan = self.state["plan"]
        self.items = archives.candidates(self.state)
        self.path = self.root / "cleanup.json"
        core.safe_path(self.path)
        self.record = core.read_json(self.path) if self.path.exists() else None
        if self.record:
            validate_record(self.record, self.state, self.items)
        core.require(self.state["phase"] == "applied" or self.record is not None, "Cleanup state is missing; do not delete recovery records")

    def save(self):
        core.atomic_json(self.path, self.record)
        if os.geteuid() == 0:
            os.chown(self.path, self.plan["operator_uid"], self.plan["operator_gid"])

    def phase(self, phase):
        # New phases deliberately fail closed in old r7's recovery validator too.
        self.state["phase"] = phase
        core.atomic_json(self.root / "upgrade.json", self.state)
        if os.geteuid() == 0:
            os.chown(self.root / "upgrade.json", self.plan["operator_uid"], self.plan["operator_gid"])

    def check(self):
        upgrade.ensure_no_pending(SimpleNamespace(node_env=Path(self.plan["env_path"]), release_id=self.plan["target_release"], bundle_id=self.plan["bundle"]))
        reasons = archives.references(self.root, self.state, self.items, self.extra)
        for path, references in reasons.items():
            core.require(not references, f"Archive is still referenced: {path}: {'; '.join(references)}")
        uninstall.check_stopped(session.runtime_plan(self.plan), allow_enabled=True, inspect_install_rules=False)
        # This command removes no active containers and changes no autostart setting.
        open_processes([Path(i["path"]) for i in self.items])

    def prepare(self):
        if self.record is not None:
            return
        targets = {}
        for item in self.items:
            path = Path(item["path"])
            core.require(path.exists(), f"Retained archive is missing: {path}")
            if item["tree"]:
                core.verify_rename(path, item["tree"])
            if item["marker"]:
                core.require(upgrade.sha(path / archives.DATASET_IDENTITY_FILE) == item["marker"], f"Old dataset marker changed: {path}")
            tree, size = archives.measure(path)
            targets[str(path)] = dict(tree=tree, size=size, preserve=private_paths(item["kind"], tree), backups={},
                                      delete_started=False, deleted=False)
        self.record = dict(schema_version=SCHEMA, upgrade_sha256=binding(self.state), targets=targets)
        self.save()

    def preserve(self):
        """Finish and verify every private copy before the first archive deletion."""
        for index, item in enumerate(self.items):
            path = Path(item["path"])
            record = self.record["targets"][str(path)]
            base = self.root / "private/retained" / str(index)
            core.safe_path(base)
            base.mkdir(mode=0o700, parents=True, exist_ok=True)
            if not record["delete_started"]:
                verify_remaining(path, record["tree"])
            for number, relative in enumerate(record["preserve"]):
                destination = base / str(number)
                core.safe_path(destination)
                if relative not in record["backups"]:
                    core.require(not record["delete_started"], f"Missing private backup for started deletion: {path}")
                    source = path if relative == "." else path / relative
                    record["backups"][relative] = core.copy_tree(source, destination)
                    self.save()
                core.verify_tree(destination, record["backups"][relative], reject_hardlinks=True)
        # Keep the pre-upgrade credentials and configuration independently verified.
        for name in ("node.env", "config", "secure"):
            core.require(name in self.state["backups"], f"Missing original private backup: {name}")
            core.verify_tree(self.root / "private" / name, self.state["backups"][name], reject_hardlinks=True)

    def run(self, confirm=input):
        core.require(sys.stdin.isatty() and sys.stdout.isatty(), "Cleanup execution requires an interactive terminal")
        with redirect_stdout(sys.stderr):
            preview = archives.report(self.root, self.node, self.extra)
        archives.show(preview)
        core.require(preview["executable"], "Cleanup is blocked; no archive was deleted")
        if self.state["phase"] == "cleaned":
            return 0
        self.check()
        phrase = f"CLEANUP {self.plan['bundle']} {self.state['operation_id']} {socket.gethostname()}"
        print("Accept the new node and review any custom consumers/unregistered legacy records before continuing.")
        print(f"Permanently give up rollback and delete the listed old data. Type exactly '{phrase}':", flush=True)
        if confirm().strip() != phrase:
            print("Cancelled; no archive was deleted.")
            return 0
        with ExitStack() as locks:
            catalog_lock(self.plan["data_root"], locks, (self.plan["operator_uid"], self.plan["operator_gid"]))
            lock = self.root / ".lock"
            core.safe_path(lock)
            lock.touch(mode=0o600, exist_ok=True)
            core.lock_file(lock, locks)
            layout = SimpleNamespace(node_env=Path(self.plan["env_path"]), release_id=self.plan["target_release"], bundle_id=self.plan["bundle"])
            locks.enter_context(self.node.node_operation_lock(layout, "upgrade-cleanup"))
            self.__init__(self.root, self.node, self.extra)
            self.check()
            for item in self.items:
                core.acquire_locks(Path(item["path"]), locks)
            archives.register(self.root)
            self.prepare()
            self.phase("cleanup_started")
            self.preserve()
            for item in self.items:
                self.check()
                # Recheck copies before each removal, including on a resumed operation.
                self.preserve()
                path = Path(item["path"])
                record = self.record["targets"][str(path)]
                core.safe_path(path)
                if record["deleted"]:
                    core.require(not path.exists(), f"Deleted archive was recreated: {path}")
                    continue
                if path.exists():
                    verify_remaining(path, record["tree"], partial=record["delete_started"])
                    record["delete_started"] = True
                    self.save()
                    if path.is_dir():
                        core.remove_tree(path)
                    else:
                        path.unlink()
                    core.sync_dir(path.parent)
                else:
                    core.require(record["delete_started"], f"Archive disappeared before deletion: {path}")
                record["deleted"] = True
                self.save()
            self.phase("cleaned")
        print(f"Upgrade archives cleaned. Configuration/private backups and audit records remain at {self.root}.")
        print("Rollback is unavailable. Run usdb-node up when ready to restart the node.")
        return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backup-dir", type=Path, required=True)
    parser.add_argument("--other-backup-dir", type=Path, action="append", default=[])
    parser.add_argument("--inspect", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    import usdb_node as node
    try:
        root = core.absolute(args.backup_dir)
        if args.inspect:
            with redirect_stdout(sys.stderr):
                value = archives.report(root, node, args.other_backup_dir)
            print(json.dumps(value, indent=2, sort_keys=True)) if args.json else archives.show(value)
            return 0 if value["executable"] else 2
        core.require(not args.json, "--json is preview-only")
        core.require(os.geteuid() == 0, "Run the reviewed cleanup runner with sudo")
        return Cleanup(root, node, args.other_backup_dir).run()
    except KeyboardInterrupt:
        print("Cleanup interrupted; repeat upgrade-cleanup with the same --backup-dir. Rollback is unavailable once cleanup starts.", file=sys.stderr)
        return 130
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"Upgrade cleanup stopped: {error}. Preserve this recovery directory and resume cleanup after resolving the cause.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
