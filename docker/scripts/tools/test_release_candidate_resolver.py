#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("release_candidate_resolver.py")
SPEC = importlib.util.spec_from_file_location("release_candidate_resolver", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RESOLVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RESOLVER)


class ReleaseCandidateResolverTests(unittest.TestCase):
    def setUp(self) -> None:
        self.usdb_revision = "b" * 40
        self.go_revision = "a" * 40
        self.source_dao_revision = "c" * 40
        self.release_id = "usdb-testnet-v0-r1"
        self.lock = {
            "schema_version": "usdb-ci-revisions:v1",
            "coordinator": "go_ethereum",
            "repositories": {
                "go_ethereum": {
                    "repository": "buckyos/go-ethereum",
                    "revision": "0" * 40,
                },
                "usdb": {
                    "repository": "buckyos/usdb",
                    "revision": self.usdb_revision,
                },
                "source_dao": {
                    "repository": "buckyos/SourceDAO",
                    "revision": self.source_dao_revision,
                },
            },
        }
        self.usdb_runs = self.runs(
            revision=self.usdb_revision,
            run_id=101,
            workflows={
                "buckyos/usdb/.github/workflows/"
                f"usdb-fast.yml@{self.usdb_revision}",
                "buckyos/usdb/.github/workflows/"
                f"usdb-services-image.yml@{self.usdb_revision}",
                "buckyos/usdb/.github/workflows/"
                f"usdb-bitcoin-image.yml@{self.usdb_revision}",
            },
        )
        self.go_runs = self.runs(
            revision=self.go_revision,
            run_id=202,
            workflows={
                "buckyos/go-ethereum/.github/workflows/"
                f"usdb-fast.yml@{self.go_revision}",
                "buckyos/go-ethereum/.github/workflows/"
                f"usdb-chain-image.yml@{self.go_revision}"
            },
        )

    def runs(self, *, revision: str, run_id: int, workflows: set[str]) -> dict:
        return {
            "workflow_runs": [
                {
                    "id": run_id,
                    "run_attempt": 2,
                    "path": (
                        ".github/workflows/usdb-release-build.yml@refs/tags/"
                        f"{self.release_id}"
                    ),
                    "head_sha": revision,
                    "head_branch": self.release_id,
                    "display_title": f"USDB release {self.release_id}",
                    "event": "push",
                    "status": "completed",
                    "conclusion": "success",
                    "referenced_workflows": [
                        {"path": path} for path in sorted(workflows)
                    ],
                }
            ]
        }

    def resolve(self) -> dict[str, str]:
        return RESOLVER.resolve_candidate(
            compatibility_lock=self.lock,
            usdb_runs=self.usdb_runs,
            go_runs=self.go_runs,
            usdb_revision=self.usdb_revision,
            go_ethereum_revision=self.go_revision,
            release_id=self.release_id,
        )

    def test_resolves_locked_revision_and_run_scoped_tags(self) -> None:
        resolved = self.resolve()
        self.assertEqual(resolved["source_dao_revision"], self.source_dao_revision)
        self.assertEqual(
            resolved["services_tag"],
            f"ghcr.io/buckyos/usdb-services:git-{self.usdb_revision}-run-101-2",
        )
        self.assertEqual(
            resolved["chain_tag"],
            f"ghcr.io/buckyos/usdb-chain:git-{self.go_revision}-run-202-2",
        )
        self.assertEqual(
            resolved["bitcoin_tag"],
            "ghcr.io/buckyos/usdb-bitcoin-core:bitcoin-28.1-"
            f"git-{self.usdb_revision}-run-101-2",
        )

    def test_release_coordinator_must_match_go_lock(self) -> None:
        self.lock["repositories"]["usdb"]["revision"] = "d" * 40
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.resolve()

    def test_missing_image_workflow_is_rejected(self) -> None:
        self.usdb_runs["workflow_runs"][0]["referenced_workflows"].pop()
        with self.assertRaisesRegex(ValueError, "exactly one successful"):
            self.resolve()

    def test_non_push_or_wrong_revision_run_is_rejected(self) -> None:
        self.go_runs["workflow_runs"][0]["event"] = "workflow_dispatch"
        with self.assertRaisesRegex(ValueError, "exactly one successful"):
            self.resolve()

    def test_master_or_another_release_tag_run_is_rejected(self) -> None:
        self.go_runs["workflow_runs"][0]["display_title"] = "USDB release master"
        with self.assertRaisesRegex(ValueError, "release-tag build"):
            self.resolve()

        self.go_runs["workflow_runs"][0]["display_title"] = "USDB release usdb-testnet-v0-r2"
        with self.assertRaisesRegex(ValueError, "release-tag build"):
            self.resolve()

    def test_invalid_release_id_is_rejected(self) -> None:
        self.release_id = "latest"
        with self.assertRaisesRegex(ValueError, "release ID"):
            self.resolve()

    def test_ambiguous_successful_runs_are_rejected(self) -> None:
        duplicate = copy.deepcopy(self.go_runs["workflow_runs"][0])
        duplicate["id"] = 203
        self.go_runs["workflow_runs"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "found 2"):
            self.resolve()


if __name__ == "__main__":
    unittest.main()
