#!/usr/bin/env python3
"""Canonical, purpose-bound Ed25519 envelopes for Bitcoin distribution artifacts.

Trust catalogs are installed independently of the download origin. OpenSSL handles
Ed25519; this module never implements cryptographic primitives itself.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


SCHEMA = "usdb-bitcoin-artifact:v1"
TRUST_SCHEMA = "usdb-bitcoin-artifact-trust:v1"
KEY_SCHEMA = "usdb-bitcoin-artifact-key:v1"
TYPES = ("bitcoin-core", "bitcoin-assumeutxo")
METADATA_LIMIT = 1024 * 1024
# Keep public artifact requests compatible with the existing snapshot download origin.
HTTP_USER_AGENT = "usdb-snapshot-verifier/1"
PUBLIC_DER_PREFIX = bytes.fromhex("302a300506032b6570032100")
PRIVATE_DER_PREFIX = bytes.fromhex("302e020100300506032b657004220420")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def canonical(value: dict) -> bytes:
    """Use a deliberately restricted canonical JSON representation, with one final LF."""
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False) + "\n").encode()


def exact(value: object, fields: set[str], label: str) -> None:
    require(isinstance(value, dict) and set(value) == fields, f"Invalid {label} fields")


def digest(value: str) -> str:
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None, "Invalid SHA-256 digest")
    return value


def regular(path: Path) -> None:
    require(not path.is_symlink() and path.is_file(), f"Expected a regular file: {path}")


def read_small(path: Path, limit: int = METADATA_LIMIT) -> bytes:
    regular(path)
    with path.open("rb") as source:
        value = source.read(limit + 1)
    require(len(value) <= limit, "Artifact metadata exceeds its size limit")
    return value


def parse_json(content: bytes) -> dict:
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, "Duplicate JSON field")
            result[key] = value
        return result

    value = json.loads(content, object_pairs_hook=pairs)
    require(isinstance(value, dict), "Artifact JSON must be an object")
    return value


def https_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password
            and not parsed.query and not parsed.fragment and not any(ord(c) <= 32 for c in value),
            "Artifact URL must be HTTPS without credentials, query, fragment or whitespace")
    return value


class HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        https_url(newurl)
        return super().redirect_request(request, fp, code, message, headers, newurl)


def fetch(url: str, destination: Path, *, limit: int) -> None:
    """Bound every download and publish atomically; never expose URL credentials in errors."""
    https_url(url)
    require(not destination.exists() and not destination.is_symlink(), "Download destination already exists")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        request = urllib.request.Request(url, headers={"Accept-Encoding": "identity", "User-Agent": HTTP_USER_AGENT})
        opener = urllib.request.build_opener(HttpsRedirect())
        with opener.open(request, timeout=30) as response, tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as output:
            temporary = Path(output.name)
            https_url(response.url)
            require(response.status == 200 and response.headers.get("Content-Encoding", "identity") == "identity",
                    "Artifact origin returned an unexpected status or encoding")
            total, last = 0, time.monotonic()
            while chunk := response.read(min(4 * 1024 * 1024, limit + 1 - total)):
                total += len(chunk)
                require(total <= limit, "Artifact download exceeds its size limit")
                output.write(chunk)
                if time.monotonic() - last >= 10:
                    print(f"Artifact download progress: bytes={total}", file=sys.stderr, flush=True)
                    last = time.monotonic()
            lengths = response.headers.get_all("Content-Length", [])
            require(not lengths or lengths == [str(total)], "Artifact download length mismatch")
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, destination)
    except (OSError, urllib.error.URLError) as error:
        raise ValueError("Artifact transport failed") from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def file_identity(path: Path) -> dict:
    """Hash the entire file with progress and reject replacement during the scan."""
    regular(path)
    sha, size, last = hashlib.sha256(), 0, time.monotonic()
    started = last
    print(f"Artifact verification started: file={path.name}, bytes={path.stat().st_size}", file=sys.stderr, flush=True)
    with path.open("rb") as source:
        before = os.fstat(source.fileno())
        while chunk := source.read(4 * 1024 * 1024):
            sha.update(chunk)
            size += len(chunk)
            if time.monotonic() - last >= 10:
                print(f"Artifact verification progress: bytes={size}, total={before.st_size}", file=sys.stderr, flush=True)
                last = time.monotonic()
        after = os.fstat(source.fileno())
    current = path.stat()
    require((before.st_size, before.st_mtime_ns, before.st_ctime_ns) == (after.st_size, after.st_mtime_ns, after.st_ctime_ns)
            and (current.st_dev, current.st_ino) == (after.st_dev, after.st_ino), "Artifact changed during verification")
    print(f"Artifact verification finished: file={path.name}, bytes={size}, elapsed_seconds={time.monotonic() - started:.1f}",
          file=sys.stderr, flush=True)
    return dict(name=path.name, sha256=sha.hexdigest(), size_bytes=size)


def openssl(arguments: list[str]) -> bytes:
    try:
        result = subprocess.run(["openssl", *arguments], capture_output=True, timeout=30)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError("Ed25519 operation could not complete") from error
    require(result.returncode == 0, "Ed25519 operation failed")
    return result.stdout


def key_id(value: str) -> str:
    require(isinstance(value, str) and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}", value) is not None, "Invalid signing key ID")
    return value


def raw_key(value: str) -> bytes:
    result = base64.b64decode(value, validate=True)
    require(len(result) == 32, "Ed25519 key must contain 32 bytes")
    return result


def keygen(directory: Path, signer: str, artifact_type: str) -> None:
    """Generate a dedicated distribution key; never overwrite an existing key directory."""
    key_id(signer)
    require(artifact_type in TYPES, "Unsupported artifact key purpose")
    directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    seed = os.urandom(32)
    with tempfile.TemporaryDirectory() as temporary:
        private = Path(temporary) / "private.der"
        private.write_bytes(PRIVATE_DER_PREFIX + seed)
        public = openssl(["pkey", "-inform", "DER", "-in", str(private), "-pubout", "-outform", "DER"])
    require(public.startswith(PUBLIC_DER_PREFIX) and len(public) == 44, "Unexpected Ed25519 public key encoding")
    secret = dict(schema_version=KEY_SCHEMA, key_id=signer, artifact_type=artifact_type,
                  secret_key_base64=base64.b64encode(seed).decode())
    descriptor = os.open(directory / "signing-key.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(canonical(secret))
    catalog = dict(schema_version=TRUST_SCHEMA, keys=[dict(key_id=signer, artifact_type=artifact_type,
                   public_key_base64=base64.b64encode(public[len(PUBLIC_DER_PREFIX):]).decode())])
    (directory / "trusted-keys.json").write_bytes(canonical(catalog))


def trusted_key(path: Path, signer: str, artifact_type: str) -> bytes:
    return catalog_key(parse_json(read_small(path)), signer, artifact_type)


def catalog_key(catalog: dict, signer: str, artifact_type: str) -> bytes:
    """Validate every catalog entry before selecting one purpose-bound public key."""
    exact(catalog, {"schema_version", "keys"}, "trusted catalog")
    require(catalog["schema_version"] == TRUST_SCHEMA and isinstance(catalog["keys"], list), "Invalid trusted catalog schema")
    ids, materials, found = set(), set(), None
    for entry in catalog["keys"]:
        exact(entry, {"key_id", "artifact_type", "public_key_base64"}, "trusted key")
        identifier = key_id(entry["key_id"])
        public = raw_key(entry["public_key_base64"])
        require(identifier not in ids and public not in materials, "Duplicate trusted key ID or key material")
        require(entry["artifact_type"] in TYPES, "Invalid trusted key purpose")
        ids.add(identifier)
        materials.add(public)
        if identifier == signer and entry["artifact_type"] == artifact_type:
            found = public
    require(found is not None, "Signer is not trusted for this artifact purpose")
    return found


def snapshot_catalog(path: Path, signer: str) -> dict:
    """Adapt only public legacy snapshot material to the UTXO signature purpose."""
    catalog = parse_json(read_small(path))
    if "schema_version" not in catalog:
        exact(catalog, {"keys"}, "snapshot trusted catalog")
        require(isinstance(catalog["keys"], list), "Invalid snapshot trusted keys")
        entries = []
        for entry in catalog["keys"]:
            exact(entry, {"key_id", "public_key_base64"}, "snapshot trusted key")
            entries.append(dict(entry, artifact_type="bitcoin-assumeutxo"))
        catalog = dict(schema_version=TRUST_SCHEMA, keys=entries)
    catalog_key(catalog, signer, "bitcoin-assumeutxo")
    return catalog


def read_signing_key(path: Path, artifact_type: str, *, reuse_snapshot_key: bool = False) -> dict:
    """Normalize an explicitly selected legacy UTXO signer without rewriting its secret file."""
    secret = parse_json(read_small(path))
    require(stat.S_IMODE(path.stat().st_mode) & 0o077 == 0, "Signing key permissions must exclude group and other users")
    if reuse_snapshot_key:
        require(artifact_type == "bitcoin-assumeutxo", "Snapshot keys are only accepted for UTXO manifests")
        if "schema_version" not in secret:
            exact(secret, {"key_id", "secret_key_base64"}, "snapshot signing key")
            secret = dict(secret, schema_version=KEY_SCHEMA, artifact_type=artifact_type)
    exact(secret, {"schema_version", "key_id", "artifact_type", "secret_key_base64"}, "signing key")
    require(secret["schema_version"] == KEY_SCHEMA and secret["artifact_type"] == artifact_type,
            "Signing key purpose or ID mismatch")
    key_id(secret["key_id"])
    raw_key(secret["secret_key_base64"])
    return secret


def require_key_matches(secret: dict, catalog: dict) -> None:
    """Check an operator's key pair before downloading or scanning a large artifact."""
    public = catalog_key(catalog, secret["key_id"], secret["artifact_type"])
    with tempfile.TemporaryDirectory() as temporary:
        private = Path(temporary) / "private.der"
        private.write_bytes(PRIVATE_DER_PREFIX + raw_key(secret["secret_key_base64"]))
        actual = openssl(["pkey", "-inform", "DER", "-in", str(private), "-pubout", "-outform", "DER"])
    require(actual == PUBLIC_DER_PREFIX + public, "Signing key does not match trusted public key")


def validate_envelope(manifest: dict, artifact_type: str) -> None:
    exact(manifest, {"schema_version", "artifact_type", "identity", "file", "upstream_verification",
                     "signature_scheme", "signing_key_id"}, "artifact manifest")
    require(manifest["schema_version"] == SCHEMA and manifest["artifact_type"] == artifact_type
            and artifact_type in TYPES and manifest["signature_scheme"] == "ed25519", "Artifact manifest schema or purpose mismatch")
    key_id(manifest["signing_key_id"])
    item = manifest["file"]
    exact(item, {"name", "sha256", "size_bytes"}, "artifact file")
    require(isinstance(item["name"], str) and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}", item["name"]) is not None,
            "Unsafe artifact file name")
    digest(item["sha256"])
    require(type(item["size_bytes"]) is int and 0 < item["size_bytes"] < 2**63, "Invalid artifact size")
    require(isinstance(manifest["identity"], dict), "Artifact identity must be an object")


def payload(manifest: dict) -> bytes:
    return (SCHEMA + ":" + manifest["artifact_type"] + "\0").encode() + canonical(manifest)


def sign(manifest: dict, secret_path: Path, trust_path: Path, *, reuse_snapshot_key: bool = False) -> bytes:
    """Sign only with a dedicated key whose public half already matches the approved catalog."""
    secret = read_signing_key(secret_path, manifest["artifact_type"], reuse_snapshot_key=reuse_snapshot_key)
    require(secret["key_id"] == manifest["signing_key_id"], "Signing key purpose or ID mismatch")
    validate_envelope(manifest, secret["artifact_type"])
    public = trusted_key(trust_path, secret["key_id"], secret["artifact_type"])
    with tempfile.TemporaryDirectory() as temporary:
        private, message = Path(temporary) / "private.der", Path(temporary) / "message"
        private.write_bytes(PRIVATE_DER_PREFIX + raw_key(secret["secret_key_base64"]))
        actual = openssl(["pkey", "-inform", "DER", "-in", str(private), "-pubout", "-outform", "DER"])
        require(actual == PUBLIC_DER_PREFIX + public, "Signing key does not match trusted public key")
        message.write_bytes(payload(manifest))
        return openssl(["pkeyutl", "-sign", "-rawin", "-keyform", "DER", "-inkey", str(private), "-in", str(message)])


def verify(content: bytes, signature: bytes, trust_path: Path, artifact_type: str) -> dict:
    manifest = parse_json(content)
    validate_envelope(manifest, artifact_type)
    require(content == canonical(manifest), "Manifest must use canonical JSON")
    require(len(signature) == 64, "Invalid Ed25519 signature length")
    public = trusted_key(trust_path, manifest["signing_key_id"], artifact_type)
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        (root / "key.der").write_bytes(PUBLIC_DER_PREFIX + public)
        (root / "message").write_bytes(payload(manifest))
        (root / "signature").write_bytes(signature)
        openssl(["pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", str(root / "key.der"),
                 "-in", str(root / "message"), "-sigfile", str(root / "signature")])
    print(f"Artifact signature verified: type={artifact_type}, signer={manifest['signing_key_id']}, manifest_sha256={hashlib.sha256(content).hexdigest()}", file=sys.stderr, flush=True)
    return manifest


def load_manifest(*, url: str = "", path: Path | None = None, trust_path: Path, artifact_type: str) -> dict:
    """Remote references are content-addressed; local signatures always sit next to their manifest."""
    require(bool(url) != (path is not None), "Choose exactly one manifest URL or local manifest file")
    if path is not None:
        return verify(read_small(path), read_small(Path(str(path) + ".sig"), 64), trust_path, artifact_type)
    https_url(url)
    name = urllib.parse.urlsplit(url).path.rsplit("/", 1)[-1]
    require(re.fullmatch(r"[0-9a-f]{64}\.json", name) is not None, "Manifest URL must end with its SHA-256 followed by .json")
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        fetch(url, root / "manifest", limit=METADATA_LIMIT)
        content = read_small(root / "manifest")
        require(hashlib.sha256(content).hexdigest() == name[:-5], "Manifest URL digest mismatch")
        fetch(url + ".sig", root / "signature", limit=64)
        return verify(content, read_small(root / "signature", 64), trust_path, artifact_type)


def write_release(directory: Path, manifest: dict, signature: bytes) -> Path:
    """Publish small release metadata only; operators upload the payload separately."""
    directory.mkdir(parents=True, exist_ok=True)
    content = canonical(manifest)
    target = directory / (hashlib.sha256(content).hexdigest() + ".json")
    # Exclusive creation makes accidental reuse or replacement of a release visible.
    with target.open("xb") as output:
        output.write(content)
    with Path(str(target) + ".sig").open("xb") as output:
        output.write(signature)
    return target
