#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("release_notes.py")
SPEC = importlib.util.spec_from_file_location("release_notes", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RELEASE_NOTES = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RELEASE_NOTES
SPEC.loader.exec_module(RELEASE_NOTES)


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory(prefix="usdb-release-notes-test-")
        self.root = Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    @staticmethod
    def fragment(change_id: str = "release-notes-v1") -> dict:
        return {
            "schema_version": "usdb-change-fragment:v1",
            "change_id": change_id,
            "type": "added",
            "scopes": ["release", "documentation"],
            "summary": "Generate structured cross-repository release notes",
            "details": [
                "Collect immutable revision ranges and structured change fragments.",
                "Report compatibility impact and unclassified commits before publication.",
            ],
            "operator_actions": [],
            "compatibility": {
                "network_reset": False,
                "data_rebuild": False,
                "config_change": False,
                "restart_required": False,
            },
            "references": [],
        }

    def init_repository(self, name: str) -> tuple[Path, str]:
        path = self.root / name
        path.mkdir()
        self.git(path, "init", "-q")
        self.git(path, "config", "user.email", "release-test@example.invalid")
        self.git(path, "config", "user.name", "Release Test")
        (path / "README.md").write_text(f"{name}\n", encoding="utf-8")
        self.git(path, "add", "README.md")
        self.git(path, "commit", "-q", "-m", "Initialize repository")
        return path, self.git(path, "rev-parse", "HEAD").strip()

    @staticmethod
    def git(path: Path, *arguments: str) -> str:
        environment = os.environ.copy()
        environment.update(
            {
                "GIT_AUTHOR_DATE": "2026-09-03T00:00:00Z",
                "GIT_COMMITTER_DATE": "2026-09-03T00:00:00Z",
            }
        )
        result = subprocess.run(
            ["git", "-C", str(path), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=environment,
        )
        return result.stdout

    def write_fragment(self, repository: Path, value: dict) -> Path:
        path = repository / ".release-notes/fragments" / f"{value['change_id']}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return path

    @staticmethod
    def manifest(release_id: str, revisions: dict[str, str], suffix: str) -> dict:
        return {
            "release_id": release_id,
            "network_bundle": {
                "bundle_id": "usdb-testnet-v0",
                "network_json_sha256": "1" * 64,
                "chain_id": 202608250,
                "network_id": 202608250,
                "genesis_sha256": "2" * 64,
                "genesis_block_hash": "0x" + "3" * 64,
                "btc_network_id": "btc-mainnet",
                "btc_index_origin_height": 963800,
                "btc_activation_registry_id": "4" * 64,
                "snapshot_trusted_keys_sha256": "5" * 64,
            },
            "runtime_compatibility": {
                "compatibility_id": "6" * 64,
                "data_layout_version": "usdb-node-data-layout:v2",
            },
            "images": {
                "usdb_services": {"reference": f"services-{suffix}"},
                "usdb_chain": {"reference": f"chain-{suffix}"},
                "bitcoin_core": {"reference": f"bitcoin-{suffix}"},
            },
            "snapshot": {
                "status": "available",
                "snapshot_release_id": "snapshot-1",
                "height": 963800,
                "record": {"sha256": "7" * 64},
            },
            "qualification": {"level": "fast"},
            "compatibility_lock": {"sha256": "8" * 64},
            "repositories": {
                "usdb": {"repository": "buckyos/usdb", "revision": revisions["usdb"]},
                "go_ethereum": {"repository": "buckyos/go-ethereum", "revision": revisions["go_ethereum"]},
                "source_dao": {"repository": "buckyos/SourceDAO", "revision": revisions["source_dao"]},
            },
        }

    def write_manifest(self, name: str, value: dict) -> Path:
        path = self.root / name
        path.write_bytes(RELEASE_NOTES.canonical_json(value))
        return path

    def repository_fixture(self) -> tuple[list, Path, Path]:
        repositories = {}
        slugs = {
            "usdb": "buckyos/usdb",
            "go_ethereum": "buckyos/go-ethereum",
            "source_dao": "buckyos/SourceDAO",
        }
        for key in sorted(slugs):
            path, previous = self.init_repository(key)
            if key == "usdb":
                fragment_path = self.write_fragment(path, self.fragment())
                self.git(path, "add", str(fragment_path.relative_to(path)))
                self.git(path, "commit", "-q", "-m", "Add release notes model", "-m", "Release-Note: release-notes-v1")
            elif key == "go_ethereum":
                (path / "change.txt").write_text("changed\n", encoding="utf-8")
                self.git(path, "add", "change.txt")
                self.git(path, "commit", "-q", "-m", "Change chain implementation")
            current = self.git(path, "rev-parse", "HEAD").strip()
            repositories[key] = (path, previous, current)
        previous_revisions = {key: value[1] for key, value in repositories.items()}
        current_revisions = {key: value[2] for key, value in repositories.items()}
        previous_manifest = self.write_manifest(
            "previous-manifest.json",
            self.manifest("usdb-testnet-v0-r11", previous_revisions, "old"),
        )
        current_manifest = self.write_manifest(
            "current-manifest.json",
            self.manifest("usdb-testnet-v0-r12", current_revisions, "new"),
        )
        specs = [
            RELEASE_NOTES.RepositorySpec(key, slugs[key], value[0], value[1], value[2])
            for key, value in repositories.items()
        ]
        return specs, previous_manifest, current_manifest

    def test_fragment_validation_rejects_duplicate_json_keys(self) -> None:
        duplicate = b'{"schema_version":"usdb-change-fragment:v1","change_id":"a","change_id":"b"}'
        with self.assertRaisesRegex(ValueError, "duplicate JSON key: change_id"):
            RELEASE_NOTES.load_json_bytes(duplicate, "duplicate.json")

    def test_fragment_directory_requires_file_name_to_match_change_id(self) -> None:
        repository, _ = self.init_repository("fragment-name")
        path = self.write_fragment(repository, self.fragment("correct-name"))
        path.rename(path.with_name("wrong-name.json"))
        with self.assertRaisesRegex(ValueError, "file name must equal change_id"):
            RELEASE_NOTES.validate_fragment_directory(repository)

    def test_fragment_rejects_redundant_network_reset_and_rebuild(self) -> None:
        fragment = self.fragment()
        fragment["compatibility"]["network_reset"] = True
        fragment["compatibility"]["data_rebuild"] = True
        with self.assertRaisesRegex(ValueError, "data_rebuild is redundant"):
            RELEASE_NOTES.validate_fragment(fragment, "fragment")

    def test_generate_collects_fragments_ranges_and_coverage(self) -> None:
        specs, previous_manifest, current_manifest = self.repository_fixture()
        result = RELEASE_NOTES.build_release_changes(
            release_id="usdb-testnet-v0-r12",
            manifest_path=current_manifest,
            previous_manifest_path=previous_manifest,
            repositories=specs,
        )
        self.assertEqual(["release-notes-v1"], [item["change_id"] for item in result["changes"]])
        self.assertEqual("restart_required", result["compatibility"]["classification"])
        self.assertEqual(1, result["repositories"]["usdb"]["coverage"]["classified"])
        self.assertEqual(1, result["repositories"]["go_ethereum"]["coverage"]["unclassified"])
        markdown = RELEASE_NOTES.render_markdown(result)
        self.assertIn("## Changes Since usdb-testnet-v0-r11", markdown)
        self.assertIn("Upgrade classification: `restart_required`", markdown)
        self.assertIn("Change chain implementation", markdown)
        output = self.root / "release-changes.json"
        output.write_bytes(RELEASE_NOTES.canonical_json(result))
        RELEASE_NOTES.validate_release_changes(RELEASE_NOTES.load_json(output), current_manifest, "usdb-testnet-v0-r12")

    def test_manifest_identity_change_requires_network_reset(self) -> None:
        specs, previous_manifest, current_manifest = self.repository_fixture()
        current = RELEASE_NOTES.load_json(current_manifest)
        current["network_bundle"]["chain_id"] += 1
        current_manifest.write_bytes(RELEASE_NOTES.canonical_json(current))
        result = RELEASE_NOTES.build_release_changes(
            release_id="usdb-testnet-v0-r12",
            manifest_path=current_manifest,
            previous_manifest_path=previous_manifest,
            repositories=specs,
        )
        self.assertEqual("network_reset", result["compatibility"]["classification"])

    def test_previous_release_must_share_network_lineage(self) -> None:
        specs, previous_manifest, current_manifest = self.repository_fixture()
        previous = RELEASE_NOTES.load_json(previous_manifest)
        previous["release_id"] = "usdb-mainnet-v0-r1"
        previous_manifest.write_bytes(RELEASE_NOTES.canonical_json(previous))
        with self.assertRaisesRegex(ValueError, "another network bundle"):
            RELEASE_NOTES.build_release_changes(
                release_id="usdb-testnet-v0-r12",
                manifest_path=current_manifest,
                previous_manifest_path=previous_manifest,
                repositories=specs,
            )

    def test_release_validation_rejects_coverage_tamper(self) -> None:
        specs, previous_manifest, current_manifest = self.repository_fixture()
        result = RELEASE_NOTES.build_release_changes(
            release_id="usdb-testnet-v0-r12",
            manifest_path=current_manifest,
            previous_manifest_path=previous_manifest,
            repositories=specs,
        )
        result["repositories"]["usdb"]["coverage"]["classified"] = 0
        with self.assertRaisesRegex(ValueError, "coverage mismatch"):
            RELEASE_NOTES.validate_release_changes(
                result, current_manifest, "usdb-testnet-v0-r12"
            )

    def test_existing_fragment_is_append_only(self) -> None:
        repository, _ = self.init_repository("append-only")
        path = self.write_fragment(repository, self.fragment("existing-change"))
        self.git(repository, "add", str(path.relative_to(repository)))
        self.git(repository, "commit", "-q", "-m", "Add initial fragment")
        previous_revision = self.git(repository, "rev-parse", "HEAD").strip()
        value = self.fragment("existing-change")
        value["summary"] = "Mutate a previously released fragment"
        self.write_fragment(repository, value)
        self.git(repository, "add", str(path.relative_to(repository)))
        self.git(repository, "commit", "-q", "-m", "Mutate fragment")
        current_revision = self.git(repository, "rev-parse", "HEAD").strip()
        spec = RELEASE_NOTES.RepositorySpec("usdb", "buckyos/usdb", repository, previous_revision, current_revision)
        with self.assertRaisesRegex(ValueError, "append-only"):
            RELEASE_NOTES._fragment_paths_for_range(spec)

    def test_release_workflows_publish_the_change_record(self) -> None:
        repository_root = MODULE_PATH.parents[3]
        candidate = (repository_root / ".github/workflows/usdb-release-candidate.yml").read_text(encoding="utf-8")
        publish = (repository_root / ".github/workflows/usdb-release-publish.yml").read_text(encoding="utf-8")
        self.assertIn("Generate cross-repository release changes", candidate)
        self.assertIn("release-changes.json.sha256", candidate)
        self.assertIn("release_notes.py validate-release", publish)
        self.assertIn("dist/release-changes.md", publish)


if __name__ == "__main__":
    unittest.main()
