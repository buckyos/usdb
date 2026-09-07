"""Public SourceDAO configuration and frozen-bundle validation shared by release tools."""
from __future__ import annotations

import hashlib
import copy
import json
import re
import shutil
from pathlib import Path, PurePosixPath
from typing import Any

FREEZE_SCHEMA = "usdb-sourcedao-bootstrap-freeze:v1"
PUBLIC_STATE_SCHEMA = "sourcedao-bootstrap-public-state:v1"
SOURCE_ARTIFACTS = {"sourcedao_bootstrap_freeze", "sourcedao_contract_golden", "sourcedao_bootstrap_source", "sourcedao_bootstrap_imported"}
PUBLIC_ARTIFACTS = SOURCE_ARTIFACTS | {"genesis", "genesis_manifest", "chain_bootstrap", "sourcedao_bootstrap", "snapshot_trusted_keys", "bootstrap_manifest", "network_environment", "compose_overlay"}

def apply_source_import(base: dict, imported: dict, report: dict) -> dict:
    """Verify shared source evidence, then copy only allocations and committee members."""
    from validate_network_bundle import require_no_runtime_secrets
    require(all(isinstance(value, dict) for value in (base, imported, report)), "source import inputs must be objects")
    require_no_runtime_secrets(report)
    if report.get("schemaVersion") == "sourcedao-bootstrap-source:v1":
        public_config(imported)
        require(report.get("configSha256") == semantic_digest(imported), "source report does not identify the imported config")
        return copy.deepcopy(imported)
    require(report.get("schemaVersion") == "sourcedao-bootstrap-source:v2", "unsupported source report schema")
    require(report.get("importedSha256") == semantic_digest(imported), "source report does not identify the imported config")
    source, checkpoint = report.get("source", {}), report.get("checkpoint", {})
    require(isinstance(source, dict) and isinstance(checkpoint, dict), "invalid source identity or checkpoint")
    require(report.get("sourceIdentitySha256") == semantic_digest(source), "source identity digest mismatch")
    require(type(checkpoint.get("number")) is int and checkpoint["number"] > 0 and
            all(isinstance(checkpoint.get(key), str) and re.fullmatch(r"0x[0-9a-fA-F]{64}", checkpoint[key]) for key in ("hash", "stateRoot")), "invalid source checkpoint")
    tokens = report.get("tokens", {})
    require(isinstance(tokens, dict) and all(isinstance(tokens.get(key), dict) for key in ("devToken", "normalToken")), "invalid source token observations")
    token = tokens.get("devToken", {})
    require(tokens.get("normalToken", {}).get("totalSupply") == "0", "unsupported original NormalToken supply")
    policy = report.get("policy", {})
    require(isinstance(policy, dict) and policy.get("committee") == "members-at-checkpoint" and policy.get("tokenAllocation") == "original-deployment-mints", "unsupported source import policy")
    allocations = token.get("allocations")
    require(isinstance(allocations, list) and all(isinstance(item, dict) and set(item) == {"address", "amount"} for item in allocations), "invalid original allocations")
    expected = {"schemaVersion": "sourcedao-bootstrap-import:v1", "sourceIdentitySha256": report["sourceIdentitySha256"], "checkpoint": checkpoint,
                "committee": {"initialMembers": report.get("committeeMembers")},
                "devToken": {"totalSupply": token.get("totalSupply"), "initAddresses": [item["address"] for item in allocations], "initAmounts": [item["amount"] for item in allocations]}}
    require(imported == expected, "shared import differs from source observations")
    config = copy.deepcopy(base)
    config["committee"]["initialMembers"] = copy.deepcopy(imported["committee"]["initialMembers"])
    config["devToken"].update(copy.deepcopy(imported["devToken"]))
    public_config(config)
    require(isinstance(token.get("reserve"), str) and re.fullmatch(r"0|[1-9][0-9]*", token["reserve"]) and
            int(token["reserve"]) + sum(int(value) for value in config["devToken"]["initAmounts"]) == int(token["totalSupply"]), "source reserve and allocations differ from original supply")
    return config

def require(value: Any, message: str) -> None:
    """Reject an invalid public input with an actionable validation error."""
    if not value:
        raise ValueError(message)

def digest(data: bytes) -> str:
    """Identify the exact artifact bytes distributed by the release."""
    return hashlib.sha256(data).hexdigest()

def semantic_digest(value: Any) -> str:
    """Match the TypeScript canonical digest for public JSON business inputs."""
    return digest(json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode())

def safe_file(bundle: Path, relative: str) -> Path:
    """Reject traversal and links outside the approved bundle."""
    require(isinstance(relative, str), "artifact path must be a string")
    parts = PurePosixPath(relative)
    require(not parts.is_absolute() and ".." not in parts.parts and "\\" not in relative, "unsafe artifact path")
    resolved = (bundle / relative).resolve()
    require(resolved.is_relative_to(bundle.resolve()) and resolved.is_file(), "artifact is missing or escapes bundle")
    return resolved

def public_config(config: dict) -> None:
    """Freeze only supported business fields; runtime credentials and paths are not accepted."""
    sections = {
        "$": {"schemaVersion", "chainId", "daoAddress", "dividendAddress", "bootstrapAdminAddress", "cycleMinLength", "transactionGasLimit", "devToken", "normalToken", "committee", "project", "tokenLockup", "acquired"},
        "devToken": {"name", "symbol", "totalSupply", "initAddresses", "initAmounts"},
        "normalToken": {"name", "symbol"},
        "committee": {"initialMembers", "initProposalId", "initDevRatio", "mainProjectName", "finalVersion", "finalDevRatio"},
        "project": {"initProjectIdCounter"}, "tokenLockup": {"unlockProjectName", "unlockVersion"}, "acquired": {"initInvestmentCount"},
    }
    for key, expected in sections.items():
        value = config if key == "$" else config.get(key)
        require(isinstance(value, dict) and set(value) == expected, f"unsupported or missing public config fields: {key}")
    require(type(config["schemaVersion"]) is int and config["schemaVersion"] == 1, "unsupported SourceDAO config schema")
    def integer(value: Any, minimum: int = 0) -> None:
        require(type(value) is int and minimum <= value <= 2**53 - 1, "invalid public configuration integer")
    def address(value: Any) -> None:
        require(isinstance(value, str) and re.fullmatch(r"0x[0-9a-fA-F]{40}", value) and int(value[2:], 16) != 0, "invalid public configuration address")
    def addresses(value: Any, nonempty: bool = False) -> None:
        require(isinstance(value, list) and (value or not nonempty), "invalid address list")
        for item in value:
            address(item)
        require(len({item.lower() for item in value}) == len(value), "duplicate configuration address")
    def amount(value: Any) -> int:
        require(isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]*", value), "token amounts must be canonical decimal strings")
        result = int(value)
        require(result < 2**256, "token amount exceeds uint256")
        return result
    integer(config["chainId"], 1); integer(config["cycleMinLength"], 1); integer(config["transactionGasLimit"], 1)
    for field in ("daoAddress", "dividendAddress", "bootstrapAdminAddress"):
        address(config[field])
    token, committee = config["devToken"], config["committee"]
    addresses(token["initAddresses"]); addresses(committee["initialMembers"], True)
    require(isinstance(token["initAmounts"], list) and len(token["initAmounts"]) == len(token["initAddresses"]), "allocation array length mismatch")
    supply = amount(token["totalSupply"])
    require(supply > 0 and sum(amount(value) for value in token["initAmounts"]) <= supply, "initial allocation exceeds supply")
    for key in ("devToken", "normalToken"):
        for field in ("name", "symbol"):
            require(isinstance(config[key][field], str) and config[key][field].strip(), "token name and symbol are required")
    for field, minimum in (("initProposalId", 0), ("initDevRatio", 101), ("finalDevRatio", 101)):
        integer(committee[field], minimum)
    integer(config["project"]["initProjectIdCounter"]); integer(config["acquired"]["initInvestmentCount"])
    for section, name, version in ((committee, "mainProjectName", "finalVersion"), (config["tokenLockup"], "unlockProjectName", "unlockVersion")):
        require(isinstance(section[name], str) and 0 < len(section[name].encode()) <= 31, "project name must fit bytes32")
        match = re.fullmatch(r"([0-9]+)\.([0-9]+)\.([0-9]+)", str(section[version]))
        require(match is not None, "invalid project version")
        major, minor, patch = map(int, match.groups())
        require(minor < 100000 and patch < 100000 and major * 10**10 + minor * 10**5 + patch <= 2**53 - 1, "project version exceeds supported range")

def copy_public_bundle(source: Path, destination: Path, network: dict, *, include_freeze: bool = True) -> None:
    """Copy declared public inputs only, never node.env, logs or recovery journals."""
    names = {"network.json", "network.env", "node.env.example", "compose.network.yml", "snapshots/balance-history-snapshot-release-record.json"}
    if (source / "README.md").is_file():
        names.add("README.md")
    for key, entry in network["artifacts"].items():
        require(key in PUBLIC_ARTIFACTS, f"unrecognized public artifact: {key}")
        if include_freeze or key not in SOURCE_ARTIFACTS:
            names.add(entry["path"])
    destination.mkdir(parents=True, exist_ok=True)
    for name in sorted(names):
        original = safe_file(source, name)
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, target)

def validate_frozen_bundle(bundle: Path, network: dict, read_json) -> None:
    """Old bundles remain readable; a declared freeze must be complete and internally consistent."""
    artifacts = network["artifacts"]
    if "sourcedao_bootstrap_freeze" not in artifacts:
        require(not (SOURCE_ARTIFACTS & set(artifacts)), "partial SourceDAO freeze artifacts")
        return
    record = read_json(safe_file(bundle, artifacts["sourcedao_bootstrap_freeze"]["path"]))
    require(record.get("schema_version") == FREEZE_SCHEMA, "unsupported SourceDAO freeze schema")
    config_file = safe_file(bundle, artifacts["sourcedao_bootstrap"]["path"])
    config = read_json(config_file); public_config(config)
    require(record.get("chain_id") == network["chain_id"] and record.get("network_bundle_id") == network["network_bundle_id"], "SourceDAO freeze network mismatch")
    require(record.get("config_sha256") == digest(config_file.read_bytes()) and record.get("config_semantic_sha256") == semantic_digest(config), "SourceDAO frozen config mismatch")
    golden = safe_file(bundle, artifacts.get("sourcedao_contract_golden", {}).get("path"))
    require(record.get("golden_sha256") == semantic_digest(read_json(golden)), "SourceDAO frozen golden mismatch")
    require(("sourcedao_bootstrap_source" in artifacts) == ("sourcedao_bootstrap_imported" in artifacts), "SourceDAO source pair is incomplete")
    for key in ("sourcedao_bootstrap_source", "sourcedao_bootstrap_imported"):
        expected = record.get("provenance", {}).get(key)
        require((key in artifacts) == (expected is not None), "SourceDAO source provenance is incomplete")
        if expected is not None:
            require(expected == digest(safe_file(bundle, artifacts[key]["path"]).read_bytes()), "SourceDAO source provenance mismatch")
    if "sourcedao_bootstrap_source" in artifacts:
        require("sourcedao_bootstrap_imported" in artifacts, "SourceDAO source pair is incomplete")
        apply_source_import(config, read_json(safe_file(bundle, artifacts["sourcedao_bootstrap_imported"]["path"])),
                            read_json(safe_file(bundle, artifacts["sourcedao_bootstrap_source"]["path"])))
