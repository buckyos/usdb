"""Verify the public candidate freeze without changing checked-in release inputs."""
import copy
import json
from pathlib import Path
import shutil
import sys
import tempfile
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
from freeze_sourcedao_bootstrap import freeze, apply_freeze, prepare, candidate_sources
from sourcedao_release import apply_source_import, copy_public_bundle, digest, semantic_digest
from validate_network_bundle import read_json, validate_network_bundle


class SourceDAOReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="sourcedao-freeze-test-")
        self.root = Path(self.temp.name)
        self.bundle = self.root / "base"
        shutil.copytree(ROOT / "docker/networks/testnet-v0", self.bundle)
        self.original = read_json(self.bundle / "artifacts/sourcedao-bootstrap-config.json")
        self.candidate = copy.deepcopy(self.original)
        self.candidate["committee"]["initialMembers"].append("0x0000000000000000000000000000000000001234")
        self.config = self.root / "candidate.json"
        self.config.write_text(json.dumps(self.candidate))
        self.imported = self.root / "imported.json"
        self.imported.write_text(json.dumps(self.original))
        self.report = self.root / "source.json"
        self.report.write_text(json.dumps({"schemaVersion": "sourcedao-bootstrap-source:v1", "configSha256": semantic_digest(self.original)}))
        self.golden = self.root / "golden.json"
        self.golden.write_text(json.dumps({"schema_version": "sourcedao-usdb-contract-golden:v1", "contracts": [{"contract_name": str(index)} for index in range(9)]}))
        self.output = self.root / "frozen"

    def tearDown(self):
        self.temp.cleanup()

    def run_freeze(self):
        return freeze(bundle=self.bundle, config=self.config, golden=self.golden, output=self.output, imported_config=self.imported, source_report=self.report)

    def test_freeze_records_overrides_and_excludes_private_files(self):
        before = (self.bundle / "network.json").read_bytes()
        for name in ("node.env", "artifacts/sourcedao-bootstrap-state.json", "artifacts/state.json.transactions.json", "artifacts/bootstrap.log"):
            (self.bundle / name).write_text("private fixture, never publish")
        self.run_freeze()
        network = validate_network_bundle(self.output)
        self.assertEqual((self.bundle / "network.json").read_bytes(), before)
        self.assertEqual(read_json(self.output / "artifacts/sourcedao-bootstrap-config.json"), self.candidate)
        record = read_json(self.output / "artifacts/sourcedao-bootstrap-freeze.json")
        self.assertEqual(record["overrides"], [{"field": "committee.initialMembers", "before": self.original["committee"]["initialMembers"], "after": self.candidate["committee"]["initialMembers"]}])
        for folder in (self.output, self.root / "kit"):
            if folder != self.output:
                copy_public_bundle(self.output, folder, network)
                validate_network_bundle(folder)
            self.assertFalse((folder / "node.env").exists())
            self.assertFalse((folder / "artifacts/sourcedao-bootstrap-state.json").exists())
            self.assertFalse(list(folder.rglob("*.transactions.json")))
            self.assertFalse(list(folder.rglob("*.log")))
        with self.assertRaisesRegex(ValueError, "new directory"):
            self.run_freeze()
        # A changed candidate cannot pass using the old freeze record even if the artifact hash is refreshed.
        candidate_file = self.output / "artifacts/sourcedao-bootstrap-config.json"
        changed = read_json(candidate_file); changed["cycleMinLength"] += 1
        candidate_file.write_text(json.dumps(changed))
        with self.assertRaisesRegex(ValueError, "hash mismatch|SHA-256 mismatch"):
            validate_network_bundle(self.output)
        manifest_path = self.output / "artifacts/usdb-genesis.manifest.json"
        manifest = read_json(manifest_path)
        manifest["sourcedao_config_sha256"] = digest(candidate_file.read_bytes())
        manifest_path.write_text(json.dumps(manifest))
        network["artifacts"]["sourcedao_bootstrap"]["sha256"] = digest(candidate_file.read_bytes())
        network["artifacts"]["genesis_manifest"]["sha256"] = digest(manifest_path.read_bytes())
        (self.output / "network.json").write_text(json.dumps(network))
        with self.assertRaisesRegex(ValueError, "frozen config mismatch"):
            validate_network_bundle(self.output)

    def test_invalid_parameters_never_publish_a_partial_bundle(self):
        for mutation in (lambda c: c.update(rpcUrl="http://private.invalid/token"), lambda c: c.update(chainId=1),
                         lambda c: c["devToken"].update(initAmounts=["999999999999999999999999999999"] * 10),
                         lambda c: c["committee"].update(initialMembers=[c["committee"]["initialMembers"][0]] * 2)):
            with self.subTest(mutation=mutation):
                candidate = copy.deepcopy(self.candidate); mutation(candidate)
                self.config.write_text(json.dumps(candidate))
                with self.assertRaises(ValueError):
                    self.run_freeze()
                self.assertFalse(self.output.exists())
        self.config.write_text('{"schemaVersion":1,"schemaVersion":1}')
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            self.run_freeze()

    def test_provenance_and_symlink_checks(self):
        self.report.write_text(json.dumps({"schemaVersion": "sourcedao-bootstrap-source:v1", "configSha256": "0" * 64}))
        with self.assertRaisesRegex(ValueError, "identify the imported config"):
            self.run_freeze()
        self.report.write_text(json.dumps({"schemaVersion": "sourcedao-bootstrap-source:v1", "configSha256": semantic_digest(self.original)}))
        public = self.bundle / "node.env.example"
        outside = self.root / "outside"; outside.write_bytes(public.read_bytes())
        public.unlink(); public.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "escapes bundle"):
            self.run_freeze()
        self.assertFalse(self.output.exists())

    def test_apply_promotes_public_inputs_and_retains_rollback_copy(self):
        original = (self.bundle / "artifacts/sourcedao-bootstrap-config.json").read_bytes()
        private_file = self.bundle / "node.env"; private_file.write_text("private fixture")
        backup = apply_freeze(bundle=self.bundle, config=self.config, golden=self.golden, imported_config=self.imported, source_report=self.report)
        validate_network_bundle(self.bundle)
        self.assertEqual(read_json(self.bundle / "artifacts/sourcedao-bootstrap-config.json"), self.candidate)
        self.assertEqual((backup / "original/artifacts/sourcedao-bootstrap-config.json").read_bytes(), original)
        self.assertEqual(private_file.read_text(), "private fixture")
        self.assertTrue((backup / "completed").exists())
        self.assertFalse((self.bundle / ".sourcedao-freeze.lock").exists())

    def test_failed_apply_restores_all_original_public_files(self):
        import os
        before = {str(file.relative_to(self.bundle)): file.read_bytes() for file in self.bundle.rglob("*") if file.is_file()}
        replace = os.replace
        def fail_network_update(source, target):
            if Path(source).name == "network.json" and "candidate" in Path(source).parts:
                raise OSError("simulated final manifest write failure")
            return replace(source, target)
        with patch("freeze_sourcedao_bootstrap.os.replace", side_effect=fail_network_update):
            with self.assertRaisesRegex(OSError, "simulated"):
                apply_freeze(bundle=self.bundle, config=self.config, golden=self.golden, imported_config=self.imported, source_report=self.report)
        after = {str(file.relative_to(self.bundle)): file.read_bytes() for file in self.bundle.rglob("*") if file.is_file()}
        self.assertEqual(before, after)
        validate_network_bundle(self.bundle)

    def shared_source(self):
        directory = self.root / "shared"
        shutil.copytree(ROOT.parent / "SourceDAO/security/sources/optimism/imports/156576688", directory)
        return directory, directory / "sourcedao-bootstrap-imported.json", directory / "sourcedao-bootstrap-source.json"

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / "docker/scripts/tools/freeze_sourcedao_bootstrap.py"), *map(str, args)],
                              cwd=self.root, text=True, capture_output=True)

    def test_prepare_and_freeze_default_files_from_another_working_directory(self):
        source, imported, report = self.shared_source()
        directory = self.root / "candidate"
        args = ["--bundle-dir", self.bundle, "--input-dir", directory]
        result = self.cli(*args, "--prepare", "--source-dir", source)
        self.assertEqual(result.returncode, 0, result.stderr)
        final = directory / "sourcedao-bootstrap-final.json"
        candidate = read_json(final)
        self.assertEqual(len(candidate["committee"]["initialMembers"]), 5)
        self.assertEqual(candidate["chainId"], self.original["chainId"])
        self.assertEqual(candidate["devToken"]["name"], self.original["devToken"]["name"])
        candidate["committee"]["initialMembers"] = ["0x0000000000000000000000000000000000001234"]
        final.write_text(json.dumps(candidate))
        edited = final.read_bytes()
        result = self.cli(*args, "--prepare", "--source-dir", source)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(final.read_bytes(), edited)
        result = self.cli(*args)
        self.assertEqual(result.returncode, 0, result.stderr)
        frozen = directory / "frozen-network-bundle"
        validate_network_bundle(frozen)
        record = read_json(frozen / "artifacts/sourcedao-bootstrap-freeze.json")
        self.assertEqual(record["overrides"], [{"field": "committee.initialMembers", "before": read_json(imported)["committee"]["initialMembers"], "after": candidate["committee"]["initialMembers"]}])
        self.assertEqual((frozen / "artifacts/sourcedao-bootstrap-source.json").read_bytes(), report.read_bytes())
        self.assertNotEqual(self.cli(*args).returncode, 0)
        self.assertEqual(final.read_bytes(), edited)

    def test_candidate_pins_source_and_base_without_following_later_imports(self):
        source, imported, report = self.shared_source()
        directory = self.root / "candidate"
        prepare(bundle=self.bundle, directory=directory, imported=imported, report=report)
        other = self.root / "next-import"; shutil.copytree(source, other)
        self.assertEqual(candidate_sources(directory, self.bundle), (imported, report))
        saved = imported.read_bytes()
        imported.write_bytes(saved + b"\n")
        with self.assertRaisesRegex(ValueError, "source evidence changed"):
            candidate_sources(directory, self.bundle)
        imported.write_bytes(saved)
        inputs_path = directory / "sourcedao-bootstrap-inputs.json"
        inputs = read_json(inputs_path); inputs["baseConfigSha256"] = "0" * 64
        inputs_path.write_text(json.dumps(inputs))
        with self.assertRaisesRegex(ValueError, "base bundle changed"):
            candidate_sources(directory, self.bundle)
        result = self.cli("--bundle-dir", self.bundle, "--network", "usdb-mainnet", "--input-dir", self.root / "wrong-network", "--prepare", "--source-dir", source)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("differs from selected network", result.stderr)
        self.assertFalse((self.root / "wrong-network").exists())

    def test_shared_source_does_not_carry_destination_policy_and_rejects_tampering(self):
        _, imported_file, report_file = self.shared_source()
        imported, report = read_json(imported_file), read_json(report_file)
        other = copy.deepcopy(self.original); other["chainId"] = 123456
        other["devToken"]["name"] = "Independent Mainnet DAO"
        first = apply_source_import(self.original, imported, report)
        second = apply_source_import(other, imported, report)
        self.assertEqual(second["chainId"], 123456)
        self.assertEqual(second["devToken"]["name"], other["devToken"]["name"])
        self.assertEqual(first["devToken"]["initAmounts"], second["devToken"]["initAmounts"])
        self.assertNotIn("chainId", imported)
        imported["devToken"]["initAmounts"][0] = "1"
        # Updating a digest cannot hide a mismatch with the retained source observations.
        report["importedSha256"] = semantic_digest(imported)
        with self.assertRaisesRegex(ValueError, "differs from source observations"):
            apply_source_import(other, imported, report)


if __name__ == "__main__":
    unittest.main()
