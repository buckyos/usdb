#!/usr/bin/env python3
"""Tests for shared project skill synchronization."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("sync_project_skills.py")
SPEC = importlib.util.spec_from_file_location("sync_project_skills", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
SYNC = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SYNC
SPEC.loader.exec_module(SYNC)


class SyncProjectSkillsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.workspace = Path(self.temp.name)
        for repository in (SYNC.CANONICAL_REPOSITORY, *SYNC.TARGET_REPOSITORIES):
            (self.workspace / repository).mkdir()
        self.canonical = (
            b"---\n"
            b"name: usdb-release-fragments\n"
            b"description: Test shared release fragments.\n"
            b"---\n\n"
            b"# Test\n"
        )
        path = SYNC.skill_path(
            self.workspace, SYNC.CANONICAL_REPOSITORY, SYNC.SHARED_SKILLS[0]
        )
        path.parent.mkdir(parents=True)
        path.write_bytes(self.canonical)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_missing_copies_are_reported(self) -> None:
        drift = SYNC.find_drift(self.workspace)
        self.assertEqual(
            list(SYNC.TARGET_REPOSITORIES), [item.repository for item in drift]
        )
        self.assertTrue(all(item.reason == "missing" for item in drift))

    def test_sync_creates_identical_copies_and_is_idempotent(self) -> None:
        updated = SYNC.sync_workspace(self.workspace)
        self.assertEqual(len(SYNC.TARGET_REPOSITORIES), len(updated))
        self.assertEqual([], SYNC.find_drift(self.workspace))
        self.assertEqual([], SYNC.sync_workspace(self.workspace))
        for path in updated:
            self.assertEqual(self.canonical, path.read_bytes())

    def test_changed_copy_is_reported_and_repaired(self) -> None:
        SYNC.sync_workspace(self.workspace)
        path = SYNC.skill_path(
            self.workspace, SYNC.TARGET_REPOSITORIES[0], SYNC.SHARED_SKILLS[0]
        )
        path.write_text("drift\n", encoding="utf-8")
        drift = SYNC.find_drift(self.workspace)
        self.assertEqual(1, len(drift))
        self.assertEqual("content differs", drift[0].reason)
        self.assertEqual([path], SYNC.sync_workspace(self.workspace))
        self.assertEqual([], SYNC.find_drift(self.workspace))

    def test_sync_rejects_missing_repository(self) -> None:
        (self.workspace / SYNC.TARGET_REPOSITORIES[0]).rmdir()
        with self.assertRaisesRegex(ValueError, "target repository is missing"):
            SYNC.sync_workspace(self.workspace)

    def test_canonical_frontmatter_name_must_match(self) -> None:
        path = SYNC.skill_path(
            self.workspace, SYNC.CANONICAL_REPOSITORY, SYNC.SHARED_SKILLS[0]
        )
        path.write_text("---\nname: wrong-skill\n---\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "name does not match"):
            SYNC.find_drift(self.workspace)


if __name__ == "__main__":
    unittest.main()
