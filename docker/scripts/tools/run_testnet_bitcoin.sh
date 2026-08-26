#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
bundle_dir="${USDB_TESTNET_BUNDLE_DIR:-${docker_dir}/networks/testnet-v0}"
node_env="${USDB_TESTNET_NODE_ENV:-${bundle_dir}/node.env}"
project_name="${USDB_TESTNET_BITCOIN_PROJECT_NAME:-usdb-testnet-v0-bitcoin}"
validator="${script_dir}/validate_network_bundle.py"

usage() {
  cat <<EOF
Usage:
  docker/scripts/tools/run_testnet_bitcoin.sh <action> [args]

Actions:
  init-rpc-auth [username]
            Create the private rpcauth file and print the generated client secret once.
  validate  Validate the bundle and Bitcoin node configuration.
  up        Create the shared network, start Bitcoin Core, then wait for readiness.
  wait      Wait for mainnet, full sync and txindex readiness.
  status    Show service state and perform a single readiness check.
  ps        Show service state.
  logs      Follow Bitcoin Core logs.
  pull      Pull the digest-pinned Bitcoin Core image.
  down      Stop Bitcoin Core without deleting its bind-mounted data directory.

Bitcoin Core is a separate Compose project. USDB runtime stop/reset operations do
not stop it and never remove BTC_NODE_DATA_HOST_DIR.
EOF
}

action="${1:-}"
if [[ -z "${action}" || "${action}" == "help" || "${action}" == "--help" || "${action}" == "-h" ]]; then
  usage
  exit 0
fi
shift || true

require_node_env() {
  if [[ ! -f "${node_env}" ]]; then
    echo "Missing node env: ${node_env}" >&2
    echo "Run docker/scripts/tools/run_testnet_runtime.sh init-env first." >&2
    exit 1
  fi
}

env_value() {
  local key="$1"
  local file="$2"
  local line
  line="$(grep -m 1 -E "^${key}=" "${file}" || true)"
  printf '%s' "${line#*=}"
}

network_name() {
  env_value USDB_DOCKER_NETWORK "${bundle_dir}/network.env"
}

ensure_network() {
  local name
  name="$(network_name)"
  if [[ -z "${name}" ]]; then
    echo "USDB_DOCKER_NETWORK is missing from network.env" >&2
    exit 1
  fi
  if ! docker network inspect "${name}" >/dev/null 2>&1; then
    docker network create \
      --label org.usdb.network-bundle=usdb-testnet-v0 \
      "${name}" >/dev/null
    echo "Created shared Docker network ${name}"
  fi
}

validate_node() {
  python3 "${validator}" \
    --bundle-dir "${bundle_dir}" \
    --node-env "${node_env}"
}

validate_bitcoin_runtime() {
  python3 "${validator}" \
    --bundle-dir "${bundle_dir}" \
    --node-env "${node_env}" \
    --require-bitcoin-runtime
}

prepare_data_dir() {
  local data_dir
  data_dir="$(env_value BTC_NODE_DATA_HOST_DIR "${node_env}")"
  if [[ -z "${data_dir}" || "${data_dir}" != /* ]]; then
    echo "BTC_NODE_DATA_HOST_DIR must be an absolute path" >&2
    exit 1
  fi
  install -d -m 0700 "${data_dir}"
}

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${bundle_dir}/network.env" \
    --env-file "${node_env}" \
    -f "${docker_dir}/compose.bitcoin.yml" \
    "$@"
}

wait_ready() {
  local timeout_secs
  timeout_secs="${BTC_READY_WAIT_TIMEOUT_SECS:-86400}"
  compose exec -T btc-node \
    python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py \
      --wait-timeout-secs "${timeout_secs}" \
      --poll-interval-secs "${BTC_READY_POLL_INTERVAL_SECS:-15}"
}

case "${action}" in
  init-rpc-auth)
    require_node_env
    rpcauth_file="$(env_value BTC_RPCAUTH_HOST_FILE "${node_env}")"
    if [[ -z "${rpcauth_file}" || "${rpcauth_file}" != /* ]]; then
      echo "BTC_RPCAUTH_HOST_FILE must be an absolute path" >&2
      exit 1
    fi
    python3 "${script_dir}/generate_bitcoin_rpcauth.py" \
      --username "${1:-usdb-testnet}" \
      --output "${rpcauth_file}"
    echo "Store the printed username/password in node.env; the password is not recoverable from rpcauth." >&2
    ;;
  validate)
    require_node_env
    validate_node
    ;;
  up)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    prepare_data_dir
    validate_bitcoin_runtime
    ensure_network
    compose config --quiet
    compose up -d "$@" btc-node
    wait_ready
    ;;
  wait)
    require_node_env
    validate_bitcoin_runtime
    wait_ready
    ;;
  status)
    require_node_env
    compose ps
    compose exec -T btc-node \
      python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py
    ;;
  ps)
    require_node_env
    compose ps "$@"
    ;;
  logs)
    require_node_env
    compose logs -f "$@" btc-node
    ;;
  pull)
    require_node_env
    validate_node
    compose pull "$@" btc-node
    ;;
  down)
    require_node_env
    compose down --remove-orphans "$@"
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
