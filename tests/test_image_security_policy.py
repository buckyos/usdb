#!/usr/bin/env python3
"""Exercise release decisions, scope changes, expiry, and malformed evidence."""

from copy import deepcopy
from datetime import date
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

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

    def test_source_mode_change_invalidates_exception(self):
        (self.root / "runtime/start.sh").chmod(0o755)
        self.assertEqual(self.evaluate()["result"], "fail")

    def test_missing_reviewed_source_fails_closed(self):
        (self.root / "runtime/start.sh").unlink()
        with self.assertRaises(ValueError):
            self.evaluate()

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
    def test_all_reviewed_os_findings_are_classified(self):
        catalog = POLICY.read_json(ROOT / ".github/security/image-vulnerability-exceptions.json")
        for image, critical, high in (("bitcoin", 5, 78), ("services", 5, 89)):
            with self.subTest(image=image):
                report = POLICY.read_json(ROOT / f"tests/fixtures/image-security/{image}.json")
                decision = POLICY.evaluate(report, catalog, image_reference=report["ArtifactName"],
                    source_revision=report["Metadata"]["ImageConfig"]["config"]["Labels"]["org.opencontainers.image.revision"],
                    source_ref="refs/tags/usdb-testnet-v0-r17",
                    enforcement="strict", root=ROOT, today=TODAY)
                self.assertEqual(decision["raw_counts"]["CRITICAL"], critical)
                self.assertEqual(decision["raw_counts"]["HIGH"], high)
                self.assertEqual(decision["blocking"], [])
                self.assertEqual(decision["accepted_count"], critical + high)


if __name__ == "__main__":
    unittest.main()
