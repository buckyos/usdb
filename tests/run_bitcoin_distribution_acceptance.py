#!/usr/bin/env python3
"""Verify real Bitcoin artifacts and consume a signed release inside the built image.

Reads operator-supplied artifacts, uses disposable keys, and never starts bitcoind.
The fixture HTTPS origin is loopback-only; the container uses host networking to
reach that origin and mounts only temporary public metadata and output storage.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import artifact_signing as SIGN
import bitcoin_release as RELEASE
from common.snapshot_range_server import SnapshotRangeServer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--core-archive", type=Path, required=True)
    parser.add_argument("--upstream-evidence", type=Path, required=True)
    parser.add_argument("--utxo-file", type=Path, help="Optional real 9.39GB file; reads the entire file but does not import it")
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix="usdb-p72-distribution-acceptance-"))
    root.chmod(0o755)
    public = root / "public"
    public.mkdir(mode=0o755)
    output = root / "output"
    output.mkdir(mode=0o755)
    report = dict(status="running", scope="Real archive/signature and optional UTXO hash; no Core import or production signer", run=str(root))
    print(f"Bitcoin distribution acceptance: {root}", flush=True)

    def run(*command):
        result = subprocess.run([*map(str, command)], capture_output=True, text=True, timeout=120)
        if result.returncode:
            raise RuntimeError(f"Acceptance command failed: {result.stderr}")
        return result.stdout

    def consume(*arguments):
        return run("docker", "run", "--rm", "--read-only", "--network", "host", "--user", f"{os.getuid()}:{os.getgid()}",
                   "--tmpfs", "/tmp:rw,noexec,nosuid,size=16m", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
                   "--mount", f"type=bind,src={public},dst=/public,readonly", "--mount", f"type=bind,src={output},dst=/output",
                   "-e", "SSL_CERT_FILE=/public/server.crt", "-e", "no_proxy=127.0.0.1", "-e", "PYTHONDONTWRITEBYTECODE=1",
                   "--entrypoint", "python3", args.image, "/opt/usdb/docker/scripts/tools/bitcoin_release.py", *arguments)

    origin = None
    try:
        report["image_id"] = run("docker", "image", "inspect", args.image, "--format", "{{.Id}}").strip()
        for name in ("artifact_signing.py", "bitcoin_release.py", "bitcoin_assumeutxo.py"):
            expected = hashlib.sha256((ROOT / "docker/scripts/tools" / name).read_bytes()).hexdigest()
            actual = run("docker", "run", "--rm", "--network", "none", "--entrypoint", "sha256sum", args.image,
                         "/opt/usdb/docker/scripts/tools/" + name).split()[0]
            if expected != actual:
                raise RuntimeError(f"Stale distribution tool in image: {name}")
        with tempfile.TemporaryDirectory(prefix="usdb-distribution-private-") as keys:
            keyroot = Path(keys)
            SIGN.keygen(keyroot / "core", "acceptance-core-only", "bitcoin-core")
            secret, trust = keyroot / "core/signing-key.json", keyroot / "core/trusted-keys.json"
            manifest = RELEASE.prepare("bitcoin-core", args.core_archive, args.upstream_evidence, secret, trust, public)
            shutil.copyfile(trust, public / "core-trusted-keys.json")
            # Publish only public fixture metadata. Private signing material is never mounted.
            for path in public.iterdir():
                path.chmod(0o644)
            origin = SnapshotRangeServer(root, b"")
            shutil.copyfile(root / "server.crt", public / "server.crt")
            base = origin.url.rsplit("/", 1)[0]
            origin.files = {"/" + manifest.name: manifest.read_bytes(), "/" + manifest.name + ".sig": Path(str(manifest) + ".sig").read_bytes(),
                            "/" + RELEASE.CORE_FILE: args.core_archive.read_bytes()}
            (public / "source.json").write_bytes(SIGN.canonical(dict(mode="usdb-signed", manifest_url=base + "/" + manifest.name)))
            report["core_consumer"] = consume("fetch-core", "--source-config", "/public/source.json", "--trusted-keys", "/public/core-trusted-keys.json", "--output-dir", "/output")
            downloaded = SIGN.file_identity(output / RELEASE.CORE_FILE)
            if downloaded != RELEASE.core_file():
                raise RuntimeError("Real archive changed across signed distribution")
            report["core_file"] = downloaded
            report["public_paths_requested"] = origin.paths
            report["core_provenance"] = json.loads((output / "core-artifact-provenance.json").read_text())
            if args.utxo_file:
                SIGN.keygen(keyroot / "utxo", "acceptance-utxo-only", "bitcoin-assumeutxo")
                utxo = RELEASE.prepare("bitcoin-assumeutxo", args.utxo_file, None, keyroot / "utxo/signing-key.json",
                                       keyroot / "utxo/trusted-keys.json", public)
                shutil.copyfile(keyroot / "utxo/trusted-keys.json", public / "utxo-trusted-keys.json")
                for path in public.iterdir():
                    path.chmod(0o644)
                verified = consume("verify", "--manifest-file", "/public/" + utxo.name, "--trusted-keys", "/public/utxo-trusted-keys.json", "--artifact-type", "bitcoin-assumeutxo")
                report["utxo_manifest"] = json.loads(verified)
            report["status"] = "pass"
    finally:
        if origin is not None:
            origin.close()
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(status=report["status"], run=str(root), image_id=report["image_id"]), indent=2))


if __name__ == "__main__":
    main()
