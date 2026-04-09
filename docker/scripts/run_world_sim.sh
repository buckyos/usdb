#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"

env_dir="${docker_dir}/local/world-sim/env"
env_file="${env_dir}/world-sim.env"
env_example="${docker_dir}/env/world-sim.env.example"

mkdir -p "${env_dir}" "${docker_dir}/local/world-sim/runtime"

if [[ ! -f "${env_file}" ]]; then
  cp "${env_example}" "${env_file}"
  cat <<EOF
Initialized ${env_file} from ${env_example}
Before the first run, set:
  - WORLD_SIM_BITCOIN_BIN_HOST_DIR
  - WORLD_SIM_ORD_BIN_HOST_PATH
EOF
fi

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"

compose() {
  docker compose \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.world-sim.yml" \
    "$@"
}

assert_world_sim_binary_paths() {
  local ord_bin_path bitcoin_bin_dir
  ord_bin_path="$(awk -F= '/^WORLD_SIM_ORD_BIN_HOST_PATH=/{print $2}' "${env_file}" | tail -n 1)"
  bitcoin_bin_dir="$(awk -F= '/^WORLD_SIM_BITCOIN_BIN_HOST_DIR=/{print $2}' "${env_file}" | tail -n 1)"

  if [[ -z "${ord_bin_path}" || "${ord_bin_path}" == /absolute/path/to/ord ]]; then
    echo "WORLD_SIM_ORD_BIN_HOST_PATH is not configured in ${env_file}" >&2
    exit 1
  fi
  if [[ -z "${bitcoin_bin_dir}" || "${bitcoin_bin_dir}" == /absolute/path/to/bitcoin/bin ]]; then
    echo "WORLD_SIM_BITCOIN_BIN_HOST_DIR is not configured in ${env_file}" >&2
    exit 1
  fi
}

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_world_sim.sh up
  docker/scripts/run_world_sim.sh up-full
  docker/scripts/run_world_sim.sh ps
  docker/scripts/run_world_sim.sh logs
  docker/scripts/run_world_sim.sh down

Modes:
  up       Start the BTC-side local stack plus world-sim, without ethw-node.
  up-full  Start the same stack and include ethw-node as part of full dev-sim.
EOF
}

action="${1:-up}"
shift || true

case "${action}" in
  up)
    assert_world_sim_binary_paths
    compose up --build btc-node snapshot-loader balance-history usdb-indexer usdb-control-plane ord-server world-sim-runner "$@"
    ;;
  up-full)
    assert_world_sim_binary_paths
    compose up --build "$@"
    ;;
  ps)
    compose ps "$@"
    ;;
  logs)
    compose logs -f "$@"
    ;;
  down)
    compose down "$@"
    ;;
  *)
    usage
    exit 1
    ;;
esac
