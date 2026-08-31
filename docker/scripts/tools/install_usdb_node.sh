#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: install_usdb_node.sh --release-id <usdb-testnet-vN-rN> [options]

Options:
  --repository <owner/name>  GitHub repository (default: buckyos/usdb)
  --release-base-url <url>   Override the release asset base URL (mirrors/testing)
  --install-root <path>      Release install root (default: ~/.local/share/usdb/releases)
  --bin-dir <path>           Directory for the usdb-node symlink (default: ~/.local/bin)
  --expected-manifest-sha256 <sha256>
                            Require the release-bound manifest digest
  --expected-node-kit-sha256 <sha256>
                            Require the release-bound node-kit digest

The installer downloads one immutable release manifest and node kit, verifies
their canonical SHA-256 files, and installs no secrets or node data.
EOF
}

release_id=""
repository="buckyos/usdb"
release_base_url=""
install_root="${HOME}/.local/share/usdb/releases"
bin_dir="${HOME}/.local/bin"
expected_manifest_sha256=""
expected_node_kit_sha256=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release-id)
      release_id="${2:?missing --release-id value}"
      shift 2
      ;;
    --repository)
      repository="${2:?missing --repository value}"
      shift 2
      ;;
    --release-base-url)
      release_base_url="${2:?missing --release-base-url value}"
      shift 2
      ;;
    --install-root)
      install_root="${2:?missing --install-root value}"
      shift 2
      ;;
    --bin-dir)
      bin_dir="${2:?missing --bin-dir value}"
      shift 2
      ;;
    --expected-manifest-sha256)
      expected_manifest_sha256="${2:?missing --expected-manifest-sha256 value}"
      shift 2
      ;;
    --expected-node-kit-sha256)
      expected_node_kit_sha256="${2:?missing --expected-node-kit-sha256 value}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

[[ "$release_id" =~ ^usdb-(testnet|mainnet)-v[0-9]+-r[1-9][0-9]*$ ]] || {
  echo "Invalid or missing --release-id" >&2
  exit 1
}
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "Invalid GitHub repository: $repository" >&2
  exit 1
}
for expected_digest in "$expected_manifest_sha256" "$expected_node_kit_sha256"; do
  [[ -z "$expected_digest" || "$expected_digest" =~ ^[0-9a-f]{64}$ ]] || {
    echo "Invalid release-bound SHA-256 digest" >&2
    exit 1
  }
done
for command in curl sha256sum tar python3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "Required command is not installed: $command" >&2
    exit 1
  }
done

install_root="$(python3 -c 'import os,sys; print(os.path.abspath(os.path.expanduser(sys.argv[1])))' "$install_root")"
bin_dir="$(python3 -c 'import os,sys; print(os.path.abspath(os.path.expanduser(sys.argv[1])))' "$bin_dir")"
release_dir="${install_root}/${release_id}"
base_url="${release_base_url:-https://github.com/${repository}/releases/download/${release_id}}"
base_url="${base_url%/}"
archive="${release_id}-node-kit.tar.gz"

mkdir -p "$install_root" "$bin_dir"
temporary="$(mktemp -d "${install_root}/.${release_id}.install.XXXXXX")"
cleanup() {
  rm -rf "$temporary"
}
trap cleanup EXIT

download() {
  local name="$1"
  curl --fail --silent --show-error --location \
    "${base_url}/${name}" --output "${temporary}/${name}"
}

download usdb-release-manifest.json
download usdb-release-manifest.json.sha256
download "$archive"
download "${archive}.sha256"

verify_checksum() {
  local name="$1"
  local checksum_file="${temporary}/${name}.sha256"
  local expected
  expected="$(sha256sum "${temporary}/${name}" | awk '{print $1}')  ${name}"
  [[ "$(cat "$checksum_file")" == "$expected" ]] || {
    echo "Checksum mismatch or non-canonical checksum file: $name" >&2
    exit 1
  }
}

verify_checksum usdb-release-manifest.json
verify_checksum "$archive"

verify_release_bound_checksum() {
  local name="$1"
  local expected="$2"
  local actual
  [[ -z "$expected" ]] && return
  actual="$(sha256sum "${temporary}/${name}" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] || {
    echo "Release-bound checksum mismatch: $name" >&2
    exit 1
  }
}

verify_release_bound_checksum usdb-release-manifest.json "$expected_manifest_sha256"
verify_release_bound_checksum "$archive" "$expected_node_kit_sha256"
python3 - "$temporary/usdb-release-manifest.json" "$release_id" <<'PY'
import json
import re
import sys

path, expected = sys.argv[1:]
def strict_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise SystemExit(f"duplicate release manifest key: {key}")
        result[key] = value
    return result

with open(path, encoding="utf-8") as source:
    manifest = json.load(source, object_pairs_hook=strict_object)
actual = manifest.get("release_id")
if actual != expected or re.fullmatch(r"usdb-(?:testnet|mainnet)-v[0-9]+-r[1-9][0-9]*", actual or "") is None:
    raise SystemExit("release manifest ID mismatch")
PY

mkdir "${temporary}/unpacked"
python3 - "${temporary}/${archive}" "${temporary}/unpacked" <<'PY'
import pathlib
import sys
import tarfile

archive, destination = sys.argv[1:]
seen = set()
with tarfile.open(archive, "r:gz") as source:
    members = source.getmembers()
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if (
            path.is_absolute()
            or not path.parts
            or path.parts[0] != "usdb-node-kit"
            or ".." in path.parts
            or member.name in seen
            or not (member.isfile() or member.isdir())
        ):
            raise SystemExit(f"unsafe node kit archive entry: {member.name}")
        seen.add(member.name)
    source.extractall(destination)
PY
kit="${temporary}/unpacked/usdb-node-kit"
[[ -x "${kit}/docker/scripts/tools/usdb_node.py" ]] || {
  echo "Node kit is missing its executable usdb-node controller" >&2
  exit 1
}
cmp "$temporary/usdb-release-manifest.json" \
  "$kit/release/usdb-release-manifest.json" >/dev/null || {
  echo "Node kit manifest does not match the published release manifest" >&2
  exit 1
}

if [[ -e "$release_dir" ]]; then
  if [[ ! -f "$release_dir/release/usdb-release-manifest.json" ]] || \
    ! cmp "$temporary/usdb-release-manifest.json" \
      "$release_dir/release/usdb-release-manifest.json" >/dev/null; then
    echo "Refusing to replace existing release directory: $release_dir" >&2
    exit 1
  fi
  echo "Release is already installed: $release_dir"
else
  mv "$kit" "$release_dir"
  echo "Installed immutable USDB node kit: $release_dir"
fi

launcher="${bin_dir}/usdb-node"
if [[ -e "$launcher" && ! -L "$launcher" ]]; then
  echo "Refusing to replace non-symlink launcher: $launcher" >&2
  exit 1
fi
ln -sfn "${release_dir}/docker/scripts/tools/usdb_node.py" "$launcher"
echo "Installed launcher: $launcher"
cat <<EOF

Next steps for a first node install:
  1. Ensure ${bin_dir} is in PATH:
       export PATH="${bin_dir}:\$PATH"
  2. Check or install supported host prerequisites:
       usdb-node prepare-host
     If Docker group membership changes, log out and back in before continuing.
  3. Configure node identity, data paths, snapshot, and firewall:
       usdb-node setup
  4. Run the explicit read-only preflight, then start services:
       usdb-node doctor
       usdb-node up
       usdb-node status

Existing node upgrade within the same network bundle:
  usdb-node activate-release
  usdb-node doctor
  usdb-node up
  usdb-node status

The installer does not run these commands automatically.
EOF
