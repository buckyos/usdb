"""Plan component reuse and explicit development-network rebuilds without deleting data."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

import node_rebuild as core
from runtime_compatibility import (
    DATA_LAYOUT_VERSION, DATASET_IDENTITY_FILE, PERSISTENT_DATA_SERVICES,
    build_dataset_identity, build_persistent_data_paths, build_runtime_compatibility,
)

SCHEMA = "usdb-node-upgrade-plan:v1"
PENDING = ".usdb-upgrade-pending.json"
# The chain database contract alone does not bind its complete consensus history.
CHAIN_FIELDS = ("bundle_id", "chain_id", "network_id", "genesis_block_hash", "genesis_sha256",
                "btc_network_id", "btc_index_origin_height", "btc_rules_scope", "btc_activation_registry_id")
STATE_FILES = ("node.mining.json", "node.peers.json", "sourcedao")
IMAGE_KEYS = {"usdb_services": "USDB_SERVICES_IMAGE", "usdb_chain": "USDB_CHAIN_IMAGE",
              "bitcoin_core": "USDB_BITCOIN_IMAGE"}


def sha(path: Path) -> str:
    """Fingerprint only small immutable metadata/configuration files."""
    core.safe_path(path)
    core.require(path.is_file(), f"Expected regular metadata file: {path}")
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ensure_no_pending(layout) -> None:
    """Block startup/config mutations while a durable upgrade owns this node."""
    path = layout.node_env.parent / PENDING
    core.safe_path(path)
    if path.exists():
        value = core.read_json(path)
        raise ValueError("UPGRADE_PENDING: finish or roll back the saved upgrade before starting or changing this node; "
                         f"usdb-node upgrade-release --resume {shlex.quote(value['backup_dir'])} --execute")


def _source_manifest(root, node):
    """Report malformed source metadata without exposing raw configuration contents."""
    try:
        return _read_source_manifest(root, node)
    except (KeyError, TypeError, AttributeError) as error:
        raise ValueError(f"Malformed source release metadata: kit={root}, error_type={type(error).__name__}") from None


def _read_source_manifest(root, node):
    """Verify an older installed kit without applying the target's frozen registry allowlist."""
    root = core.absolute(root)
    core.safe_path(root)
    value = node._load_release_manifest(root)
    network = value["network_bundle"]
    bundled = root / "docker/networks" / network["bundle_id"] / "network.json"
    core.require(sha(bundled) == network["network_json_sha256"], "Source kit network checksum mismatch")
    raw = core.read_json(bundled)
    artifacts = raw["artifacts"]
    for item in artifacts.values():
        relative = Path(item["path"])
        core.require(not relative.is_absolute() and ".." not in relative.parts, "Unsafe source artifact path")
        core.require(sha(bundled.parent / relative) == item["sha256"], f"Source artifact checksum mismatch: {relative}")
    genesis = core.read_json(bundled.parent / artifacts["genesis_manifest"]["path"])
    expected = dict(bundle_id=raw["network_bundle_id"], bundle_status=raw["status"],
                    chain_id=raw["chain_id"], network_id=raw["network_id"],
                    genesis_block_hash=genesis["block_hash"], genesis_sha256=artifacts["genesis"]["sha256"],
                    btc_network_id=raw["btc_source"]["network_id"],
                    btc_index_origin_height=raw["btc_source"]["index_origin_height"],
                    btc_activation_registry_id=raw["btc_source"]["activation_registry_id"],
                    snapshot_trusted_keys_sha256=artifacts["snapshot_trusted_keys"]["sha256"])
    from registry_scope import rules_scope_identity
    expected.update(rules_scope_identity(raw["btc_source"]))
    core.require(all(network.get(k) == v for k, v in expected.items()), "Source network identity differs from its manifest")
    if "assumeutxo_bootstrap" in artifacts:
        from assumeutxo_deployment import state_identity
        native = core.read_json(bundled.parent / artifacts["assumeutxo_bootstrap"]["path"])
        core.require(network.get("balance_history_bootstrap") == state_identity(native), "Source bootstrap identity mismatch")
    else:
        core.require("balance_history_bootstrap" not in network, "Unexpected source bootstrap identity")
    core.require(value["runtime_compatibility"] == build_runtime_compatibility(network),
                 "Source runtime contract is unsupported; use its original tools or a reviewed migration")
    return value


def _matches(value, env):
    return (value["runtime_compatibility"]["compatibility_id"] == env.get("USDB_RUNTIME_COMPATIBILITY_ID")
            and all(value["images"][k]["reference"] == env.get(v) for k, v in IMAGE_KEYS.items()))


def find_source(layout, env, node, from_kit=None):
    """Select an exact installed configuration baseline; never infer it from rN ordering."""
    if from_kit:
        root = core.absolute(from_kit)
        value = _source_manifest(root, node)
        core.require(_matches(value, env), "--from-kit does not match the configured images and runtime compatibility ID")
        return root, value
    candidates = {layout.kit_root}
    for parent in {layout.kit_root.parent, Path.home() / ".local/share/usdb/releases"}:
        core.safe_path(parent)
        if parent.is_dir():
            candidates.update(p for p in parent.iterdir() if re.fullmatch(re.escape(layout.bundle_id) + r"-r[1-9][0-9]*", p.name))
    matches = []
    for path in sorted(candidates):
        try:
            value = _source_manifest(path, node)
            if _matches(value, env):
                matches.append((path, value))
        except (OSError, ValueError, KeyError):
            continue
    core.require(matches, "Cannot identify the configured source release. Keep the old kit and pass --from-kit /absolute/path/to/old-kit")
    identities = {json.dumps(v["network_bundle"], sort_keys=True) for _, v in matches}
    core.require(len(identities) == 1, "Configured images match ambiguous network identities; specify --from-kit")
    return matches[-1]


def changes(before, after, prefix=""):
    """Return public field-level differences, without inspecting private node settings."""
    result = []
    if isinstance(before, dict) and isinstance(after, dict):
        for key in sorted(set(before) | set(after)):
            result.extend(changes(before.get(key), after.get(key), f"{prefix}.{key}" if prefix else key))
    elif before != after:
        result.append(dict(field=prefix, previous=before, target=after))
    return result


def plan(layout, node, from_kit=None):
    """Read metadata/markers only. No service probes, locks, directory creation or secrets in output."""
    ensure_no_pending(layout)
    core.safe_path(layout.node_env)
    env = node.read_env(layout.node_env)
    core.require(env.get("USDB_DATA_LAYOUT") == DATA_LAYOUT_VERSION, "Legacy data layout needs a separate reviewed migration")
    core.require(bool(env.get("USDB_DATA_ROOT")), "Configured USDB_DATA_ROOT is missing")
    root = core.absolute(env["USDB_DATA_ROOT"])
    core.safe_path(root)
    source_root, source = find_source(layout, env, node, from_kit)
    old_network, old_contract = source["network_bundle"], source["runtime_compatibility"]
    new_network, new_contract = layout.network_identity, layout.runtime_compatibility
    old_paths = build_persistent_data_paths(root, old_network, old_contract)
    new_paths = build_persistent_data_paths(root, new_network, new_contract)
    blocked = []
    import usdb_minting
    if usdb_minting.activation_updates(env):
        blocked.append("Ord version migration is also required; this combined rebuild needs a separate reviewed plan")
    if old_network["bundle_id"] != layout.bundle_id:
        blocked.append("Cross-bundle deployment requires separate setup; this command upgrades one configured bundle")
    changed_chain = [k for k in CHAIN_FIELDS if old_network.get(k) != new_network.get(k)]
    changed_contracts = [s for s in old_contract["services"] if old_contract["services"][s] != new_contract["services"][s]]
    # Current executor has no BTC/BH storage migrator or bootstrap-mode converter.
    for service in ("bitcoin_core", "balance_history"):
        if service in changed_contracts:
            blocked.append(f"{service} contract changed: an explicit source-data migration/rebuild procedure is required")
    reset = bool(changed_chain or "usdb_chain" in changed_contracts)
    classification = "network_reset" if reset else "data_rebuild" if changed_contracts else "compatible"
    if classification == "compatible":
        blocked = [b for b in blocked if not b.startswith("Ord version migration")]
    if reset and new_network.get("bundle_status") != "development-resettable":
        blocked.append("Automatic chain reset is limited to explicitly resettable development networks; mainnet needs an activation/migration plan")
    if source.get("snapshot") != node._load_release_manifest(layout.kit_root).get("snapshot"):
        # A new signed download record alone is not a data migration. The bootstrap
        # mode and contract must nevertheless remain identical for this executor.
        old_snap, new_snap = source.get("snapshot", {}), layout.snapshot
        if any(old_snap.get(k) != new_snap.get(k) for k in ("status", "bootstrap_mode", "contract")):
            blocked.append("Snapshot bootstrap mode or contract changed; use the dedicated bootstrap migration")
    if env.get("SNAPSHOT_MODE") == "paired-checkpoint" and classification != "compatible":
        blocked.append("Paired checkpoints bind old indexer state; select a reviewed fresh bootstrap before rebuilding")
    components = []
    for key, service in PERSISTENT_DATA_SERVICES.items():
        old, new = old_paths[key], new_paths[key]
        core.require(env.get(key) == str(old), f"Configured {key} does not match the source release's derived path")
        core.safe_path(old)
        core.safe_path(new)
        marker = old / DATASET_IDENTITY_FILE
        core.require(core.read_json(marker) == build_dataset_identity(service, old_contract),
                     f"Source dataset marker mismatch: service={service}, path={old}")
        rebuild = service in changed_contracts or (reset and service in {"usdb_indexer", "usdb_chain", "control_plane"})
        if rebuild and old != new and new.exists():
            blocked.append(f"Target dataset already exists; preserve and inspect it instead of overwriting: {new}")
        components.append(dict(service=service, env_key=key, action="rebuild" if rebuild else "reuse",
                               source=str(old), target=str(new), marker_sha256=sha(marker),
                               reason="consensus history changed" if reset and service in {"usdb_indexer", "usdb_chain", "control_plane"}
                               else "service contract changed" if rebuild else "service contract unchanged"))
    return dict(schema_version=SCHEMA, classification=classification, executable=not blocked, blockers=blocked,
                source_release=source["release_id"], target_release=layout.release_id, bundle=layout.bundle_id,
                source_kit=str(source_root), target_kit=str(layout.kit_root), env_path=str(layout.node_env), data_root=str(root),
                source_manifest_sha256=sha(source_root / "release/usdb-release-manifest.json"),
                target_manifest_sha256=sha(layout.manifest_path), env_sha256=sha(layout.node_env),
                previous_compatibility_id=old_contract["compatibility_id"], target_compatibility_id=new_contract["compatibility_id"],
                changed_chain_fields=changed_chain,
                differences=changes(old_contract, new_contract, "runtime") + changes(
                    {k: old_network.get(k) for k in CHAIN_FIELDS}, {k: new_network.get(k) for k in CHAIN_FIELDS}, "network"),
                components=components, mining_action="disable and reauthorize on the new chain" if reset else "preserve",
                rollback_boundary="Only before starting services; all old datasets remain preserved")


def show(value):
    print(f"USDB upgrade: {value['source_release']} -> {value['target_release']}")
    print(f"Compatibility: {value['classification']}; executable={value['executable']}")
    print(f"Runtime ID: {value['previous_compatibility_id']} -> {value['target_compatibility_id']}")
    for item in value["components"]:
        print(f"  {item['action'].upper()} {item['service']}: {item['reason']}")
        print(f"    {item['source']} -> {item['target']}")
    for item in value["differences"]:
        print(f"  Changed {item['field']}: {item['previous']} -> {item['target']}")
    for reason in value["blockers"]:
        print(f"  BLOCKED: {reason}")
    if value["classification"] == "network_reset":
        print("Old USDB chain state, SourceDAO operation records and mining authorization will be isolated. Bitcoin/BH reuse is checked separately.")
    print("No database migration or data deletion is performed. Rebuilds keep old datasets; wallets and node identity are preserved.")


def add_parser(subparsers):
    """Expose preview separately from explicitly confirmed execution/recovery."""
    preview = subparsers.add_parser("upgrade-plan", help="Compare releases and preview component reuse/rebuild without changes")
    preview.add_argument("--from-kit", type=Path, help="old installed kit when automatic exact matching is unavailable")
    preview.add_argument("--json", action="store_true")
    apply = subparsers.add_parser("upgrade-release", help="Preview or explicitly execute a recoverable component rebuild")
    apply.add_argument("--from-kit", type=Path)
    apply.add_argument("--backup-dir", type=Path, help="new private operation directory outside data/configuration/release paths")
    apply.add_argument("--resume", type=Path, help="resume an existing operation directory")
    apply.add_argument("--rollback", action="store_true", help="with --resume, restore the previous stopped state before services have started")
    apply.add_argument("--execute", action="store_true", help="require stopped services, private backup and interactive confirmation")
    apply.add_argument("--json", action="store_true", help="preview only")


def dispatch(args, layout, node):
    """Delegate privileged filesystem changes to the installed, journal-bound runner."""
    import node_upgrade_session as session
    if args.command == "upgrade-plan":
        value = plan(layout, node, args.from_kit)
        print(json.dumps(value, indent=2, sort_keys=True)) if args.json else show(value)
        return 0 if value["executable"] else 2
    core.require(not (args.execute and args.json), "--json is preview-only")
    core.require(not args.rollback or args.resume, "--rollback requires --resume")
    core.require(not args.resume or not (args.backup_dir or args.from_kit), "--resume cannot be combined with --backup-dir/--from-kit")
    if args.resume:
        backup = core.absolute(args.resume)
        record = session.read(backup)
        value = record["plan"]
        core.require(value["target_kit"] == str(layout.kit_root) and value["target_manifest_sha256"] == sha(layout.manifest_path),
                     "Recovery requires the exact target release kit recorded in the journal")
    else:
        value = plan(layout, node, args.from_kit)
        backup = args.backup_dir
    print(json.dumps(value, indent=2, sort_keys=True)) if args.json else show(value)
    if not args.execute:
        if not args.json:
            print("Preview only. Compatible updates use activate-release; rebuilds require down, then upgrade-release --backup-dir PATH --execute.")
        return 0 if value["executable"] else 2
    core.require(value["executable"], "Upgrade plan is blocked; no changes were made")
    core.require(value["classification"] != "compatible", "Use usdb-node activate-release for a compatible upgrade")
    core.require(sys.stdin.isatty() and sys.stdout.isatty(), "Upgrade execution requires an interactive terminal")
    core.require(backup is not None, "--backup-dir is required for the first execution")
    if not args.resume:
        backup = session.stage(value, core.absolute(backup), node)
    command = [sys.executable, "-B", str(Path(session.__file__).resolve()), "--resume", str(backup)]
    if args.rollback:
        command.append("--rollback")
    if os.geteuid() != 0:
        command = ["sudo", "--", *command]
    print("Upgrade recovery command: " + shlex.join(command), flush=True)
    return subprocess.run(command, check=False).returncode
