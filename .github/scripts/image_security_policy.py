#!/usr/bin/env python3
"""Apply expiring, artifact-scoped exceptions without filtering scan evidence."""

from __future__ import annotations

import argparse
from collections import Counter
from datetime import date, datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

SEVERITIES = {"UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL"}
BLOCKING = {"HIGH", "CRITICAL"}
IMAGE_RE = re.compile(r"ghcr\.io/buckyos/[a-z0-9-]+@sha256:[0-9a-f]{64}")
TESTNET_RE = re.compile(r"refs/tags/usdb-testnet-v[0-9]+-r[1-9][0-9]*")
POLICY_SCHEMA = "usdb-image-exceptions:v1"


def require(condition: bool, message: str) -> None:
    """Reject incomplete evidence and ambiguous exception configuration."""
    if not condition:
        raise ValueError(message)


def strict_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        require(key not in result, f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(), object_pairs_hook=strict_object)
    require(isinstance(value, dict), f"Expected JSON object: {path}")
    return value


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_rust_coverage(report: dict, binaries: list[str]) -> None:
    """Do not interpret missing binary dependency metadata as a clean scan."""
    scanned = {target.get("Target", "").lstrip("/") for target in report.get("Results", [])
               if target.get("Class") == "lang-pkgs" and target.get("Type") == "rustbinary"
               and isinstance(target.get("Packages"), list) and target["Packages"]}
    missing = set(path.lstrip("/") for path in binaries) - scanned
    require(not missing, f"Missing Rust binary dependency coverage: {sorted(missing)}")


def source_fingerprint(root: Path, paths: list[str]) -> str:
    """Bind reviews to source bytes, executable modes, and added/deleted files."""
    require(bool(paths), "Review must identify its source paths")
    for path in paths:
        parts = Path(path).parts
        require(parts and not Path(path).is_absolute() and ".." not in parts,
                f"Invalid reviewed source path: {path}")
        require((root / path).exists(), f"Reviewed source path is missing: {path}")
    files = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others",
         "--exclude-standard", "--", *paths],
        check=True, capture_output=True,
    ).stdout.split(b"\0")
    digest = hashlib.sha256()
    require(any(files), "Reviewed source selection is empty")
    for name in sorted(set(files) - {b""}):
        path = root / name.decode("utf-8")
        require(path.is_file() and not path.is_symlink(),
                f"Reviewed source is missing or not a regular file: {path}")
        executable = b"x" if path.stat().st_mode & 0o111 else b"-"
        digest.update(name + b"\0" + executable + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def validate_policy(policy: dict) -> None:
    require(set(policy) == {"schema_version", "profiles", "exceptions"},
            "Unexpected exception catalog fields")
    require(policy["schema_version"] == POLICY_SCHEMA, "Unsupported exception schema")
    require(isinstance(policy["profiles"], dict), "Profiles must be an object")
    require(isinstance(policy["exceptions"], list), "Exceptions must be an array")
    for image, profile in policy["profiles"].items():
        require(IMAGE_RE.fullmatch(image + "@sha256:" + "0" * 64) is not None,
                "Invalid exception image repository")
        require(set(profile) == {"os", "platform", "source_paths", "source_sha256",
                                 "baseline_image", "baseline_report_sha256", "review"},
                f"Unexpected profile fields: {image}")
        require(profile["os"] == {"family": "debian", "name": "12.15"},
                "This catalog only supports the reviewed Debian 12.15 baseline")
        require(profile["platform"] == "linux/amd64", "Unreviewed image platform")
        for key in ("source_sha256", "baseline_report_sha256"):
            require(re.fullmatch(r"[0-9a-f]{64}", profile[key]) is not None,
                    f"Invalid profile {key}")
        require(profile["baseline_image"].startswith(image + "@") and
                IMAGE_RE.fullmatch(profile["baseline_image"]) is not None,
                "Invalid baseline digest")
        require(isinstance(profile["source_paths"], list) and
                all(isinstance(x, str) for x in profile["source_paths"]),
                "Invalid reviewed source paths")
        require(isinstance(profile["review"], str) and bool(profile["review"].strip()),
                "Missing review reference")
    seen = set()
    for item in policy["exceptions"]:
        require(set(item) == {"advisory_id", "severity", "images", "packages", "decision",
                              "owner", "reviewed_at", "expires_at", "reason", "conditions",
                              "remediation", "references"}, "Unexpected exception fields")
        require(re.fullmatch(r"CVE-[0-9]{4}-[0-9]{4,}", item["advisory_id"]) is not None,
                "Exceptions require an exact CVE identifier")
        require(item["severity"] in BLOCKING, "Invalid exception severity")
        require(item["decision"] in {"not_affected", "not_reachable", "mitigated"},
                "Invalid exception decision")
        for key in ("owner", "reason", "conditions", "remediation"):
            require(isinstance(item[key], str) and bool(item[key].strip()),
                    f"Missing exception {key}")
        require(isinstance(item["references"], list) and item["references"] and
                all(isinstance(x, str) and x.startswith("https://") for x in item["references"]),
                "Exceptions require advisory/review references")
        reviewed = date.fromisoformat(item["reviewed_at"])
        expires = date.fromisoformat(item["expires_at"])
        require(0 < (expires - reviewed).days <= 30, "Exception lifetime must be 1-30 days")
        require(isinstance(item["packages"], dict) and item["packages"],
                "Exception must name exact packages and versions")
        require(all(isinstance(k, str) and k and isinstance(v, str) and v and
                    not any(c in k + v for c in "*?<>")
                    for k, v in item["packages"].items()), "Package wildcards are prohibited")
        require(isinstance(item["images"], list) and item["images"], "Missing exception images")
        for image in item["images"]:
            require(image in policy["profiles"], f"Unknown image profile: {image}")
            for package, version in item["packages"].items():
                key = (image, item["advisory_id"], package, version)
                require(key not in seen, f"Overlapping exceptions: {key}")
                seen.add(key)


def evaluate(report: dict, policy: dict, *, image_reference: str, source_revision: str,
             source_ref: str, enforcement: str, root: Path, today: date | None = None) -> dict:
    """Return all raw counts and per-finding decisions; enforcement happens after upload."""
    today = today or datetime.now(timezone.utc).date()
    require(IMAGE_RE.fullmatch(image_reference) is not None, "Expected immutable GHCR image")
    require(re.fullmatch(r"[0-9a-f]{40}", source_revision) is not None, "Invalid source revision")
    require(enforcement in {"strict", "report-only"}, "Invalid enforcement mode")
    require(report.get("SchemaVersion") == 2 and report.get("ArtifactType") == "container_image",
            "Invalid Trivy container report")
    metadata = report.get("Metadata", {})
    require(report.get("ArtifactName") == image_reference and
            image_reference in metadata.get("RepoDigests", []), "Report image digest mismatch")
    labels = metadata.get("ImageConfig", {}).get("config", {}).get("Labels", {})
    require(labels.get("org.opencontainers.image.revision") == source_revision,
            "Report image source revision mismatch")
    require(isinstance(report.get("Results"), list) and report["Results"],
            "Report has no scan targets")
    validate_policy(policy)
    image = image_reference.split("@")[0]
    profile = policy["profiles"].get(image)
    scope_errors = []
    if not TESTNET_RE.fullmatch(source_ref):
        scope_errors.append("exceptions apply only to formal testnet tags")
    if profile:
        actual_os = metadata.get("OS", {})
        config = metadata.get("ImageConfig", {})
        if {"family": actual_os.get("Family"), "name": actual_os.get("Name")} != profile["os"]:
            scope_errors.append("reviewed operating system changed")
        if f"{config.get('os')}/{config.get('architecture')}" != profile["platform"]:
            scope_errors.append("reviewed platform changed")
        if source_fingerprint(root, profile["source_paths"]) != profile["source_sha256"]:
            scope_errors.append("reviewed source/build/deployment inputs changed")
    else:
        scope_errors.append("no reviewed image profile")
    raw_counts = Counter({x: 0 for x in sorted(SEVERITIES)})
    accepted = []
    blocked = []
    for target in report["Results"]:
        require(isinstance(target, dict) and isinstance(target.get("Target"), str) and
                isinstance(target.get("Class"), str) and isinstance(target.get("Type"), str),
                "Malformed scan target")
        vulnerabilities = target.get("Vulnerabilities", [])
        require(isinstance(vulnerabilities, list), "Malformed vulnerabilities array")
        for finding in vulnerabilities:
            require(isinstance(finding, dict), "Malformed vulnerability")
            for field in ("VulnerabilityID", "PkgName", "InstalledVersion", "Severity"):
                require(isinstance(finding.get(field), str) and bool(finding[field]),
                        f"Vulnerability missing {field}")
            severity = finding["Severity"]
            require(severity in SEVERITIES, "Unknown severity value")
            raw_counts[severity] += 1
            if severity not in BLOCKING:
                continue
            entry = {"advisory_id": finding["VulnerabilityID"], "package": finding["PkgName"],
                     "version": finding["InstalledVersion"], "severity": severity,
                     "target": target["Target"], "fixed_version": finding.get("FixedVersion", "")}
            candidates = [e for e in policy["exceptions"] if image in e["images"] and
                          e["advisory_id"] == finding["VulnerabilityID"] and
                          e["packages"].get(finding["PkgName"]) == finding["InstalledVersion"] and
                          e["severity"] == severity]
            reasons = list(scope_errors)
            if target["Class"] != "os-pkgs" or target["Type"] != "debian":
                reasons.append("OS exceptions never cover language or static dependencies")
            if finding.get("FixedVersion"):
                reasons.append("a fixed package is available; upgrade or review again")
            if not candidates:
                reasons.append("no exact CVE/package/version/severity exception")
            for exception in candidates:
                if not date.fromisoformat(exception["reviewed_at"]) <= today < date.fromisoformat(exception["expires_at"]):
                    reasons.append("exception is expired or not yet valid")
            if reasons:
                blocked.append({**entry, "reasons": reasons})
            else:
                exception = candidates[0]
                accepted.append({**entry, **{k: exception[k] for k in
                    ("decision", "owner", "expires_at", "reason", "conditions", "remediation")}})
    return {"schema_version": "usdb-image-policy-decision:v1", "image_reference": image_reference,
            "source_revision": source_revision, "source_ref": source_ref, "evaluated_on": str(today),
            "enforcement": enforcement, "raw_counts": dict(raw_counts),
            "accepted_count": len(accepted), "blocking_count": len(blocked),
            "accepted": accepted, "blocking": blocked,
            "result": "fail" if enforcement == "strict" and blocked else "pass"}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("evaluate", help="Write decisions before uploading evidence")
    check.add_argument("--report", type=Path, required=True)
    check.add_argument("--exceptions", type=Path)
    check.add_argument("--image-reference", required=True)
    check.add_argument("--source-revision", required=True)
    check.add_argument("--source-ref", required=True)
    check.add_argument("--enforcement", choices=("strict", "report-only"), required=True)
    check.add_argument("--repository-root", type=Path, default=Path.cwd())
    check.add_argument("--output", type=Path, required=True)
    check.add_argument("--required-rust-binary", action="append", default=[])
    enforce = sub.add_parser("enforce", help="Fail for unresolved strict findings after evidence upload")
    enforce.add_argument("--decision", type=Path, required=True)
    fingerprint = sub.add_parser("fingerprint", help="Print source fingerprint for a reviewed scope")
    fingerprint.add_argument("--repository-root", type=Path, default=Path.cwd())
    fingerprint.add_argument("paths", nargs="+")
    args = parser.parse_args()
    try:
        if args.command == "fingerprint":
            print(source_fingerprint(args.repository_root, args.paths))
            return 0
        if args.command == "enforce":
            decision = read_json(args.decision)
            require(decision.get("schema_version") == "usdb-image-policy-decision:v1",
                    "Invalid policy decision")
            print(f"Image vulnerability gate: {decision['result']}; "
                  f"accepted={decision['accepted_count']}; unresolved={decision['blocking_count']}")
            for finding in decision["blocking"]:
                print(f"{finding['advisory_id']} {finding['package']}@{finding['version']}: "
                      + "; ".join(finding["reasons"]))
            return int(decision["result"] != "pass")
        policy = read_json(args.exceptions) if args.exceptions else {
            "schema_version": POLICY_SCHEMA, "profiles": {}, "exceptions": []}
        report = read_json(args.report)
        require_rust_coverage(report, args.required_rust_binary)
        decision = evaluate(report, policy, image_reference=args.image_reference,
                            source_revision=args.source_revision, source_ref=args.source_ref,
                            enforcement=args.enforcement, root=args.repository_root)
        decision["report_sha256"] = sha256(args.report)
        decision["exceptions_sha256"] = sha256(args.exceptions) if args.exceptions else None
        args.output.write_text(json.dumps(decision, indent=2, sort_keys=True) + "\n")
        print(f"Policy evaluated: accepted={decision['accepted_count']}; "
              f"unresolved={decision['blocking_count']}; result={decision['result']}")
        return 0
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        print(f"Image security policy error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
