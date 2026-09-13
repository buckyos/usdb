#!/usr/bin/env python3
"""Publish pinned UTXO files through the existing snapshot signer and S3 lifecycle."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

import artifact_signing as signing
import bitcoin_assumeutxo as bootstrap
import bitcoin_release as bitcoin
import snapshot_distribution as distribution

PREPARED_SCHEMA = "usdb-assumeutxo-prepared:v1"
FINALIZED_SCHEMA = "usdb-assumeutxo-finalized:v1"
RECORD_SCHEMA = "usdb-assumeutxo-release-record:v1"
TRUST_FILE = "bitcoin-artifacts.trusted-keys.json"
PURPOSE = "bitcoin-assumeutxo"


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def code_identity() -> dict:
    """Retain source hashes as well as HEAD so uncommitted publisher code is identifiable."""
    directory = Path(__file__).resolve().parent
    result = subprocess.run(["git", "-C", str(directory), "rev-parse", "HEAD"], capture_output=True, text=True)
    revision = result.stdout.strip() if result.returncode == 0 else None
    files = [Path(__file__), directory / "artifact_signing.py", directory / "bitcoin_release.py",
             directory / "bitcoin_assumeutxo.py", directory / "snapshot_distribution.py"]
    wrapper = directory.parents[2] / "src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"
    if wrapper.is_file():
        files.append(wrapper)
    return dict(revision=revision, tools_sha256={path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in files})


def immutable(path: Path, content: bytes) -> None:
    """Publish a complete small file atomically, accepting only identical retries."""
    bootstrap.regular_file(path)
    if path.exists():
        signing.require(signing.read_small(path) == content, f"Immutable release file differs: {path}")
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent) as output:
        output.write(content)
        output.flush()
        os.fsync(output.fileno())
        os.chmod(output.name, 0o644)
        os.link(output.name, path)
    bootstrap.sync_directory(path.parent)


def stamp(path: Path) -> tuple:
    signing.regular(path)
    value = path.stat()
    return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns


def verify_public_file(url: str, item: dict) -> None:
    """Stream anonymous HTTPS bytes into a digest without retaining a second large snapshot."""
    signing.https_url(url)
    started = last = time.monotonic()
    size, digest = 0, hashlib.sha256()
    print(f"Public verification started: file={item['name']}, bytes={item['size_bytes']}, url={url}", file=sys.stderr, flush=True)
    opener = urllib.request.build_opener(signing.HttpsRedirect())
    request = urllib.request.Request(url, headers={"Accept-Encoding": "identity", "User-Agent": signing.HTTP_USER_AGENT})
    try:
        response = opener.open(request, timeout=30)
    except urllib.error.HTTPError as error:
        error.close()
        raise ValueError(f"Public artifact request failed: file={item['name']}, url={url}, http_status={error.code}; "
                         "check the public download origin access policy") from error
    with response:
        signing.https_url(response.url)
        signing.require(response.status == 200 and response.headers.get("Content-Encoding", "identity") == "identity",
                        "Public artifact returned an unexpected status or encoding")
        lengths = response.headers.get_all("Content-Length", [])
        signing.require(not lengths or lengths == [str(item["size_bytes"])], "Public artifact length mismatch")
        while chunk := response.read(min(4 * 1024 * 1024, item["size_bytes"] + 1 - size)):
            size += len(chunk)
            signing.require(size <= item["size_bytes"], "Public artifact exceeds its pinned size")
            digest.update(chunk)
            if time.monotonic() - last >= 10:
                print(f"Public verification progress: bytes={size}, total={item['size_bytes']}", file=sys.stderr, flush=True)
                last = time.monotonic()
    signing.require(size == item["size_bytes"] and digest.hexdigest() == item["sha256"], "Public artifact SHA-256/size mismatch")
    print(f"Public verification finished: file={item['name']}, bytes={size}, elapsed_seconds={time.monotonic() - started:.1f}",
          file=sys.stderr, flush=True)


class Release:
    """Keep large input in place; atomically freeze only public metadata and local provenance."""

    def __init__(self, root: Path, trust: Path, signer: str):
        self.root = root.expanduser().absolute()
        self.trust = trust.expanduser().absolute()
        self.signer = signing.key_id(signer)
        self.prepared = self.root / "prepared"
        self.finalized = self.root / "artifact-finalized.json"

    def catalog(self) -> bytes:
        return signing.canonical(signing.snapshot_catalog(self.trust, self.signer))

    def load(self, *, scan: bool = True, source: Path | None = None) -> tuple[dict, dict, Path]:
        """Always anchor packaged public keys to the separately supplied operator catalog."""
        signing.require(self.prepared.is_dir() and not self.prepared.is_symlink(), "Run create before this operation")
        local = signing.parse_json(signing.read_small(self.prepared / "prepared.json"))
        signing.exact(local, {"schema_version", "source_file", "manifest", "producer", "created_at_utc"}, "prepared UTXO release")
        signing.require(local["schema_version"] == PREPARED_SCHEMA, "Prepared UTXO schema mismatch")
        signing.require(isinstance(local["manifest"], str) and re.fullmatch(r"[0-9a-f]{64}\.json", local["manifest"]) is not None,
                        "Invalid prepared manifest name")
        signing.exact(local["producer"], {"revision", "tools_sha256"}, "publisher identity")
        signing.require(local["producer"]["revision"] is None or re.fullmatch(r"[0-9a-f]{40}", local["producer"]["revision"]) is not None,
                        "Invalid publisher revision")
        signing.require(isinstance(local["producer"]["tools_sha256"], dict), "Invalid publisher source hashes")
        for name, digest in local["producer"]["tools_sha256"].items():
            signing.require(re.fullmatch(r"[a-z_]+\.(py|sh)", name) is not None, "Invalid publisher source name")
            signing.digest(digest)
        signing.require(signing.read_small(self.prepared / TRUST_FILE) == self.catalog(), "Prepared public catalog differs from operator trust")
        manifest_path = self.prepared / local["manifest"]
        signing.require(hashlib.sha256(signing.read_small(manifest_path)).hexdigest() == manifest_path.stem,
                        "Prepared manifest filename hash mismatch")
        manifest = signing.load_manifest(path=manifest_path, trust_path=self.prepared / TRUST_FILE, artifact_type=PURPOSE)
        bitcoin.validate_utxo(manifest)
        signing.require(manifest["signing_key_id"] == self.signer, "Prepared signer differs from selected snapshot signer")
        path = Path(local["source_file"])
        signing.require(path.is_absolute() and path.name == bitcoin.UTXO_FILE, "Prepared UTXO source path is invalid")
        if source is not None:
            signing.require(path == source.expanduser().absolute(), "Prepared UTXO source differs; use the original path")
        if scan:
            signing.require(signing.file_identity(path) == manifest["file"], "UTXO source differs from signed file")
        return local, manifest, path

    def create(self, source: Path | None, secret: Path, url: str = "") -> dict:
        """Prepare once or verify an existing candidate using the original key and input."""
        if url:
            signing.https_url(url)
        catalog = self.catalog()
        key = signing.read_signing_key(secret, PURPOSE, reuse_snapshot_key=True)
        signing.require(key["key_id"] == self.signer, "Snapshot signing key ID differs from selected signer")
        signing.require_key_matches(key, signing.parse_json(catalog))
        if self.prepared.exists() or self.prepared.is_symlink():
            local, manifest, _ = self.load(source=source)
            signature = signing.sign(manifest, secret, self.prepared / TRUST_FILE, reuse_snapshot_key=True)
            signing.require(signature == signing.read_small(self.prepared / (local["manifest"] + ".sig")), "Existing release signature differs")
            return self.describe()
        source = source.expanduser().absolute() if source is not None else self.root / "download" / bitcoin.UTXO_FILE
        signing.require(source.name == bitcoin.UTXO_FILE, "UTXO input must retain its pinned filename")
        if not source.exists():
            signing.require(bool(url), "Provide an existing --snapshot-file or an HTTPS --source-url")
            bootstrap.download_snapshot(bootstrap.pinned_snapshot({}), source, url)
        with tempfile.TemporaryDirectory(dir=self.root, prefix=".prepare-") as temporary:
            stage = Path(temporary) / "public"
            stage.mkdir(mode=0o755)
            immutable(stage / TRUST_FILE, catalog)
            manifest = bitcoin.prepare(PURPOSE, source, None, secret, stage / TRUST_FILE, stage, reuse_snapshot_key=True)
            local = dict(schema_version=PREPARED_SCHEMA, source_file=str(source), manifest=manifest.name,
                         producer=code_identity(), created_at_utc=now())
            immutable(stage / "prepared.json", signing.canonical(local))
            for path in stage.iterdir():
                path.chmod(0o644)
                with path.open("rb") as stream:
                    os.fsync(stream.fileno())
            bootstrap.sync_directory(stage)
            os.rename(stage, self.prepared)
            bootstrap.sync_directory(self.root)
        return self.describe()

    def final_identity(self, local: dict, manifest: dict) -> dict:
        return dict(schema_version=FINALIZED_SCHEMA, snapshot=manifest["identity"], signing_key_id=self.signer,
                    manifest_sha256=local["manifest"][:-5],
                    signature_sha256=hashlib.sha256(signing.read_small(self.prepared / (local["manifest"] + ".sig"))).hexdigest(),
                    trusted_keys_sha256=hashlib.sha256(self.catalog()).hexdigest(), producer=local["producer"])

    def load_finalized(self, local: dict, manifest: dict) -> dict:
        expected = self.final_identity(local, manifest)
        value = signing.parse_json(signing.read_small(self.finalized))
        signing.exact(value, {*expected, "finalized_at_utc", "finalizer"}, "UTXO finalization")
        signing.require({key: value[key] for key in expected} == expected, "UTXO finalization identity mismatch")
        signing.require(signing.read_small(self.finalized) == signing.canonical(value), "UTXO finalization must be canonical")
        return value

    def finalize(self, source: Path | None = None) -> dict:
        local, manifest, _ = self.load(source=source)
        if self.finalized.exists() or self.finalized.is_symlink():
            self.load_finalized(local, manifest)
        else:
            value = dict(self.final_identity(local, manifest), finalized_at_utc=now(), finalizer=code_identity())
            immutable(self.finalized, signing.canonical(value))
        return self.describe()

    def describe(self) -> dict:
        local, manifest, path = self.load(scan=False)
        finalized = self.finalized.exists() or self.finalized.is_symlink()
        if finalized:
            self.load_finalized(local, manifest)
        return dict(phase="finalized" if finalized else "created", snapshot=manifest["identity"], signer_id=self.signer,
                    snapshot_file=str(path), manifest_file=str(self.prepared / local["manifest"]),
                    trusted_keys_file=str(self.prepared / TRUST_FILE), finalization_file=str(self.finalized),
                    note="Metadata verified; this status does not scan the UTXO file or probe publication")

    def record(self, public_base: str, *, scan: bool = True, source: Path | None = None) -> tuple[dict, dict[str, Path]]:
        """Build only the UTXO inventory; the existing split DB record contract stays intact."""
        base = signing.https_url(public_base.rstrip("/"))
        local, manifest, path = self.load(scan=scan, source=source)
        self.load_finalized(local, manifest)
        finalized_sha = hashlib.sha256(signing.read_small(self.finalized)).hexdigest()
        prefix = f"bitcoin/utxo/{manifest['identity']['base_height']}/{finalized_sha}"
        sources = dict(snapshot=path, manifest=self.prepared / local["manifest"],
                       signature=self.prepared / (local["manifest"] + ".sig"),
                       trusted_keys=self.prepared / TRUST_FILE, finalization=self.finalized)
        files = {}
        for role, file in sources.items():
            item = manifest["file"] if role == "snapshot" else dict(name=file.name, size_bytes=file.stat().st_size,
                                                                    sha256=hashlib.sha256(signing.read_small(file)).hexdigest())
            files[role] = dict(item, object_key=f"{prefix}/{file.name}")
        return dict(schema_version=RECORD_SCHEMA, artifact_type=PURPOSE, snapshot=manifest["identity"],
                    signing_key_id=self.signer, public_base_url=base, files=files), sources

    def prepare_release(self, base: str, source: Path | None = None) -> dict:
        record, _ = self.record(base, source=source)
        return self.save_record(record)

    def save_record(self, record: dict) -> dict:
        result = self.record_result(record)
        immutable(Path(result["record_file"]), signing.canonical(record))
        return result

    def record_result(self, record: dict) -> dict:
        """Derive public URLs and local paths from the verified record, never operator overrides."""
        content = signing.canonical(record)
        digest = hashlib.sha256(content).hexdigest()
        path = self.root / "records" / (digest + ".json")
        return dict(record_file=str(path), record_sha256=digest,
                    record_url=f"{record['public_base_url']}/snapshot-records/assumeutxo/v1/{digest}.json",
                    source_url=f"{record['public_base_url']}/{record['files']['snapshot']['object_key']}",
                    manifest_url=f"{record['public_base_url']}/{record['files']['manifest']['object_key']}",
                    manifest_file=str(self.prepared / record["files"]["manifest"]["name"]),
                    trusted_keys_file=str(self.prepared / TRUST_FILE))

    def published_result(self) -> dict:
        """Recheck the saved publication receipt against the signed, finalized local inventory."""
        receipt = self.root / "publish-result.json"
        signing.require(receipt.is_file(), "Run publish successfully before deploy")
        result = signing.parse_json(signing.read_small(receipt))
        signing.require(result.get("status") == "published", "Deploy requires a completed publication")
        digest = signing.digest(result.get("record_sha256"))
        # Do not follow paths or use URLs supplied by an unchecked receipt.
        content = signing.read_small(self.root / "records" / (digest + ".json"))
        signing.require(hashlib.sha256(content).hexdigest() == digest, "Published record digest mismatch")
        record = signing.parse_json(content)
        expected, _ = self.record(record["public_base_url"], scan=False)
        signing.require(content == signing.canonical(expected), "Published record differs from finalized release")
        inputs = self.record_result(expected)
        signing.exact(result, {*inputs, "status", "public_verified_at_utc", "objects"}, "publication result")
        signing.require(all(result[key] == value for key, value in inputs.items()), "Publication result differs from verified record")
        signing.exact(result["objects"], {*expected["files"], "record"}, "published objects")
        signing.require(all(value in ("uploaded", "existing") for value in result["objects"].values()), "Publication has incomplete objects")
        verified_at = datetime.fromisoformat(result["public_verified_at_utc"])
        signing.require(verified_at.tzinfo is not None, "Publication verification time must include a timezone")
        return result

    def deploy(self, source: Path, output: Path, origin_hash: str, *, prepare_only: bool = False) -> dict:
        """Prepare and register reproducible release inputs; never contact or start a node."""
        from assumeutxo_deployment import prepare_bundle, PUBLICATION_ARTIFACT, PUBLICATION_PATH
        from release_bundle import register, write_input
        from validate_network_bundle import validate_network_bundle
        source, output = source.expanduser().absolute(), output.expanduser().absolute()
        signing.require(not output.is_symlink(), "Deployment output must not be a symlink")
        signing.require(not output.resolve().is_relative_to(source.resolve()), "Deployment output must be outside the source bundle")
        published = self.published_result()
        print(f"Deployment preparation started: source_bundle={source}, output_dir={output}, "
              f"record_sha256={published['record_sha256']}, origin_block_hash={origin_hash}", file=sys.stderr, flush=True)
        arguments = (origin_hash, published["source_url"], Path(published["manifest_file"]), Path(published["trusted_keys_file"]))
        publication = Path(published["record_file"])
        if output.exists():
            validate_network_bundle(output)
            with tempfile.TemporaryDirectory(prefix="usdb-deploy-compare-") as temporary:
                expected = prepare_bundle(source, Path(temporary) / "bundle", *arguments, publication)
                current_files = {str(path.relative_to(output)) for path in output.rglob("*") if path.is_file()}
                expected_files = {str(path.relative_to(expected)) for path in expected.rglob("*") if path.is_file()}
                signing.require(current_files in (expected_files, expected_files - {PUBLICATION_PATH}), "Existing deployment file inventory differs")
                actual_network = signing.parse_json(signing.read_small(output / "network.json"))
                expected_network = signing.parse_json(signing.read_small(expected / "network.json"))
                # Earlier deploy exports lacked only this new public record binding.
                actual_network["artifacts"].pop(PUBLICATION_ARTIFACT, None)
                expected_network["artifacts"].pop(PUBLICATION_ARTIFACT, None)
                signing.require(actual_network == expected_network, "Existing deployment network differs; choose a new output directory")
                for name in current_files - {"network.json"}:
                    signing.require(signing.read_small(output / name) == signing.read_small(expected / name),
                                    f"Existing deployment file differs: {name}")
                write_input(output / PUBLICATION_PATH, signing.read_small(expected / PUBLICATION_PATH))
                write_input(output / "network.json", signing.read_small(expected / "network.json"), replace=True)
            bundle = output
        else:
            bundle = prepare_bundle(source, output, *arguments, publication)
        network = signing.parse_json(signing.read_small(bundle / "network.json"))
        result = dict(status="deployment_prepared", bundle_dir=str(bundle), network_bundle_id=network["network_bundle_id"],
                      origin_height=network["btc_source"]["index_origin_height"], origin_block_hash=origin_hash,
                      source_url=published["source_url"], snapshot_record_url=published["record_url"],
                      snapshot_record_sha256=published["record_sha256"], public_verified_at_utc=published["public_verified_at_utc"])
        if not prepare_only:
            selection = register(source, bundle, publication, published)
            result.update(status="deployment_integrated", release_input_file=str(selection))
        print(f"Deployment preparation finished: bundle_dir={bundle}, origin_height={result['origin_height']}", file=sys.stderr, flush=True)
        return result

    def publish(self, base: str, client: distribution.AwsCliClient, source: Path | None = None) -> dict:
        record, files = self.record(base, source=source)
        result = self.save_record(record)
        stamps = {role: stamp(path) for role, path in files.items()}
        statuses = {}
        # Reuse the existing immutable S3 upload checks and multipart configuration.
        # Only this allowlist is uploaded; prepared.json and the private key stay local.
        for role, path in files.items():
            item = record["files"][role]
            statuses[role] = distribution._publish_object(client, path, item["object_key"], item["sha256"], item["size_bytes"],
                                                          "application/json" if path.suffix == ".json" else "application/octet-stream")
            signing.require(stamp(path) == stamps[role], "Release input changed during upload")
            verify_public_file(f"{record['public_base_url']}/{item['object_key']}", item)
        signing.require(all(stamp(path) == stamps[role] for role, path in files.items()), "Release input changed during publication")
        signing.require(self.catalog() == signing.read_small(self.prepared / TRUST_FILE), "Operator trust changed during publication")
        path = Path(result["record_file"])
        item = dict(name=path.name, size_bytes=path.stat().st_size, sha256=result["record_sha256"])
        key = f"snapshot-records/assumeutxo/v1/{path.name}"
        statuses["record"] = distribution._publish_object(client, path, key, item["sha256"], item["size_bytes"], "application/json")
        verify_public_file(result["record_url"], item)
        result.update(status="published", public_verified_at_utc=now(), objects=statuses)
        bootstrap.save_json(self.root / "publish-result.json", result)
        return result

    def verify_published(self, base: str) -> dict:
        record, _ = self.record(base, scan=False)
        result = self.save_record(record)
        for item in record["files"].values():
            verify_public_file(f"{record['public_base_url']}/{item['object_key']}", item)
        content = signing.canonical(record)
        verify_public_file(result["record_url"], dict(name=Path(result["record_file"]).name,
                                                     size_bytes=len(content), sha256=result["record_sha256"]))
        return dict(result, status="public_verified", public_verified_at_utc=now())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["paths", "status", "preflight", "create", "verify", "finalize", "prepare-release", "publish", "verify-published", "deploy"])
    parser.add_argument("--root-dir", required=True, type=Path)
    parser.add_argument("--signing-key", required=True, type=Path)
    parser.add_argument("--trusted-keys", required=True, type=Path)
    parser.add_argument("--signer-id", required=True)
    parser.add_argument("--snapshot-file", type=Path)
    parser.add_argument("--source-url", default="")
    parser.add_argument("--height", type=int, default=935000)
    parser.add_argument("--block-hash", "--expected-block-hash")
    parser.add_argument("--public-base-url", default=distribution.DEFAULT_PUBLIC_BASE_URL)
    parser.add_argument("--bucket", default=distribution.DEFAULT_BUCKET)
    parser.add_argument("--endpoint-url", default=distribution.DEFAULT_ENDPOINT_URL)
    parser.add_argument("--aws-region", default=distribution.DEFAULT_AWS_REGION)
    parser.add_argument("--aws-profile", default="usdb-snapshot-publisher")
    parser.add_argument("--s3-upload-concurrency", type=int, default=distribution.DEFAULT_S3_UPLOAD_CONCURRENCY)
    parser.add_argument("--s3-chunk-size-mib", type=int, default=distribution.DEFAULT_S3_CHUNK_SIZE_MIB)
    parser.add_argument("--aws-executable", default="aws", help=argparse.SUPPRESS)
    parser.add_argument("--progress", action="store_true")
    parser.add_argument("--source-bundle", type=Path, help="deploy: base network bundle supplying the business origin height")
    parser.add_argument("--output-dir", type=Path, help="deploy: new native bundle directory or a matching previous export")
    parser.add_argument("--origin-block-hash", help="deploy: Bitcoin block hash at the source bundle's business origin height")
    parser.add_argument("--prepare-only", action="store_true", help="deploy: export a bundle without registering source inputs for CI")
    args = parser.parse_args()
    deployment_args = (args.source_bundle, args.output_dir, args.origin_block_hash)
    if args.command == "deploy" and not all(deployment_args):
        parser.error("deploy requires --source-bundle, --output-dir and --origin-block-hash")
    if args.command != "deploy" and (any(deployment_args) or args.prepare_only):
        parser.error("--source-bundle, --output-dir and --origin-block-hash require deploy")
    try:
        identity = bitcoin.utxo_identity()
        signing.require(args.height == identity["base_height"], "UTXO height must be the compiled base height 935000, not the USDB origin")
        signing.require(args.block_hash is None or args.block_hash.lower() == identity["base_hash"], "UTXO base hash differs from checkpoint")
        signing.https_url(args.public_base_url.rstrip("/"))
        release = Release(args.root_dir, args.trusted_keys, args.signer_id)
        if args.command == "paths":
            result = dict(root_dir=str(release.root), signing_key=str(args.signing_key), trusted_keys=str(args.trusted_keys),
                          signer_id=args.signer_id, snapshot_file=str(args.snapshot_file) if args.snapshot_file else None,
                          bucket=args.bucket, public_base_url=args.public_base_url, endpoint_url=args.endpoint_url,
                          aws_profile=args.aws_profile, aws_region=args.aws_region,
                          s3_upload_concurrency=args.s3_upload_concurrency, s3_chunk_size_mib=args.s3_chunk_size_mib)
        elif args.command == "status":
            result = release.describe() if release.prepared.exists() or release.prepared.is_symlink() else dict(phase="not_created", root_dir=str(release.root))
        elif args.command == "preflight":
            catalog = signing.parse_json(release.catalog())
            key = signing.read_signing_key(args.signing_key, PURPOSE, reuse_snapshot_key=True)
            signing.require(key["key_id"] == release.signer, "Snapshot signer ID mismatch")
            signing.require_key_matches(key, catalog)
            if args.snapshot_file:
                signing.require(signing.file_identity(args.snapshot_file) == dict(name=bitcoin.UTXO_FILE,
                                size_bytes=bitcoin.UTXO_SIZE, sha256=identity["file_sha256"]), "UTXO file identity mismatch")
            else:
                signing.https_url(args.source_url)
            result = dict(status="preflight_passed", snapshot=identity)
        else:
            signing.require(not args.signing_key.expanduser().resolve().is_relative_to(release.root.resolve()),
                            "Release workspace must not contain the signing key")
            with bootstrap.exclusive_directory(release.root):
                if args.command == "create":
                    result = release.create(args.snapshot_file, args.signing_key, args.source_url)
                elif args.command == "finalize":
                    result = release.finalize(args.snapshot_file)
                elif args.command == "verify":
                    release.load(source=args.snapshot_file)
                    result = dict(release.describe(), file_verified=True)
                elif args.command == "prepare-release":
                    result = release.prepare_release(args.public_base_url, args.snapshot_file)
                elif args.command == "verify-published":
                    result = release.verify_published(args.public_base_url)
                elif args.command == "deploy":
                    result = release.deploy(args.source_bundle, args.output_dir, args.origin_block_hash, prepare_only=args.prepare_only)
                else:
                    if args.progress:
                        os.environ["USDB_SNAPSHOT_FORCE_PROGRESS"] = "1"
                    with distribution.AwsCliClient(endpoint_url=args.endpoint_url, bucket=args.bucket, region=args.aws_region,
                            profile=args.aws_profile or None, upload_concurrency=args.s3_upload_concurrency,
                            multipart_chunk_size_mib=args.s3_chunk_size_mib, executable=args.aws_executable) as client:
                        result = release.publish(args.public_base_url, client, args.snapshot_file)
        print(json.dumps(result, indent=2, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"UTXO snapshot operation failed: action={args.command}, root={args.root_dir}, error={error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
