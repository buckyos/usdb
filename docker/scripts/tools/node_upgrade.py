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
        raise ValueError("UPGRADE_PENDING: finish the saved upgrade before starting or changing this node; rollback is limited to its documented boundary; "
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
            # Exact manifest/config matching below also supports read-only cross-bundle previews.
            candidates.update(p for p in parent.iterdir() if node.RELEASE_ID_RE.fullmatch(p.name))
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
    import node_protocol_upgrade
    protocol = node_protocol_upgrade.candidate(source_root, source, layout)
    reset = not protocol and bool(changed_chain or "usdb_chain" in changed_contracts)
    classification = "protocol_upgrade" if protocol else "network_reset" if reset else "data_rebuild" if changed_contracts else "compatible"
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
        adopt = protocol and service == "usdb_indexer" and service in changed_contracts
        rebuild = not protocol and (service in changed_contracts or (reset and service in {"usdb_indexer", "usdb_chain", "control_plane"}))
        if (rebuild or adopt) and old != new and new.exists():
            blocked.append(f"Target dataset already exists; preserve and inspect it instead of overwriting: {new}")
        components.append(dict(service=service, env_key=key, action="adopt" if adopt else "rebuild" if rebuild else "reuse",
                               source=str(old), target=str(new), marker_sha256=sha(marker),
                               reason="verify executed history before retaining dataset" if adopt else "consensus history changed" if reset and service in {"usdb_indexer", "usdb_chain", "control_plane"}
                               else "service contract changed" if rebuild else "service contract unchanged"))
    return dict(schema_version=SCHEMA, classification=classification, executable=not blocked, blockers=blocked,
                cross_bundle=old_network["bundle_id"] != layout.bundle_id,
                source_bundle=old_network["bundle_id"],
                source_release=source["release_id"], target_release=layout.release_id, bundle=layout.bundle_id,
                source_kit=str(source_root), target_kit=str(layout.kit_root), env_path=str(layout.node_env), data_root=str(root),
                source_manifest_sha256=sha(source_root / "release/usdb-release-manifest.json"),
                target_manifest_sha256=sha(layout.manifest_path), env_sha256=sha(layout.node_env),
                previous_compatibility_id=old_contract["compatibility_id"], target_compatibility_id=new_contract["compatibility_id"],
                changed_chain_fields=changed_chain,
                differences=changes(old_contract, new_contract, "runtime") + changes(
                    {k: old_network.get(k) for k in CHAIN_FIELDS}, {k: new_network.get(k) for k in CHAIN_FIELDS}, "network"),
                components=components, mining_action="disable and reauthorize on the new chain" if reset else "preserve",
                rollback_boundary="Before metadata writes only; afterwards resume forward" if protocol else "Only before starting services; all old datasets remain preserved")


SERVICE_LABELS = {"bitcoin_core": "Bitcoin Core", "balance_history": "balance-history",
                  "usdb_indexer": "USDB indexer", "usdb_chain": "USDB chain", "control_plane": "control-plane"}
CHAIN_LABELS = {"bundle_id": "network bundle", "chain_id": "Chain ID", "network_id": "P2P network ID",
                "genesis_block_hash": "genesis block", "genesis_sha256": "genesis configuration",
                "btc_network_id": "Bitcoin network", "btc_index_origin_height": "BTC index origin",
                "btc_rules_scope": "BTC rules scope", "btc_activation_registry_id": "BTC activation registry"}


def show(value, *, details=False):
    """Keep the decision readable; full identifiers and paths are opt-in diagnostics."""
    print(f"USDB upgrade: {value['source_release']} -> {value['target_release']}")
    labels = {"compatible": "existing data can be reused", "data_rebuild": "incompatible data must be rebuilt",
              "network_reset": "USDB chain must restart from genesis",
              "protocol_upgrade": "retain history after both offline service checks pass"}
    if value.get("cross_bundle"):
        print("Result: different network - target setup required; old network data stays preserved")
    else:
        print(f"Result: {value['classification']} - {labels[value['classification']]}")
    separate_setup = value.get("cross_bundle") and len(value["blockers"]) == 1
    print("Preflight: PASSED (execution still requires a stopped node)" if value["executable"]
          else "Preflight: SEPARATE_SETUP_REQUIRED" if separate_setup else "Preflight: BLOCKED")
    for reason in value["blockers"]:
        print(f"  - {reason}")
    print("\n  Component        Action   Data handling")
    for item in value["components"]:
        handling = "Move existing data after registry checks" if item["action"] == "adopt" else "Keep existing data" if item["action"] == "reuse" else (
            "New directory; old data kept" if item["source"] != item["target"] else "Archive old directory, then rebuild")
        label = SERVICE_LABELS.get(item["service"], item["service"])
        print(f"  {label:<16} {item['action'].upper():<8} {handling}")
    if value["changed_chain_fields"]:
        print("\nChanges: " + "; ".join(CHAIN_LABELS.get(k, k) for k in value["changed_chain_fields"]))
    elif value["classification"] == "data_rebuild":
        print("\nChanges: service data contracts")
    if value.get("cross_bundle"):
        print("On network switch: old chain/governance state stays in its original directories; new-network mining requires authorization.")
    elif value["classification"] == "network_reset":
        print("On reset: old SourceDAO state will be isolated; mining must be reauthorized.")
    if value["classification"] == "protocol_upgrade":
        print("Database compatibility: PENDING offline checks at each service's actual committed height.")
        print("After metadata writes begin, interrupted upgrades must resume forward; no automatic rollback or rebuild.")
    print("Old data, wallets and node identity are retained; no automatic data deletion.")
    if details:
        print("\nDetails:")
        print(f"  Runtime ID (source): {value['previous_compatibility_id']}")
        print(f"  Runtime ID (target): {value['target_compatibility_id']}")
        for item in value["components"]:
            print(f"  {SERVICE_LABELS.get(item['service'], item['service'])}: {item['reason']}")
            if item["source"] == item["target"]:
                print(f"    Directory: {item['source']}")
            else:
                print(f"    Source: {item['source']}")
                print(f"    Target: {item['target']}")
        for item in value["differences"]:
            if item["field"] != "runtime.compatibility_id":
                print(f"  {item['field']}: {item['previous']} -> {item['target']}")
    else:
        print("Use --details for paths and hashes, or --json for the complete plan.")


def show_next_steps(value, *, command=("usdb-node",), from_kit=None, backup=None,
                    resume=False, rollback=False, phase=None):
    """Recommend only the path allowed by this plan and its saved operation state."""
    def emit(*arguments):
        print("  " + shlex.join([*command, *map(str, arguments)]))

    print("\nNext steps (not executed):")
    if not value["executable"]:
        if value.get("cross_bundle"):
            print("  This selects a different network. Automatic upgrade execution is not supported across bundles.")
            if len(value["blockers"]) != 1:
                print("  Additional blockers require review before configuring the target; preserve old data and configuration.")
                return
            print("  Stop the old network with its original kit:")
            print("  " + shlex.join([str(Path(value["source_kit"]) / "docker/scripts/tools/usdb_node.py"),
                                    "--node-env", value["env_path"], "down"]))
            print(f"  Configure the target network with its default config path; select Host data root: {value['data_root']}")
            print("  " + shlex.join(["usdb-node", "--kit-root", value["target_kit"], "setup"]))
            print("  setup checks and retires supported old autostart services sharing data; no separate controller disable is needed.")
            print("  Reuse still requires matching dataset identities; inspect any additional blockers above before setup.")
            return
        print("  Resolve the blockers above, then rerun upgrade-plan. Do not activate or rebuild yet.")
        return
    if phase == "rolled_back":
        print(f"  This operation was rolled back. Use the original kit: {value['source_kit']}")
        print("  Plan a new operation if you want to upgrade again.")
        return
    if resume and (rollback or phase == "rolling_back"):
        print("  Rollback is allowed only before new data/configuration has changed.")
        emit("upgrade-release", "--resume", backup, "--rollback", "--execute")
        print(f"  After rollback, use the original kit: {value['source_kit']}")
        return
    if resume and phase != "applied":
        print("  Continue the saved operation with this target kit; services must remain stopped.")
        emit("upgrade-release", "--resume", backup, "--execute")
        return
    if phase != "applied":
        if value["classification"] == "network_reset":
            print("  Coordinate this development-network reset with the network operator first.")
        emit("down")
        source = ["--from-kit", str(from_kit)] if from_kit else []
        if value["classification"] == "compatible":
            emit("activate-release", *source)
        else:
            if backup is None:
                backup = "/absolute/path/to/new-private-upgrade-backup"
                print("  Replace the backup path below with a NEW private directory outside node data/config/kit paths.")
            emit("upgrade-release", *source, "--backup-dir", backup, "--execute")
            print("  After it succeeds (services remain stopped):")
    else:
        print("  Upgrade already applied. Continue with the target kit:")
    if value["classification"] != "compatible":
        print("  For background operation, refresh the controller using your existing options:")
        emit("controller", "install")
    emit("doctor")
    emit("up")
    emit("status")


def _command_prefix(args, node):
    """Keep explicitly selected kit/configuration paths in copyable recommendations."""
    result = ["usdb-node"]
    if args.kit_root != node.KIT_ROOT:
        result += ["--kit-root", str(args.kit_root)]
    if args.node_env is not None:
        result += ["--node-env", str(args.node_env)]
    return result


def add_parser(subparsers):
    """Expose preview separately from explicitly confirmed execution/recovery."""
    preview = subparsers.add_parser("upgrade-plan", help="Compare releases and preview data reuse; defaults to the newest prior network before target setup",
        description="Read-only compatibility and data reuse preview. If target setup is missing, prefer the same network family, then the highest numeric vN; "
                    "use the global --node-env option to override the comparison source. Version order does not establish compatibility. "
                    "Cross-network deployment requires separate setup, not automatic upgrade execution.")
    preview.add_argument("--from-kit", type=Path, help="old installed kit when automatic exact matching is unavailable")
    preview.add_argument("--details", action="store_true", help="include full paths and hashes in human-readable output")
    preview.add_argument("--json", action="store_true", help="print the read-only upgrade plan as JSON")
    apply = subparsers.add_parser("upgrade-release", help="Preview or execute a checked protocol upgrade or component rebuild")
    apply.add_argument("--from-kit", type=Path, help="old installed kit when automatic exact matching is unavailable")
    apply.add_argument("--backup-dir", type=Path, help="new private operation directory outside data/configuration/release paths")
    apply.add_argument("--resume", type=Path, help="resume an existing operation directory")
    apply.add_argument("--rollback", action="store_true", help="with --resume, restore stopped state; unavailable after protocol metadata writes begin")
    apply.add_argument("--execute", action="store_true", help="require stopped services, private backup and interactive confirmation")
    apply.add_argument("--details", action="store_true", help="include full paths and hashes in human-readable preview")
    apply.add_argument("--json", action="store_true", help="preview only")


def dispatch(args, layout, node):
    """Delegate privileged filesystem changes to the installed, journal-bound runner."""
    import node_upgrade_session as session
    if args.command == "upgrade-plan":
        source_layout = layout
        selection = None
        if not layout.node_env.exists() and not layout.node_env.is_symlink():
            if args.node_env is not None:
                raise ValueError(f"Selected node configuration does not exist: {layout.node_env}; check --node-env or run setup")
            import node_network_switch
            candidates = node_network_switch.configurations(layout, node)
            source_layout = node_network_switch.preview_source(layout, node, candidates=candidates)
            selected = next(item for item in candidates if item["path"] == source_layout.node_env)
            selection = node_network_switch.source_selection(candidates, selected)
            if not args.json:
                print("\n".join(node_network_switch.source_selection_lines(selection)))
        value = plan(source_layout, node, args.from_kit)
        if selection is not None:
            value["source_selection"] = selection
        if args.json:
            print(json.dumps(value, indent=2, sort_keys=True))
        else:
            show(value, details=args.details)
            show_next_steps(value, command=_command_prefix(args, node), from_kit=args.from_kit)
        return 0 if value["executable"] else 2
    core.require(not (args.execute and args.json), "--json is preview-only")
    core.require(not args.rollback or args.resume, "--rollback requires --resume")
    core.require(not args.resume or not (args.backup_dir or args.from_kit), "--resume cannot be combined with --backup-dir/--from-kit")
    phase = None
    if args.resume:
        backup = core.absolute(args.resume)
        record = session.read(backup)
        value = record["plan"]
        phase = record["phase"]
        core.require(not (args.rollback and record.get("protocol", {}).get("writes_started")),
                     "Protocol metadata writes already started; resume forward with --resume PATH --execute, rollback is unavailable")
        core.require(phase not in {"cleanup_started", "cleaned"},
                     "Cleanup relinquished rollback for this record; use upgrade-status or resume upgrade-cleanup")
        core.require(value["target_kit"] == str(layout.kit_root) and value["target_manifest_sha256"] == sha(layout.manifest_path),
                     "Recovery requires the exact target release kit recorded in the journal")
    else:
        value = plan(layout, node, args.from_kit)
        backup = args.backup_dir
    if not args.execute:
        if args.json:
            print(json.dumps(value, indent=2, sort_keys=True))
        else:
            if args.resume:
                print(f"Saved operation: {phase}")
            show(value, details=args.details)
            show_next_steps(value, command=_command_prefix(args, node), from_kit=args.from_kit,
                            backup=backup, resume=bool(args.resume), rollback=args.rollback, phase=phase)
        return 0 if value["executable"] else 2
    if not value["executable"]:
        show(value, details=args.details)
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
