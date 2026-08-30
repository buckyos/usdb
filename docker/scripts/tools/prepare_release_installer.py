#!/usr/bin/env python3
"""Generate a release-bound USDB bootstrap installer."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shlex
import tempfile
from pathlib import Path


RELEASE_ID_RE = re.compile(r"^usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_release_installer(
    *,
    release_id: str,
    repository: str,
    installer_path: Path,
    manifest_path: Path,
    node_kit_path: Path,
    output_path: Path,
    release_base_url: str | None = None,
) -> Path:
    if RELEASE_ID_RE.fullmatch(release_id) is None:
        raise ValueError(f"invalid release ID: {release_id}")
    if REPOSITORY_RE.fullmatch(repository) is None:
        raise ValueError(f"invalid GitHub repository: {repository}")
    installer = installer_path.resolve()
    manifest = manifest_path.resolve()
    node_kit = node_kit_path.resolve()
    output = output_path.resolve()
    for path in (installer, manifest, node_kit):
        if not path.is_file():
            raise ValueError(f"release installer input is missing: {path}")
    if installer.name != "install_usdb_node.sh":
        raise ValueError("generic installer must be named install_usdb_node.sh")
    if manifest.name != "usdb-release-manifest.json":
        raise ValueError("manifest must be named usdb-release-manifest.json")
    if output.exists():
        raise ValueError(f"refusing to replace release installer: {output}")
    base_url = (
        release_base_url.rstrip("/")
        if release_base_url is not None
        else f"https://github.com/{repository}/releases/download/{release_id}"
    )
    if not base_url or "\n" in base_url or "\r" in base_url:
        raise ValueError("invalid release asset base URL")

    archive_name = f"{release_id}-node-kit.tar.gz"
    if node_kit.name != archive_name:
        raise ValueError(f"node kit must be named {archive_name}")
    script = f"""#!/usr/bin/env bash
set -euo pipefail

release_id={shlex.quote(release_id)}
release_base_url={shlex.quote(base_url)}
installer_sha256={shlex.quote(_sha256(installer))}
manifest_sha256={shlex.quote(_sha256(manifest))}
node_kit_sha256={shlex.quote(_sha256(node_kit))}

for command in bash curl sha256sum; do
  command -v "$command" >/dev/null 2>&1 || {{
    echo "Required command is not installed: $command" >&2
    exit 1
  }}
done

temporary="$(mktemp -d "${{TMPDIR:-/tmp}}/.usdb-release-installer.XXXXXX")"
cleanup() {{
  rm -rf "$temporary"
}}
trap cleanup EXIT

installer="$temporary/install_usdb_node.sh"
curl --fail --silent --show-error --location \
  "$release_base_url/install_usdb_node.sh" --output "$installer"
printf '%s  %s\n' "$installer_sha256" "$installer" | sha256sum -c - >/dev/null

bash "$installer" "$@" \
  --release-id "$release_id" \
  --release-base-url "$release_base_url" \
  --expected-manifest-sha256 "$manifest_sha256" \
  --expected-node-kit-sha256 "$node_kit_sha256"
"""
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    temporary = Path(temporary_name)
    try:
        os.fchmod(descriptor, 0o755)
        with os.fdopen(descriptor, "w", encoding="utf-8") as target:
            target.write(script)
            target.flush()
            os.fsync(target.fileno())
        temporary.replace(output)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--repository", default="buckyos/usdb")
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--node-kit", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--release-base-url")
    args = parser.parse_args()
    try:
        path = build_release_installer(
            release_id=args.release_id,
            repository=args.repository,
            installer_path=args.installer,
            manifest_path=args.manifest,
            node_kit_path=args.node_kit,
            output_path=args.output,
            release_base_url=args.release_base_url,
        )
    except (OSError, ValueError) as error:
        raise SystemExit(f"USDB release installer preparation failed: {error}") from error
    print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
