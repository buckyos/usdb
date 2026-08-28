#!/usr/bin/env python3
"""Resolve one release candidate artifact and verify immutable GitHub releases."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
ARTIFACT_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
CANDIDATE_WORKFLOW_PATH = ".github/workflows/usdb-release-candidate.yml"


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(
        path.read_text(encoding="utf-8"),
        object_pairs_hook=_object_without_duplicates,
    )
    if not isinstance(value, dict):
        raise ValueError(f"JSON document must be an object: {path}")
    return value


def _require_release_id(value: str) -> None:
    if RELEASE_ID_RE.fullmatch(value) is None:
        raise ValueError("release ID must use usdb-{testnet|mainnet}-vN-rN")


def _require_revision(value: str) -> None:
    if REVISION_RE.fullmatch(value) is None:
        raise ValueError("USDB revision must be a full lowercase Git SHA")


def _workflow_path_matches(value: Any) -> bool:
    return (
        isinstance(value, str)
        and value.split("@", 1)[0] == CANDIDATE_WORKFLOW_PATH
    )


def resolve_candidate(
    artifacts_payload: dict[str, Any],
    runs_payload: dict[str, Any],
    *,
    release_id: str,
    usdb_revision: str,
) -> dict[str, str]:
    """Resolve exactly one successful, unexpired candidate artifact."""

    _require_release_id(release_id)
    _require_revision(usdb_revision)
    runs = runs_payload.get("workflow_runs")
    if not isinstance(runs, list):
        raise ValueError("candidate workflow response is missing workflow_runs")
    matching_runs = [
        run
        for run in runs
        if isinstance(run, dict)
        and _workflow_path_matches(run.get("path"))
        and run.get("display_title") == f"USDB candidate {release_id}"
        and run.get("event") == "workflow_dispatch"
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("head_sha") == usdb_revision
        and run.get("head_branch") == release_id
    ]
    if len(matching_runs) != 1:
        ids = sorted(
            run.get("id")
            for run in matching_runs
            if isinstance(run.get("id"), int)
        )
        raise ValueError(
            f"expected exactly one successful candidate run for "
            f"{release_id}@{usdb_revision}, found {len(matching_runs)}: {ids}"
        )
    run = matching_runs[0]
    run_id = run.get("id")
    run_attempt = run.get("run_attempt")
    if not isinstance(run_id, int) or run_id <= 0:
        raise ValueError("candidate workflow run ID must be a positive integer")
    if not isinstance(run_attempt, int) or run_attempt <= 0:
        raise ValueError("candidate workflow run attempt must be a positive integer")

    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        raise ValueError("candidate artifact response is missing artifacts")
    expected_name = f"{release_id}-manifest"
    matching_artifacts: list[dict[str, Any]] = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            continue
        workflow_run = artifact.get("workflow_run")
        if (
            artifact.get("name") != expected_name
            or artifact.get("expired") is not False
            or not isinstance(workflow_run, dict)
            or workflow_run.get("id") != run_id
            or workflow_run.get("head_sha") != usdb_revision
        ):
            continue
        matching_artifacts.append(artifact)
    if len(matching_artifacts) != 1:
        ids = sorted(
            artifact.get("id")
            for artifact in matching_artifacts
            if isinstance(artifact.get("id"), int)
        )
        raise ValueError(
            f"expected exactly one active {expected_name} artifact from run {run_id}, "
            f"found {len(matching_artifacts)}: {ids}"
        )
    artifact = matching_artifacts[0]
    artifact_id = artifact.get("id")
    artifact_digest = artifact.get("digest")
    if not isinstance(artifact_id, int) or artifact_id <= 0:
        raise ValueError("candidate artifact ID must be a positive integer")
    if (
        not isinstance(artifact_digest, str)
        or ARTIFACT_DIGEST_RE.fullmatch(artifact_digest) is None
    ):
        raise ValueError("candidate artifact must have a lowercase SHA-256 digest")
    return {
        "candidate_run_id": str(run_id),
        "candidate_run_attempt": str(run_attempt),
        "candidate_artifact_id": str(artifact_id),
        "candidate_artifact_digest": artifact_digest,
    }


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_existing_release(
    release: dict[str, Any],
    *,
    release_id: str,
    title: str,
    notes: str,
    assets: list[Path],
    prerelease: bool,
) -> str:
    """Require an existing GitHub Release to equal the intended publication."""

    _require_release_id(release_id)
    expected_assets = {path.name: f"sha256:{_sha256(path)}" for path in assets}
    if len(expected_assets) != len(assets):
        raise ValueError("release asset file names must be unique")
    if release.get("tag_name") != release_id:
        raise ValueError("existing release tag does not match the release ID")
    if release.get("name") != title:
        raise ValueError("existing release title does not match")
    body = release.get("body")
    if not isinstance(body, str) or body.rstrip("\n") != notes.rstrip("\n"):
        raise ValueError("existing release notes do not match")
    if release.get("draft") is not False:
        raise ValueError("existing release must be published, not draft")
    if release.get("prerelease") is not prerelease:
        raise ValueError("existing release prerelease flag does not match")

    release_assets = release.get("assets")
    if not isinstance(release_assets, list):
        raise ValueError("existing release is missing assets")
    actual_assets: dict[str, str] = {}
    for asset in release_assets:
        if not isinstance(asset, dict):
            raise ValueError("existing release asset must be an object")
        name = asset.get("name")
        digest = asset.get("digest")
        if not isinstance(name, str) or not isinstance(digest, str):
            raise ValueError("existing release asset must expose name and digest")
        if asset.get("state") != "uploaded":
            raise ValueError(f"existing release asset is not fully uploaded: {name}")
        if name in actual_assets:
            raise ValueError(f"duplicate existing release asset: {name}")
        actual_assets[name] = digest
    if actual_assets != expected_assets:
        raise ValueError(
            f"existing release assets do not match: "
            f"expected={sorted(expected_assets)}, actual={sorted(actual_assets)}"
        )
    url = release.get("html_url")
    if not isinstance(url, str) or not url.startswith("https://github.com/"):
        raise ValueError("existing release URL is invalid")
    return url


def _write_outputs(path: Path | None, values: dict[str, str]) -> None:
    if path is None:
        return
    with path.open("a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    resolve = subparsers.add_parser("resolve-candidate")
    resolve.add_argument("--artifacts", type=Path, required=True)
    resolve.add_argument("--runs", type=Path, required=True)
    resolve.add_argument("--release-id", required=True)
    resolve.add_argument("--usdb-revision", required=True)
    resolve.add_argument("--github-output", type=Path)

    verify = subparsers.add_parser("verify-release")
    verify.add_argument("--release", type=Path, required=True)
    verify.add_argument("--release-id", required=True)
    verify.add_argument("--title", required=True)
    verify.add_argument("--notes", type=Path, required=True)
    verify.add_argument("--asset", action="append", type=Path, required=True)
    verify.add_argument("--prerelease", action="store_true")
    verify.add_argument("--github-output", type=Path)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        if args.command == "resolve-candidate":
            resolved = resolve_candidate(
                load_json(args.artifacts),
                load_json(args.runs),
                release_id=args.release_id,
                usdb_revision=args.usdb_revision,
            )
        else:
            url = verify_existing_release(
                load_json(args.release),
                release_id=args.release_id,
                title=args.title,
                notes=args.notes.read_text(encoding="utf-8"),
                assets=args.asset,
                prerelease=args.prerelease,
            )
            resolved = {"release_url": url}
        _write_outputs(args.github_output, resolved)
        print(json.dumps(resolved, sort_keys=True))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release publish resolution error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
