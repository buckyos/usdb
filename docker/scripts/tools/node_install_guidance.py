#!/usr/bin/env python3
"""Read local deployment metadata and print one applicable post-install path."""

import argparse
import os
from pathlib import Path
import re
import shlex
from types import SimpleNamespace

import node_network_switch as networks
import node_rebuild as core
import node_upgrade as upgrade
import node_upgrade_archives as archives
import node_upgrade_session as session
import usdb_node as node


def metadata(path):
    """Bound local reads and never echo private JSON contents or parser errors."""
    try:
        core.safe_path(path)
        info = path.stat()
        if not path.is_file() or info.st_uid != os.getuid() or info.st_size > 1024 * 1024:
            raise ValueError("unsupported metadata")
        value = core.read_json(path)
        if not isinstance(value, dict):
            raise ValueError("expected object")
        return value
    except (OSError, ValueError):
        raise ValueError(f"Cannot inspect deployment metadata {path}; check access and format. Values were not printed.") from None


def recovery_records(layout, configurations):
    """Inspect pending markers and registered journals only, without scanning databases."""
    references = []
    # An interrupted operation may leave a marker even when node.env is missing.
    directories = ({layout.node_env.parent} if layout.node_env.exists()
                   else set(networks.configuration_directories(layout)))
    for directory in sorted(directories):
        marker = directory / upgrade.PENDING
        if marker.exists() or marker.is_symlink():
            references.append((marker, metadata(marker), directory / "node.env"))
    for item in configurations:
        try:
            catalog = core.absolute(item["env"]["USDB_DATA_ROOT"]) / archives.CATALOG
            core.safe_path(catalog)
            if catalog.exists():
                for path in sorted(catalog.glob("*.json")):
                    references.append((path, metadata(path), None))
        except (OSError, ValueError):
            raise ValueError(f"Cannot inspect upgrade registrations for {item['path']}; check the configured data root and catalog.") from None
    result = {}
    relevant = {str(item["path"]) for item in configurations} | {str(layout.node_env)}
    for reference, pointer, expected_env in references:
        try:
            backup = core.absolute(pointer["backup_dir"])
            record = metadata(backup / "upgrade.json")
            plan = record["plan"]
            if (record["schema_version"] != session.SCHEMA or plan["schema_version"] != upgrade.SCHEMA
                    or record["operation_id"] != pointer["operation_id"]
                    or not re.fullmatch(r"[0-9a-f]{32}", record["operation_id"])):
                raise ValueError("record identity mismatch")
            if expected_env is not None and (plan["env_path"] != str(expected_env)
                                             or plan["target_release"] != pointer["target_release"]):
                raise ValueError("pending marker mismatch")
            if expected_env is None and (plan["env_path"] not in relevant
                    or record["phase"] in {*session.TERMINAL, *archives.CLEANUP_PHASES}):
                continue
            if record["phase"] not in {"staged", "prepared", "rolling_back", *session.TERMINAL}:
                raise ValueError("unknown phase")
            kit = core.absolute(plan["target_kit"])
            core.absolute(plan["env_path"])
            core.require(node.RELEASE_ID_RE.fullmatch(plan["target_release"]), "invalid release")
            result[str(backup)] = dict(backup=backup, kit=kit, env=plan["env_path"],
                release=plan["target_release"], phase=record["phase"],
                manifest_sha256=plan["target_manifest_sha256"])
        except (OSError, ValueError, KeyError, TypeError):
            raise ValueError(f"Cannot validate saved upgrade referenced by {reference}; inspect the marker and upgrade.json before setup or activation.") from None
    return list(result.values())


def recovery_lines(records):
    """Recommend preview through the journal's exact target kit, never blind execution."""
    lines = ["Deployment: unfinished upgrade requires review", "Do not start a new setup or activation before reviewing this operation."]
    for record in records:
        lines += [f"  Saved target: {record['release']} | phase={record['phase']}",
                  f"  Saved backup: {record['backup']}", f"  Recovery kit: {record['kit']}"]
        if record["phase"] in session.TERMINAL:
            lines.append("  The operation is terminal but its pending marker remains; inspect the record before proceeding.")
        script = record["kit"] / "docker/scripts/tools/usdb_node.py"
        try:
            core.safe_path(script)
            manifest = node._load_release_manifest(record["kit"])
            valid = (script.is_file() and manifest["release_id"] == record["release"]
                     and upgrade.sha(record["kit"] / "release/usdb-release-manifest.json") == record["manifest_sha256"])
        except (OSError, ValueError):
            valid = False
        if not valid:
            lines.append("  Restore the exact saved target kit at this path before recovery; the newly selected launcher may be a different release.")
            continue
        lines += ["  Preview the saved operation:", "    " + shlex.join(["python3", str(script),
                  "--node-env", record["env"], "upgrade-release", "--resume", str(record["backup"])])]
    lines.append("Follow the recovery preview's next steps; it checks whether resume or rollback is applicable.")
    return lines


def render(kit_root, bin_dir):
    """Select advice using local metadata only; no service probes, sudo, or writes."""
    manifest = node._load_release_manifest(kit_root)
    bundle = manifest["network_bundle"]["bundle_id"]
    layout = SimpleNamespace(bundle_id=bundle, node_env=Path.home() / ".config/usdb" / bundle / "node.env")
    target = None
    if layout.node_env.exists() or layout.node_env.is_symlink():
        target = dict(bundle=bundle, path=layout.node_env,
                      env=networks.read_configuration(layout.node_env, node))
    previous = [] if target else networks.configurations(layout, node)
    records = recovery_records(layout, [target] if target else previous)
    lines = [f"Target network: {bundle}", f"Target configuration: {layout.node_env}",
             "Running services: not checked; installing the tool does not activate service images.", ""]
    if str(bin_dir) not in os.environ.get("PATH", "").split(os.pathsep):
        lines += ["Make the installed command available in this shell:",
                  "  export PATH=" + shlex.quote(str(bin_dir)) + ':"$PATH"', ""]
    if records:
        lines += recovery_lines(records)
    elif target:
        lines += ["Deployment: existing target network configuration", "Next step:", "  usdb-node upgrade-plan",
                  "Use the plan's recommended commands; do not run setup again for a release upgrade.",
                  "A repeated installation may need no activation. Compatibility and running state are not inferred from version numbers."]
    elif previous:
        selected = networks.default_source(layout, previous)
        lines += ["Deployment: target network setup required", "Target configuration has not been created."]
        lines += networks.source_selection_lines(networks.source_selection(previous, selected))
        lines += [f"Existing data root: {shlex.quote(selected['env']['USDB_DATA_ROOT'])}", "", "Next step:",
                  "  usdb-node upgrade-plan",
                  "The plan checks data reuse and gives the old kit's stop command and target setup command.",
                  "Select the original data root during setup to reuse compatible datasets; then run doctor and up.",
                  "down alone leaves autostart enabled. Before reboot, complete setup: it checks and disables supported old autostart sharing data."]
    else:
        lines += ["Deployment: node setup required (no standard node configuration found)", "Next steps:",
                  "  usdb-node prepare-host", "  usdb-node setup", "  usdb-node doctor", "  usdb-node up", "  usdb-node status",
                  "If Docker group membership changes, log out and back in before continuing.",
                  "Retained data may still exist: select its original Host data root during setup.",
                  "Use setup --no-controller only for intentional foreground or non-systemd operation.",
                  "For a configuration outside the standard directory, inspect it with usdb-node --node-env /absolute/path/to/node.env upgrade-plan first."]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kit-root", type=Path, required=True)
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        print(render(args.kit_root, args.bin_dir))
    except (OSError, ValueError, KeyError, TypeError) as error:
        # Expected diagnostic errors are already redacted; avoid exposing raw JSON/OS payloads.
        message = str(error) if isinstance(error, ValueError) and str(error).startswith((
            "Cannot inspect", "Cannot validate", "Multiple configurations")) else f"Local metadata could not be interpreted ({type(error).__name__})."
        print("Deployment advice unavailable: " + message)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
