#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"
repo_root="$(cd "${docker_dir}/.." && pwd)"

bitcoin_bin_host_dir="${WORLD_SIM_RELEASE_BITCOIN_BIN_HOST_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ord_bin_host_path="${WORLD_SIM_RELEASE_ORD_BIN_HOST_PATH:-/home/bucky/ord/target/release/ord}"
ord_source="${WORLD_SIM_RELEASE_ORD_SOURCE:-git-tag}"
ord_version="${WORLD_SIM_RELEASE_ORD_VERSION:-0.23.3}"
bitcoin_image="${WORLD_SIM_BITCOIN_IMAGE:-usdb-bitcoin28-regtest:local}"
tools_image="${WORLD_SIM_TOOLS_IMAGE:-usdb-world-sim-tools:local}"
stage_dir="${docker_dir}/.build-world-sim"

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"

require_executable() {
  local path="${1:?path is required}"
  local label="${2:?label is required}"
  [[ -x "${path}" ]] || {
    echo "Missing executable ${label}: ${path}" >&2
    exit 1
  }
}

cleanup() {
  rm -rf "${stage_dir}"
}
trap cleanup EXIT

require_executable "${bitcoin_bin_host_dir}/bitcoind" "host bitcoind"
require_executable "${bitcoin_bin_host_dir}/bitcoin-cli" "host bitcoin-cli"

rm -rf "${stage_dir}"
mkdir -p "${stage_dir}/bitcoin/bin" "${stage_dir}/ord/bin"

install -m 0755 "${bitcoin_bin_host_dir}/bitcoind" "${stage_dir}/bitcoin/bin/bitcoind"
install -m 0755 "${bitcoin_bin_host_dir}/bitcoin-cli" "${stage_dir}/bitcoin/bin/bitcoin-cli"
if [[ -x "${bitcoin_bin_host_dir}/bitcoin-wallet" ]]; then
  install -m 0755 "${bitcoin_bin_host_dir}/bitcoin-wallet" "${stage_dir}/bitcoin/bin/bitcoin-wallet"
fi

case "${ord_source}" in
  local)
    require_executable "${ord_bin_host_path}" "host ord"
    echo "Packaging local ord binary from ${ord_bin_host_path}" >&2
    install -m 0755 "${ord_bin_host_path}" "${stage_dir}/ord/bin/ord"
    ord_build_mode="staged-binary"
    ;;
  git-tag|release)
    echo "Building ord ${ord_version} from the official git tag inside Docker" >&2
    printf '#!/usr/bin/env bash\nexit 1\n' > "${stage_dir}/ord/bin/ord"
    chmod 0755 "${stage_dir}/ord/bin/ord"
    ord_build_mode="git-tag"
    ;;
  *)
    echo "Unsupported WORLD_SIM_RELEASE_ORD_SOURCE: ${ord_source}" >&2
    echo "Expected one of: local, git-tag" >&2
    exit 1
    ;;
esac

echo "Building ${bitcoin_image} from ${bitcoin_bin_host_dir}" >&2
docker build \
  -f "${docker_dir}/Dockerfile.world-sim-bitcoin" \
  -t "${bitcoin_image}" \
  "${repo_root}"

echo "Building ${tools_image} with ord source ${ord_source}" >&2
docker build \
  -f "${docker_dir}/Dockerfile.world-sim-tools" \
  --build-arg "ORD_BUILD_MODE=${ord_build_mode}" \
  --build-arg "ORD_VERSION=${ord_version}" \
  -t "${tools_image}" \
  "${repo_root}"

echo "Validating packaged binaries" >&2
docker run --rm "${bitcoin_image}" /opt/bitcoin/bin/bitcoind --version | head -n 1
docker run --rm "${bitcoin_image}" /opt/bitcoin/bin/bitcoin-cli --version | head -n 1
docker run --rm "${tools_image}" /opt/ord/bin/ord --version
