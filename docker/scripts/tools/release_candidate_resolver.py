#!/usr/bin/env python3
"""Resolve a release candidate from a compatibility lock and successful CI runs."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import Any


SCHEMA_VERSION = "usdb-ci-revisions:v2"
GIT_REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
RELEASE_WORKFLOW_PATH = ".github/workflows/usdb-release-build.yml"
EXPECTED_COORDINATOR = {
    "repository": "buckyos/go-ethereum",
    "directory": "go-ethereum",
}
EXPECTED_DEPENDENCIES = {
    "usdb": {"repository": "buckyos/usdb", "directory": "usdb"},
    "source_dao": {
        "repository": "buckyos/SourceDAO",
        "directory": "SourceDAO",
    },
}


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(
        path.read_text(encoding="utf-8"),
        object_pairs_hook=_object_without_duplicates,
    )
    if not isinstance(value, dict):
        raise ValueError(f"JSON document must be an object: {path}")
    return value


def require_revision(value: Any, context: str) -> str:
    if not isinstance(value, str) or GIT_REVISION_RE.fullmatch(value) is None:
        raise ValueError(f"{context} must be a full lowercase Git SHA")
    return value


def require_release_id(value: Any) -> str:
    if not isinstance(value, str) or RELEASE_ID_RE.fullmatch(value) is None:
        raise ValueError("release ID must use usdb-{testnet|mainnet}-vN-rN")
    return value


def workflow_path_matches(value: Any, expected: str) -> bool:
    return isinstance(value, str) and value.split("@", 1)[0] == expected


def resolve_locked_revisions(
    lock: dict[str, Any], usdb_revision: str, go_ethereum_revision: str
) -> str:
    require_revision(usdb_revision, "usdb revision")
    require_revision(go_ethereum_revision, "go-ethereum revision")
    if lock.get("schema_version") != SCHEMA_VERSION:
        raise ValueError("unsupported compatibility lock schema")
    expected_top_level = {"schema_version", "coordinator", "dependencies", "toolchains"}
    if set(lock) != expected_top_level:
        raise ValueError("compatibility lock top-level keys mismatch")
    if lock.get("coordinator") != EXPECTED_COORDINATOR:
        raise ValueError("compatibility lock coordinator identity mismatch")
    dependencies = lock.get("dependencies")
    if not isinstance(dependencies, dict) or set(dependencies) != set(
        EXPECTED_DEPENDENCIES
    ):
        raise ValueError("compatibility lock dependencies mismatch")

    for key, identity in EXPECTED_DEPENDENCIES.items():
        entry = dependencies.get(key)
        if not isinstance(entry, dict):
            raise ValueError(f"compatibility lock dependency mismatch for {key}")
        if set(entry) != {"repository", "directory", "revision"}:
            raise ValueError(f"compatibility lock dependency keys mismatch for {key}")
        if entry.get("repository") != identity["repository"]:
            raise ValueError(f"compatibility lock repository mismatch for {key}")
        if entry.get("directory") != identity["directory"]:
            raise ValueError(f"compatibility lock directory mismatch for {key}")

    locked_usdb_revision = require_revision(
        dependencies["usdb"].get("revision"),
        "compatibility lock usdb revision",
    )
    if locked_usdb_revision != usdb_revision:
        raise ValueError(
            "release coordinator revision does not match the go-ethereum compatibility lock"
        )
    return require_revision(
        dependencies["source_dao"].get("revision"),
        "compatibility lock SourceDAO revision",
    )


def select_successful_run(
    payload: dict[str, Any],
    *,
    revision: str,
    release_id: str,
    expected_referenced_workflows: set[str],
    repository: str,
) -> dict[str, int]:
    require_revision(revision, f"{repository} revision")
    require_release_id(release_id)
    workflow_runs = payload.get("workflow_runs")
    if not isinstance(workflow_runs, list):
        raise ValueError(f"{repository} workflow API response is missing workflow_runs")

    candidates: list[dict[str, Any]] = []
    for run in workflow_runs:
        if not isinstance(run, dict):
            continue
        if (
            not workflow_path_matches(run.get("path"), RELEASE_WORKFLOW_PATH)
            or run.get("head_sha") != revision
            or run.get("display_title") != f"USDB release {release_id}"
            or run.get("event") != "push"
            or run.get("status") != "completed"
            or run.get("conclusion") != "success"
        ):
            continue
        referenced = run.get("referenced_workflows")
        if not isinstance(referenced, list):
            continue
        referenced_paths = {
            entry.get("path")
            for entry in referenced
            if isinstance(entry, dict) and isinstance(entry.get("path"), str)
        }
        if not expected_referenced_workflows.issubset(referenced_paths):
            continue
        candidates.append(run)

    if len(candidates) != 1:
        ids = sorted(
            run.get("id") for run in candidates if isinstance(run.get("id"), int)
        )
        raise ValueError(
            f"expected exactly one successful {repository} release-tag build "
            f"for {release_id}@{revision}, found {len(candidates)}: {ids}"
        )

    run_id = candidates[0].get("id")
    run_attempt = candidates[0].get("run_attempt")
    if not isinstance(run_id, int) or run_id <= 0:
        raise ValueError(f"{repository} workflow run ID must be a positive integer")
    if not isinstance(run_attempt, int) or run_attempt <= 0:
        raise ValueError(f"{repository} workflow run attempt must be a positive integer")
    return {"id": run_id, "attempt": run_attempt}


def resolve_candidate(
    *,
    compatibility_lock: dict[str, Any],
    usdb_runs: dict[str, Any],
    go_runs: dict[str, Any],
    usdb_revision: str,
    go_ethereum_revision: str,
    release_id: str,
) -> dict[str, str]:
    require_release_id(release_id)
    source_dao_revision = resolve_locked_revisions(
        compatibility_lock, usdb_revision, go_ethereum_revision
    )
    usdb_run = select_successful_run(
        usdb_runs,
        revision=usdb_revision,
        release_id=release_id,
        repository="buckyos/usdb",
        expected_referenced_workflows={
            f"buckyos/usdb/.github/workflows/usdb-fast.yml@{usdb_revision}",
            f"buckyos/usdb/.github/workflows/usdb-services-image.yml@{usdb_revision}",
            f"buckyos/usdb/.github/workflows/usdb-bitcoin-image.yml@{usdb_revision}",
        },
    )
    go_run = select_successful_run(
        go_runs,
        revision=go_ethereum_revision,
        release_id=release_id,
        repository="buckyos/go-ethereum",
        expected_referenced_workflows={
            "buckyos/go-ethereum/.github/workflows/"
            f"usdb-fast.yml@{go_ethereum_revision}",
            "buckyos/go-ethereum/.github/workflows/"
            f"usdb-chain-image.yml@{go_ethereum_revision}",
        },
    )
    return {
        "source_dao_revision": source_dao_revision,
        "usdb_run_id": str(usdb_run["id"]),
        "usdb_run_attempt": str(usdb_run["attempt"]),
        "go_run_id": str(go_run["id"]),
        "go_run_attempt": str(go_run["attempt"]),
        "services_tag": (
            "ghcr.io/buckyos/usdb-services:"
            f"git-{usdb_revision}-run-{usdb_run['id']}-{usdb_run['attempt']}"
        ),
        "bitcoin_tag": (
            "ghcr.io/buckyos/usdb-bitcoin-core:bitcoin-28.1-"
            f"git-{usdb_revision}-run-{usdb_run['id']}-{usdb_run['attempt']}"
        ),
        "chain_tag": (
            "ghcr.io/buckyos/usdb-chain:"
            f"git-{go_ethereum_revision}-run-{go_run['id']}-{go_run['attempt']}"
        ),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compatibility-lock", type=pathlib.Path, required=True)
    parser.add_argument("--usdb-runs", type=pathlib.Path, required=True)
    parser.add_argument("--go-runs", type=pathlib.Path, required=True)
    parser.add_argument("--usdb-revision", required=True)
    parser.add_argument("--go-ethereum-revision", required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--github-output", type=pathlib.Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        resolved = resolve_candidate(
            compatibility_lock=load_json(args.compatibility_lock),
            usdb_runs=load_json(args.usdb_runs),
            go_runs=load_json(args.go_runs),
            usdb_revision=args.usdb_revision,
            go_ethereum_revision=args.go_ethereum_revision,
            release_id=args.release_id,
        )
        if args.github_output is not None:
            with args.github_output.open("a", encoding="utf-8") as output:
                for key, value in resolved.items():
                    output.write(f"{key}={value}\n")
        print(json.dumps(resolved, sort_keys=True))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release candidate resolution error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
