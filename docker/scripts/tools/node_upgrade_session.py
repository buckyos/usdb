"""Journaled, same-filesystem isolation for stopped development-node upgrades.

Old databases are renamed or left in place, never deleted. Recovery requires the
exact target kit and refuses external configuration/path changes.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import hashlib
import os
import re
import shlex
from pathlib import Path
import socket
import subprocess
import sys
import uuid

import node_rebuild as core
import node_uninstall as uninstall
import node_upgrade as upgrade
from runtime_compatibility import (DATASET_IDENTITY_FILE, PERSISTENT_DATA_SERVICES,
                                   build_dataset_identity, build_persistent_data_paths)

SCHEMA = "usdb-node-upgrade-session:v1"
TERMINAL = {"applied", "rolled_back"}


def read(root):
    value = core.read_json(root / "upgrade.json")
    core.require(value.get("schema_version") == SCHEMA, "Unsupported upgrade journal")
    return value


def runtime_plan(value):
    """Reuse stopped/shared-container and systemd-definition checks from uninstall."""
    config = Path(value["env_path"]).parent
    units = [f"/etc/systemd/system/{prefix}-{value['bundle']}.service" for prefix in uninstall.UNITS]
    scope = {value["data_root"], str(config), *units}
    return dict(home=value["operator_home"], data_root=value["data_root"], bundle=value["bundle"],
                env_path=value["env_path"], env_sha256=value["env_sha256"], scope=sorted(scope), units=units,
                targets=[dict(key="controller-unit" if i == 0 else uninstall.UNITS[i], path=p)
                         for i, p in enumerate(units)])


def stage(value, root, node):
    """Create only a private journal after read-only preflight; never alter live datasets here."""
    core.safe_path(root)
    core.require(not root.exists(), "Operation directory already exists; use --resume instead")
    for path in (value["data_root"], str(Path(value["env_path"]).parent), value["source_kit"], value["target_kit"]):
        core.require(not core.overlap(root, Path(path)), f"Backup directory overlaps protected path: {path}")
    home = uninstall.operator_home()
    env_path = Path(value["env_path"])
    core.require(env_path == home / ".config/usdb" / value["bundle"] / "node.env",
                 "Rebuild execution requires the standard bundle-scoped node.env; custom layouts need manual review")
    value = dict(value, operator_home=str(home), operator_uid=env_path.stat().st_uid, operator_gid=env_path.stat().st_gid)
    core.require(value["operator_uid"] == home.stat().st_uid, "Node configuration owner differs from the operating account")
    _idle(value, node)
    uninstall.check_stopped(runtime_plan(value), allow_enabled=True, inspect_install_rules=False)
    core.require(upgrade.sha(env_path) == value["env_sha256"], "Configuration changed after planning")
    root.mkdir(mode=0o700, parents=True)
    core.sync_dir(root.parent)
    core.atomic_json(root / "upgrade.json", dict(schema_version=SCHEMA, plan=value, operation_id=uuid.uuid4().hex,
                                                phase="staged", entries=None, created={}, backups={}, units={}, events=[]))
    import node_upgrade_archives
    import node_upgrade_cleanup
    with ExitStack() as locks:
        node_upgrade_cleanup.catalog_lock(value["data_root"], locks, (value["operator_uid"], value["operator_gid"]))
        node_upgrade_archives.register(root)
    return root


def _idle(value, node):
    """Do not supersede other durable operator transactions."""
    layout = node.load_release_layout(Path(value["target_kit"]), Path(value["env_path"]))
    import usdb_sourcedao
    import usdb_mining
    import usdb_peers
    usdb_sourcedao.require_idle(layout)
    core.require(not usdb_mining.pending(layout) and not usdb_peers.pending(layout),
                 "Finish pending mining/peer operations before upgrading")
    core.require(not node._read_resource_state(layout).get("pending"), "Finish pending resource transitions before upgrading")
    return layout


class Session:
    """An interruptible operation, bound to immutable manifests and original file identities."""
    def __init__(self, root, node):
        self.root, self.node = core.absolute(root), node
        core.safe_path(self.root)
        self.state = read(root)
        self.plan = self.state["plan"]
        self.layout = node.load_release_layout(Path(self.plan["target_kit"]), Path(self.plan["env_path"]))
        core.require(upgrade.sha(self.layout.manifest_path) == self.plan["target_manifest_sha256"], "Target release changed since planning")
        core.require(upgrade.sha(Path(self.plan["source_kit"]) / "release/usdb-release-manifest.json") == self.plan["source_manifest_sha256"],
                     "Source release changed since planning")
        self.pending = self.layout.node_env.parent / upgrade.PENDING
        self.runtime = runtime_plan(self.plan)
        self.uid, self.gid = self.plan["operator_uid"], self.plan["operator_gid"]
        self.validate_journal()

    def validate_journal(self):
        """Re-derive all writable destinations; a journal is not authority for arbitrary paths."""
        value, state = self.plan, self.state
        core.require(re.fullmatch(r"[0-9a-f]{32}", state["operation_id"]), "Invalid operation ID")
        core.require(state["phase"] not in {"cleanup_started", "cleaned"}, "Upgrade cleanup has relinquished rollback; use upgrade-status or resume upgrade-cleanup")
        core.require(state["phase"] in {"staged", "prepared", "rolling_back", *TERMINAL}, "Invalid operation phase")
        home = core.absolute(value["operator_home"])
        core.safe_path(home)
        core.require(self.layout.node_env == home / ".config/usdb" / self.layout.bundle_id / "node.env", "Unexpected journal configuration path")
        core.require(self.layout.node_env.stat().st_uid == home.stat().st_uid == self.uid,
                     "Upgrade operator ownership changed")
        core.require(self.root.stat().st_uid == self.uid and self.root.stat().st_mode & 0o077 == 0,
                     "Upgrade backup must remain private and owned by the operator")
        for key in ("data_root", "source_kit", "target_kit"):
            path = core.absolute(value[key])
            core.safe_path(path)
            core.require(not core.overlap(self.root, path), f"Upgrade backup overlaps {key}")
        core.require(not core.overlap(self.root, self.layout.node_env.parent), "Upgrade backup overlaps configuration")
        source = upgrade._source_manifest(Path(value["source_kit"]), self.node)
        original_path = self.root / "private/node.env" if "node.env" in state["backups"] else self.layout.node_env
        core.require(upgrade.sha(original_path) == value["env_sha256"], "Saved original configuration changed")
        original_env = self.node.read_env(original_path)
        core.require(original_env.get("USDB_DATA_ROOT") == value["data_root"] and upgrade._matches(source, original_env),
                     "Journal differs from the original node configuration/source release")
        old, new = source["runtime_compatibility"], self.layout.runtime_compatibility
        chain_changes = [k for k in upgrade.CHAIN_FIELDS if source["network_bundle"].get(k) != self.layout.network_identity.get(k)]
        service_changes = {s for s in old["services"] if old["services"][s] != new["services"][s]}
        reset = bool(chain_changes or "usdb_chain" in service_changes)
        classification = "network_reset" if reset else "data_rebuild" if service_changes else "compatible"
        core.require(value["schema_version"] == upgrade.SCHEMA and value["executable"] and not value["blockers"]
                     and value["classification"] == classification != "compatible", "Journal is not an executable rebuild")
        core.require(source["network_bundle"]["bundle_id"] == value["bundle"] == self.layout.bundle_id, "Cross-bundle rebuild is unsupported")
        core.require(not reset or self.layout.network_identity.get("bundle_status") == "development-resettable", "Chain reset requires a resettable development network")
        core.require(not {"bitcoin_core", "balance_history"} & service_changes, "Unsupported source-data migration")
        core.require(value["previous_compatibility_id"] == old["compatibility_id"] and value["target_compatibility_id"] == new["compatibility_id"], "Journal compatibility identity mismatch")
        old_paths = build_persistent_data_paths(Path(value["data_root"]), source["network_bundle"], old)
        new_paths = build_persistent_data_paths(Path(value["data_root"]), self.layout.network_identity, new)
        core.require(all(original_env.get(key) == str(path) for key, path in old_paths.items() if key in PERSISTENT_DATA_SERVICES),
                     "Journal source paths differ from the original configuration")
        core.require(len(value["components"]) == len(PERSISTENT_DATA_SERVICES), "Incomplete component plan")
        entries, targets, seen = {}, set(), set()
        for item in value["components"]:
            key = item["env_key"]
            core.require(key in PERSISTENT_DATA_SERVICES and key not in seen, "Unknown/duplicate component")
            seen.add(key)
            service = PERSISTENT_DATA_SERVICES[key]
            rebuild = service in service_changes or (reset and service in {"usdb_indexer", "usdb_chain", "control_plane"})
            core.require(item["service"] == service and item["source"] == str(old_paths[key])
                         and item["target"] == str(new_paths[key]) and item["action"] == ("rebuild" if rebuild else "reuse"),
                         f"Journal component path/action mismatch: {service}")
            if rebuild:
                targets.add(item["target"])
                if item["source"] == item["target"]:
                    entries[item["source"]] = service
        if reset:
            entries.update({str(self.layout.node_env.parent / name): "state:" + name for name in (*upgrade.STATE_FILES, "monitor")})
            targets.add(str(self.layout.node_env.parent / "monitor"))
        seen.clear()
        for entry in state["entries"] or []:
            path = Path(entry["source"])
            core.require(entry["source"] not in seen and entries.get(str(path)) == entry["kind"] and
                         entry["archive"] == str(path.with_name(path.name + ".before-upgrade-" + state["operation_id"])),
                         "Unexpected isolation entry in upgrade journal")
            seen.add(str(path))
            core.safe_path(path)
            core.safe_path(Path(entry["archive"]))
        for key, info in state["created"].items():
            path = Path(key)
            core.require(key in targets and info["temporary"] == str(path.with_name(path.name + ".new-upgrade-" + state["operation_id"])),
                         "Unexpected created dataset in upgrade journal")
            core.safe_path(path)
            core.safe_path(Path(info["temporary"]))
        allowed_units = {Path(p).name for p in self.runtime["units"]}
        core.require(set(state["units"]) <= allowed_units and all(v["mode"] in {"enabled", "enabled-runtime"} for v in state["units"].values()),
                     "Unexpected service activation in upgrade journal")

    def original_env(self):
        """Check the saved configuration again before any forward or rollback write."""
        path = self.root / "private/node.env"
        core.require(upgrade.sha(path) == self.plan["env_sha256"], "Saved original configuration changed")
        return path.read_text()

    def own(self, path):
        if os.geteuid() == 0:
            os.chown(path, self.uid, self.gid)

    def save(self):
        core.atomic_json(self.root / "upgrade.json", self.state)
        self.own(self.root / "upgrade.json")

    def event(self, phase, path=""):
        self.state["events"].append(dict(phase=phase, path=str(path)))
        self.save()
        print(f"Upgrade {phase}: {path}", flush=True)

    def check(self):
        core.safe_path(self.layout.node_env)
        if "node.env" in self.state["backups"]:
            self.original_env()
        expected = {self.plan["env_sha256"], self.state.get("candidate_sha256")}
        core.require(upgrade.sha(self.layout.node_env) in expected, "Node configuration changed outside this upgrade; preserve the journal")
        for item in self.plan["components"]:
            for key in ("source", "target"):
                core.safe_path(Path(item[key]))
            # Retained datasets must retain their original identity throughout the operation.
            if item["action"] == "reuse":
                core.require(upgrade.sha(Path(item["source"]) / DATASET_IDENTITY_FILE) == item["marker_sha256"],
                             f"Retained {item['service']} marker changed")
        if self.pending.exists():
            core.require(core.read_json(self.pending) == self.pending_value(), "Another upgrade owns the pending marker")
        uninstall.check_stopped(self.runtime, allow_enabled=True)

    def pending_value(self):
        return dict(operation_id=self.state["operation_id"], backup_dir=str(self.root), target_release=self.layout.release_id)

    def stop_autostart(self):
        pending = uninstall.check_stopped(self.runtime, allow_enabled=True)
        for unit, mode in pending.items():
            self.state["units"].setdefault(unit, dict(mode=mode, sha256=upgrade.sha(Path("/etc/systemd/system") / unit)))
            self.save()
            self.event("disable_autostart", unit)
            core.command(["systemctl", "disable", *(["--runtime"] if mode == "enabled-runtime" else []), "--", unit])
        uninstall.check_stopped(self.runtime)

    def restore_autostart(self):
        for unit, record in self.state["units"].items():
            mode = record["mode"]
            core.require(upgrade.sha(Path("/etc/systemd/system") / unit) == record["sha256"], "Service definition changed during upgrade")
            # Reuse definition/drop-in/Also checks before enabling only this node's units.
            uninstall.check_stopped(self.runtime, allow_enabled=True)
            core.command(["systemctl", "enable", *(["--runtime"] if mode == "enabled-runtime" else []), "--", unit])
        uninstall.check_stopped(self.runtime, allow_enabled=True)

    def backup(self, source, name):
        destination = self.root / "private" / name
        destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if name not in self.state["backups"]:
            record = core.copy_tree(source, destination)
            self.state["backups"][name] = record
            self.save()
        else:
            core.verify_tree(destination, self.state["backups"][name], reject_hardlinks=True)
        return destination

    def prepare(self):
        if self.state["entries"] is not None:
            return
        core.require(upgrade.sha(self.layout.node_env) == self.plan["env_sha256"], "Original configuration changed")
        # Back up settings independently of database isolation. No raw config enters the public plan/journal.
        self.backup(self.layout.node_env, "node.env")
        self.backup(self.layout.node_env.parent, "config")
        env = self.node.read_env(self.layout.node_env)
        secure = Path(env["BTC_RPCAUTH_HOST_FILE"]).parent
        core.require(secure == Path(self.plan["data_root"]) / "networks" / self.plan["bundle"] / "secure", "Unexpected private credential directory")
        self.backup(secure, "secure")
        entries = []
        for item in self.plan["components"]:
            if item["action"] != "rebuild":
                continue
            source, target = Path(item["source"]), Path(item["target"])
            core.require(upgrade.sha(source / DATASET_IDENTITY_FILE) == item["marker_sha256"], f"Source marker changed: {source}")
            if source == target:
                entries.append(self.entry(source, item["service"]))
            else:
                core.require(not target.exists(), f"Target dataset already exists: {target}")
        if self.plan["classification"] == "network_reset":
            for name in (*upgrade.STATE_FILES, "monitor"):
                path = self.layout.node_env.parent / name
                core.safe_path(path)
                if path.exists():
                    entries.append(self.entry(path, "state:" + name))
        self.state["entries"] = entries
        self.state["phase"] = "prepared"
        self.save()

    def entry(self, source, kind):
        archive = source.with_name(source.name + ".before-upgrade-" + self.state["operation_id"])
        core.safe_path(archive)
        core.require(not archive.exists(), f"Archive path already exists: {archive}")
        core.check_mounts(source)
        tree = core.inventory(source)
        return dict(source=str(source), archive=str(archive), kind=kind, tree=tree)

    def remove_stopped_containers(self):
        runtime = uninstall.runtime_plan(self.runtime)
        for item in core.containers():
            self.check()
            if core.selected_container(runtime, item):
                core.require(item["state"] in {"exited", "created", "dead"}, "Container started during upgrade")
                core.command(["docker", "rm", "--", item["id"]])
                self.event("container_removed", item["id"])
            else:
                changed = [self.layout.node_env.parent, *[Path(c["source"]) for c in self.plan["components"] if c["action"] == "rebuild"]]
                core.require(not any(core.overlap(p, Path(m["Source"])) for p in changed for m in item["mounts"]
                                     if m.get("Source", "").startswith("/")),
                             "Another container references a rebuild dataset; review it separately")

    def isolate(self):
        for entry in self.state["entries"]:
            self.check()
            source, archive = Path(entry["source"]), Path(entry["archive"])
            if archive.exists():
                core.verify_rename(archive, entry["tree"])
                continue
            core.verify_rename(source, entry["tree"])
            self.event("isolate_started", source)
            os.rename(source, archive)
            core.sync_dir(source.parent)
            self.event("isolated", archive)

    def old_data(self, item):
        return next((Path(e["archive"]) for e in self.state["entries"] if e["source"] == item["source"]), Path(item["source"]))

    def create(self, destination, marker=None, preserve=()):
        """Publish fully prepared empty datasets atomically; recover the rename by inode."""
        key = str(destination)
        temporary = destination.with_name(destination.name + ".new-upgrade-" + self.state["operation_id"])
        info = self.state["created"].get(key)
        if info and info.get("tree") and destination.exists():
            core.verify_rename(destination, info["tree"])
            return
        core.safe_path(temporary)
        core.require(not destination.exists(), f"Unexpected target data at {destination}")
        if info is None:
            core.require(not temporary.exists(), f"Unexpected staging directory: {temporary}")
            info = dict(temporary=str(temporary), inode=None)
            self.state["created"][key] = info
            self.save()
        if info["inode"] is None:
            if temporary.exists():
                core.require(temporary.is_dir() and not any(temporary.iterdir()), "Unowned non-empty staging directory")
            else:
                temporary.mkdir(mode=0o700, parents=True)
            self.own(temporary)
            info["inode"] = core.stamp(temporary)[:2]
            self.save()
        core.require(core.stamp(temporary)[:2] == info["inode"], "Staging directory was replaced")
        if not info.get("tree"):
            if marker is not None:
                core.atomic_json(temporary / DATASET_IDENTITY_FILE, marker)
                self.own(temporary / DATASET_IDENTITY_FILE)
            for source, relative in preserve:
                if source.exists():
                    target = temporary / relative
                    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                    core.copy_tree(source, target)
            # Service containers and operator helpers must keep the configured ownership.
            for directory, _, files in os.walk(temporary):
                self.own(Path(directory))
                for name in files:
                    self.own(Path(directory) / name)
            info["tree"] = core.inventory(temporary)
            self.save()
        core.verify_rename(temporary, info["tree"])
        self.event("publish_dataset_started", destination)
        os.rename(temporary, destination)
        core.sync_dir(destination.parent)
        self.event("dataset_ready", destination)

    def initialize(self):
        for item in self.plan["components"]:
            if item["action"] == "rebuild":
                old = self.old_data(item)
                preserve = [(old / p, p) for p in ("geth/nodekey", "nodekey", "keystore")] if item["service"] == "usdb_chain" else []
                self.create(Path(item["target"]), build_dataset_identity(item["service"], self.layout.runtime_compatibility), preserve)
        monitor = next((e for e in self.state["entries"] if e["kind"] == "state:monitor"), None)
        if monitor:
            old = Path(monitor["archive"])
            self.create(Path(monitor["source"]), preserve=[(old / p, p) for p in ("config.json", "notifications/config.json")])

    def write_config(self):
        original = self.original_env()
        updates = {**self.layout.images, "USDB_RUNTIME_COMPATIBILITY_ID": self.plan["target_compatibility_id"],
                   **{i["env_key"]: i["target"] for i in self.plan["components"]}}
        if self.plan["classification"] == "network_reset":
            updates.update(USDB_NODE_ROLE="full", USDB_MINER_ADDRESS="", USDB_MINER_THREADS="1")
        candidate = self.node.upsert_env(original, updates)
        digest = hashlib.sha256(candidate.encode()).hexdigest()
        core.require(not self.state.get("candidate_sha256") or self.state["candidate_sha256"] == digest, "Candidate configuration changed")
        self.state["candidate_sha256"] = digest
        self.save()
        self.check()
        self.node._atomic_write_private(self.layout.node_env, candidate)
        self.own(self.layout.node_env)
        core.sync_dir(self.layout.node_env.parent)
        self.node._validate_node_config(self.layout, require_runtime=False, require_bitcoin_runtime=True, allow_pending_upgrade=True)
        self.node._validate_node_release_images(self.layout)
        self.event("configuration_ready", self.layout.node_env)

    def rollback(self):
        if self.state["entries"] is None:
            core.require(upgrade.sha(self.layout.node_env) == self.plan["env_sha256"], "Configuration changed before preparation")
            self.finish("rolled_back")
            return
        # Any service startup would mutate these fresh datasets. Refuse to discard even
        # those changes; stopped state alone is not a safe rollback boundary.
        for path, info in self.state["created"].items():
            target = Path(path)
            aborted = target.with_name(target.name + ".aborted-upgrade-" + self.state["operation_id"])
            core.safe_path(aborted)
            if aborted.exists():
                core.require(info.get("tree") is not None, "Incomplete rollback archive")
                core.verify_rename(aborted, info["tree"])
                continue
            candidate = target if target.exists() else Path(info["temporary"])
            if candidate.exists():
                if not info.get("tree"):
                    if info["inode"] is None and candidate == Path(info["temporary"]):
                        core.require(candidate.is_dir() and not any(candidate.iterdir()), "Unowned non-empty staging directory")
                        info["inode"] = core.stamp(candidate)[:2]
                    core.require(candidate == Path(info["temporary"]) and core.stamp(candidate)[:2] == info["inode"], "Unowned partial preparation")
                    info["tree"] = core.inventory(candidate)
                    self.save()
                core.verify_rename(candidate, info["tree"])
        self.state["phase"] = "rolling_back"
        self.save()
        for path, info in self.state["created"].items():
            target = Path(path)
            aborted = target.with_name(target.name + ".aborted-upgrade-" + self.state["operation_id"])
            if aborted.exists():
                continue
            candidate = target if target.exists() else Path(info["temporary"])
            if candidate.exists():
                os.rename(candidate, aborted)
                core.sync_dir(candidate.parent)
        for entry in reversed(self.state["entries"]):
            source, archive = Path(entry["source"]), Path(entry["archive"])
            if archive.exists():
                core.require(not source.exists(), f"Rollback source path occupied: {source}")
                core.verify_rename(archive, entry["tree"])
                os.rename(archive, source)
                core.sync_dir(source.parent)
            else:
                core.verify_rename(source, entry["tree"])
        self.node._atomic_write_private(self.layout.node_env, self.original_env())
        self.own(self.layout.node_env)
        self.finish("rolled_back")
        print(f"Rollback complete. Use the original kit before startup: {self.plan['source_kit']}")

    def finish(self, phase):
        # Old unit files still name the previous kit. A successful rebuild leaves
        # autostart disabled until the new kit installs its controller via up.
        if phase == "rolled_back":
            self.restore_autostart()
        self.state["phase"] = phase
        self.save()
        core.safe_path(self.pending)
        if self.pending.exists():
            core.require(core.read_json(self.pending) == self.pending_value(), "Pending ownership changed")
            self.pending.unlink()
            core.sync_dir(self.pending.parent)

    def run(self, *, rollback=False, confirm=input):
        core.require(sys.stdin.isatty() and sys.stdout.isatty(), "Upgrade execution requires an interactive terminal")
        self.check()
        upgrade.show(self.plan)
        phrase = f"{'ROLLBACK' if rollback else 'UPGRADE'} {self.plan['bundle']} {socket.gethostname()}"
        print(f"Old datasets will be retained. Type exactly '{phrase}' to continue:", flush=True)
        if confirm().strip() != phrase:
            print("Cancelled; node data and configuration are unchanged.")
            return 0
        with ExitStack() as locks:
            import node_upgrade_cleanup
            node_upgrade_cleanup.catalog_lock(self.plan["data_root"], locks, (self.plan["operator_uid"], self.plan["operator_gid"]))
            core.lock_file(self.root / ".lock", locks) if (self.root / ".lock").exists() else self._new_lock(locks)
            locks.enter_context(self.node.node_operation_lock(self.layout, "upgrade-release"))
            # Re-read after locking: another completed operation may have advanced the journal.
            self.__init__(self.root, self.node)
            self.check()
            if self.state["phase"] == "rolled_back":
                core.require(rollback, "This operation was rolled back; plan a new upgrade")
                return 0
            if self.state["phase"] == "applied" and not rollback:
                self.finish("applied")
                return 0
            _idle(self.plan, self.node)
            for path in (self.layout.node_env.parent / "monitor/run.lock", self.layout.node_env.parent / "sourcedao/.operation.lock"):
                if path.exists():
                    core.lock_file(path, locks)
            for item in self.plan["components"]:
                core.acquire_locks(Path(item["source"]), locks)
            self.stop_autostart()
            core.atomic_json(self.pending, self.pending_value())
            self.own(self.pending)
            if rollback or self.state["phase"] == "rolling_back":
                self.rollback()
            else:
                self.prepare()
                self.remove_stopped_containers()
                self.isolate()
                self.initialize()
                self.write_config()
                self.finish("applied")
                print(f"Upgrade prepared; services remain stopped. Recovery record: {self.root}")
                print("Inspect retained data: " + shlex.join(["usdb-node", "upgrade-status", "--backup-dir", str(self.root)]))
                print("Run usdb-node doctor, then usdb-node up. Reauthorize mining only after the new chain and pass are ready.")
        return 0

    def _new_lock(self, stack):
        path = self.root / ".lock"
        core.safe_path(path)
        path.touch(mode=0o600, exist_ok=False)
        core.lock_file(path, stack)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resume", type=Path, required=True)
    parser.add_argument("--rollback", action="store_true")
    args = parser.parse_args()
    import usdb_node as node
    try:
        core.require(os.geteuid() == 0, "Run the reviewed upgrade runner with sudo")
        return Session(core.absolute(args.resume), node).run(rollback=args.rollback)
    except KeyboardInterrupt:
        print("Upgrade interrupted; preserve old data and resume the same operation directory.", file=sys.stderr)
        return 130
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"Upgrade stopped: {error}. Old data is retained; resume the same operation directory.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
