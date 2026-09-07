#!/usr/bin/env python3
"""Freeze a reviewed SourceDAO candidate into a new public network bundle."""
from __future__ import annotations

import argparse
import copy
import json
import os
import re
import shutil
import tempfile
from pathlib import Path

from sourcedao_release import SOURCE_ARTIFACTS, FREEZE_SCHEMA, apply_source_import, copy_public_bundle, digest, public_config, require, semantic_digest
from validate_network_bundle import read_json, validate_network_bundle

def differences(before, after, path="") -> list[dict]:
    """Record explicit post-import business changes for release review."""
    if before == after:
        return []
    if isinstance(before, dict) and isinstance(after, dict) and before.keys() == after.keys():
        return [item for key in sorted(before) for item in differences(before[key], after[key], f"{path}.{key}".lstrip("."))]
    return [{"field": path, "before": before, "after": after}]

def freeze(*, bundle: Path, config: Path, golden: Path, output: Path, imported_config: Path | None = None, source_report: Path | None = None) -> Path:
    """Validate before publishing; the input bundle and candidate files remain untouched."""
    bundle, output = bundle.resolve(), output.resolve()
    require(not output.exists() and not output.is_relative_to(bundle), "output must be a new directory outside the input bundle")
    network = validate_network_bundle(bundle)
    candidate = read_json(config); public_config(candidate)
    reviewed_golden = read_json(golden)
    contracts = reviewed_golden.get("contracts")
    require(reviewed_golden.get("schema_version") == "sourcedao-usdb-contract-golden:v1" and isinstance(contracts, list) and len(contracts) == 9, "reviewed golden must contain nine contracts")
    require(bool(imported_config) == bool(source_report), "--imported-config and --source-report must be supplied together")
    original = read_json(bundle / "artifacts/sourcedao-bootstrap-config.json")
    provenance = {}
    if imported_config and source_report:
        original = apply_source_import(original, read_json(imported_config), read_json(source_report))
        provenance = {"sourcedao_bootstrap_source": digest(source_report.read_bytes()), "sourcedao_bootstrap_imported": digest(imported_config.read_bytes())}
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".sourcedao-freeze-", dir=output.parent))
    try:
        copy_public_bundle(bundle, staging, network, include_freeze=False)
        network = copy.deepcopy(network)
        for key in SOURCE_ARTIFACTS:
            network["artifacts"].pop(key, None)
        def write(relative: str, value) -> bytes:
            data = (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()
            filename = staging / relative
            filename.parent.mkdir(parents=True, exist_ok=True)
            filename.write_bytes(data)
            return data
        def artifact(key: str, relative: str, data: bytes) -> None:
            (staging / relative).write_bytes(data)
            network["artifacts"][key] = {"path": relative, "sha256": digest(data)}
        data = write("artifacts/sourcedao-bootstrap-config.json", candidate)
        artifact("sourcedao_bootstrap", "artifacts/sourcedao-bootstrap-config.json", data)
        manifest = read_json(staging / "artifacts/usdb-genesis.manifest.json")
        manifest["sourcedao_config_sha256"] = digest(data)
        manifest_data = write("artifacts/usdb-genesis.manifest.json", manifest)
        artifact("genesis_manifest", "artifacts/usdb-genesis.manifest.json", manifest_data)
        artifact("sourcedao_contract_golden", "artifacts/sourcedao-contract-golden.json", golden.read_bytes())
        if imported_config and source_report:
            artifact("sourcedao_bootstrap_source", "artifacts/sourcedao-bootstrap-source.json", source_report.read_bytes())
            artifact("sourcedao_bootstrap_imported", "artifacts/sourcedao-bootstrap-imported.json", imported_config.read_bytes())
        record = {"schema_version": FREEZE_SCHEMA, "network_bundle_id": network["network_bundle_id"], "chain_id": network["chain_id"],
                  "config_sha256": digest(data), "config_semantic_sha256": semantic_digest(candidate), "golden_sha256": semantic_digest(reviewed_golden),
                  "provenance": provenance, "overrides": differences(original, candidate)}
        freeze_data = write("artifacts/sourcedao-bootstrap-freeze.json", record)
        artifact("sourcedao_bootstrap_freeze", "artifacts/sourcedao-bootstrap-freeze.json", freeze_data)
        write("network.json", network)
        validate_network_bundle(staging)
        require(not output.exists(), "output appeared during freeze; refusing to replace it")
        os.rename(staging, output)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return output

USDB_ROOT = Path(__file__).resolve().parents[3]
SECURITY_ROOT = USDB_ROOT.parent / "SourceDAO/security"
INPUTS_FILE = "sourcedao-bootstrap-inputs.json"

def prepare(*, bundle: Path, directory: Path, imported: Path, report: Path) -> Path:
    """Create one editable target candidate and pin references to immutable shared evidence."""
    require(not directory.exists(), "candidate directory already exists; keep edited final or choose a new --input-dir")
    network = validate_network_bundle(bundle)
    base = read_json(bundle / "artifacts/sourcedao-bootstrap-config.json")
    candidate = apply_source_import(base, read_json(imported), read_json(report))
    require(all(candidate[field] == base[field] for field in ("chainId", "daoAddress", "dividendAddress", "bootstrapAdminAddress")), "imported legacy config belongs to another target identity; use shared source format")
    manifest = read_json(bundle / "artifacts/usdb-genesis.manifest.json")
    inputs = {"schemaVersion": "sourcedao-bootstrap-inputs:v1", "networkBundleId": network["network_bundle_id"],
              "chainId": network["chain_id"], "genesisHash": manifest["block_hash"], "baseConfigSha256": semantic_digest(base),
              "imported": {"path": os.path.relpath(imported.resolve(), directory.resolve()), "sha256": digest(imported.read_bytes())},
              "report": {"path": os.path.relpath(report.resolve(), directory.resolve()), "sha256": digest(report.read_bytes())}}
    directory.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".sourcedao-prepare-", dir=directory.parent))
    try:
        for name, value in (("sourcedao-bootstrap-final.json", candidate), (INPUTS_FILE, inputs)):
            (staging / name).write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
        require(not directory.exists(), "candidate directory appeared during preparation")
        os.rename(staging, directory)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return directory / "sourcedao-bootstrap-final.json"

def candidate_sources(directory: Path, bundle: Path) -> tuple[Path, Path]:
    """Resolve pinned source files without depending on today's source checkpoint setting."""
    inputs = read_json(directory / INPUTS_FILE)
    require(isinstance(inputs, dict), "candidate inputs must be an object")
    network = validate_network_bundle(bundle)
    manifest = read_json(bundle / "artifacts/usdb-genesis.manifest.json")
    base = read_json(bundle / "artifacts/sourcedao-bootstrap-config.json")
    require(inputs.get("schemaVersion") == "sourcedao-bootstrap-inputs:v1" and inputs.get("networkBundleId") == network["network_bundle_id"] and
            inputs.get("chainId") == network["chain_id"] and inputs.get("genesisHash") == manifest["block_hash"] and
            inputs.get("baseConfigSha256") == semantic_digest(base), "candidate base bundle changed; review and prepare a new candidate")
    files = []
    for key in ("imported", "report"):
        entry = inputs.get(key, {})
        require(isinstance(entry, dict), "candidate source reference must be an object")
        require(isinstance(entry.get("path"), str) and not Path(entry["path"]).is_absolute(), "candidate source reference must be relative")
        filename = (directory / entry["path"]).resolve()
        require(digest(filename.read_bytes()) == entry.get("sha256"), "candidate source evidence changed")
        files.append(filename)
    return files[0], files[1]

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--network", default=os.environ.get("SOURCE_DAO_NETWORK"), help="target profile (default: usdb-testnet-v0)")
    for name, help_text in {
        "bundle-dir": "base bundle (default: sibling USDB docker/networks for the selected network)",
        "config": "final config (default: <input-dir>/sourcedao-bootstrap-final.json)",
        "contract-golden": "reviewed golden (default: SourceDAO/security/usdb-contract-golden.json)",
        "input-dir": "candidate directory (default: SourceDAO/security/candidate/<network>)",
        "source-dir": "shared imported/source files for --prepare (default: source config's pinned imports/<block>)",
    }.items():
        parser.add_argument(f"--{name}", type=Path, help=help_text)
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument("--output-dir", type=Path, help="new frozen bundle (default: <input-dir>/frozen-network-bundle)")
    destination.add_argument("--apply", action="store_true", help="promote validated public files into the source bundle, keeping a rollback copy")
    destination.add_argument("--prepare", action="store_true", help="create final candidate and pinned source references; never freeze or overwrite it")
    parser.add_argument("--imported-config", type=Path)
    parser.add_argument("--source-report", type=Path)
    args = parser.parse_args()
    try:
        selected = args.network or "usdb-testnet-v0"
        require(re.fullmatch(r"[a-z0-9][a-z0-9-]*", selected), "invalid network name")
        bundle = args.bundle_dir or USDB_ROOT / "docker/networks" / ("testnet-v0" if selected == "usdb-testnet-v0" else selected)
        network = validate_network_bundle(bundle)
        require(not args.network or network["network_bundle_id"] == selected, "bundle differs from selected network")
        directory = args.input_dir or SECURITY_ROOT / "candidate" / network["network_bundle_id"]
        imported, report = args.imported_config, args.source_report
        require(bool(imported) == bool(report), "--imported-config and --source-report must be supplied together")
        require(not args.source_dir or not imported, "use --source-dir or explicit source files, not both")
        if args.prepare:
            require(not args.config and not args.contract_golden, "--prepare does not accept --config or --contract-golden")
            source_dir = args.source_dir
            if source_dir is None and imported is None:
                source_config = read_json(SECURITY_ROOT / "sources/optimism/sourcedao-opmain-import-source.json")
                require(isinstance(source_config, dict) and type(source_config.get("blockNumber")) is int and source_config["blockNumber"] > 0, "source config must pin a positive blockNumber")
                source_dir = SECURITY_ROOT / "sources/optimism/imports" / str(source_config["blockNumber"])
            if source_dir:
                imported, report = source_dir / "sourcedao-bootstrap-imported.json", source_dir / "sourcedao-bootstrap-source.json"
            print(prepare(bundle=bundle, directory=directory, imported=imported, report=report))
            return 0
        require(not args.source_dir, "--source-dir is for --prepare; freeze uses pinned candidate inputs")
        if (not args.config or args.input_dir) and (directory / INPUTS_FILE).exists():
            pinned_imported, pinned_report = candidate_sources(directory, bundle)
            require(not imported or (imported.resolve(), report.resolve()) == (pinned_imported, pinned_report), "explicit source files differ from candidate inputs")
            imported, report = pinned_imported, pinned_report
        elif imported is None and not args.config:
            imported, report = directory / "sourcedao-bootstrap-imported.json", directory / "sourcedao-bootstrap-source.json"
        inputs = dict(bundle=bundle, config=args.config or directory / "sourcedao-bootstrap-final.json", golden=args.contract_golden or SECURITY_ROOT / "usdb-contract-golden.json", imported_config=imported, source_report=report)
        require(inputs["config"].is_file(), "final config is missing; run --prepare and review sourcedao-bootstrap-final.json before freeze")
        if args.apply:
            print(f"Applied frozen configuration; rollback copy: {apply_freeze(**inputs)}")
        else:
            print(freeze(**inputs, output=args.output_dir or directory / "frozen-network-bundle"))
    except (ValueError, OSError) as error:
        parser.exit(1, f"SourceDAO freeze failed: {error}\n")
    return 0

def apply_freeze(*, bundle: Path, **inputs) -> Path:
    """Promote a checked candidate; publish network.json last and retain recoverable originals."""
    bundle = bundle.resolve()
    lock = bundle / ".sourcedao-freeze.lock"
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError as error:
        raise ValueError("another freeze owns the bundle; verify the lock owner before recovery") from error
    with os.fdopen(descriptor, "w") as stream:
        stream.write(json.dumps({"pid": os.getpid()}))
    backup = Path(tempfile.mkdtemp(prefix=".sourcedao-freeze-backup-", dir=bundle.parent))
    candidate = backup / "candidate"
    previous: dict[str, bool] = {}
    changed: list[str] = []
    try:
        freeze(bundle=bundle, output=candidate, **inputs)
        names = [str(file.relative_to(candidate)) for file in candidate.rglob("*") if file.is_file()]
        names.sort(key=lambda name: (name == "network.json", name))
        for name in names:
            original = bundle / name
            previous[name] = original.is_file()
            if previous[name]:
                saved = backup / "original" / name
                saved.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(original, saved)
        (backup / "rollback.json").write_text(json.dumps({"bundle": str(bundle), "files": previous}, indent=2) + "\n")
        for name in names:
            target = bundle / name
            target.parent.mkdir(parents=True, exist_ok=True)
            os.replace(candidate / name, target)
            changed.append(name)
        validate_network_bundle(bundle)
        (backup / "completed").touch()
        return backup
    except BaseException:
        for name in reversed(changed):
            if previous[name]:
                os.replace(backup / "original" / name, bundle / name)
            else:
                (bundle / name).unlink()
        raise
    finally:
        lock.unlink()

if __name__ == "__main__":
    raise SystemExit(main())
