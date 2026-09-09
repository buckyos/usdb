#!/usr/bin/env python3
"""Exercise release decisions, scope changes, expiry, and malformed evidence."""

from copy import deepcopy
from datetime import date
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / ".github/scripts/image_security_policy.py"
SPEC = importlib.util.spec_from_file_location("image_security_policy", SCRIPT)
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)
IMAGE = "ghcr.io/buckyos/usdb-services"
REFERENCE = IMAGE + "@sha256:" + "1" * 64
TODAY = date(2026, 9, 7)


class ImageSecurityPolicyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="usdb-image-policy-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        (self.root / "runtime").mkdir()
        (self.root / "runtime/start.sh").write_text("exit 0\n")
        self.report = {
            "SchemaVersion": 2, "ArtifactType": "container_image", "ArtifactName": REFERENCE,
            "Metadata": {"RepoDigests": [REFERENCE], "OS": {"Family": "debian", "Name": "12.15"},
                         "ImageConfig": {"os": "linux", "architecture": "amd64",
                             "config": {"Labels": {"org.opencontainers.image.revision": "3" * 40}}}},
            "Results": [{"Target": REFERENCE, "Class": "os-pkgs", "Type": "debian",
                         "Vulnerabilities": [{"VulnerabilityID": "CVE-2023-45853",
                             "Severity": "CRITICAL", "PkgName": "zlib1g",
                             "InstalledVersion": "1:1.2.13.dfsg-1"}]}],
        }
        self.catalog = {
            "schema_version": POLICY.POLICY_SCHEMA,
            "profiles": {IMAGE: {"os": {"family": "debian", "name": "12.15"},
                "platform": "linux/amd64", "source_paths": ["runtime"],
                "source_sha256": POLICY.source_fingerprint(self.root, ["runtime"]),
                "baseline_image": REFERENCE, "baseline_report_sha256": "2" * 64,
                "review": "Reviewed test fixture"}},
            "exceptions": [{"advisory_id": "CVE-2023-45853", "severity": "CRITICAL",
                "images": [IMAGE], "packages": {"zlib1g": "1:1.2.13.dfsg-1"},
                "decision": "not_affected", "owner": "buckyos/usdb maintainers",
                "reviewed_at": "2026-09-07", "expires_at": "2026-10-07",
                "reason": "Vulnerable MiniZip is not built in this package.",
                "conditions": "Reviewed Debian/amd64 build only.",
                "remediation": "Re-evaluate on package or build changes.",
                "references": ["https://security-tracker.debian.org/tracker/CVE-2023-45853"]}],
        }

    def evaluate(self, **kwargs):
        arguments = dict(image_reference=REFERENCE, source_revision="3" * 40,
                         source_ref="refs/tags/usdb-testnet-v0-r17", enforcement="strict",
                         root=self.root, today=TODAY)
        arguments.update(kwargs)
        return POLICY.evaluate(self.report, self.catalog, **arguments)

    def finding(self):
        return self.report["Results"][0]["Vulnerabilities"][0]

    def test_matching_exception_preserves_raw_findings(self):
        original = deepcopy(self.report)
        decision = self.evaluate()
        self.assertEqual(decision["result"], "pass")
        self.assertEqual(decision["raw_counts"]["CRITICAL"], 1)
        self.assertEqual(decision["accepted_count"], 1)
        self.assertEqual(decision["blocking_count"], 0)
        self.assertEqual(self.report, original)

    def test_new_cve_still_blocks(self):
        self.finding()["VulnerabilityID"] = "CVE-2026-99999"
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_package_version_and_severity_are_exact(self):
        original = deepcopy(self.finding())
        for field, value in (("PkgName", "minizip"), ("InstalledVersion", "1:1.2.13.dfsg-2"),
                             ("Severity", "HIGH")):
            with self.subTest(field=field):
                self.report["Results"][0]["Vulnerabilities"][0] = {**original, field: value}
                self.assertEqual(self.evaluate()["result"], "fail")

    def test_available_fix_requires_remediation(self):
        self.finding()["FixedVersion"] = "1:1.3.dfsg-1"
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_expires_at_start_of_expiry_date_utc(self):
        self.assertEqual(self.evaluate(today=date(2026, 10, 6))["result"], "pass")
        self.assertEqual(self.evaluate(today=date(2026, 10, 7))["result"], "fail")
        self.assertEqual(self.evaluate(today=date(2026, 9, 6))["result"], "fail")

    def test_mainnet_and_branches_do_not_inherit_testnet_exceptions(self):
        for ref in ("refs/tags/usdb-mainnet-v0-r1", "refs/heads/master", "refs/pull/1/merge"):
            with self.subTest(ref=ref):
                self.assertEqual(self.evaluate(source_ref=ref)["result"], "fail")

    def test_os_and_architecture_changes_invalidate_exception(self):
        self.report["Metadata"]["ImageConfig"]["architecture"] = "arm64"
        self.assertEqual(self.evaluate()["result"], "fail")
        self.report["Metadata"]["ImageConfig"]["architecture"] = "amd64"
        self.report["Metadata"]["OS"]["Name"] = "12.16"
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_source_change_and_new_file_invalidate_exception(self):
        path = self.root / "runtime/start.sh"
        path.write_text("exec perl\n")
        self.assertEqual(self.evaluate()["result"], "fail")
        path.write_text("exit 0\n")
        (self.root / "runtime/new-handler.py").write_text("import xml.etree\n")
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_scope_diagnostics_identify_expected_and_actual_inputs(self):
        expected = self.catalog["profiles"][IMAGE]["source_sha256"]
        (self.root / "runtime/start.sh").write_text("exec perl\n")
        decision = self.evaluate()
        review = decision["source_review"]
        self.assertFalse(review["matches"])
        self.assertEqual(review["expected_sha256"], expected)
        self.assertEqual(review["actual_sha256"], POLICY.source_fingerprint(self.root, ["runtime"]))
        self.assertNotEqual(review["actual_sha256"], expected)
        self.assertEqual(decision["blocking_count"], 1)

    def test_scope_preflight_is_read_only_and_writes_failure_evidence(self):
        catalog = self.root / "exceptions.json"
        output = self.root / "scope.json"
        original = json.dumps(self.catalog)
        catalog.write_text(original)
        command = [sys.executable, str(SCRIPT), "check-scope", "--exceptions", str(catalog),
                   "--repository-root", str(self.root), "--output", str(output)]
        for changed in (False, True):
            if changed:
                (self.root / "runtime/new-handler.py").write_text("import xml.etree\n")
            for mode in ("strict", "report-only"):
                with self.subTest(changed=changed, mode=mode):
                    result = subprocess.run(command + ["--enforcement", mode],
                                            capture_output=True, text=True)
                    self.assertEqual(result.returncode, int(changed and mode == "strict"), result.stderr)
                    evidence = json.loads(output.read_text())
                    self.assertEqual(evidence["result"], "fail" if changed else "pass")
                    self.assertEqual(evidence["enforcement"], mode)
                    self.assertIn(IMAGE, result.stdout)
                    self.assertIn("expected_sha256=", result.stdout)
                    self.assertIn("actual_sha256=", result.stdout)
                    self.assertEqual(catalog.read_text(), original)
                    if changed:
                        self.assertIn("STALE", result.stdout)
                        self.assertIn("do not auto-refresh", result.stderr)
        self.assertEqual(subprocess.run(command, capture_output=True).returncode, 1)

    def test_scope_preflight_records_deleted_files_and_rejects_empty_profiles(self):
        (self.root / "runtime/start.sh").unlink()
        evidence = POLICY.check_source_reviews(self.catalog, self.root)
        self.assertEqual(evidence["result"], "fail")
        self.assertIn("error", evidence["profiles"][IMAGE])
        with self.assertRaisesRegex(ValueError, "requires image profiles"):
            POLICY.check_source_reviews({"schema_version": POLICY.POLICY_SCHEMA,
                                         "profiles": {}, "exceptions": []}, self.root)

    def test_fast_gate_reaches_compilation_with_stale_review(self):
        (self.root / "runtime/start.sh").write_text("changed build input\n")
        script = self.root / "src/btc/scripts/run_fast_ci.sh"
        script.parent.mkdir(parents=True)
        script.write_bytes((ROOT / "src/btc/scripts/run_fast_ci.sh").read_bytes())
        policy = self.root / ".github/scripts/image_security_policy.py"
        policy.parent.mkdir(parents=True)
        policy.write_bytes(SCRIPT.read_bytes())
        catalog = self.root / ".github/security/image-vulnerability-exceptions.json"
        catalog.parent.mkdir()
        catalog.write_text(json.dumps(self.catalog))
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        for name in ("cargo", "rustc", "shellcheck"):
            stub = bin_dir / name
            stub.write_text('#!/bin/sh\necho reached-toolchain >&2\nexit 99\n')
            stub.chmod(0o755)
        result = subprocess.run(["bash", str(script)], capture_output=True, text=True,
                                env={**os.environ, "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"]})
        self.assertEqual(result.returncode, 99, result.stdout + result.stderr)
        self.assertNotIn("STALE", result.stdout)
        self.assertIn("reached-toolchain", result.stderr)

    def test_source_mode_change_invalidates_exception(self):
        (self.root / "runtime/start.sh").chmod(0o755)
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_missing_reviewed_source_fails_closed(self):
        subprocess.run(["git", "-C", str(self.root), "add", "runtime/start.sh"], check=True)
        (self.root / "runtime/start.sh").unlink()
        decision = self.evaluate()
        self.assertEqual(decision["result"], "fail")
        self.assertIsNone(decision["source_review"]["actual_sha256"])
        diagnostic = self.evaluate(enforcement="report-only")
        self.assertEqual(diagnostic["result"], "pass")
        self.assertEqual(diagnostic["blocking_count"], 1)
        self.assertIn("error", diagnostic["source_review"])

    def test_deleted_source_path_is_stale_but_unsafe_catalog_paths_are_errors(self):
        (self.root / "runtime/start.sh").unlink()
        (self.root / "runtime").rmdir()
        decision = self.evaluate(enforcement="report-only")
        self.assertEqual(decision["result"], "pass")
        self.assertFalse(decision["source_review"]["matches"])
        self.catalog["profiles"][IMAGE]["source_paths"].append("../outside")
        with self.assertRaisesRegex(ValueError, "Invalid reviewed source path"):
            self.evaluate(enforcement="report-only")

    def test_os_exception_cannot_cover_static_or_language_dependency(self):
        target = self.report["Results"][0]
        target.update(Class="lang-pkgs", Type="cargo")
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_different_image_never_inherits_exception(self):
        reference = REFERENCE.replace("services", "chain")
        self.report["ArtifactName"] = reference
        self.report["Metadata"]["RepoDigests"] = [reference]
        self.assertEqual(self.evaluate(image_reference=reference)["result"], "fail")

    def test_report_digest_mismatch_fails_closed(self):
        self.report["Metadata"]["RepoDigests"] = [REFERENCE.replace("1", "2")]
        with self.assertRaises(ValueError):
            self.evaluate()

    def test_report_source_revision_mismatch_fails_closed(self):
        with self.assertRaises(ValueError):
            self.evaluate(source_revision="4" * 40)

    def test_unreviewed_image_blocks_even_without_catalog(self):
        self.catalog = {"schema_version": POLICY.POLICY_SCHEMA, "profiles": {}, "exceptions": []}
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_report_only_retains_unresolved_findings(self):
        self.catalog["exceptions"] = []
        decision = self.evaluate(enforcement="report-only")
        self.assertEqual(decision["result"], "pass")
        self.assertEqual(decision["blocking_count"], 1)

    def test_stale_scope_cli_reports_findings_before_optional_enforcement(self):
        (self.root / "runtime/start.sh").write_text("changed build input\n")
        report = self.root / "report.json"
        catalog = self.root / "catalog.json"
        output = self.root / "decision.json"
        report.write_text(json.dumps(self.report))
        catalog.write_text(json.dumps(self.catalog))
        for mode in ("strict", "report-only"):
            with self.subTest(mode=mode):
                result = subprocess.run([sys.executable, str(SCRIPT), "evaluate",
                    "--report", str(report), "--exceptions", str(catalog),
                    "--image-reference", REFERENCE, "--source-revision", "3" * 40,
                    "--source-ref", "refs/tags/usdb-testnet-v0-r22", "--enforcement", mode,
                    "--repository-root", str(self.root), "--output", str(output)],
                    capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("STALE", result.stdout)
                decision = json.loads(output.read_text())
                self.assertFalse(decision["source_review"]["matches"])
                self.assertEqual(decision["blocking_count"], 1)
                self.assertEqual(decision["accepted_count"], 0)
                self.assertIn("reviewed source/build/deployment inputs changed",
                              decision["blocking"][0]["reasons"])
                enforced = subprocess.run([sys.executable, str(SCRIPT), "enforce",
                                           "--decision", str(output)], capture_output=True)
                self.assertEqual(enforced.returncode, int(mode == "strict"))

    def test_scope_report_only_rejects_malformed_catalog(self):
        catalog = self.root / "catalog.json"
        self.catalog["exceptions"][0]["owner"] = ""
        catalog.write_text(json.dumps(self.catalog))
        result = subprocess.run([sys.executable, str(SCRIPT), "check-scope",
            "--exceptions", str(catalog), "--repository-root", str(self.root),
            "--enforcement", "report-only"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("Missing exception owner", result.stderr)

    def test_clean_report_passes_with_expired_unused_exceptions(self):
        self.report["Results"][0]["Vulnerabilities"] = []
        self.assertEqual(self.evaluate(today=date(2026, 12, 1))["result"], "pass")

    def test_malformed_report_fails_in_all_lanes(self):
        for mode in ("strict", "report-only"):
            for bad in (None, {}, []):
                with self.subTest(mode=mode, results=bad):
                    self.report["Results"] = bad
                    with self.assertRaises(ValueError):
                        self.evaluate(enforcement=mode)

    def test_malformed_finding_cannot_disappear_from_counts(self):
        self.finding().pop("Severity")
        with self.assertRaises(ValueError):
            self.evaluate()

    def test_catalog_requires_owner_expiry_and_exact_package(self):
        original = deepcopy(self.catalog["exceptions"][0])
        for field, value in (("owner", ""), ("expires_at", "2026-11-07"),
                             ("packages", {"zlib*": "*"})):
            with self.subTest(field=field):
                self.catalog["exceptions"][0] = {**original, field: value}
                with self.assertRaises(ValueError):
                    self.evaluate()

    def test_duplicate_exceptions_are_rejected(self):
        self.catalog["exceptions"].append(deepcopy(self.catalog["exceptions"][0]))
        with self.assertRaises(ValueError):
            self.evaluate()

    def test_duplicate_json_keys_are_rejected(self):
        path = self.root / "duplicate.json"
        path.write_text('{"expires_at":"2026-10-07","expires_at":"2099-01-01"}')
        with self.assertRaises(ValueError):
            POLICY.read_json(path)

    def test_cli_writes_blocking_evidence_before_enforcement(self):
        report = self.root / "report.json"
        decision = self.root / "decision.json"
        report.write_text(json.dumps(self.report))
        result = subprocess.run([sys.executable, str(SCRIPT), "evaluate", "--report", str(report),
            "--image-reference", REFERENCE, "--source-revision", "3" * 40,
            "--source-ref", "refs/tags/usdb-testnet-v0-r17", "--enforcement", "strict",
            "--repository-root", str(self.root), "--output", str(decision)], capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(decision.read_text())["result"], "fail")
        result = subprocess.run([sys.executable, str(SCRIPT), "enforce", "--decision", str(decision)],
                                capture_output=True)
        self.assertEqual(result.returncode, 1)

    def test_missing_binary_metadata_is_not_a_clean_scan(self):
        with self.assertRaises(ValueError):
            POLICY.require_rust_coverage(self.report, ["usr/local/bin/usdb-indexer"])
        self.report["Results"].append({"Target": "usr/local/bin/usdb-indexer",
            "Class": "lang-pkgs", "Type": "rustbinary", "Packages": [{"Name": "bytes", "Version": "1.12.1"}]})
        POLICY.require_rust_coverage(self.report, ["/usr/local/bin/usdb-indexer"])
        self.report["Results"][-1]["Packages"] = []
        with self.assertRaises(ValueError):
            POLICY.require_rust_coverage(self.report, ["usr/local/bin/usdb-indexer"])


class ReviewedImageReportsTests(unittest.TestCase):
    def test_historical_os_findings_require_the_recorded_review_scope(self):
        catalog = POLICY.read_json(ROOT / ".github/security/image-vulnerability-exceptions.json")
        for image, critical, high in (("bitcoin", 5, 78), ("services", 5, 89)):
            with self.subTest(image=image):
                report = POLICY.read_json(ROOT / f"tests/fixtures/image-security/{image}.json")
                repository = report["ArtifactName"].split("@")[0]
                profile = catalog["profiles"][repository]
                self.assertEqual(report["ArtifactName"], profile["baseline_image"])
                # Historical reports test classification at their review date, not current release eligibility.
                # Real source hashing is covered by the isolated filesystem tests above and explicit review CI.
                for matches in (True, False):
                    with self.subTest(matches=matches), patch.object(POLICY, "source_fingerprint",
                            return_value=profile["source_sha256"] if matches else "0" * 64):
                        decision = POLICY.evaluate(report, catalog, image_reference=report["ArtifactName"],
                            source_revision=report["Metadata"]["ImageConfig"]["config"]["Labels"]["org.opencontainers.image.revision"],
                            source_ref="refs/tags/usdb-testnet-v0-r17",
                            enforcement="strict", root=ROOT, today=TODAY)
                    self.assertEqual(decision["raw_counts"]["CRITICAL"], critical)
                    self.assertEqual(decision["raw_counts"]["HIGH"], high)
                    self.assertEqual(decision["accepted_count"], critical + high if matches else 0)
                    self.assertEqual(decision["blocking_count"], 0 if matches else critical + high)
                    self.assertEqual(decision["result"], "pass" if matches else "fail")


class ReleaseSecurityWorkflowTests(unittest.TestCase):
    def test_manual_batch_requires_exact_image_repositories_and_digests(self):
        workflow = (ROOT / ".github/workflows/release-security-review.yml").read_text()
        validation = workflow.split("        run: |\n", 1)[1].split("\n  scope:", 1)[0]
        services = REFERENCE
        bitcoin = REFERENCE.replace("usdb-services", "usdb-bitcoin-core")
        for services_ref, bitcoin_ref, mode, expected in (
                (services, bitcoin, "report-only", 0),
                (services, bitcoin, "strict", 0),
                (services.replace("@sha256:", ":"), bitcoin, "strict", 1),
                (services, services, "report-only", 1),
                (services, bitcoin, "disabled", 1)):
            with self.subTest(services=services_ref, bitcoin=bitcoin_ref, mode=mode):
                with tempfile.TemporaryDirectory(prefix="usdb-review-workflow-") as directory:
                    output = Path(directory) / "output"
                    result = subprocess.run(["bash", "-c", validation], cwd=ROOT,
                        env={**os.environ, "SERVICES_IMAGE": services_ref, "BITCOIN_IMAGE": bitcoin_ref,
                             "ENFORCEMENT": mode, "GITHUB_OUTPUT": str(output)},
                        capture_output=True, text=True)
                    self.assertEqual(result.returncode, expected, result.stderr)
                    if expected == 0:
                        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT,
                                                           text=True).strip()
                        self.assertEqual(output.read_text().strip(), "revision=" + revision)
                    else:
                        self.assertFalse(output.exists())

    def test_image_producers_only_enforce_mainnet_releases(self):
        for image in ("services", "bitcoin"):
            workflow = (ROOT / f".github/workflows/usdb-{image}-image.yml").read_text()
            # Execute the producer's actual policy selection without building or publishing an image.
            selection = workflow.split('          scan_enforcement="report-only"\n', 1)[1].split(
                "          {\n", 1)[0]
            for ref_type, ref_name, expected in (
                    ("branch", "master", "report-only"),
                    ("tag", "usdb-testnet-v0-r22", "report-only"),
                    ("tag", "usdb-mainnet-v1-r1", "strict")):
                with self.subTest(image=image, ref=ref_name):
                    result = subprocess.run(["bash", "-c", 'set -euo pipefail\n'
                        'scan_enforcement="report-only"\n' + selection + 'echo "$scan_enforcement"\n'],
                        env={**os.environ, "GITHUB_REF_TYPE": ref_type, "GITHUB_REF_NAME": ref_name},
                        capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip(), expected)


if __name__ == "__main__":
    unittest.main()
