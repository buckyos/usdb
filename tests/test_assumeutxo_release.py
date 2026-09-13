#!/usr/bin/env python3
"""Verify snapshot-key reuse, immutable release recovery, and public UTXO delivery."""

from contextlib import redirect_stderr, redirect_stdout
import copy
import hashlib
import io
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import artifact_signing as SIGN
import assumeutxo_deployment as DEPLOY
import assumeutxo_release as RELEASE
import bitcoin_assumeutxo as BOOT
import bitcoin_release as BITCOIN
import release_bundle as BUNDLE
import release_manifest as MANIFEST
from common.bitcoin_artifact import PublishedSnapshotStore
from common.native_node import native_kit
from common.snapshot_range_server import SnapshotRangeServer

BASE_BUNDLE = ROOT / "docker/networks/testnet-v0"
WRAPPER = ROOT / "src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"
ORIGIN_HASH = "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551"


class AssumeutxoReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-utxo-release-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.base_bundle = self.root / "base-bundle"
        shutil.copytree(BASE_BUNDLE, self.base_bundle, ignore=shutil.ignore_patterns("release-bootstrap.json", "release-inputs"))
        self.signer = "usdb-mainnet-snapshot-v1"
        key_root = self.root / "keys"
        SIGN.keygen(key_root, self.signer, "bitcoin-assumeutxo")
        self.secret, self.trust = key_root / "signing-key.json", key_root / "trusted-keys.json"
        # These are exactly the legacy Rust snapshot-keygen JSON shapes.
        key = json.loads(self.secret.read_text())
        self.secret.write_bytes(SIGN.canonical({name: key[name] for name in ("key_id", "secret_key_base64")}))
        public = json.loads(self.trust.read_text())["keys"][0]
        self.trust.write_bytes(SIGN.canonical(dict(keys=[{name: public[name] for name in ("key_id", "public_key_base64")}])))
        self.original_key, self.original_trust = self.secret.read_bytes(), self.trust.read_bytes()
        self.payload = b"UTXO snapshot input fixture\0" * 4096
        self.source = self.root / BITCOIN.UTXO_FILE
        self.source.write_bytes(self.payload)
        self.identity = dict(BITCOIN.utxo_identity(), file_sha256=hashlib.sha256(self.payload).hexdigest())
        self.patch(BITCOIN, "utxo_identity", return_value=self.identity)
        self.patch(BITCOIN, "UTXO_SIZE", len(self.payload))
        self.patch(DEPLOY, "UTXO_SIZE", len(self.payload))
        self.release = RELEASE.Release(self.root / "release", self.trust, self.signer)
        self.release.root.mkdir()
        self.log = io.StringIO()
        logger = redirect_stderr(self.log)
        logger.__enter__()
        self.addCleanup(logger.__exit__, None, None, None)

    def patch(self, target, name, *args, **kwargs):
        patcher = mock.patch.object(target, name, *args, **kwargs)
        result = patcher.start()
        self.addCleanup(patcher.stop)
        return result

    def prepare(self):
        return self.release.create(self.source, self.secret)

    def finalize(self):
        self.prepare()
        return self.release.finalize()

    def published(self):
        self.finalize()
        _, base, store = self.origin()
        result = self.release.publish(base, store)
        self.patch(DEPLOY, "checkpoint_metadata", return_value=self.identity)
        import assumeutxo_bootstrap
        self.patch(assumeutxo_bootstrap, "checkpoint_metadata", return_value=self.identity)
        return result

    def origin(self):
        origin = SnapshotRangeServer(self.root, self.payload)
        self.addCleanup(origin.close)
        patcher = mock.patch.dict(os.environ, {"SSL_CERT_FILE": str(self.root / "server.crt"), "no_proxy": "127.0.0.1"})
        patcher.start()
        self.addCleanup(patcher.stop)
        return origin, origin.url.rsplit("/", 1)[0], PublishedSnapshotStore(origin)

    def test_reuses_exact_legacy_key_without_copying_secret_or_large_input(self):
        info = self.finalize()
        self.assertEqual(self.secret.read_bytes(), self.original_key)
        self.assertEqual(self.trust.read_bytes(), self.original_trust)
        catalog = json.loads(Path(info["trusted_keys_file"]).read_text())
        self.assertEqual(catalog["keys"][0]["key_id"], self.signer)
        self.assertEqual(catalog["keys"][0]["public_key_base64"], json.loads(self.original_trust)["keys"][0]["public_key_base64"])
        self.assertEqual(catalog["keys"][0]["artifact_type"], "bitcoin-assumeutxo")
        manifest = SIGN.load_manifest(path=Path(info["manifest_file"]), trust_path=Path(info["trusted_keys_file"]), artifact_type="bitcoin-assumeutxo")
        self.assertEqual(manifest["signing_key_id"], self.signer)
        self.assertEqual(manifest["identity"], self.identity)
        for path in self.release.root.rglob("*"):
            if path.is_file():
                self.assertNotIn(b"secret_key_base64", path.read_bytes())
                self.assertNotEqual(path.name, BITCOIN.UTXO_FILE)

    def test_legacy_adapter_is_explicit_and_cannot_authorize_core(self):
        with self.assertRaisesRegex(ValueError, "fields"):
            SIGN.read_signing_key(self.secret, "bitcoin-assumeutxo")
        with self.assertRaisesRegex(ValueError, "only accepted for UTXO"):
            SIGN.read_signing_key(self.secret, "bitcoin-core", reuse_snapshot_key=True)
        with self.assertRaisesRegex(ValueError, "fields"):
            SIGN.trusted_key(self.trust, self.signer, "bitcoin-assumeutxo")

    def test_new_format_signer_still_works(self):
        secret = json.loads(self.original_key)
        self.secret.write_bytes(SIGN.canonical(dict(secret, schema_version=SIGN.KEY_SCHEMA, artifact_type="bitcoin-assumeutxo")))
        self.trust.write_bytes(SIGN.canonical(SIGN.snapshot_catalog(self.trust, self.signer)))
        self.assertEqual(self.prepare()["phase"], "created")

    def test_wrong_key_permissions_signer_and_untrusted_public_key_fail(self):
        self.secret.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "permissions"):
            self.prepare()
        self.secret.chmod(0o600)
        value = json.loads(self.original_key)
        self.secret.write_bytes(SIGN.canonical(dict(value, key_id="different")))
        with self.assertRaisesRegex(ValueError, "ID differs"):
            self.prepare()
        self.secret.write_bytes(self.original_key)
        catalog = json.loads(self.original_trust)
        catalog["keys"][0]["public_key_base64"] = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        self.trust.write_bytes(SIGN.canonical(catalog))
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.prepare()
        self.assertFalse(self.release.prepared.exists())

    def test_legacy_catalog_rejects_duplicates_and_wrong_key_lengths(self):
        catalog = json.loads(self.original_trust)
        for second in (copy.deepcopy(catalog["keys"][0]), dict(catalog["keys"][0], key_id="second")):
            self.trust.write_bytes(SIGN.canonical(dict(keys=[catalog["keys"][0], second])))
            with self.assertRaisesRegex(ValueError, "Duplicate"):
                self.prepare()
        catalog["keys"][0]["public_key_base64"] = "AA=="
        self.trust.write_bytes(SIGN.canonical(catalog))
        with self.assertRaisesRegex(ValueError, "32 bytes"):
            self.prepare()

    def test_same_key_cannot_reuse_a_signature_from_the_old_manifest_domain(self):
        info = self.prepare()
        manifest = SIGN.parse_json(Path(info["manifest_file"]).read_bytes())
        with mock.patch.object(SIGN, "payload", return_value=b"usdb.balance-history.core-snapshot-manifest-signature:v1\0" + SIGN.canonical(manifest)):
            signature = SIGN.sign(manifest, self.secret, Path(info["trusted_keys_file"]), reuse_snapshot_key=True)
        with self.assertRaisesRegex(ValueError, "Ed25519"):
            SIGN.verify(SIGN.canonical(manifest), signature, Path(info["trusted_keys_file"]), "bitcoin-assumeutxo")

    def test_create_and_finalize_are_idempotent_after_restarting_publisher(self):
        first = self.finalize()
        frozen = self.release.finalized.read_bytes()
        self.release = RELEASE.Release(self.release.root, self.trust, self.signer)
        self.assertEqual(self.prepare(), first)
        self.assertEqual(self.release.finalize(), first)
        self.assertEqual(self.release.finalized.read_bytes(), frozen)
        self.secret.unlink()
        self.assertEqual(self.release.finalize(), first)

    def test_bad_or_replaced_input_fails_before_publishing_metadata(self):
        self.source.write_bytes(b"bad")
        with self.assertRaisesRegex(ValueError, "compiled checkpoint"):
            self.prepare()
        self.assertFalse(self.release.prepared.exists())
        self.source.write_bytes(self.payload)
        self.prepare()
        moved = self.root / "elsewhere" / self.source.name
        moved.parent.mkdir()
        moved.write_bytes(self.payload)
        with self.assertRaisesRegex(ValueError, "original path"):
            self.release.create(moved, self.secret)
        self.source.write_bytes(b"X" * len(self.payload))
        with self.assertRaisesRegex(ValueError, "signed file"):
            self.release.finalize()
        self.assertFalse(self.release.finalized.exists())

    def test_create_can_resume_a_download_without_a_bitcoin_node(self):
        self.source.unlink()
        origin, base, _ = self.origin()
        self.patch(BOOT, "pinned_snapshot", return_value=BOOT.Snapshot(935000, self.identity["base_hash"], self.identity["file_sha256"], len(self.payload)))
        info = self.release.create(self.source, self.secret, origin.url)
        self.assertEqual(Path(info["snapshot_file"]).read_bytes(), self.payload)
        self.assertTrue(origin.requests)

    def test_interrupted_metadata_creation_retries_without_overwriting_input(self):
        with mock.patch.object(RELEASE.os, "rename", side_effect=OSError("Injected interruption")):
            with self.assertRaises(OSError):
                self.prepare()
        self.assertFalse(self.release.prepared.exists())
        self.assertEqual(self.source.read_bytes(), self.payload)
        self.assertEqual(self.prepare()["phase"], "created")

    def test_tampered_signature_catalog_and_finalization_are_rejected(self):
        info = self.finalize()
        signature = Path(info["manifest_file"] + ".sig")
        original = signature.read_bytes()
        signature.write_bytes(b"X" * 64)
        with self.assertRaisesRegex(ValueError, "Ed25519"):
            self.release.finalize()
        signature.write_bytes(original)
        catalog = Path(info["trusted_keys_file"])
        original_catalog = catalog.read_bytes()
        catalog.write_bytes(SIGN.canonical(dict(schema_version=SIGN.TRUST_SCHEMA, keys=[])))
        with self.assertRaisesRegex(ValueError, "operator trust"):
            self.release.finalize()
        catalog.write_bytes(original_catalog)
        value = json.loads(self.release.finalized.read_text())
        value["manifest_sha256"] = "a" * 64
        self.release.finalized.write_bytes(SIGN.canonical(value))
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            self.release.finalize()

    def test_publication_requires_finalization_and_finishes_record_last(self):
        self.prepare()
        origin, base, store = self.origin()
        with self.assertRaises(ValueError):
            self.release.publish(base, store)
        self.assertFalse(store.uploads)
        self.release.finalize()
        result = self.release.publish(base, store)
        self.assertEqual(result["status"], "published")
        self.assertTrue(store.uploads[-1].startswith("snapshot-records/assumeutxo/v1/"))
        self.assertEqual(len(store.uploads), 6)
        self.assertEqual(len(origin.paths), 6)
        record = json.loads(Path(result["record_file"]).read_text())
        self.assertEqual(record["schema_version"], RELEASE.RECORD_SCHEMA)
        self.assertNotIn(str(self.root), Path(result["record_file"]).read_text())
        before = store.uploads[:]
        second = self.release.publish(base, store)
        self.assertEqual(second["record_sha256"], result["record_sha256"])
        self.assertTrue(all(status == "existing" for status in second["objects"].values()))
        self.assertEqual(store.uploads, before)
        self.assertEqual(self.release.verify_published(base)["status"], "public_verified")

    def test_public_failure_does_not_publish_record_and_retry_checks_existing_objects(self):
        self.finalize()
        origin, base, store = self.origin()
        store.after_upload = lambda path, key: origin.files.__setitem__("/" + key, b"X" * len(self.payload)) if path == self.source else None
        with self.assertRaisesRegex(ValueError, "Public artifact SHA"):
            self.release.publish(base, store)
        self.assertEqual(len(store.uploads), 1)
        self.assertFalse((self.release.root / "publish-result.json").exists())
        origin.files["/" + store.uploads[0]] = self.payload
        store.after_upload = None
        result = self.release.publish(base, store)
        self.assertEqual(result["objects"]["snapshot"], "existing")
        self.assertEqual(result["status"], "published")

    def test_origin_client_policy_and_retry_preserve_uploaded_snapshot(self):
        self.finalize()
        origin, base, store = self.origin()
        # Reproduce the public origin rejecting Python's default client identity.
        origin.required_user_agent = "usdb-snapshot-verifier/1"
        with mock.patch.object(SIGN, "HTTP_USER_AGENT", "Python-urllib/3.11"):
            with self.assertRaisesRegex(ValueError, "http_status=403") as failure:
                self.release.publish(base, store)
        self.assertIn(base, str(failure.exception))
        self.assertIn(self.source.name, str(failure.exception))
        self.assertEqual(len(store.uploads), 1)
        self.assertFalse((self.release.root / "publish-result.json").exists())

        result = self.release.publish(base, store)
        self.assertEqual(result["status"], "published")
        self.assertEqual(result["objects"]["snapshot"], "existing")
        self.assertEqual(len(store.uploads), 6)
        self.assertTrue(store.uploads[-1].startswith("snapshot-records/assumeutxo/v1/"))
        self.assertEqual(self.release.verify_published(base)["status"], "public_verified")
        # Node-side manifest and signature downloads use the same public origin.
        manifest = SIGN.load_manifest(url=result["manifest_url"], trust_path=Path(result["trusted_keys_file"]),
                                      artifact_type="bitcoin-assumeutxo")
        self.assertEqual(manifest["identity"], self.identity)

    def test_upload_interruption_resumes_and_never_overwrites_conflicting_object(self):
        info = self.finalize()
        _, base, store = self.origin()
        store.fail_role = Path(info["manifest_file"]).name
        with self.assertRaisesRegex(OSError, "interruption"):
            self.release.publish(base, store)
        self.assertEqual(len(store.uploads), 1)
        store.fail_role = None
        key = store.uploads[0]
        store.metadata[key]["ContentLength"] += 1
        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            self.release.publish(base, store)
        store.metadata[key]["ContentLength"] -= 1
        self.assertEqual(self.release.publish(base, store)["status"], "published")

    def test_input_change_during_upload_and_revoked_trust_block_release(self):
        self.finalize()
        _, base, store = self.origin()
        store.after_upload = lambda path, key: self.source.write_bytes(b"changed") if path == self.source else None
        with self.assertRaisesRegex(ValueError, "changed during upload"):
            self.release.publish(base, store)
        self.assertEqual(len(store.uploads), 1)
        self.trust.write_bytes(SIGN.canonical(dict(keys=[])))
        with self.assertRaisesRegex(ValueError, "not trusted"):
            self.release.publish(base, store)
        self.assertEqual(len(store.uploads), 1)

    def test_published_manifest_and_catalog_feed_the_existing_native_bundle(self):
        self.finalize()
        _, base, store = self.origin()
        result = self.release.publish(base, store)
        self.patch(DEPLOY, "checkpoint_metadata", return_value=self.identity)
        # The service-side environment validator independently uses the same catalog.
        import assumeutxo_bootstrap
        self.patch(assumeutxo_bootstrap, "checkpoint_metadata", return_value=self.identity)
        bundle = DEPLOY.prepare_bundle(self.base_bundle, self.root / "bundle",
                "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551", result["source_url"],
                Path(result["manifest_file"]), Path(result["trusted_keys_file"]))
        network = json.loads((bundle / "network.json").read_text())
        contract = DEPLOY.load_contract(bundle, network)
        self.assertEqual(contract["distribution"]["mode"], "usdb-signed")
        url, _ = BOOT.resolve_distribution("usdb-signed", result["source_url"], "",
                bundle / contract["distribution"]["manifest"]["path"], bundle / contract["distribution"]["trusted_keys"]["path"])
        self.assertEqual(url, result["source_url"])

    def test_deploy_cli_consumes_publication_without_private_key_file_scan_or_network(self):
        published = self.published()
        self.secret.unlink()
        self.source.unlink()
        self.patch(SIGN, "file_identity", side_effect=AssertionError("Deploy must not scan the large input"))
        self.patch(RELEASE, "verify_public_file", side_effect=AssertionError("Deploy uses the saved publication result"))
        self.patch(RELEASE.distribution, "AwsCliClient", side_effect=AssertionError("Deploy must not upload"))
        base = self.base_bundle
        before = (base / "network.json").read_bytes()
        output = self.root / "bundle"
        argv = ["assumeutxo_release.py", "deploy", "--root-dir", str(self.release.root),
                "--signing-key", str(self.secret), "--trusted-keys", str(self.trust), "--signer-id", self.signer,
                "--source-bundle", str(base), "--output-dir", str(output), "--origin-block-hash", ORIGIN_HASH,
                "--public-base-url", "https://changed.example.org", "--source-url", "https://wrong.example.org/input.dat"]
        stdout = io.StringIO()
        with mock.patch.object(sys, "argv", argv), redirect_stdout(stdout):
            self.assertEqual(RELEASE.main(), 0, self.log.getvalue())
        result = json.loads(stdout.getvalue())
        self.assertEqual(result["status"], "deployment_integrated")
        self.assertEqual(result["bundle_dir"], str(output))
        self.assertEqual(result["source_url"], published["source_url"])
        self.assertEqual(result["snapshot_record_sha256"], published["record_sha256"])
        self.assertEqual(result["origin_height"], 963800)
        contract = DEPLOY.load_contract(output, json.loads((output / "network.json").read_text()))
        self.assertEqual(contract["distribution"]["source_url"], published["source_url"])
        self.assertEqual(contract["distribution"]["mode"], "usdb-signed")
        self.assertEqual((base / "network.json").read_bytes(), before)
        for path in output.rglob("*"):
            if path.is_file():
                self.assertNotIn(b"secret_key_base64", path.read_bytes())
                self.assertNotEqual(path.name, BITCOIN.UTXO_FILE)

    def test_deploy_rejects_missing_or_incomplete_publication(self):
        self.finalize()
        self.release.prepare_release("https://downloads.example.org")
        output = self.root / "bundle"
        with self.assertRaisesRegex(ValueError, "publish successfully"):
            self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        self.assertFalse(output.exists())
        published = self.published()
        for change in (dict(status="public_verified"), dict(objects={"snapshot": "uploaded"}),
                       dict(public_verified_at_utc="2026-09-13T00:00:00")):
            with self.subTest(change=change):
                BOOT.save_json(self.release.root / "publish-result.json", dict(published, **change))
                with self.assertRaises(ValueError):
                    self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
                self.assertFalse(output.exists())

    def test_deploy_rejects_redirected_receipt_and_modified_record(self):
        published = self.published()
        receipt = self.release.root / "publish-result.json"
        output = self.root / "bundle"
        for key in ("source_url", "manifest_file", "trusted_keys_file", "record_file", "record_url"):
            with self.subTest(key=key):
                BOOT.save_json(receipt, dict(published, **{key: "https://wrong.example.org/redirect"}))
                with self.assertRaisesRegex(ValueError, "differs from verified record"):
                    self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
                self.assertFalse(output.exists())
        BOOT.save_json(receipt, published)
        Path(published["record_file"]).write_bytes(b"{}\n")
        with self.assertRaisesRegex(ValueError, "record digest"):
            self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        self.assertFalse(output.exists())

    def test_deploy_rechecks_signature_and_operator_trust(self):
        published = self.published()
        output = self.root / "bundle"
        signature = Path(published["manifest_file"] + ".sig")
        original = signature.read_bytes()
        signature.write_bytes(b"X" * 64)
        with self.assertRaisesRegex(ValueError, "Ed25519"):
            self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        signature.write_bytes(original)
        self.trust.write_bytes(SIGN.canonical(dict(keys=[])))
        with self.assertRaisesRegex(ValueError, "not trusted"):
            self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        self.assertFalse(output.exists())

    def test_deploy_preserves_existing_bundle_and_rejects_invalid_origin(self):
        self.published()
        base = self.base_bundle
        output = self.root / "bundle"
        with self.assertRaises(ValueError):
            self.release.deploy(base, output, "invalid")
        self.assertFalse(output.exists())
        self.release.deploy(base, output, ORIGIN_HASH)
        before = {str(path.relative_to(output)): path.read_bytes() for path in output.rglob("*") if path.is_file()}
        self.assertEqual(self.release.deploy(base, output, ORIGIN_HASH)["status"], "deployment_integrated")
        self.assertEqual({str(path.relative_to(output)): path.read_bytes() for path in output.rglob("*") if path.is_file()}, before)
        with self.assertRaisesRegex(ValueError, "outside the source"):
            self.release.deploy(base, base / "unexpected-output", ORIGIN_HASH)

    def test_deploy_reuses_old_export_and_reconstructs_candidate_publish_and_node_kit(self):
        published = self.published()
        output = self.root / "export"
        DEPLOY.prepare_bundle(self.base_bundle, output, ORIGIN_HASH, published["source_url"],
                              Path(published["manifest_file"]), Path(published["trusted_keys_file"]))
        self.assertFalse((output / DEPLOY.PUBLICATION_PATH).exists())
        integrated = self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        self.assertEqual(integrated["status"], "deployment_integrated")
        self.assertTrue((output / DEPLOY.PUBLICATION_PATH).is_file())
        selection = self.base_bundle / BUNDLE.INPUT
        self.assertEqual(str(selection), integrated["release_input_file"])
        self.assertNotIn(str(self.root), selection.read_text())
        # Simulate CI using a source checkout with no publisher workspace or credentials.
        checkout = self.root / "checkout"
        shutil.copytree(self.base_bundle, checkout)
        shutil.rmtree(self.release.root)
        self.source.unlink()
        self.secret.unlink()
        candidate = BUNDLE.prepare(checkout, self.root / "candidate")
        publish = BUNDLE.prepare(checkout, self.root / "publish")
        for path in candidate.rglob("*"):
            if path.is_file():
                self.assertEqual(path.read_bytes(), (publish / path.relative_to(candidate)).read_bytes())
        state = MANIFEST.build_snapshot_state(publish)
        self.assertEqual(state["status"], "native")
        self.assertEqual(state["record"]["sha256"], published["record_sha256"])
        self.assertEqual(state["record"]["url"], published["record_url"])
        layout = native_kit(self.root, bundle=publish)
        self.assertEqual(layout.snapshot["status"], "native")
        self.assertEqual(layout.snapshot["record"], state["record"])
        self.assertEqual(MANIFEST.build_network_identity(layout.bundle_dir), MANIFEST.build_network_identity(candidate))

    def test_release_selection_and_prepare_only_preserve_legacy_behavior(self):
        self.assertEqual(BUNDLE.prepare(self.base_bundle, self.root / "legacy"), self.base_bundle)
        self.assertFalse((self.root / "legacy").exists())
        self.published()
        result = self.release.deploy(self.base_bundle, self.root / "export", ORIGIN_HASH, prepare_only=True)
        self.assertEqual(result["status"], "deployment_prepared")
        self.assertFalse((self.base_bundle / BUNDLE.INPUT).exists())

    def test_broken_release_input_never_falls_back_to_legacy(self):
        self.published()
        self.release.deploy(self.base_bundle, self.root / "export", ORIGIN_HASH)
        selection = self.base_bundle / BUNDLE.INPUT
        original = selection.read_bytes()
        value = json.loads(original)
        for role in BUNDLE.ROLES:
            changed = copy.deepcopy(value)
            changed["files"][role]["sha256"] = "0" * 64
            selection.write_bytes(SIGN.canonical(changed))
            with self.subTest(role=role), self.assertRaisesRegex(ValueError, "input digest"):
                BUNDLE.prepare(self.base_bundle, self.root / "bad")
            self.assertFalse((self.root / "bad").exists())
        selection.write_bytes(original)
        network = self.base_bundle / "network.json"
        network.write_bytes(network.read_bytes() + b"\n")
        with self.assertRaisesRegex(ValueError, "Base network changed"):
            BUNDLE.prepare(self.base_bundle, self.root / "bad")

    def test_native_public_verification_uses_record_metadata_and_one_byte_range(self):
        self.finalize()
        origin, base, store = self.origin()
        published = self.release.publish(base, store)
        self.patch(DEPLOY, "checkpoint_metadata", return_value=self.identity)
        import assumeutxo_bootstrap
        self.patch(assumeutxo_bootstrap, "checkpoint_metadata", return_value=self.identity)
        output = self.root / "bundle"
        self.release.deploy(self.base_bundle, output, ORIGIN_HASH)
        self.patch(BUNDLE.distribution, "verify_public_release", side_effect=AssertionError("Native verification must not use old BH records"))
        before = len(origin.paths)
        result = BUNDLE.verify_public(output)
        self.assertEqual(result["record_url"], published["record_url"])
        self.assertEqual(len(origin.paths) - before, 6)
        self.assertEqual(origin.requests[-1], (0, 0))
        manifest_path = "/" + self.release.record(base, scan=False)[0]["files"]["manifest"]["object_key"]
        original = origin.files[manifest_path]
        origin.files[manifest_path] = b"X" * len(original)
        with self.assertRaisesRegex(ValueError, "metadata mismatch"):
            BUNDLE.verify_public(output)

    def wrapper(self, *args, **env):
        variables = dict(os.environ, USDB_ROOT=str(self.root / "usdb"), SNAPSHOT_ROOT=str(self.root / "wrapper"),
                         SNAPSHOT_KEY_ROOT=str(self.secret.parent), SNAPSHOT_SIGNER_ID=self.signer,
                         BITCOIN_BIN_DIR="/nonexistent/bitcoin", BITCOIN_DATA_DIR="/nonexistent/bitcoin", **env)
        return subprocess.run(["bash", str(WRAPPER), *args], capture_output=True, text=True, env=variables)

    def test_wrapper_selects_utxo_without_node_build_or_keygen(self):
        result = self.wrapper("paths", "--snapshot-type", "assumeutxo", SNAPSHOT_AWS_PROFILE="existing-publisher",
                              SNAPSHOT_S3_BUCKET="existing-bucket", SNAPSHOT_PUBLIC_BASE_URL="https://downloads.example.org")
        self.assertEqual(result.returncode, 0, result.stderr)
        info = json.loads(result.stdout)
        self.assertEqual(info["bucket"], "existing-bucket")
        self.assertEqual(info["aws_profile"], "existing-publisher")
        self.assertEqual(info["signing_key"], str(self.secret.parent / (self.signer + ".signing-key.json")))
        self.assertFalse((self.root / "wrapper").exists())
        status = self.wrapper("status", SNAPSHOT_TYPE="assumeutxo")
        self.assertEqual(json.loads(status.stdout)["phase"], "not_created")
        invalid = self.wrapper("create", "--snapshot-type", "assumeutxo", "--height", "963800")
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("compiled base height", invalid.stderr)
        self.assertFalse((self.root / "wrapper").exists())
        invalid_type = self.wrapper("create", "--snapshot-type", "typo")
        self.assertNotEqual(invalid_type.returncode, 0)
        invalid_deploy = self.wrapper("deploy", "--snapshot-type", "assumeutxo")
        self.assertNotEqual(invalid_deploy.returncode, 0)
        self.assertIn("deploy requires --source-bundle", invalid_deploy.stderr)
        missing_publication = self.wrapper("deploy", "--snapshot-type", "assumeutxo",
                "--source-bundle", str(self.base_bundle),
                "--output-dir", str(self.root / "wrapper-bundle"), "--origin-block-hash", ORIGIN_HASH)
        self.assertNotEqual(missing_publication.returncode, 0)
        self.assertIn("publish successfully", missing_publication.stderr)
        self.assertFalse((self.root / "wrapper-bundle").exists())

    def test_lock_and_symlink_prevent_concurrent_or_redirected_publication(self):
        with BOOT.exclusive_directory(self.release.root):
            with self.assertRaisesRegex(ValueError, "owns this directory"):
                with BOOT.exclusive_directory(self.release.root):
                    self.fail("Concurrent lock succeeded")
        self.prepare()
        final = self.release.finalized
        final.symlink_to(self.root / "outside.json")
        with self.assertRaisesRegex(ValueError, "regular file"):
            self.release.finalize()
        self.assertFalse((self.root / "outside.json").exists())


if __name__ == "__main__":
    unittest.main()
