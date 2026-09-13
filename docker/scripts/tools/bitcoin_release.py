#!/usr/bin/env python3
"""Prepare signed Bitcoin distributions and fetch Core archives for image builds."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.parse

import artifact_signing as signing
from assumeutxo_bootstrap import checkpoint_metadata


CORE_VERSION = "31.1"
CORE_PLATFORM = "x86_64-linux-gnu"
CORE_FILE = "bitcoin-31.1-x86_64-linux-gnu.tar.gz"
CORE_SHA256 = "b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e"
CORE_SIZE = 90293352
GUIX_REVISION = "f6a216c90095f5e316318760da6411fe465b7482"
UPSTREAM_POLICY = "bitcoin-core-31.1-three-signers:v1"
RELEASE_BASE = "https://bitcoincore.org/bin/bitcoin-core-31.1"
KEYS_BASE = f"https://raw.githubusercontent.com/bitcoin-core/guix.sigs/{GUIX_REVISION}/builder-keys"
KEYS = {
    "achow101.gpg": ("1b31ab2e336d5ee44dd4d9f5703ad4445798fcee3b1352ab75afa145e737224a", "152812300785C96444D3334D17565732E08E5E41"),
    "fanquake.gpg": ("d9d6d89f5d9da0811ed94469f790d5bd0e185b7ee678e7ca7b3121995ded3d26", "CFB16E21C950F67FA95E558F2EEB9F5CC09526C1"),
    "hebasto.gpg": ("f305e90cb6b3cfe47beb71a51242395f1fc8d46840a1d2345283df2fefca021b", "D1DBF2C4B96F2DEBF4C16654410108112E7EA81F"),
}
UTXO_FILE = "mainnet-935000-utxos.dat"
UTXO_SIZE = 9387990306


def default_trust_path() -> Path:
    tools = Path(__file__).resolve().parent
    packaged = tools.parent / "data/trust/bitcoin-artifacts.trusted-keys.json"
    return packaged if packaged.is_file() else tools.parents[1] / "trust/bitcoin-artifacts.trusted-keys.json"


def core_identity() -> dict:
    return dict(version=CORE_VERSION, platform=CORE_PLATFORM, distribution="upstream-mirror")


def core_file() -> dict:
    return dict(name=CORE_FILE, sha256=CORE_SHA256, size_bytes=CORE_SIZE)


def utxo_identity() -> dict:
    return checkpoint_metadata(935000)


def validate_core(manifest: dict) -> None:
    """USDB signatures attest unchanged official bytes; independent builds need another policy."""
    signing.require(manifest["identity"] == core_identity() and manifest["file"] == core_file(), "Core release does not match the pinned upstream binary")
    proof = manifest["upstream_verification"]
    signing.exact(proof, {"policy", "guix_revision", "required_signers", "files"}, "upstream verification")
    signing.require(proof["policy"] == UPSTREAM_POLICY and proof["guix_revision"] == GUIX_REVISION
                    and proof["required_signers"] == sorted(value[1] for value in KEYS.values()), "Upstream verification policy mismatch")
    signing.require(isinstance(proof["files"], list) and len(proof["files"]) == 5, "Missing upstream verification evidence")
    names = set()
    for item in proof["files"]:
        signing.exact(item, {"name", "sha256", "size_bytes"}, "upstream evidence file")
        signing.require(item["name"] in {*KEYS, "SHA256SUMS", "SHA256SUMS.asc"} and item["name"] not in names,
                        "Unknown or duplicate upstream evidence file")
        signing.digest(item["sha256"])
        signing.require(type(item["size_bytes"]) is int and 0 < item["size_bytes"] <= signing.METADATA_LIMIT, "Invalid upstream evidence size")
        if item["name"] in KEYS:
            signing.require(item["sha256"] == KEYS[item["name"]][0], "Upstream public key hash mismatch")
        names.add(item["name"])


def validate_utxo(manifest: dict) -> None:
    identity = utxo_identity()
    signing.require(signing.canonical(manifest["identity"]) == signing.canonical(identity) and manifest["file"] == dict(name=UTXO_FILE, sha256=identity["file_sha256"], size_bytes=UTXO_SIZE)
                    and manifest["upstream_verification"] is None, "UTXO release does not match the compiled checkpoint")


def verify_upstream(evidence: Path, archive: Path) -> dict:
    """Require all three pinned signers using an isolated keyring and archived evidence."""
    files = []
    for name in ("SHA256SUMS", "SHA256SUMS.asc", *KEYS):
        content = signing.read_small(evidence / name)
        item = dict(name=name, sha256=hashlib.sha256(content).hexdigest(), size_bytes=len(content))
        if name in KEYS:
            signing.require(item["sha256"] == KEYS[name][0], "Pinned upstream public key file mismatch")
        files.append(item)
    with tempfile.TemporaryDirectory(prefix="usdb-core-keyring-") as temporary:
        command = ["gpg", "--batch", "--no-options", "--homedir", temporary, "--no-auto-key-retrieve"]
        try:
            imported = subprocess.run([*command, "--import", *[str(evidence / name) for name in KEYS]], capture_output=True, timeout=30)
            signing.require(imported.returncode == 0, "Upstream key import failed")
            result = subprocess.run([*command, "--status-fd", "1", "--verify", str(evidence / "SHA256SUMS.asc"), str(evidence / "SHA256SUMS")],
                                    capture_output=True, text=True, timeout=30)
            # The multi-signature file contains additional, intentionally untrusted
            # signers. Their missing keys do not substitute for our three VALIDSIGs.
            valid = {line.split()[2] for line in result.stdout.splitlines() if line.startswith("[GNUPG:] VALIDSIG ")}
            signing.require(all(value[1] in valid for value in KEYS.values()), "Required upstream signatures did not all verify")
        finally:
            subprocess.run(["gpgconf", "--homedir", temporary, "--kill", "gpg-agent"], capture_output=True, timeout=15)
    sums = signing.read_small(evidence / "SHA256SUMS").decode("ascii").splitlines()
    matching = [line for line in sums if line.endswith("  " + CORE_FILE)]
    signing.require(matching == [f"{CORE_SHA256}  {CORE_FILE}"], "Signed checksum file does not contain the pinned Core archive")
    actual = signing.file_identity(archive)
    signing.require(actual == core_file(), "Core archive identity mismatch")
    print("Core upstream verification passed: version=31.1, required_signers=3", file=sys.stderr, flush=True)
    return dict(policy=UPSTREAM_POLICY, guix_revision=GUIX_REVISION,
                required_signers=sorted(value[1] for value in KEYS.values()), files=files)


def download_upstream(directory: Path, release_base: str, keys_base: str) -> Path:
    signing.https_url(release_base)
    signing.https_url(keys_base)
    for name in (CORE_FILE, "SHA256SUMS", "SHA256SUMS.asc", *KEYS):
        base = keys_base if name in KEYS else release_base
        signing.fetch(base.rstrip("/") + "/" + name, directory / name,
                      limit=CORE_SIZE if name == CORE_FILE else signing.METADATA_LIMIT)
    return directory / CORE_FILE


def prepare(artifact_type: str, archive: Path, evidence: Path | None, secret: Path, trust: Path, output: Path) -> Path:
    """Validate payload before signing; private keys never enter an image or published directory."""
    key = signing.parse_json(signing.read_small(secret))
    proof = verify_upstream(evidence, archive) if artifact_type == "bitcoin-core" and evidence is not None else None
    manifest = dict(schema_version=signing.SCHEMA, artifact_type=artifact_type,
                    identity=core_identity() if artifact_type == "bitcoin-core" else utxo_identity(),
                    file=signing.file_identity(archive), upstream_verification=proof,
                    signature_scheme="ed25519", signing_key_id=key["key_id"])
    (validate_core if artifact_type == "bitcoin-core" else validate_utxo)(manifest)
    signature = signing.sign(manifest, secret, trust)
    signing.verify(signing.canonical(manifest), signature, trust, artifact_type)
    path = signing.write_release(output, manifest, signature)
    print(f"Signed artifact release prepared: manifest={path}", file=sys.stderr, flush=True)
    return path


def fetch_core(config_path: Path, trust: Path, output: Path) -> Path:
    """Select one trust policy before I/O; errors never trigger a weaker fallback."""
    config = signing.parse_json(signing.read_small(config_path))
    mode = config.get("mode")
    signing.require(mode in {"upstream", "usdb-signed"}, "Core source mode must be upstream or usdb-signed")
    signing.exact(config, {"mode", "release_base_url", "keys_base_url"} if mode == "upstream" else {"mode", "manifest_url"}, "Core source config")
    output.mkdir(parents=True, exist_ok=True)
    target = output / CORE_FILE
    signing.require(not target.exists() and not target.is_symlink(), "Core output archive already exists")
    print(f"Core artifact fetch started: mode={mode}, version={CORE_VERSION}", file=sys.stderr, flush=True)
    with tempfile.TemporaryDirectory(dir=output, prefix=".core-fetch-") as temporary:
        root = Path(temporary)
        if mode == "upstream":
            archive = download_upstream(root, config["release_base_url"], config["keys_base_url"])
            proof = verify_upstream(root, archive)
            provenance = dict(mode=mode, upstream_verification=proof, file=core_file())
        else:
            url = config["manifest_url"]
            manifest = signing.load_manifest(url=url, trust_path=trust, artifact_type="bitcoin-core")
            validate_core(manifest)
            archive = root / CORE_FILE
            signing.fetch(urllib.parse.urljoin(url, CORE_FILE), archive, limit=CORE_SIZE)
            signing.require(signing.file_identity(archive) == manifest["file"], "Downloaded Core archive hash mismatch")
            provenance = dict(mode=mode, manifest_sha256=hashlib.sha256(signing.canonical(manifest)).hexdigest(), manifest=manifest)
        os.link(archive, target)
        (output / "core-artifact-provenance.json").write_bytes(signing.canonical(provenance))
    return target


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    generate = commands.add_parser("keygen", help="Generate an isolated key and purpose-bound public catalog")
    generate.add_argument("--artifact-type", choices=signing.TYPES, required=True)
    generate.add_argument("--key-id", required=True)
    generate.add_argument("--output-dir", type=Path, required=True)
    download = commands.add_parser("download-upstream", help="Download and verify official Core artifacts before preparing a release")
    download.add_argument("--output-dir", type=Path, required=True)
    download.add_argument("--release-base-url", default=RELEASE_BASE)
    download.add_argument("--keys-base-url", default=KEYS_BASE)
    for name in ("prepare-core", "prepare-utxo"):
        prepare_parser = commands.add_parser(name, help="Verify payload and create a USDB-signed, content-addressed manifest")
        prepare_parser.add_argument("--artifact", type=Path, required=True)
        prepare_parser.add_argument("--signing-key", type=Path, required=True)
        prepare_parser.add_argument("--trusted-keys", type=Path, required=True)
        prepare_parser.add_argument("--output-dir", type=Path, required=True)
        if name == "prepare-core":
            prepare_parser.add_argument("--upstream-evidence", type=Path, required=True)
    build = commands.add_parser("fetch-core", help="Fetch and verify a Core archive using the selected source config")
    build.add_argument("--source-config", type=Path, required=True)
    build.add_argument("--trusted-keys", type=Path, required=True)
    build.add_argument("--output-dir", type=Path, required=True)
    verify_parser = commands.add_parser("verify", help="Verify a local or remote signed manifest against the compiled Bitcoin identity")
    source = verify_parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--manifest-url")
    source.add_argument("--manifest-file", type=Path)
    verify_parser.add_argument("--trusted-keys", type=Path, required=True)
    verify_parser.add_argument("--artifact-type", choices=signing.TYPES, required=True)
    args = parser.parse_args()
    try:
        if args.command == "keygen":
            signing.keygen(args.output_dir, args.key_id, args.artifact_type)
        elif args.command == "download-upstream":
            args.output_dir.mkdir(parents=True, exist_ok=False)
            archive = download_upstream(args.output_dir, args.release_base_url, args.keys_base_url)
            proof = verify_upstream(args.output_dir, archive)
            (args.output_dir / "upstream-verification.json").write_bytes(signing.canonical(proof))
        elif args.command.startswith("prepare-"):
            prepare("bitcoin-core" if args.command == "prepare-core" else "bitcoin-assumeutxo", args.artifact,
                    getattr(args, "upstream_evidence", None), args.signing_key, args.trusted_keys, args.output_dir)
        elif args.command == "fetch-core":
            fetch_core(args.source_config, args.trusted_keys, args.output_dir)
        else:
            manifest = signing.load_manifest(url=args.manifest_url or "", path=args.manifest_file, trust_path=args.trusted_keys, artifact_type=args.artifact_type)
            (validate_core if args.artifact_type == "bitcoin-core" else validate_utxo)(manifest)
            print(json.dumps(manifest, sort_keys=True))
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"Bitcoin artifact operation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
