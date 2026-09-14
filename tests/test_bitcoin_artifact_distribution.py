#!/usr/bin/env python3
"""Exercise real GPG/Ed25519 verification, HTTPS delivery, and bootstrap trust gates."""

from contextlib import redirect_stderr
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import artifact_signing as SIGN
import bitcoin_release as RELEASE
import bitcoin_assumeutxo as BOOT
from common.bitcoin_artifact import UpstreamFixture
from common.snapshot_range_server import SnapshotRangeServer


class ArtifactDistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture_dir = tempfile.TemporaryDirectory(prefix="usdb-artifact-upstream-")
        cls.evidence = Path(cls.fixture_dir.name)
        cls.upstream = UpstreamFixture(cls.evidence)
        cls.archive_bytes = b"test official archive\0" * 1000
        cls.archive_sha = hashlib.sha256(cls.archive_bytes).hexdigest()
        (cls.evidence / RELEASE.CORE_FILE).write_bytes(cls.archive_bytes)
        (cls.evidence / "SHA256SUMS").write_text(f"{cls.archive_sha}  {RELEASE.CORE_FILE}\n")
        cls.upstream.sign_sums()
        cls.original_sums = (cls.evidence / "SHA256SUMS").read_bytes()
        cls.original_signature = (cls.evidence / "SHA256SUMS.asc").read_bytes()

    @classmethod
    def tearDownClass(cls):
        cls.upstream.close()
        cls.fixture_dir.cleanup()

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-artifact-distribution-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name, value in (("CORE_SHA256", self.archive_sha), ("CORE_SIZE", len(self.archive_bytes)), ("KEYS", self.upstream.keys)):
            patch = mock.patch.object(RELEASE, name, value)
            patch.start()
            self.addCleanup(patch.stop)
        self.log = io.StringIO()
        log = redirect_stderr(self.log)
        log.__enter__()
        self.addCleanup(log.__exit__, None, None, None)
        self.secret, self.trust = self.key("core", "bitcoin-core")
        (self.evidence / "SHA256SUMS").write_bytes(self.original_sums)
        (self.evidence / "SHA256SUMS.asc").write_bytes(self.original_signature)

    def key(self, name, purpose):
        directory = self.root / name
        SIGN.keygen(directory, name, purpose)
        return directory / "signing-key.json", directory / "trusted-keys.json"

    def manifest(self):
        path = RELEASE.prepare("bitcoin-core", self.evidence / RELEASE.CORE_FILE, self.evidence,
                               self.secret, self.trust, self.root / "release")
        return path, SIGN.parse_json(path.read_bytes())

    def origin(self):
        origin = SnapshotRangeServer(self.root, self.archive_bytes)
        self.addCleanup(origin.close)
        patch = mock.patch.dict(os.environ, {"SSL_CERT_FILE": str(self.root / "server.crt"), "no_proxy": "127.0.0.1"})
        patch.start()
        self.addCleanup(patch.stop)
        return origin, origin.url.rsplit("/", 1)[0]

    def config(self, value):
        path = self.root / "source.json"
        path.write_bytes(SIGN.canonical(value))
        return path

    def test_upstream_download_checks_all_three_real_signatures(self):
        origin, base = self.origin()
        for name in (RELEASE.CORE_FILE, "SHA256SUMS", "SHA256SUMS.asc", *RELEASE.KEYS):
            origin.files["/" + name] = (self.evidence / name).read_bytes()
        config = self.config(dict(mode="upstream", release_base_url=base, keys_base_url=base))
        archive = RELEASE.fetch_core(config, self.trust, self.root / "build")
        self.assertEqual(archive.read_bytes(), self.archive_bytes)
        provenance = json.loads((archive.parent / "core-artifact-provenance.json").read_text())
        self.assertEqual(provenance["mode"], "upstream")
        self.assertEqual(len(provenance["upstream_verification"]["required_signers"]), 3)

    def test_missing_signer_or_tampered_checksums_cannot_be_signed(self):
        self.upstream.sign_sums(count=2)
        with self.assertRaisesRegex(ValueError, "signatures"):
            self.manifest()
        self.assertFalse((self.root / "release").exists())
        (self.evidence / "SHA256SUMS.asc").write_bytes(self.original_signature)
        (self.evidence / "SHA256SUMS").write_bytes(self.original_sums + b"tampered\n")
        with self.assertRaisesRegex(ValueError, "signatures"):
            self.manifest()

    def test_wrong_key_file_and_wrong_archive_are_rejected_before_signing(self):
        first = next(iter(RELEASE.KEYS))
        original = (self.evidence / first).read_bytes()
        try:
            (self.evidence / first).write_bytes(original + b"x")
            with self.assertRaisesRegex(ValueError, "public key"):
                self.manifest()
        finally:
            (self.evidence / first).write_bytes(original)
        bad = self.root / RELEASE.CORE_FILE
        bad.write_bytes(b"bad")
        with self.assertRaisesRegex(ValueError, "archive identity"):
            RELEASE.prepare("bitcoin-core", bad, self.evidence, self.secret, self.trust, self.root / "release")

    def test_signed_https_source_needs_no_upstream_and_records_provenance(self):
        path, manifest = self.manifest()
        origin, base = self.origin()
        origin.files = {"/" + path.name: path.read_bytes(), "/" + path.name + ".sig": Path(str(path) + ".sig").read_bytes(),
                        "/" + RELEASE.CORE_FILE: self.archive_bytes}
        config = self.config(dict(mode="usdb-signed", manifest_url=base + "/" + path.name))
        with mock.patch.object(RELEASE, "verify_upstream", side_effect=AssertionError("Consumer must not contact upstream")):
            archive = RELEASE.fetch_core(config, self.trust, self.root / "build")
        self.assertEqual(archive.read_bytes(), self.archive_bytes)
        self.assertEqual(origin.paths, ["/" + path.name, "/" + path.name + ".sig", "/" + RELEASE.CORE_FILE])
        provenance = json.loads((archive.parent / "core-artifact-provenance.json").read_text())
        self.assertEqual(provenance["manifest"], manifest)
        self.assertEqual(provenance["mode"], "usdb-signed")

    def test_bad_signature_and_bad_download_never_fall_back(self):
        path, _ = self.manifest()
        origin, base = self.origin()
        origin.files = {"/" + path.name: path.read_bytes(), "/" + path.name + ".sig": b"x" * 64,
                        "/" + RELEASE.CORE_FILE: self.archive_bytes}
        config = self.config(dict(mode="usdb-signed", manifest_url=base + "/" + path.name))
        with self.assertRaisesRegex(ValueError, "Ed25519"):
            RELEASE.fetch_core(config, self.trust, self.root / "build")
        self.assertNotIn("/" + RELEASE.CORE_FILE, origin.paths)
        origin.files["/" + path.name + ".sig"] = Path(str(path) + ".sig").read_bytes()
        origin.files["/" + RELEASE.CORE_FILE] = b"x" * len(self.archive_bytes)
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            RELEASE.fetch_core(config, self.trust, self.root / "build")
        self.assertFalse((self.root / "build" / RELEASE.CORE_FILE).exists())
        self.assertNotIn("/SHA256SUMS", origin.paths)

    def test_manifest_pin_canonical_json_and_duplicate_fields(self):
        path, manifest = self.manifest()
        signature = Path(str(path) + ".sig").read_bytes()
        with self.assertRaisesRegex(ValueError, "canonical"):
            SIGN.verify(json.dumps(manifest, indent=2).encode(), signature, self.trust, "bitcoin-core")
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            SIGN.parse_json(b'{"a":1,"a":2}')
        origin, base = self.origin()
        origin.files["/" + path.name] = path.read_bytes() + b" "
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            SIGN.load_manifest(url=base + "/" + path.name, trust_path=self.trust, artifact_type="bitcoin-core")
        self.assertEqual(len(origin.paths), 1)

    def test_trust_rotation_unknown_signer_and_cross_purpose_are_rejected(self):
        path, manifest = self.manifest()
        content, signature = path.read_bytes(), Path(str(path) + ".sig").read_bytes()
        _, unknown = self.key("unknown", "bitcoin-core")
        with self.assertRaisesRegex(ValueError, "not trusted"):
            SIGN.verify(content, signature, unknown, "bitcoin-core")
        with self.assertRaisesRegex(ValueError, "purpose mismatch"):
            SIGN.verify(content, signature, self.trust, "bitcoin-assumeutxo")
        catalog = json.loads(self.trust.read_text())
        catalog["keys"].extend(json.loads(unknown.read_text())["keys"])
        self.trust.write_bytes(SIGN.canonical(catalog))
        self.assertEqual(SIGN.verify(content, signature, self.trust, "bitcoin-core"), manifest)
        catalog["keys"].pop(0)
        self.trust.write_bytes(SIGN.canonical(catalog))
        with self.assertRaisesRegex(ValueError, "not trusted"):
            SIGN.verify(content, signature, self.trust, "bitcoin-core")

    def test_duplicate_success_notice_never_skips_signature_or_trust_validation(self):
        path, manifest = self.manifest()
        content, signature = path.read_bytes(), Path(str(path) + ".sig").read_bytes()
        # Manifest preparation already verified once in this same process.
        with mock.patch.object(SIGN, "openssl", wraps=SIGN.openssl) as verify:
            for _ in range(2):
                self.assertEqual(SIGN.verify(content, signature, self.trust, "bitcoin-core"), manifest)
            self.assertEqual(verify.call_count, 2)
        self.assertEqual(self.log.getvalue().count("Artifact signature verified:"), 1)
        # A previous success cannot authorize a changed signature or revoked key.
        with self.assertRaises(ValueError):
            SIGN.verify(content, bytes([signature[0] ^ 1]) + signature[1:], self.trust, "bitcoin-core")
        catalog = json.loads(self.trust.read_text())
        catalog["keys"] = []
        self.trust.write_bytes(SIGN.canonical(catalog))
        with self.assertRaises(ValueError):
            SIGN.verify(content, signature, self.trust, "bitcoin-core")

    def test_catalog_rejects_duplicate_ids_material_and_legacy_snapshot_catalog(self):
        catalog = json.loads(self.trust.read_text())
        entry = catalog["keys"][0]
        for second in (dict(entry), dict(entry, key_id="another", artifact_type="bitcoin-assumeutxo")):
            self.trust.write_bytes(SIGN.canonical(dict(catalog, keys=[entry, second])))
            with self.assertRaisesRegex(ValueError, "Duplicate"):
                SIGN.trusted_key(self.trust, "core", "bitcoin-core")
        self.trust.write_bytes(SIGN.canonical(dict(keys=[entry])))
        with self.assertRaisesRegex(ValueError, "fields"):
            SIGN.trusted_key(self.trust, "core", "bitcoin-core")

    def test_signing_key_permissions_mismatch_and_existing_directory(self):
        _, manifest = self.manifest()
        self.secret.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "permissions"):
            SIGN.sign(manifest, self.secret, self.trust)
        self.secret.chmod(0o600)
        secret = json.loads(self.secret.read_text())
        secret["secret_key_base64"] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        self.secret.write_bytes(SIGN.canonical(secret))
        with self.assertRaisesRegex(ValueError, "does not match"):
            SIGN.sign(manifest, self.secret, self.trust)
        with self.assertRaises(FileExistsError):
            SIGN.keygen(self.root / "core", "core", "bitcoin-core")

    def utxo_manifest(self):
        secret, trust = self.key("utxo", "bitcoin-assumeutxo")
        identity = RELEASE.utxo_identity()
        manifest = dict(schema_version=SIGN.SCHEMA, artifact_type="bitcoin-assumeutxo", identity=identity,
                        file=dict(name=RELEASE.UTXO_FILE, sha256=identity["file_sha256"], size_bytes=RELEASE.UTXO_SIZE),
                        upstream_verification=None, signature_scheme="ed25519", signing_key_id="utxo")
        signature = SIGN.sign(manifest, secret, trust)
        path = SIGN.write_release(self.root / "utxo-release", manifest, signature)
        return path, manifest, secret, trust

    def test_signed_utxo_offline_manifest_and_mirror_preserve_compiled_identity(self):
        path, manifest, _, trust = self.utxo_manifest()
        url, provenance = BOOT.resolve_distribution("usdb-signed", "https://mirror.example/snapshot", "", path, trust)
        self.assertEqual(url, "https://mirror.example/snapshot")
        self.assertEqual(provenance["signing_key_id"], "utxo")
        self.assertEqual(BOOT.pinned_snapshot({}).file_sha256, manifest["identity"]["file_sha256"])
        with self.assertRaisesRegex(ValueError, "require"):
            BOOT.resolve_distribution("pinned", url, "", path, trust)

    def test_even_trusted_utxo_signer_cannot_change_checkpoint(self):
        path, original, secret, trust = self.utxo_manifest()
        for field, value in (("base_height", 935001), ("base_hash", "0" * 64), ("file_sha256", "0" * 64), ("hash_serialized_3", "0" * 64)):
            manifest = copy.deepcopy(original)
            manifest["identity"][field] = value
            signature = SIGN.sign(manifest, secret, trust)
            path.write_bytes(SIGN.canonical(manifest))
            Path(str(path) + ".sig").write_bytes(signature)
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "compiled checkpoint"):
                BOOT.resolve_distribution("usdb-signed", "", "", path, trust)

    def test_signed_utxo_cli_rejects_before_rpc_and_download(self):
        path, _, _, trust = self.utxo_manifest()
        Path(str(path) + ".sig").write_bytes(b"z" * 64)
        with mock.patch.object(BOOT, "Rpc", side_effect=AssertionError("RPC reached")), mock.patch.object(BOOT, "download_snapshot", side_effect=AssertionError("Download reached")), mock.patch.object(sys, "argv", [
            "bitcoin_assumeutxo.py", "bootstrap", "--snapshot-file", str(self.root / "snapshot"),
            "--distribution-mode", "usdb-signed", "--manifest-file", str(path), "--trusted-keys", str(trust),
        ]):
            self.assertEqual(BOOT.main(), 1)
        self.assertFalse((self.root / "snapshot").exists())

    def test_utxo_prepare_signed_https_download_and_local_reuse(self):
        secret, trust = self.key("utxo", "bitcoin-assumeutxo")
        source = self.root / RELEASE.UTXO_FILE
        source.write_bytes(self.archive_bytes)
        identity = dict(RELEASE.utxo_identity(), file_sha256=self.archive_sha)
        origin, base = self.origin()
        snapshot = BOOT.Snapshot(identity["base_height"], identity["base_hash"], self.archive_sha, len(self.archive_bytes))
        with mock.patch.object(RELEASE, "utxo_identity", return_value=identity), mock.patch.object(RELEASE, "UTXO_SIZE", len(self.archive_bytes)), mock.patch.object(BOOT, "pinned_snapshot", return_value=snapshot):
            path = RELEASE.prepare("bitcoin-assumeutxo", source, None, secret, trust, self.root / "release")
            origin.files["/" + path.name] = path.read_bytes()
            origin.files["/" + path.name + ".sig"] = Path(str(path) + ".sig").read_bytes()
            target = self.root / "download" / RELEASE.UTXO_FILE
            common = ["bitcoin_assumeutxo.py", "download", "--snapshot-file", str(target), "--distribution-mode", "usdb-signed", "--trusted-keys", str(trust), "--reserve-bytes", "0"]
            with mock.patch.object(sys, "argv", [*common, "--manifest-url", base + "/" + path.name]):
                self.assertEqual(BOOT.main(), 0)
            self.assertEqual(target.read_bytes(), self.archive_bytes)
            requests = list(origin.paths)
            with mock.patch.object(sys, "argv", [*common, "--manifest-file", str(path)]):
                self.assertEqual(BOOT.main(), 0)
            self.assertEqual(origin.paths, requests)

    def test_url_size_and_config_guards(self):
        for url in ("http://example.com/file", "https://user:pass@example.com/file", "https://example.com/file?q=x", "https://example.com/\nfile"):
            with self.assertRaises(ValueError):
                SIGN.https_url(url)
        origin, base = self.origin()
        origin.files["/large"] = b"x" * 65
        with self.assertRaisesRegex(ValueError, "size limit"):
            SIGN.fetch(base + "/large", self.root / "large", limit=64)
        self.assertFalse((self.root / "large").exists())
        with self.assertRaisesRegex(ValueError, "mode"):
            RELEASE.fetch_core(self.config(dict(mode="auto")), self.trust, self.root / "build")
        with self.assertRaisesRegex(ValueError, "fields"):
            RELEASE.fetch_core(self.config(dict(mode="usdb-signed", manifest_url=base, release_base_url=base)), self.trust, self.root / "build")


if __name__ == "__main__":
    unittest.main()
