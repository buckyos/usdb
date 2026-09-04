#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("release_publish_resolver.py")
REPOSITORY_ROOT = MODULE_PATH.parents[3]
CANDIDATE_WORKFLOW = REPOSITORY_ROOT / ".github/workflows/usdb-release-candidate.yml"
PUBLISH_WORKFLOW = REPOSITORY_ROOT / ".github/workflows/usdb-release-publish.yml"
SPEC = importlib.util.spec_from_file_location("release_publish_resolver", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RESOLVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RESOLVER)


class ReleasePublishResolverTests(unittest.TestCase):
    def setUp(self) -> None:
        self.release_id = "usdb-testnet-v0-r1"
        self.usdb_revision = "a" * 40
        self.run_id = 101
        self.run = {
            "id": self.run_id,
            "run_attempt": 2,
            "path": (
                ".github/workflows/usdb-release-candidate.yml@refs/tags/"
                + self.release_id
            ),
            "display_title": f"USDB candidate {self.release_id}",
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "head_sha": self.usdb_revision,
            "head_branch": self.release_id,
        }
        self.artifact = {
            "id": 202,
            "name": f"{self.release_id}-manifest",
            "expired": False,
            "digest": "sha256:" + "b" * 64,
            "workflow_run": {
                "id": self.run_id,
                "head_sha": self.usdb_revision,
            },
        }

    def resolve(self) -> dict[str, str]:
        return RESOLVER.resolve_candidate(
            {"artifacts": [self.artifact]},
            {"workflow_runs": [self.run]},
            release_id=self.release_id,
            usdb_revision=self.usdb_revision,
        )

    def test_resolves_exact_candidate_run_and_artifact(self) -> None:
        self.assertEqual(
            self.resolve(),
            {
                "candidate_run_id": "101",
                "candidate_run_attempt": "2",
                "candidate_artifact_id": "202",
                "candidate_artifact_digest": "sha256:" + "b" * 64,
            },
        )

    def test_duplicate_candidate_run_is_rejected(self) -> None:
        duplicate = copy.deepcopy(self.run)
        duplicate["id"] = 102
        with self.assertRaisesRegex(ValueError, "exactly one successful candidate"):
            RESOLVER.resolve_candidate(
                {"artifacts": [self.artifact]},
                {"workflow_runs": [self.run, duplicate]},
                release_id=self.release_id,
                usdb_revision=self.usdb_revision,
            )

    def test_expired_or_wrong_run_artifact_is_rejected(self) -> None:
        self.artifact["expired"] = True
        with self.assertRaisesRegex(ValueError, "exactly one active"):
            self.resolve()
        self.artifact["expired"] = False
        self.artifact["workflow_run"]["id"] = 999
        with self.assertRaisesRegex(ValueError, "exactly one active"):
            self.resolve()

    def test_wrong_tag_dispatch_is_rejected(self) -> None:
        self.run["head_branch"] = "master"
        with self.assertRaisesRegex(ValueError, "exactly one successful candidate"):
            self.resolve()

    def test_display_title_is_not_part_of_candidate_identity(self) -> None:
        self.run["display_title"] = (
            f"USDB candidate {self.release_id} from {self.release_id}"
        )
        self.assertEqual(self.resolve()["candidate_run_id"], str(self.run_id))

    def test_release_workflows_derive_release_id_only_from_selected_tag(self) -> None:
        candidate = CANDIDATE_WORKFLOW.read_text(encoding="utf-8")
        publish = PUBLISH_WORKFLOW.read_text(encoding="utf-8")

        self.assertNotIn("inputs.release_id", candidate)
        self.assertNotIn("inputs.release_id", publish)
        self.assertIn("run-name: USDB candidate ${{ github.ref_name }}", candidate)
        self.assertIn("run-name: Publish ${{ github.ref_name }}", publish)
        self.assertIn("RELEASE_ID: ${{ github.ref_name }}", candidate)
        self.assertIn("RELEASE_ID: ${{ github.ref_name }}", publish)
        self.assertIn("release-title", publish)
        self.assertIn("--qualification-level", publish)

    def test_release_title_is_canonical_for_each_qualification(self) -> None:
        for level in ("fast", "nightly", "weekly"):
            self.assertEqual(
                RESOLVER.canonical_release_title(self.release_id, level),
                f"[{level.upper()}] testnet-v0-r1",
            )
        self.assertEqual(
            RESOLVER.canonical_release_title("usdb-mainnet-v12-r34", "weekly"),
            "[WEEKLY] mainnet-v12-r34",
        )
        with self.assertRaisesRegex(ValueError, "qualification level"):
            RESOLVER.canonical_release_title(self.release_id, "custom")

    def test_verifies_exact_existing_release(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            first = root / "manifest.json"
            second = root / "manifest.json.sha256"
            first.write_text("manifest\n", encoding="utf-8")
            second.write_text("checksum\n", encoding="utf-8")
            notes = "release notes\n"
            release = {
                "tag_name": self.release_id,
                "name": "[FAST] testnet-v0-r1",
                "body": notes,
                "draft": False,
                "prerelease": True,
                "html_url": f"https://github.com/buckyos/usdb/releases/{self.release_id}",
                "assets": [
                    {
                        "name": first.name,
                        "digest": "sha256:" + RESOLVER._sha256(first),
                        "state": "uploaded",
                    },
                    {
                        "name": second.name,
                        "digest": "sha256:" + RESOLVER._sha256(second),
                        "state": "uploaded",
                    },
                ],
            }
            url = RESOLVER.verify_existing_release(
                release,
                release_id=self.release_id,
                qualification_level="fast",
                notes=notes,
                assets=[first, second],
                prerelease=True,
            )
            self.assertEqual(url, release["html_url"])

            release["assets"][0]["digest"] = "sha256:" + "0" * 64
            with self.assertRaisesRegex(ValueError, "assets do not match"):
                RESOLVER.verify_existing_release(
                    release,
                    release_id=self.release_id,
                    qualification_level="fast",
                    notes=notes,
                    assets=[first, second],
                    prerelease=True,
                )

    def test_release_title_must_match_qualification(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            asset = Path(temp_dir) / "manifest.json"
            asset.write_text("manifest\n", encoding="utf-8")
            release = {
                "tag_name": self.release_id,
                "name": "[WEEKLY] testnet-v0-r1",
                "body": "notes\n",
                "draft": False,
                "prerelease": True,
                "html_url": f"https://github.com/buckyos/usdb/releases/{self.release_id}",
                "assets": [
                    {
                        "name": asset.name,
                        "digest": "sha256:" + RESOLVER._sha256(asset),
                        "state": "uploaded",
                    }
                ],
            }
            with self.assertRaisesRegex(ValueError, "title does not match"):
                RESOLVER.verify_existing_release(
                    release,
                    release_id=self.release_id,
                    qualification_level="fast",
                    notes="notes\n",
                    assets=[asset],
                    prerelease=True,
                )


if __name__ == "__main__":
    unittest.main()
