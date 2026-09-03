#!/usr/bin/env python3
"""Synchronize shared project skills from USDB into sibling repositories."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


CANONICAL_REPOSITORY = "usdb"
TARGET_REPOSITORIES = ("go-ethereum", "SourceDAO")
SHARED_SKILLS = ("usdb-release-fragments",)
SKILL_NAME_RE = re.compile(r"^name:[ \t]*([^ \t\r\n]+)[ \t]*$", re.MULTILINE)


@dataclass(frozen=True)
class SkillDrift:
    repository: str
    skill: str
    path: Path
    reason: str


def default_workspace_root() -> Path:
    return Path(__file__).resolve().parents[4]


def skill_path(workspace_root: Path, repository: str, skill: str) -> Path:
    return workspace_root / repository / ".agents" / "skills" / skill / "SKILL.md"


def canonical_skill(workspace_root: Path, skill: str) -> tuple[Path, bytes]:
    path = skill_path(workspace_root, CANONICAL_REPOSITORY, skill)
    if not path.is_file():
        raise ValueError(f"canonical skill is missing: {path}")
    data = path.read_bytes()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError(f"canonical skill is not UTF-8: {path}") from exc
    match = SKILL_NAME_RE.search(text)
    if match is None or match.group(1) != skill:
        raise ValueError(f"canonical skill name does not match directory: {path}")
    return path, data


def find_drift(workspace_root: Path) -> list[SkillDrift]:
    drift: list[SkillDrift] = []
    for skill in SHARED_SKILLS:
        _, canonical = canonical_skill(workspace_root, skill)
        canonical_hash = hashlib.sha256(canonical).hexdigest()
        for repository in TARGET_REPOSITORIES:
            path = skill_path(workspace_root, repository, skill)
            if not path.is_file():
                drift.append(SkillDrift(repository, skill, path, "missing"))
                continue
            target_hash = hashlib.sha256(path.read_bytes()).hexdigest()
            if target_hash != canonical_hash:
                drift.append(SkillDrift(repository, skill, path, "content differs"))
    return drift


def atomic_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        output.write(data)
        output.flush()
        os.fsync(output.fileno())
    try:
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def sync_workspace(workspace_root: Path) -> list[Path]:
    updated: list[Path] = []
    for repository in TARGET_REPOSITORIES:
        repository_root = workspace_root / repository
        if not repository_root.is_dir():
            raise ValueError(f"target repository is missing: {repository_root}")
    for skill in SHARED_SKILLS:
        _, canonical = canonical_skill(workspace_root, skill)
        for repository in TARGET_REPOSITORIES:
            path = skill_path(workspace_root, repository, skill)
            if path.is_file() and path.read_bytes() == canonical:
                continue
            atomic_write(path, canonical)
            updated.append(path)
    return updated


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Check or synchronize shared USDB project skills."
    )
    parser.add_argument("command", choices=("check", "sync"))
    parser.add_argument(
        "--workspace-root",
        type=Path,
        default=default_workspace_root(),
        help="Directory containing usdb, go-ethereum, and SourceDAO.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    workspace_root = args.workspace_root.resolve()
    try:
        if args.command == "sync":
            updated = sync_workspace(workspace_root)
            for path in updated:
                print(f"UPDATED {path.relative_to(workspace_root)}")
        drift = find_drift(workspace_root)
    except ValueError as exc:
        print(f"project skill synchronization error: {exc}", file=sys.stderr)
        return 2
    if drift:
        for item in drift:
            print(
                f"OUT-OF-SYNC {item.repository}/{item.skill}: {item.reason}: "
                f"{item.path}",
                file=sys.stderr,
            )
        print(
            "Run the sync command from the USDB repository before committing.",
            file=sys.stderr,
        )
        return 1
    print(
        f"PASS shared project skills: {len(SHARED_SKILLS)} skill(s), "
        f"{1 + len(TARGET_REPOSITORIES)} repositories"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
