"""Inventory retained upgrade data and prove its provenance before explicit cleanup."""
from __future__ import annotations

import copy
from contextlib import redirect_stdout
import json
import os
from pathlib import Path
import pwd
import re
import shlex
import subprocess
import sys
from types import SimpleNamespace

import node_rebuild as core
import node_uninstall as uninstall
import node_upgrade as upgrade
import node_upgrade_session as session
from runtime_compatibility import DATASET_IDENTITY_FILE

CATALOG = ".usdb-upgrades"
CLEANUP_PHASES = {"cleanup_started", "cleaned"}


def register(root):
    """Register the location, never configuration secrets, of a durable upgrade record."""
    state = session.read(root)
    value = state["plan"]
    directory = core.absolute(value["data_root"]) / CATALOG
    core.safe_path(directory)
    directory.mkdir(mode=0o700, exist_ok=True)
    core.require(re.fullmatch(r"[0-9a-f]{32}", state["operation_id"]), "Invalid upgrade operation ID")
    path = directory / (state["operation_id"] + ".json")
    core.safe_path(path)
    record = dict(backup_dir=str(root), operation_id=state["operation_id"])
    if path.exists():
        core.require(core.read_json(path) == record, f"Upgrade registration changed: {path}")
    else:
        core.atomic_json(path, record)
    if os.geteuid() == 0:
        for item in (directory, path):
            os.chown(item, value["operator_uid"], value["operator_gid"])


def load(root, node):
    """Reuse path/contract validation without demanding that old target services stay unused."""
    root = core.absolute(root)
    core.safe_path(root)
    state = session.read(root)
    value = state["plan"]
    for role in ("source", "target"):
        path = core.absolute(value[role + "_kit"]) / "release/usdb-release-manifest.json"
        core.require(upgrade.sha(path) == value[role + "_manifest_sha256"], f"{role.title()} release changed since upgrade")
    target = upgrade._source_manifest(Path(value["target_kit"]), node)
    # The normal recovery validator proves every destination from the two immutable
    # kits and saved original node.env. It does not require today's env hash.
    validator = session.Session.__new__(session.Session)
    validator.root, validator.node = root, node
    validator.state, validator.plan = copy.deepcopy(state), value
    if state["phase"] in CLEANUP_PHASES:
        validator.state["phase"] = "applied"
    validator.layout = SimpleNamespace(node_env=Path(value["env_path"]), bundle_id=target["network_bundle"]["bundle_id"],
                                       network_identity=target["network_bundle"], runtime_compatibility=target["runtime_compatibility"])
    validator.uid = value["operator_uid"]
    validator.runtime = session.runtime_plan(value)
    validator.validate_journal()
    core.require(state["phase"] in {"applied", *CLEANUP_PHASES},
                 f"Cleanup requires an applied upgrade; recorded phase={state['phase']}. Finish recovery first")
    core.require(state["entries"] is not None and "node.env" in state["backups"], "Upgrade archive preparation is incomplete")
    return state


def candidates(state):
    """Select only old rebuilt datasets and precisely recorded isolation siblings."""
    result = [dict(kind=e["kind"], path=e["archive"], tree=e["tree"], marker=None) for e in state["entries"]]
    archived = {e["source"] for e in state["entries"]}
    for item in state["plan"]["components"]:
        if item["action"] != "rebuild":
            continue
        if item["source"] == item["target"]:
            core.require(item["source"] in archived, f"Missing retained archive for {item['service']}")
        else:
            result.append(dict(kind=item["service"], path=item["source"], tree=None, marker=item["marker_sha256"]))
    paths = [i["path"] for i in result]
    core.require(len(paths) == len(set(paths)), "Duplicate cleanup archive")
    for item in result:
        core.require(item["kind"] in {"usdb_indexer", "usdb_chain", "control_plane", *["state:" + n for n in (*upgrade.STATE_FILES, "monitor")]},
                     f"Unsupported archive kind: {item['kind']}")
        path = core.absolute(item["path"])
        core.safe_path(path)
        for current in state["plan"]["components"]:
            core.require(not core.overlap(path, Path(current["target"])), "Retained archive overlaps target dataset")
    return result


def record_paths(root, state, extra=()):
    """Discover registered records and legacy siblings; explicit paths cover other r7 locations."""
    paths = {root, *[core.absolute(p) for p in extra]}
    catalog = Path(state["plan"]["data_root"]) / CATALOG
    core.safe_path(catalog)
    if catalog.exists():
        for entry in sorted(catalog.glob("*.json")):
            core.safe_path(entry)
            record = core.read_json(entry)
            path = core.absolute(record["backup_dir"])
            other = session.read(path)
            core.require(other["operation_id"] == record["operation_id"] == entry.stem,
                         f"Upgrade catalogue identity mismatch: {entry}")
            paths.add(path)
    for path in root.parent.iterdir():
        if not path.is_symlink() and path.is_dir() and (path / "upgrade.json").exists():
            paths.add(path)
    return sorted(paths)


def configuration_paths(state):
    """Inspect standard per-account node configurations, including other networks/users."""
    homes = {Path(state["plan"]["operator_home"])}
    homes.update(Path(p.pw_dir) for p in pwd.getpwall() if p.pw_uid == 0 or 1000 <= p.pw_uid < 65534)
    paths = {Path(state["plan"]["env_path"])}
    for home in sorted(homes):
        base = home / ".config/usdb"
        core.safe_path(base)
        if base.exists():
            paths.update(p / "node.env" for p in base.iterdir() if p.is_dir() and (p / "node.env").exists())
    return sorted(paths)


def references(root, state, items, extra=()):
    """Fail closed on unavailable evidence; never treat unreadable references as absent."""
    reasons = {i["path"]: [] for i in items}
    def add(path, description):
        for candidate in reasons:
            if core.overlap(Path(candidate), path):
                reasons[candidate].append(description)

    for config in configuration_paths(state):
        core.safe_path(config)
        env = uninstall.read_env(config)
        for key, value in env.items():
            # The shared data root itself is a namespace, not a reference to all its children.
            if key != "USDB_DATA_ROOT" and (key.endswith("_HOST_DIR") or key.endswith("_HOST_FILE")) and value:
                add(core.absolute(value), f"configuration: {config} ({key})")
    for path in record_paths(root, state, extra):
        if path == root:
            continue
        other = session.read(path)
        if other["phase"] == "cleaned":
            continue
        for item in other["plan"]["components"]:
            for key in ("source", "target"):
                add(core.absolute(item[key]), f"upgrade record: {path}")
        for item in other["entries"] or []:
            add(core.absolute(item["archive"]), f"upgrade archive: {path}")
    for container in core.containers():
        for mount in container["mounts"]:
            if mount.get("Source", "").startswith("/"):
                add(core.absolute(mount["Source"]), f"container: {container['id']} ({container['state']})")
    return reasons


def measure(path):
    """Report logical and allocated bytes separately; neither predicts filesystem reclaim exactly."""
    tree = core.inventory(path)
    logical, allocated = 0, 0
    for name, entry in tree.items():
        file = path if name == "." else path / name
        info = file.lstat()
        if entry["kind"] == "file":
            core.require(info.st_nlink == 1, f"Archive contains a hardlink: {file}")
            logical += info.st_size
        allocated += info.st_blocks * 512
    return tree, dict(logical_bytes=logical, allocated_bytes=allocated, entries=len(tree))


def report(root, node, extra=()):
    state = load(root, node)
    items = candidates(state)
    blockers = []
    try:
        refs = references(root, state, items, extra)
    except PermissionError:
        raise
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        refs = {}
        blockers.append(f"Reference inspection incomplete: {error}")
    import node_upgrade_cleanup as cleanup_tools
    cleanup = core.read_json(root / "cleanup.json") if (root / "cleanup.json").exists() else {}
    if cleanup:
        cleanup_tools.validate_record(cleanup, state, items)
    core.require(state["phase"] == "applied" or cleanup, "Cleanup journal is missing; preserve the recovery directory")
    rows = []
    for item in items:
        path = Path(item["path"])
        row = dict(kind=item["kind"], path=str(path), references=refs.get(str(path), []))
        record = cleanup.get("targets", {}).get(str(path), {})
        if not path.exists():
            row["status"] = "cleaned" if record.get("delete_started") else "missing"
            if row["status"] == "missing":
                blockers.append(f"Retained archive is missing outside cleanup: {path}")
        else:
            try:
                tree, size = measure(path)
                if not record:
                    if item["tree"]:
                        expected = item["tree"]
                        core.require(tree.keys() == expected.keys(), f"Archive entries changed: {path}")
                        for name, entry in tree.items():
                            old = expected[name]
                            core.require(entry["kind"] == old["kind"] and all(entry["stamp"][n] == old["stamp"][n] for n in (0, 1, 2, 3, 5)),
                                         f"Archive identity or metadata changed: {path / name}")
                    if item["marker"]:
                        core.require(upgrade.sha(path / DATASET_IDENTITY_FILE) == item["marker"], f"Old dataset marker changed: {path}")
                preserve = set(cleanup_tools.private_paths(item["kind"], tree))
                size["private_preserve_bytes"] = sum(v["stamp"][2] for name, v in tree.items() if v["kind"] == "file"
                                                     and (name in preserve or any(str(p) in preserve for p in Path(name).parents)))
                row.update(size, status="referenced" if row["references"] else "retained")
                if state["phase"] == "cleaned" or record.get("deleted"):
                    blockers.append(f"Previously cleaned archive path was recreated: {path}")
            except PermissionError:
                raise
            except (OSError, ValueError) as error:
                row.update(status="blocked", detail=str(error))
                blockers.append(f"Cannot inspect {path}: {error}")
        if row["references"]:
            blockers.append(f"Archive is referenced: {path}")
        rows.append(row)
    try:
        _, backup_usage = measure(root)
    except PermissionError:
        raise
    except (OSError, ValueError) as error:
        backup_usage = {}
        blockers.append(f"Cannot inspect backup directory: {error}")
    return dict(schema_version="usdb-upgrade-archives:v1", backup_usage=backup_usage, backup_dir=str(root), operation_id=state["operation_id"],
                phase=state["phase"], source_release=state["plan"]["source_release"], target_release=state["plan"]["target_release"],
                archives=rows, blockers=blockers, executable=not blockers,
                backup_contents=["upgrade.json", "private/node.env", "private/config", "private/secure", "cleanup.json (after cleanup starts)",
                                 "private/retained (verified private/unknown files preserved by cleanup)"],
                reference_scope="Standard account configs, all Docker mounts, registered upgrades, sibling backups and --other-backup-dir records; review custom external consumers manually")


def size_text(value):
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024 or unit == "TiB":
            return f"{value:.1f} {unit}"
        value /= 1024


def show(value):
    print(f"USDB upgrade archives | {value['source_release']} -> {value['target_release']}")
    print(f"Record: {value['backup_dir']} | phase: {value['phase']}")
    print("\nStatus       Allocated       Logical  Component / retained location")
    for row in value["archives"]:
        print(f"{row['status']:<12} {size_text(row['allocated_bytes']) if 'allocated_bytes' in row else 'unknown':>11}  {size_text(row['logical_bytes']) if 'logical_bytes' in row else 'unknown':>11}  {row['kind']}")
        print(f"  {row['path']}")
        if "private_preserve_bytes" in row:
            print(f"  Preserve in backup: {size_text(row['private_preserve_bytes'])} of private/unknown files")
        for reason in row["references"]:
            print(f"  In use: {reason}")
    print(f"\nBackup directory allocated: {size_text(value['backup_usage']['allocated_bytes']) if value['backup_usage'] else 'unknown'}")
    print("Backup directory keeps recovery/configuration/private data; large databases remain at the locations above.")
    print("Allocated size is not guaranteed reclaimed space; private/unknown files are copied before removal.")
    for blocker in value["blockers"]:
        print("BLOCKED: " + blocker)
    print("Reference scope: " + value["reference_scope"])
    if value["phase"] == "cleaned":
        print("Cleanup completed; rollback is unavailable. Keep this record and its private backups.")
    else:
        print("Preview only. After accepting the new node, stop its services and explicitly execute upgrade-cleanup.")
        print("Cleanup permanently gives up rollback for this upgrade; repeat the same command to recover an interrupted cleanup.")


def add_parser(subparsers):
    for name, help_text in (("upgrade-status", "Inspect retained data for one upgrade record"),
                            ("upgrade-cleanup", "Preview or explicitly delete unreferenced upgrade archives")):
        parser = subparsers.add_parser(name, help=help_text)
        parser.add_argument("--backup-dir", required=True, type=Path, help="existing upgrade recovery directory (including r7 records)")
        parser.add_argument("--other-backup-dir", action="append", type=Path, default=[], help="additional legacy upgrade record outside the registry/sibling directories")
        parser.add_argument("--json", action="store_true", help="read-only structured inventory")
        if name == "upgrade-cleanup":
            parser.add_argument("--execute", action="store_true", help="require stopped services, verified private backups and interactive confirmation")


def dispatch(args, layout, node):
    """Use the installed helper for privileged reads/deletion; previews never create records."""
    root = core.absolute(args.backup_dir)
    state = session.read(root)
    core.require(state["plan"]["env_path"] == str(layout.node_env), "Upgrade record belongs to another node configuration")
    execute = getattr(args, "execute", False)
    core.require(not (execute and args.json), "--json is preview-only")
    if not execute:
        try:
            with redirect_stdout(sys.stderr):
                value = report(root, node, args.other_backup_dir)
            if args.json:
                print(json.dumps(value, indent=2, sort_keys=True))
            else:
                show(value)
                if value["executable"] and value["phase"] != "cleaned":
                    print("  " + shlex.join([*upgrade._command_prefix(args, node), "upgrade-cleanup", "--backup-dir", str(root),
                                            *[v for p in args.other_backup_dir for v in ("--other-backup-dir", str(p))], "--execute"]))
            return 0 if value["executable"] else 2
        except PermissionError:
            print("Archive inspection requires sudo for protected files; this remains read-only.", file=sys.stderr)
    import node_upgrade_cleanup as cleanup
    command = [sys.executable, "-B", str(Path(cleanup.__file__).resolve()), "--backup-dir", str(root)]
    if not execute:
        command.append("--inspect")
    if args.json:
        command.append("--json")
    for path in args.other_backup_dir:
        command.extend(["--other-backup-dir", str(core.absolute(path))])
    if os.geteuid() != 0:
        command = ["sudo", "--", *command]
    result = subprocess.run(command, check=False).returncode
    if result == 0 and not execute and not args.json and state["phase"] != "cleaned":
        print("  " + shlex.join([*upgrade._command_prefix(args, node), "upgrade-cleanup", "--backup-dir", str(root),
                                *[v for p in args.other_backup_dir for v in ("--other-backup-dir", str(p))], "--execute"]))
    return result
