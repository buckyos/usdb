#!/usr/bin/env python3
"""Build deterministic cross-repository USDB release change records."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any


FRAGMENT_SCHEMA_VERSION = "usdb-change-fragment:v1"
RELEASE_CHANGES_SCHEMA_VERSION = "usdb-release-changes:v1"
RELEASE_ID_RE = re.compile(
    r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$"
)
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
CHANGE_ID_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
FRAGMENT_PATH_PREFIX = ".release-notes/fragments/"
CHANGE_TYPES = {
    "added",
    "changed",
    "deprecated",
    "fixed",
    "internal",
    "removed",
    "security",
}
CHANGE_TYPE_TITLES = {
    "security": "Security",
    "removed": "Removed",
    "deprecated": "Deprecated",
    "added": "Added",
    "changed": "Changed",
    "fixed": "Fixed",
    "internal": "Internal",
}
CHANGE_TYPE_ORDER = tuple(CHANGE_TYPE_TITLES)
SCOPES = {
    "balance-history",
    "bitcoin",
    "consensus",
    "deployment",
    "documentation",
    "indexer",
    "node-runtime",
    "release",
    "security",
    "sourcedao",
    "testing",
}
COMPATIBILITY_KEYS = {
    "config_change",
    "data_rebuild",
    "network_reset",
    "restart_required",
}
REPOSITORY_KEYS = {"go_ethereum", "source_dao", "usdb"}


@dataclass(frozen=True)
class RepositorySpec:
    key: str
    slug: str
    path: Path
    previous_revision: str | None
    current_revision: str


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path: Path) -> dict[str, Any]:
    return load_json_bytes(path.read_bytes(), str(path))


def load_json_bytes(data: bytes, source: str) -> dict[str, Any]:
    try:
        value = json.loads(
            data.decode("utf-8"), object_pairs_hook=_object_without_duplicates
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid JSON document {source}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"JSON document must be an object: {source}")
    return value


def canonical_json(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def release_lineage(release_id: str) -> tuple[str, int]:
    require(RELEASE_ID_RE.fullmatch(release_id) is not None, "invalid release ID")
    bundle_id, sequence = release_id.rsplit("-r", 1)
    return bundle_id, int(sequence)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def require_exact_keys(value: dict[str, Any], expected: set[str], context: str) -> None:
    actual = set(value)
    require(
        actual == expected,
        f"{context} keys mismatch: missing={sorted(expected - actual)}, "
        f"unknown={sorted(actual - expected)}",
    )


def _require_text(value: Any, context: str, *, max_length: int = 1000) -> str:
    require(isinstance(value, str), f"{context} must be a string")
    normalized = value.strip()
    require(normalized == value and normalized != "", f"{context} must be trimmed and non-empty")
    require("\n" not in value and "\r" not in value, f"{context} must be one line")
    require(len(value) <= max_length, f"{context} exceeds {max_length} characters")
    return value


def _require_text_array(
    value: Any,
    context: str,
    *,
    allow_empty: bool,
    max_items: int = 12,
    max_length: int = 1000,
) -> list[str]:
    require(isinstance(value, list), f"{context} must be an array")
    if not allow_empty:
        require(len(value) > 0, f"{context} must not be empty")
    require(len(value) <= max_items, f"{context} exceeds {max_items} items")
    result = [
        _require_text(item, f"{context}[{index}]", max_length=max_length)
        for index, item in enumerate(value)
    ]
    require(len(result) == len(set(result)), f"{context} must not contain duplicates")
    return result


def validate_fragment(value: dict[str, Any], source: str) -> dict[str, Any]:
    require_exact_keys(
        value,
        {
            "change_id",
            "compatibility",
            "details",
            "operator_actions",
            "references",
            "schema_version",
            "scopes",
            "summary",
            "type",
        },
        source,
    )
    require(
        value["schema_version"] == FRAGMENT_SCHEMA_VERSION,
        f"{source}.schema_version must be {FRAGMENT_SCHEMA_VERSION}",
    )
    change_id = _require_text(value["change_id"], f"{source}.change_id", max_length=96)
    require(
        CHANGE_ID_RE.fullmatch(change_id) is not None,
        f"{source}.change_id must be lowercase kebab-case",
    )
    change_type = _require_text(value["type"], f"{source}.type", max_length=24)
    require(change_type in CHANGE_TYPES, f"{source}.type is unsupported")
    scopes = _require_text_array(
        value["scopes"], f"{source}.scopes", allow_empty=False, max_items=8, max_length=40
    )
    require(set(scopes) <= SCOPES, f"{source}.scopes contains an unsupported scope")
    summary = _require_text(value["summary"], f"{source}.summary", max_length=160)
    details = _require_text_array(
        value["details"], f"{source}.details", allow_empty=False, max_items=12
    )
    operator_actions = _require_text_array(
        value["operator_actions"],
        f"{source}.operator_actions",
        allow_empty=True,
        max_items=8,
    )
    references = _require_text_array(
        value["references"],
        f"{source}.references",
        allow_empty=True,
        max_items=12,
        max_length=500,
    )
    for index, reference in enumerate(references):
        require(
            reference.startswith("https://"),
            f"{source}.references[{index}] must be an HTTPS URL",
        )
    compatibility = value["compatibility"]
    require(isinstance(compatibility, dict), f"{source}.compatibility must be an object")
    require_exact_keys(compatibility, COMPATIBILITY_KEYS, f"{source}.compatibility")
    for key, enabled in compatibility.items():
        require(type(enabled) is bool, f"{source}.compatibility.{key} must be boolean")
    require(
        not (compatibility["network_reset"] and compatibility["data_rebuild"]),
        f"{source}.compatibility.data_rebuild is redundant when network_reset is true",
    )
    return {
        "schema_version": FRAGMENT_SCHEMA_VERSION,
        "change_id": change_id,
        "type": change_type,
        "scopes": sorted(scopes),
        "summary": summary,
        "details": details,
        "operator_actions": operator_actions,
        "compatibility": {key: compatibility[key] for key in sorted(COMPATIBILITY_KEYS)},
        "references": sorted(references),
    }


def validate_fragment_directory(repository_root: Path) -> list[dict[str, Any]]:
    fragment_dir = repository_root / FRAGMENT_PATH_PREFIX
    if not fragment_dir.exists():
        return []
    require(fragment_dir.is_dir(), f"fragment path is not a directory: {fragment_dir}")
    fragments: list[dict[str, Any]] = []
    seen: set[str] = set()
    for path in sorted(fragment_dir.iterdir()):
        require(path.is_file() and not path.is_symlink(), f"fragment must be a regular file: {path}")
        require(path.suffix == ".json", f"fragment must use .json: {path}")
        value = validate_fragment(load_json(path), str(path))
        require(path.stem == value["change_id"], f"fragment file name must equal change_id: {path}")
        require(value["change_id"] not in seen, f"duplicate change_id: {value['change_id']}")
        seen.add(value["change_id"])
        fragments.append(value)
    return fragments


def _run_git(repository: Path, *arguments: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repository), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"git {' '.join(arguments)} failed in {repository}: {detail}")
    return result.stdout


def _validate_repository_spec(spec: RepositorySpec) -> None:
    require(spec.key in REPOSITORY_KEYS, f"unsupported repository key: {spec.key}")
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", spec.slug) is not None, f"invalid repository slug: {spec.slug}")
    require(spec.path.is_dir(), f"repository path does not exist: {spec.path}")
    require(REVISION_RE.fullmatch(spec.current_revision) is not None, f"invalid current revision for {spec.key}")
    if spec.previous_revision is not None:
        require(REVISION_RE.fullmatch(spec.previous_revision) is not None, f"invalid previous revision for {spec.key}")
    for revision in (spec.previous_revision, spec.current_revision):
        if revision is not None:
            _run_git(spec.path, "cat-file", "-e", f"{revision}^{{commit}}")
    if spec.previous_revision is not None:
        _run_git(
            spec.path,
            "merge-base",
            "--is-ancestor",
            spec.previous_revision,
            spec.current_revision,
        )


def _fragment_paths_for_range(spec: RepositorySpec) -> list[str]:
    if spec.previous_revision is None:
        output = _run_git(
            spec.path,
            "ls-tree",
            "-r",
            "--name-only",
            spec.current_revision,
            "--",
            FRAGMENT_PATH_PREFIX,
        )
        return sorted(path for path in output.splitlines() if path.endswith(".json"))
    output = _run_git(
        spec.path,
        "diff",
        "--name-status",
        "--no-renames",
        spec.previous_revision,
        spec.current_revision,
        "--",
        FRAGMENT_PATH_PREFIX,
    )
    paths: list[str] = []
    for line in output.splitlines():
        status, path = line.split("\t", 1)
        if not path.endswith(".json"):
            continue
        require(
            status == "A",
            f"release fragments are append-only; {spec.key}:{path} has status {status}",
        )
        paths.append(path)
    return sorted(paths)


def _load_range_fragments(spec: RepositorySpec) -> list[dict[str, Any]]:
    fragments: list[dict[str, Any]] = []
    for path in _fragment_paths_for_range(spec):
        content = _run_git(spec.path, "show", f"{spec.current_revision}:{path}").encode("utf-8")
        fragment = validate_fragment(
            load_json_bytes(content, f"{spec.key}:{path}"), f"{spec.key}:{path}"
        )
        require(
            Path(path).stem == fragment["change_id"],
            f"fragment file name must equal change_id: {spec.key}:{path}",
        )
        fragment["source_repository"] = spec.key
        fragment["source_path"] = path
        fragments.append(fragment)
    return fragments


def _commit_records(spec: RepositorySpec, known_change_ids: set[str]) -> list[dict[str, Any]]:
    revision_range = spec.current_revision
    if spec.previous_revision is not None:
        revision_range = f"{spec.previous_revision}..{spec.current_revision}"
    output = _run_git(
        spec.path,
        "log",
        "--reverse",
        "--format=%H%x1f%s%x1f%b%x1e",
        revision_range,
    )
    records: list[dict[str, Any]] = []
    trailer_re = re.compile(r"^Release-Note:\s*([^\s]+)\s*$", re.MULTILINE)
    for raw_record in output.split("\x1e"):
        raw_record = raw_record.strip("\n")
        if not raw_record:
            continue
        fields = raw_record.split("\x1f", 2)
        require(len(fields) == 3, f"could not parse commit metadata for {spec.key}")
        revision, subject, body = fields
        trailers = trailer_re.findall(body)
        require(len(trailers) <= 1, f"commit {revision} has multiple Release-Note trailers")
        release_note = trailers[0] if trailers else None
        if release_note == "none":
            classification = "exempt"
        elif release_note in known_change_ids:
            classification = "classified"
        else:
            classification = "unclassified"
        records.append(
            {
                "revision": revision,
                "subject": subject,
                "release_note": release_note,
                "classification": classification,
            }
        )
    return records


def _nested(value: dict[str, Any], path: str) -> Any:
    current: Any = value
    for part in path.split("."):
        if not isinstance(current, dict) or part not in current:
            return None
        current = current[part]
    return current


MANIFEST_DIFF_FIELDS = {
    "network_bundle.bundle_id": "network_reset",
    "network_bundle.network_json_sha256": "network_reset",
    "network_bundle.chain_id": "network_reset",
    "network_bundle.network_id": "network_reset",
    "network_bundle.genesis_sha256": "network_reset",
    "network_bundle.genesis_block_hash": "network_reset",
    "network_bundle.btc_network_id": "network_reset",
    "network_bundle.btc_index_origin_height": "network_reset",
    "network_bundle.btc_activation_registry_id": "network_reset",
    "network_bundle.snapshot_trusted_keys_sha256": "config_change",
    "runtime_compatibility.compatibility_id": "data_rebuild",
    "runtime_compatibility.data_layout_version": "data_rebuild",
    "images.usdb_services.reference": "restart_required",
    "images.usdb_chain.reference": "restart_required",
    "images.bitcoin_core.reference": "restart_required",
    "snapshot.status": "optional_snapshot",
    "snapshot.snapshot_release_id": "optional_snapshot",
    "snapshot.height": "optional_snapshot",
    "snapshot.record.sha256": "optional_snapshot",
    "qualification.level": "qualification",
    "compatibility_lock.sha256": "source",
    "repositories.usdb.revision": "source",
    "repositories.go_ethereum.revision": "source",
    "repositories.source_dao.revision": "source",
}


def _manifest_changes(
    previous_manifest: dict[str, Any] | None, current_manifest: dict[str, Any]
) -> list[dict[str, Any]]:
    if previous_manifest is None:
        return []
    changes: list[dict[str, Any]] = []
    for path, impact in MANIFEST_DIFF_FIELDS.items():
        previous = _nested(previous_manifest, path)
        current = _nested(current_manifest, path)
        if previous != current:
            changes.append(
                {"path": path, "impact": impact, "previous": previous, "current": current}
            )
    return changes


def _compatibility_summary(
    fragments: list[dict[str, Any]], manifest_changes: list[dict[str, Any]]
) -> dict[str, Any]:
    flags = {key: False for key in COMPATIBILITY_KEYS}
    for fragment in fragments:
        for key in COMPATIBILITY_KEYS:
            flags[key] = flags[key] or fragment["compatibility"][key]
    for change in manifest_changes:
        impact = change["impact"]
        if impact in flags:
            flags[impact] = True
    if flags["network_reset"]:
        flags["data_rebuild"] = False
    if flags["network_reset"]:
        classification = "network_reset"
    elif flags["data_rebuild"]:
        classification = "data_rebuild"
    elif flags["config_change"]:
        classification = "config_change"
    elif flags["restart_required"]:
        classification = "restart_required"
    else:
        classification = "in_place"
    actions = sorted(
        {
            action
            for fragment in fragments
            for action in fragment["operator_actions"]
        }
    )
    return {
        "classification": classification,
        "flags": {key: flags[key] for key in sorted(flags)},
        "operator_actions": actions,
        "manifest_changes": manifest_changes,
    }


def build_release_changes(
    *,
    release_id: str,
    manifest_path: Path,
    previous_manifest_path: Path | None,
    repositories: list[RepositorySpec],
) -> dict[str, Any]:
    current_bundle_id, current_sequence = release_lineage(release_id)
    require({spec.key for spec in repositories} == REPOSITORY_KEYS, "all three repositories are required exactly once")
    require(len(repositories) == len(REPOSITORY_KEYS), "repository keys must be unique")
    for spec in repositories:
        _validate_repository_spec(spec)

    manifest = load_json(manifest_path)
    require(manifest.get("release_id") == release_id, "manifest release ID mismatch")
    manifest_repositories = manifest.get("repositories")
    require(isinstance(manifest_repositories, dict), "manifest repositories must be an object")
    for spec in repositories:
        entry = manifest_repositories.get(spec.key)
        require(isinstance(entry, dict), f"manifest repository missing: {spec.key}")
        require(entry.get("repository") == spec.slug, f"repository slug mismatch for {spec.key}")
        require(entry.get("revision") == spec.current_revision, f"current revision mismatch for {spec.key}")

    previous_manifest = None
    previous_release: dict[str, Any] | None = None
    if previous_manifest_path is not None:
        previous_manifest = load_json(previous_manifest_path)
        previous_release_id = previous_manifest.get("release_id")
        require(
            isinstance(previous_release_id, str)
            and RELEASE_ID_RE.fullmatch(previous_release_id) is not None,
            "previous manifest has an invalid release ID",
        )
        previous_bundle_id, previous_sequence = release_lineage(previous_release_id)
        require(previous_bundle_id == current_bundle_id, "previous release belongs to another network bundle")
        require(previous_sequence < current_sequence, "previous release sequence must be lower than current")
        for spec in repositories:
            previous_entry = _nested(previous_manifest, f"repositories.{spec.key}")
            require(isinstance(previous_entry, dict), f"previous manifest repository missing: {spec.key}")
            require(previous_entry.get("repository") == spec.slug, f"previous repository slug mismatch for {spec.key}")
            require(previous_entry.get("revision") == spec.previous_revision, f"previous revision mismatch for {spec.key}")
        previous_release = {
            "release_id": previous_release_id,
            "manifest_sha256": sha256_file(previous_manifest_path),
        }
    else:
        require(
            all(spec.previous_revision is None for spec in repositories),
            "previous revisions require a previous manifest",
        )

    fragments: list[dict[str, Any]] = []
    for spec in sorted(repositories, key=lambda item: item.key):
        fragments.extend(_load_range_fragments(spec))
    fragments.sort(key=lambda item: (CHANGE_TYPE_ORDER.index(item["type"]), item["change_id"]))
    change_ids = [fragment["change_id"] for fragment in fragments]
    require(len(change_ids) == len(set(change_ids)), "change_id must be unique across repositories")
    known_change_ids = set(change_ids)

    repository_records: dict[str, Any] = {}
    for spec in sorted(repositories, key=lambda item: item.key):
        commits = _commit_records(spec, known_change_ids)
        counts = {
            status: sum(1 for commit in commits if commit["classification"] == status)
            for status in ("classified", "exempt", "unclassified")
        }
        compare_url = (
            f"https://github.com/{spec.slug}/compare/"
            f"{spec.previous_revision}...{spec.current_revision}"
            if spec.previous_revision is not None
            else f"https://github.com/{spec.slug}/commit/{spec.current_revision}"
        )
        repository_records[spec.key] = {
            "repository": spec.slug,
            "previous_revision": spec.previous_revision,
            "current_revision": spec.current_revision,
            "compare_url": compare_url,
            "commit_count": len(commits),
            "coverage": counts,
            "commits": commits,
        }

    manifest_changes = _manifest_changes(previous_manifest, manifest)
    return {
        "schema_version": RELEASE_CHANGES_SCHEMA_VERSION,
        "release_id": release_id,
        "previous_release": previous_release,
        "manifest_sha256": sha256_file(manifest_path),
        "coverage_enforced": False,
        "compatibility": _compatibility_summary(fragments, manifest_changes),
        "changes": fragments,
        "repositories": repository_records,
    }


def _display_value(value: Any) -> str:
    rendered = json.dumps(value, sort_keys=True, separators=(",", ":"))
    if len(rendered) > 120:
        return rendered[:117] + "..."
    return rendered


def render_markdown(release_changes: dict[str, Any]) -> str:
    previous = release_changes["previous_release"]
    previous_label = previous["release_id"] if previous is not None else "initial release"
    compatibility = release_changes["compatibility"]
    lines = [
        f"## Changes Since {previous_label}",
        "",
        f"Upgrade classification: `{compatibility['classification']}`.",
        "",
    ]
    actions = compatibility["operator_actions"]
    lines.extend(["### Operator Actions", ""])
    if actions:
        lines.extend(f"- {action}" for action in actions)
    else:
        lines.append("- No release-specific operator action is declared.")
    lines.append("")

    changes = release_changes["changes"]
    if not changes:
        lines.extend(
            [
                "### Structured Changes",
                "",
                "No structured change fragments were added in these revision ranges.",
                "",
            ]
        )
    for change_type in CHANGE_TYPE_ORDER:
        typed = [change for change in changes if change["type"] == change_type]
        if not typed:
            continue
        lines.extend([f"### {CHANGE_TYPE_TITLES[change_type]}", ""])
        for change in typed:
            lines.append(f"#### {change['summary']}")
            lines.append("")
            lines.extend(f"- {detail}" for detail in change["details"])
            lines.append(f"- Scopes: `{', '.join(change['scopes'])}`")
            lines.append(
                f"- Source: `{change['source_repository']}:{change['source_path']}` "
                f"(`{change['change_id']}`)"
            )
            for reference in change["references"]:
                lines.append(f"- Reference: {reference}")
            lines.append("")

    lines.extend(["### Compatibility Evidence", ""])
    manifest_changes = compatibility["manifest_changes"]
    if manifest_changes:
        for change in manifest_changes:
            lines.append(
                f"- `{change['path']}` ({change['impact']}): "
                f"`{_display_value(change['previous'])}` -> "
                f"`{_display_value(change['current'])}`"
            )
    elif previous is None:
        lines.append("- No prior published manifest is available for comparison.")
    else:
        lines.append("- No tracked manifest identity or compatibility field changed.")
    lines.append("")

    lines.extend(["### Source Ranges", ""])
    for key, repository in release_changes["repositories"].items():
        coverage = repository["coverage"]
        lines.append(
            f"- [{repository['repository']}]({repository['compare_url']}): "
            f"{repository['commit_count']} commits; "
            f"{coverage['classified']} classified, {coverage['exempt']} exempt, "
            f"{coverage['unclassified']} unclassified."
        )
    lines.extend(
        [
            "",
            "> Commit coverage is report-only in schema v1. Review every unclassified commit; a later schema may enforce complete classification.",
            "",
            "<details>",
            "<summary>Commit inventory</summary>",
            "",
        ]
    )
    for repository in release_changes["repositories"].values():
        lines.append(f"#### {repository['repository']}")
        lines.append("")
        if repository["commits"]:
            for commit in repository["commits"]:
                note = commit["release_note"] or "missing"
                lines.append(
                    f"- `{commit['revision'][:12]}` {commit['subject']} "
                    f"(`[Release-Note: {note}]`, {commit['classification']})"
                )
        else:
            lines.append("- No commits in range.")
        lines.append("")
    lines.extend(["</details>", ""])
    return "\n".join(lines)


def validate_release_changes(
    release_changes: dict[str, Any], manifest_path: Path, expected_release_id: str
) -> None:
    require_exact_keys(
        release_changes,
        {
            "changes",
            "compatibility",
            "coverage_enforced",
            "manifest_sha256",
            "previous_release",
            "release_id",
            "repositories",
            "schema_version",
        },
        "release changes",
    )
    require(release_changes["schema_version"] == RELEASE_CHANGES_SCHEMA_VERSION, "unsupported release changes schema")
    require(release_changes["release_id"] == expected_release_id, "release changes ID mismatch")
    require(RELEASE_ID_RE.fullmatch(expected_release_id) is not None, "invalid expected release ID")
    require(release_changes["coverage_enforced"] is False, "schema v1 coverage must remain report-only")
    require(release_changes["manifest_sha256"] == sha256_file(manifest_path), "release changes manifest checksum mismatch")
    manifest = load_json(manifest_path)
    require(manifest.get("release_id") == expected_release_id, "manifest release ID mismatch")
    previous_release = release_changes["previous_release"]
    if previous_release is not None:
        require(isinstance(previous_release, dict), "previous release must be null or an object")
        require_exact_keys(previous_release, {"release_id", "manifest_sha256"}, "previous release")
        require(
            isinstance(previous_release["release_id"], str)
            and RELEASE_ID_RE.fullmatch(previous_release["release_id"]) is not None,
            "previous release ID is invalid",
        )
        require(
            isinstance(previous_release["manifest_sha256"], str)
            and re.fullmatch(r"[0-9a-f]{64}", previous_release["manifest_sha256"]) is not None,
            "previous release manifest checksum is invalid",
        )
        previous_bundle_id, previous_sequence = release_lineage(previous_release["release_id"])
        current_bundle_id, current_sequence = release_lineage(expected_release_id)
        require(previous_bundle_id == current_bundle_id, "previous release belongs to another network bundle")
        require(previous_sequence < current_sequence, "previous release sequence must be lower than current")
    repositories = release_changes["repositories"]
    require(isinstance(repositories, dict), "release changes repositories must be an object")
    require_exact_keys(repositories, REPOSITORY_KEYS, "release changes repositories")
    for key, repository in repositories.items():
        require(isinstance(repository, dict), f"release changes repository {key} must be an object")
        require_exact_keys(
            repository,
            {
                "commit_count",
                "commits",
                "compare_url",
                "coverage",
                "current_revision",
                "previous_revision",
                "repository",
            },
            f"release changes repository {key}",
        )
        require(repository.get("repository") == _nested(manifest, f"repositories.{key}.repository"), f"release changes repository slug mismatch: {key}")
        require(repository.get("current_revision") == _nested(manifest, f"repositories.{key}.revision"), f"release changes current revision mismatch: {key}")
        previous_revision = repository["previous_revision"]
        require(
            previous_revision is None
            or (
                isinstance(previous_revision, str)
                and REVISION_RE.fullmatch(previous_revision) is not None
            ),
            f"release changes previous revision is invalid: {key}",
        )
        require(
            (previous_release is None) == (previous_revision is None),
            f"release changes previous revision presence mismatch: {key}",
        )
        current_revision = repository["current_revision"]
        require(
            isinstance(current_revision, str)
            and REVISION_RE.fullmatch(current_revision) is not None,
            f"release changes current revision is invalid: {key}",
        )
        expected_compare_url = (
            f"https://github.com/{repository['repository']}/compare/"
            f"{previous_revision}...{current_revision}"
            if previous_revision is not None
            else f"https://github.com/{repository['repository']}/commit/{current_revision}"
        )
        require(repository["compare_url"] == expected_compare_url, f"release changes compare URL mismatch: {key}")
        commits = repository["commits"]
        require(isinstance(commits, list), f"release changes commits must be an array: {key}")
        require(repository["commit_count"] == len(commits), f"release changes commit count mismatch: {key}")
        actual_coverage = {status: 0 for status in ("classified", "exempt", "unclassified")}
        for index, commit in enumerate(commits):
            context = f"release changes repository {key} commits[{index}]"
            require(isinstance(commit, dict), f"{context} must be an object")
            require_exact_keys(commit, {"classification", "release_note", "revision", "subject"}, context)
            require(
                isinstance(commit["revision"], str)
                and REVISION_RE.fullmatch(commit["revision"]) is not None,
                f"{context}.revision is invalid",
            )
            _require_text(commit["subject"], f"{context}.subject", max_length=1000)
            require(
                commit["release_note"] is None or isinstance(commit["release_note"], str),
                f"{context}.release_note must be null or a string",
            )
            require(commit["classification"] in actual_coverage, f"{context}.classification is invalid")
            actual_coverage[commit["classification"]] += 1
        coverage = repository["coverage"]
        require(isinstance(coverage, dict), f"release changes coverage must be an object: {key}")
        require_exact_keys(coverage, set(actual_coverage), f"release changes coverage {key}")
        require(coverage == actual_coverage, f"release changes coverage mismatch: {key}")
    changes = release_changes["changes"]
    require(isinstance(changes, list), "release changes changes must be an array")
    seen: set[str] = set()
    for index, change in enumerate(changes):
        require(isinstance(change, dict), f"release changes changes[{index}] must be an object")
        source_repository = change.get("source_repository")
        source_path = change.get("source_path")
        require_exact_keys(
            change,
            {
                "change_id",
                "compatibility",
                "details",
                "operator_actions",
                "references",
                "schema_version",
                "scopes",
                "source_path",
                "source_repository",
                "summary",
                "type",
            },
            f"release changes changes[{index}]",
        )
        base = {key: value for key, value in change.items() if key not in {"source_repository", "source_path"}}
        validated = validate_fragment(base, f"release changes changes[{index}]")
        require(base == validated, f"release changes fragment is not normalized at index {index}")
        require(source_repository in REPOSITORY_KEYS, f"invalid change source repository at index {index}")
        require(isinstance(source_path, str) and source_path.startswith(FRAGMENT_PATH_PREFIX), f"invalid change source path at index {index}")
        require(Path(source_path).stem == validated["change_id"], f"change source path mismatch at index {index}")
        require(validated["change_id"] not in seen, "duplicate change_id in release changes")
        seen.add(validated["change_id"])
    compatibility = release_changes["compatibility"]
    require(isinstance(compatibility, dict), "release changes compatibility must be an object")
    require_exact_keys(compatibility, {"classification", "flags", "manifest_changes", "operator_actions"}, "release changes compatibility")
    require(compatibility["classification"] in {"in_place", "restart_required", "config_change", "data_rebuild", "network_reset"}, "invalid release compatibility classification")
    flags = compatibility["flags"]
    require(isinstance(flags, dict), "release changes compatibility flags must be an object")
    require_exact_keys(flags, COMPATIBILITY_KEYS, "release changes compatibility flags")
    require(all(type(value) is bool for value in flags.values()), "release changes compatibility flags must be boolean")
    expected_flags = {key: False for key in COMPATIBILITY_KEYS}
    for change in changes:
        for key in COMPATIBILITY_KEYS:
            expected_flags[key] = expected_flags[key] or change["compatibility"][key]
    manifest_changes = compatibility["manifest_changes"]
    require(isinstance(manifest_changes, list), "release changes manifest changes must be an array")
    seen_paths: set[str] = set()
    for index, change in enumerate(manifest_changes):
        context = f"release changes manifest changes[{index}]"
        require(isinstance(change, dict), f"{context} must be an object")
        require_exact_keys(change, {"current", "impact", "path", "previous"}, context)
        path = change["path"]
        require(path in MANIFEST_DIFF_FIELDS, f"{context}.path is unsupported")
        require(path not in seen_paths, f"duplicate release changes manifest path: {path}")
        seen_paths.add(path)
        require(change["impact"] == MANIFEST_DIFF_FIELDS[path], f"{context}.impact mismatch")
        if change["impact"] in expected_flags:
            expected_flags[change["impact"]] = True
    if expected_flags["network_reset"]:
        expected_flags["data_rebuild"] = False
    require(flags == expected_flags, "release changes compatibility flags mismatch")
    expected_classification = "in_place"
    for flag in ("restart_required", "config_change", "data_rebuild", "network_reset"):
        if flags[flag]:
            expected_classification = flag
    require(compatibility["classification"] == expected_classification, "release changes compatibility classification mismatch")
    operator_actions = compatibility["operator_actions"]
    require(isinstance(operator_actions, list), "release changes operator actions must be an array")
    expected_actions = sorted({action for change in changes for action in change["operator_actions"]})
    require(operator_actions == expected_actions, "release changes operator actions mismatch")


def _parse_repository_spec(values: list[str]) -> RepositorySpec:
    key, slug, path, previous, current = values
    return RepositorySpec(
        key=key,
        slug=slug,
        path=Path(path).resolve(),
        previous_revision=None if previous == "-" else previous,
        current_revision=current,
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate_fragments = subparsers.add_parser("validate-fragments")
    validate_fragments.add_argument("--repository-root", type=Path, required=True)

    generate = subparsers.add_parser("generate")
    generate.add_argument("--release-id", required=True)
    generate.add_argument("--manifest", type=Path, required=True)
    generate.add_argument("--previous-manifest", type=Path)
    generate.add_argument(
        "--repository",
        action="append",
        nargs=5,
        metavar=("KEY", "GITHUB_SLUG", "PATH", "PREVIOUS_REVISION_OR_DASH", "CURRENT_REVISION"),
        required=True,
    )
    generate.add_argument("--output-json", type=Path, required=True)
    generate.add_argument("--output-markdown", type=Path, required=True)

    validate = subparsers.add_parser("validate-release")
    validate.add_argument("--release-id", required=True)
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--changes", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "validate-fragments":
        validate_fragment_directory(args.repository_root.resolve())
        return 0
    if args.command == "generate":
        repositories = [_parse_repository_spec(values) for values in args.repository]
        release_changes = build_release_changes(
            release_id=args.release_id,
            manifest_path=args.manifest,
            previous_manifest_path=args.previous_manifest,
            repositories=repositories,
        )
        args.output_json.parent.mkdir(parents=True, exist_ok=True)
        args.output_markdown.parent.mkdir(parents=True, exist_ok=True)
        args.output_json.write_bytes(canonical_json(release_changes))
        args.output_markdown.write_text(render_markdown(release_changes), encoding="utf-8")
        return 0
    release_changes = load_json(args.changes)
    require(
        args.changes.read_bytes() == canonical_json(release_changes),
        "release changes JSON must use canonical encoding",
    )
    validate_release_changes(release_changes, args.manifest, args.release_id)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValueError as exc:
        raise SystemExit(f"release notes validation error: {exc}") from exc
